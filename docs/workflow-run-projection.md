# Foreground Workflow run projection (WF-06)

Live WorkflowRun/WorkflowRuntime owns execution. Its process-local
WorkflowReadModel commits bounded native read records directly at execution
boundaries. No Event Journal entry is input to this read model. The journal
continues to own evidence/history and durable workspace recovery facts.
RuntimeClientProjection owns external projection, cursor allocation, replay,
and subscriptions. TUI owns formatting, expansion, focus and navigation only.

## Identity and state

A run has configured Workflow identity, SHA-256 of its admitted definition,
the admitted resource revision, conversation/attempt/invocation identity, and
outer ToolCallId. Replacing a definition does not relabel an admitted program.
Instances contain concrete block paths and invocation vectors; Loop iterations
append a fresh one-based component and Parallel branches have distinct static
paths. There is no authoritative current-node slot. All seven node kinds and
block instances share the same executor boundaries. Pending rows mean not yet
admitted, including unselected successors; they do not promise future work.

Node waits distinguish Tool, Agent, capacity, workspace, Questionnaire,
Approval, Review and settlement. Native ToolInvocationId and child references
preserve owner correlation. Interaction references are correlated from the
existing native coordinator projection under the client lock. The root HITL
queue is the only response surface. Child inspection remains read-only.

Execution completed is not a business acceptance. Structured check `passed`
facts, Review decisions and Loop satisfied/exhausted exits are shown separately.
Full Workflow-local values and arguments are not copied into the read model.
The canonical outer ToolResult remains the declared business result.
Candidate references contain native content identity and version, not just HEAD.
During admitted candidate use, old checks/Review do not certify mutable contents.
After native settlement, references for different candidates remain historical.
The retained handoff is a bounded path/disposition reference, not workspace data.

## Synchronization

WorkflowRevision, EventJournalSequence and RuntimeClientCursor are independent
domains. No conversion or ordering comparison crosses those domains.

Each native read mutation installs a new WorkflowRevision and a complete bounded
cut under a leaf mutex. It releases that mutex before sending a coalescing watch
wakeup. No callbacks into other execution owners or client locks occur there.
The watch carries no execution data; execution never waits for a subscriber.

Every Runtime Client host lock acquisition drains ordinary native observations,
then reads the retained native cuts after its last WorkflowRevision. It folds
each cut, allocates its own cursor and publishes WorkflowsUpdated while still
holding the client lock. Snapshot copying and cursor return use this same lock.
After reading cuts it drains ordinary observations again: the coordinator
publishes settlement before returning to a Workflow human waiter. Correlation
only annotates an already-native matching wait, using the exact run, block,
node and visit; it cannot turn a settled row back into a pending interaction.
Thus a native mutation concurrent with a prepared cut is either in that cut or
in a later retained cut. The watch is subscribed before the worker waits; a
notification concurrent with folding remains observable to its next changed
call. Requests also read native cuts directly without depending on worker timing.

If the first available native revision skips revisions, the client installs the
authoritative cut and invalidates older replay cursors. Consumers receive
resync_required and obtain a fresh snapshot. Expiration of the external replay
ring has the same explicit repair behavior. Neither case consults journal
absence. WorkflowsUpdated is a complete replacement: event folding and snapshot
replacement converge without a client-side Workflow interpreter.

## Bounds and lifetime

| Dimension | Limit |
| --- | --- |
| Retained run records (active and retired combined) | 8 |
| Concrete rows per run | 512 |
| Settled rows per run | 128 |
| Serialized run record | 256 KiB |
| Native coherent cuts | 128 and 8 MiB, whichever is reached first |
| Runtime Client replay | configured event count (default 4096); Workflow payloads additionally capped at 16 MiB |
| Handoff path | 1024 bytes with explicit truncation flag |
| Program block nesting | existing maximum 8 |

Finished rows retire first, then pending rows, then other rows if necessary to
honor byte/count bounds. omitted_instances records omissions explicitly. Old
terminal run records retire before active records; omitted_runs records run
omissions. Records contain no WorkflowRun, future, waiter, cancellation token,
filesystem lease, child history, environment, or value map. Native cuts expire
independently of the currently retained run records. An overlarge identity
record is omitted rather than publishing an ambiguous truncated identity.

## Cancellation and reconnect

### Historical identity outlives detailed retention

A still-visible canonical Workflow ToolCall must never become an ordinary Tool
card just because its native detailed run was retired. The native outer Tool
executor therefore attaches `ToolExecutionResult.workflow`: configured Workflow
ID and immutable admitted program digest. This is typed runtime metadata, not
tool-owned content, and is excluded from the model-facing result projection.
Runtime Client carries it with the existing canonical result/foreground
settlement; it does not reconstruct it from journal events or catalog names.

The full native run view wins when available. Otherwise the card identifies a
Workflow invocation and explicitly displays “Historical Workflow details
unavailable (retired or process reopened)”. This says nothing about execution
success, business acceptance, current candidate applicability or control rights.

There is no tombstone registry. The additional identity is at most 192 serialized
bytes (64-byte configured ID and 64-character digest), exactly one per existing
outer result. Additional projected storage is bounded by 192 times the number
of Tool results on the current canonical/foreground surface; removal of a result
removes its metadata too. No independent history, eviction queue or executable
owner survives. Native detail retention remains eight runs with terminal-first
eviction. All ordinary results have no Workflow identity metadata.

Detailed retirement is a process-local retention decision. Process reopen loses
all live run details but may retain legitimately committed outer-result identity.
Neither restores execution or waiters. Client replay expiration instead requires
a fresh snapshot of the current surface, including these existing result facts.
Event Journal evidence remains separate and is never replayed to recover details.

The real stdio regression executes nine native terminal Workflow calls, asserts
eight retained runs and one omitted run, keeps the oldest canonical call visible,
and renders it through production reduction/correlation/Tool-card code. It checks
explicit unavailability versus an otherwise identical ordinary card, reconnect
and process reopen, unchanged canonical history and exact provider request count.
Provider gates separate the nine invocations with snapshot cuts, independently
of OS scheduling and replay throughput. There are exactly ten scripted provider
requests; inspecting or reopening adds none.

### Execution cancellation

The existing foreground invocation/attempt cancellation signal is the sole
authority. A read-only observer publishes draining when cancellation is seen,
then awaits the same execution future. Node and iteration frontiers still use
the original cancellation/budget admission checks. Local native owners drain;
terminal cancellation is published only after actual Workflow settlement.
Repeated cancellation intent does not publish another draining transition.
Cancellation does not roll back candidate contents or remove useful handoff.

Live detach/reconnect/resync reads the same process-owned records and pending
coordinator references. It admits no work or human requests. Fresh process
composition creates an empty Workflow read model. Durable inspection can show
existing committed evidence/resource recovery outcomes, but never rebuilds
Workflow ownership, continuations, waiters, or cancellation controls. New
attempt/invocation identities reject stale old-run observations and references.

Runtime Client protocol 22 adds required snapshot workflows and the
workflows_updated replacement event. Rust, TypeScript, negotiation tests and
the connection event allow-list change together; protocol 21 is rejected.

## Regression boundaries

The native read-model tests gate a prepared snapshot before cancellation or
node completion and fold it only after the concurrent native commit. The
subsequent cut must remain observable after the returned client cursor.
`workflow_skipped_revision_requires_resync_and_snapshot_repairs` exercises the
complementary expired-cut path; `native_observation_lag_does_not_change_loop_requests_or_outcome`
runs 100 real native check iterations without consuming observations and
checks exact starts, exhaustion, truncation and canonical isolation.

The existing Parallel child tests now assert simultaneous concrete child rows
and observe draining before releasing staged child cancellation settlement.
The nested Loop and candidate acceptance tests inspect native concrete
iterations and historical accepted candidate references. Separate identity
tests reject old runs, blocks, iterations and visits; a coordinator projection
test rejects a queued old Review against a settled native row.

Rust and TypeScript share `tests/fixtures/runtime-client/workflow-v22.json`.
The TUI projection tests prove replacement/snapshot convergence, pure rendering,
concurrent waits, nested layout, orphan visibility after truncation, historical
acceptance and honest cancellation/exhaustion text. One real stdio integration
test covers native execution through correlation/rendering, live reconnect,
and reopening the same durable Session in a fresh process without new requests
or restored Workflow ownership. Existing child-inspector and root HITL suites
remain the response-routing and inspection-isolation contracts.

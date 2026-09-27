# Repaired read-domain ownership

`ConversationObservation` has three independent responsibilities: semantic DTO fold,
Trace frontier publication, and durable transcript/response/statistics/occupancy
invalidation. `projection::invalidates_read_domains` exhaustively classifies the
observation union; it is a positive dependency list, not “everything except tokens”.

| Observation | Durable derived domain | Trace |
| --- | --- | --- |
| `JournalBatch` | Its enclosed facts decide | One invalidation for its represented successful frontier |
| `Published` | Unwrap its enclosed fact | Receipt staging belongs to PendingObservations |
| `Committed` | Canonical transcript, response members, Tool result associations, Todo | Corresponding committed Journal anchor |
| `PublicationSettled` | Audit transcript entry | No independent Trace anchor |
| `InboundEnqueued`, `InboundDrained`, `PendingInboundChanged` | Pending/canonical transcript and edited/removed pending entries | Adoption Journal anchor only |
| `InteractionPending`, `InteractionSettled` | Only when carrying a conversation-local audit | Requested/settled Journal anchor |
| `Event`, `ManualCompactionEvent` | Positive RuntimeEvent list below | Closed native Journal vocabulary below |
| `Workflow`, `GoalChanged` | No | Only Workflow run lifecycle Journal anchors |
| `InboundAdopted` | No independent reread; committed input/Attempt facts own it | Adoption Journal anchor |
| `Status`, `PublicationOpened`, `Publication` | No | No |
| `Background`, `SubagentLifecycle` | No | Corresponding ownership/terminal Journal anchors |
| `ToolProgress`, `SubagentWorkspace`, `SubagentActivity` | No | No |
| `Capability`, `Resources`, `AttemptAdmitted`, `SessionModelChanged` | No | No independent anchor |
| `Shutdown`, `InteractionRemoved`, `DurableFailure`, `DurabilityFailed` | No | No |

The RuntimeEvent read dependencies are Attempt start/all terminals; Turn start;
request start/completion/failure; Assistant/Tool committed-message facts; and
compaction start/completion. Other RuntimeEvents do not request durable enrichment.
Tool execution start/settlement publishes Session state and Trace, while its
canonical Tool message owns the durable transcript association. Statistics and
response timing use the bounded represented Journal prefix; occupancy additionally
uses the prepared request's immutable snapshot. Publication deltas never reread it.

## Read/install cut

A live host owns one projection identity. The read fence consists of a monotonic
revision advanced by each durable-domain observation plus the represented Journal
frontier required by those dependencies. `read_through` advances for read-domain
facts; `journal_through` independently advances for Trace. Trace-only progress
cannot supersede a durable candidate whose dependencies are unchanged. The worker captures this fence after folding queued semantic facts, and
only when no earlier native Journal publication is outstanding. Store reads run in
one blocking task outside the projection mutex. While it waits, the same semantic
worker continues folding observations, including shutdown and admission state.

Completion folds queued facts again. Only the matching revision/frontier with no
unpublished native receipt may install the result. Validation, replacement of the
bounded derived domains, cursor allocation and `ReadDomainsUpdated` publication
are one critical section: that is the installation linearization point. A stale
success or error is discarded, leaving the latest cut dirty. No second event queue
or durable authority exists. Store errors are explicit; a new invalidation or an
explicit authoritative request can retry, without an automatic failure spin.

### Finite authoritative snapshots

`lock_snapshot_state` makes at most **three** read/validate attempts, with no sleep
or debounce. A candidate is installed only at the matching revision and represented
Journal frontier, after folding queued semantics and checking unpublished receipts.
If another materializer already repaired the current cut, the request can use that
cut without another read. An unpublished receipt permits the preceding cut only
when its derived domains are already clean. No dirty semantic/derived mixture is
returned as a successful authoritative snapshot.

At success, the host mutex protects both the clean Session DTO and client cursor;
`snapshot_cut()` copies those and the Trace frontier together. This is the snapshot
linearization point. It names a cut that existed during the request, not the newest
state when the response reaches a client. The bounded replay ring owns later client
cursors. Superseded results never install. Three superseded candidates return the
existing typed `RuntimeClientError::RuntimeFailure` with the fixed diagnostic
`snapshot cut superseded during all 3 read attempts; retry authoritative acquisition`.
Clients' existing read-failure behavior keeps recovery explicit; no mutation is retried
and no new wire variant/version is needed.

Explicit snapshot requests use the existing admitted synchronous Store request path,
including pending-mailbox repair; they do not launch detached blocking work. Pending
readback uses its publication guard outside the projection mutex and validates the
semantic fence. Bootstrap precedes live host publication. Inspection rebuilds run
outside host synchronization. `snapshot_with_trace` captures the Session DTO/cursor
before independently enriching Trace; Trace never changes live Session fields.

### Presentation read execution and termination

At host construction, `ConversationStore::presentation_reader` creates one scoped
read authority sharing the SQLite connection and its storage-access lease, but no
runtime, Session residency, provider, App Server or projection ownership. Only the
background materializer uses this scope. Its single `spawn_blocking` task remains
owned by the projection worker; dropping the handle is not cancellation.

SQLite connection serialization now leases the connection from a condition-protected
slot. SQL runs while holding the lease, never the condition mutex. Ordinary Store
users retain serialized access. Presentation connection waits check their scope's
cancellation flag and wake on cancellation without requiring the current writer
to release the connection. During a presentation lease SQLite busy waiting is disabled
and a progress handler observes cancellation every 1,000 VM operations. The lease
restores the ordinary 5,000ms busy policy and removes the handler before another
Store user acquires it. No execution write is interrupted by presentation cancellation.

Native shutdown observations cancel the background scope and stop new background
reads. Session manager unloading cancels it before waiting for admitted operation
leases, then performs native shutdown and `drain_projection`. Closing delivery cancels
again idempotently and joins the worker **and its read task**, proving termination.
A host drop also requests cancellation; Tokio's ordinary runtime destruction still
joins the now-terminating task. `src/main.rs` and global runtime shutdown policy are
unchanged. No thread is deliberately leaked or abandoned through a timeout. Closed
or shutting-down delivery cannot install late results; the revision/frontier remains
the installation authority during normal operation.

`AttemptSettled` remains semantic execution completion, independent of enrichment.
Explicit final snapshots can read settled authority after the background scope has
closed. This is why that request path does not reuse a permanently cancelled reader.

### Field ownership

| Field | Owner / coherent-cut rule |
| --- | --- |
| Canonical messages, transient content, Attempt/control state | Incremental semantic fold at the captured client cursor |
| Pending inbox | Semantic observations, with explicit mailbox readback fenced before repair |
| Newest transcript page | Durable read result, installed only at its revision/frontier |
| Response decorations and statistics | Bounded Journal prefix of that same durable read cut |
| Occupancy | Immutable request snapshot resolved through that same prefix |
| Todo | Semantic projection; read-domain event carries the current Todo at installation |
| Trace | Independent Journal frontier and bounded Trace reads; cannot mutate Session after cursor capture |

### Process ownership audit

OS signal -> AppServerHost admission fence -> SessionRuntimeManager drain/unload ->
read-scope cancellation -> admitted-operation drain -> ConversationRuntime shutdown
-> projection worker/read join -> composition/storage lease release -> transport and
reaper joins -> `run_process` returns -> ordinary Tokio Runtime drop -> OS exit.

The App Server owns its transport task, connection tasks and idle reaper. The manager
owns loading/unloading tasks and admitted operation leases (including catalog and
composition blocking tasks). ConversationRuntime owns model attempts, interactions,
background/subagent/workflow settlement and resource shutdown. MCP stdio servers, managed-Python preparation processes and
Tool/process supervisors retain their existing composition/resource/settlement owners;
Bash/interactive and subagent supervisors own and reap their child processes.
Presentation reads create no children or OS-thread pool. The projection worker owns exactly one Tokio blocking read with Store-only
authority. That task is cancelled and joined before normal residency release.
The existing App Server deadline/second-signal forced exit remains unchanged and
reports unproven native settlement; this repair adds no process-wide timeout policy.

## Trace and Web handoff

`PendingObservations::trace_fact_requires_publication` covers Attempt/Turn/request
(including retry and context contributions), adoption, Assistant/Tool committed
messages and Tool execution, compaction, Background commits/terminals, Subagent
ownership/terminals, Workflow run lifecycle and interaction anchors. The queue
releases their represented frontier only after native installation. A JournalBatch
emits one `TraceChanged` regardless of other Session publications. Per-event
fallback Trace signals were removed. The real queue/batch regression coalesces
Attempt start, Turn start and Tool start while proving all Session events survive.

Web `trace_changed` performs its own coalesced `session/trace` read, never a
`session/snapshot` read. `read_domains_updated` updates Session only. During a
snapshot acquisition, both overlapping events and resync retain the subscribe
requirement. Resync additionally marks the old continuation untrusted. The acquired
snapshot installs N, then registers after exactly N, with existing target/generation
fencing. Native transport can emit registered replay before the subscribe ACK.
The existing refresh owner consumes contiguous events above the acquired N while
controls remain resynchronizing: the old exhausted registration cannot emit N+1,
and any in-flight old event at/below N is a duplicate. This needs no browser event
buffer or extra recovery machine. Only the ACK enables attached controls.
A rejected bounded replay retries authoritative acquisition at most three
times; failure leaves the attachment stale. Mutations are never replayed.

The fixed long rendering measurement lives in
`test/incremental-performance.measurement.tsx`, selected only by
`pnpm test:issue-420-performance` (one worker). Ordinary `pnpm test` retains cursor,
recovery, independent equivalence, vocabulary, zero streaming snapshots, stable
message identity and deterministic single-frame/history-reading scroll tests.

## Final ownership audit against the PR base

| Question | Code/contract evidence |
| --- | --- |
| Can derived I/O hold projection/control synchronization? | `ClientInner::lock_state` only folds semantics; worker `ReadDomainCut::read`, explicit snapshot pending/domain reads and historical rebuilds run without the host mutex. Shutdown/drain regression holds materialization with channels. |
| Can a Session publication suppress Trace? | `apply(JournalBatch)` always publishes Trace once after its successful represented frontier; no cursor comparison or per-event fallback remains. |
| Can durable Session updates trigger Trace? | Web notification handler tests only `trace_changed`; production-client RPC-count regression covers both domains and burst coalescing. |
| Can acquiring resync disappear? | The existing `resubscribe` owner is updated before checking `acquiring`, and invalid continuation marks `resynchronizing`; ACK, cursor and bounded failure tests cover the handoff. |
| Do normal deltas reread snapshots? | The sole Session snapshot RPC is in explicit/recovery `performRefresh`; contiguous streaming regression and both fixed measurements report zero. |
| Does Chat have one automatic writer? | `presentation/layout/ChatViewport.tsx` contains the sole viewport `scrollTop` assignment in its RAF commit; no Chat renderer/observer writes it. Trajectory remains separate. |
| Is canonical identity stable? | `AgentTranscript` uses `MessageSeat` keyed by `message:<native message_id>` for transient/canonical branches, suppresses streaming when canonical ID exists, and preserves reasoning DOM. Unchanged presentation tests and browser fixture prove it. |
| Is the ordinary suite benchmark-independent? | The unchanged `.test.*` discovery excludes the renamed `.measurement.tsx`; the dedicated config selects it with one worker. Deterministic correctness tests remain ordinary tests. |
| Is the process shutdown contract retained? | The exact SQLite-boundary process test is unchanged and passes. Neither timeout nor native execution ownership changed. |
| Are #419/#422 creation/FirstSubmissions/ACK/generation retained? | No creation, initial-model, mutation correlation or generation code changed in the repair. Existing native conformance, Web first-submit and browser startup/uncertain-write contracts are included in validation. |

## Follow-up terminal self-review

1. Presentation SQL owns a connection lease, never the projection mutex.
2. Native shutdown cancels the scope; it does not wait behind an uncancellable read.
3. App Server control/admission state remains independent; manager cancellation precedes operation drain.
4. The blocking task returns after cancellation and is joined before normal process return.
5. Dropping a handle is never considered terminal proof; Drop requests cancellation only.
6. Snapshot acquisition has at most three candidates, with explicit typed exhaustion.
7. Success copies a clean Session DTO and cursor in one critical section.
8. Exact durable revision/required-prefix validation rejects obsolete results.
9. The retained replay ring owns C+1 onward, including events committed before response delivery.
10. Trace has its own frontier and invalidation; a Trace-only fact cannot retire a durable candidate.
11. Web acquisition-time resync and ACK/replay handling are unchanged.
12. The fixed measurement stays outside ordinary Vitest discovery.
13. Chat's sole RAF writer, native message seat and narrow subscriptions are unchanged.
14. Creation, FirstSubmissions, mutation ACK and generation ownership are unchanged.

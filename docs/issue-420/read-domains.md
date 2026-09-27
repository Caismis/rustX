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

### Live failure and authoritative repair

`read_failure` records failure to establish the **current** durable dependency
fence `(read_revision, read_through)`. It is stored only after that exact fence
passes `install_read_domains`; every new invalidating revision clears the old
failure and marks its own cut dirty. Trace-only or unrelated progress does neither.
Thus the message has one unambiguous cut owner without a second failure frontier.

A matching background failure clears dirty, records failure and wakes subscribers.
Polling fails closed with Lagged/resync. There is no automatic error retry loop.
A successful background or authoritative result uses the same installation owner:
it accepts only an unexhausted, exactly matching fence that is dirty or failed.
Success clears failure/dirty and replaces the derived domains, publishing
`ReadDomainsUpdated` only when values changed. A late failure after another owner
has already established that cut is rejected, so it cannot re-poison the result.

After a candidate's Store read succeeds outside synchronization, the host folds
queued semantics and offers its result under projection synchronization. The host
must still be live (not inspection, closed or shutting down), with no unpublished
receipt; the shared installer then validates the exact dependency fence. A stale
candidate changes nothing: it cannot clear a newer failure/dirty bit, replace
newer values, or publish a false update. Its own historical response still succeeds.
A same-cut success repairs an idle failed projection without another semantic
event, reconnect or snapshot; subscription after the returned C resumes normally.
Any actual candidate Store error still fails explicitly before this opportunity.

### Finite authoritative snapshot candidates

`SnapshotCandidate` captures the complete semantic Session DTO, client cursor C,
durable dependency frontier, transcript membership evidence and independent Trace
frontier under the projection mutex after folding represented observations. That
capture is the semantic linearization point. It makes one request-owned copy;
completion never consults the mutable live projection again. Live progress to N
cannot invalidate C and there is no optimistic retry-to-latest loop.

The transcript read uses captured membership, not the newest database page:

- the bootstrap transcript prefix supplies immutable canonical history;
- subsequent canonical identities come from the captured semantic messages;
- publication audit identities are retained only for the newest page plus one
  overflow row (65); immutable Journal audits/terminal rows use the captured prefix;
- pending message bodies and their transcript positions are frozen at capture,
  so later edits, removals or adoption cannot rewrite C;
- mutable Tool-result associations are filtered by captured canonical membership.

These are references/copies of existing semantic facts, not another event queue
or durable truth. SQLite selects the newest 64 eligible entries and its exact
older-page continuation. Response decorations, statistics and occupancy read the
same fixed durable dependency prefix. A later completion or Tool result cannot
leak backward. Todo remains the captured semantic projection.

Pending readback occurs under the mailbox publication guard outside the projection
mutex. Repair is accepted only at its captured read-domain fence with no unpublished
receipt. If either changed, capture uses the already represented semantic pending
view instead; it never chases the latest durable pending table. An unpublished
receipt cannot inject new pending state into an old client cursor.

Completion populates the candidate's request-owned copy at its captured cursor.
It then offers that successful result to the shared live installation primitive;
this optional repair never changes the candidate or makes its success depend on
installation. Only an actual change to live derived values allocates a subsequent
`ReadDomainsUpdated` cursor. `snapshot(C)` followed by
`subscribe(after C)` replays C+1...N. If history was evicted, subscription returns
typed `ResyncRequired`; snapshot acquisition does not predict replay retention.
Actual storage errors remain typed RuntimeFailure, but ordinary progress is not
an error and no diagnostic string controls retry behavior. No protocol change is
needed.

SnapshotGet, App Server snapshots and attachment initialization share this native
candidate. Attachment allocation/control fencing remains under its original mutex,
and registration uses the candidate's exact C. The background read owner separately
validates revision/frontier before live installation: stale for installation does
not mean invalid for an already captured request.

Read-only durable inspection copies Surface, canonical history, transcript and
Journal frontier in one database read transaction, without a projection mutex.
Journal folding then walks only that finite prefix, even if a writer advances.
It completes the same candidate using the copied transcript seed. Independent Trace
enrichment never rewrites Session fields, for either live or inspection snapshots.

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
| Newest transcript page | Candidate membership and prefix; background installation separately validates revision/frontier |
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

Web Trace reads capture a per-Session authority generation in addition to the
connection generation and exact attachment target. Installing authoritative
snapshot Trace data supersedes reads admitted under the prior authority, even
when an overlapping cache merge preserves its interval epoch. Resync reset and
explicit latest-window replacement also supersede prior reads. Tail, older-page,
and detail responses from a superseded authority are discarded; pending paging
and detail loading markers are released with the new authority. Runtime Client
cursors never order Trace reads. The tail owner retains one active read and one
coalesced dirty bit; a later invalidation reads under the current authority,
without requiring a Session snapshot or replaying any mutation.

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
| Can derived I/O hold projection/control synchronization? | `ClientInner::lock_state` only folds semantics; worker `SnapshotCandidate::read_domains`, explicit snapshot pending/domain reads and historical rebuilds run without the host mutex. Shutdown/drain regression holds materialization with channels. |
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
6. Snapshot acquisition completes one captured candidate; later progress cannot invalidate it.
7. Capture copies semantic DTO, cursor and durable membership in one critical section; completion is restricted to those dependencies.
8. Exact durable revision/required-prefix validation rejects obsolete results.
9. The retained replay ring owns C+1 onward; eviction returns typed ResyncRequired at subscription.
10. Trace has its own frontier and invalidation; a Trace-only fact cannot retire a durable candidate.
11. Web acquisition-time resync and ACK/replay handling are unchanged.
12. The fixed measurement stays outside ordinary Vitest discovery.
13. Chat's sole RAF writer, native message seat and narrow subscriptions are unchanged.
14. Creation, FirstSubmissions, mutation ACK and generation ownership are unchanged.

## Captured-cut self-review

Snapshot completion succeeds while the head moves, independently of optional
fenced live repair. Canonical membership, frozen pending bodies, filtered Tool associations and
fixed Journal prefixes exclude later state while including all represented facts.
Replay supplies every later cursor or explicitly refuses an evicted cursor.
Background installation remains revision-fenced; the same historical data can be
valid for a candidate and stale for installation. No supersede diagnostic or string
retry remains. Normal active process snapshots succeed. Store cancellation still
joins the worker. Trace, Web/TUI recovery/mutation rules, Chat's writer/message seat,
and #419/#422 startup/FirstSubmissions/ACK/generation ownership are unchanged.

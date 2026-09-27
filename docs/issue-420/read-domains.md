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
frontier. The worker captures this fence after folding queued semantic facts, and
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

Snapshot requests repeat finite read/validate cuts outside the projection lock
until one installs or another reader has already repaired the current cut. They
never turn ordinary concurrent semantic progress into a storage error. Pending-inbox readback also releases projection ownership while reading and
validates the semantic fence before installation. Durable inspection rebuilds are
read outside the host lock. Bootstrap reads occur before live host publication. Trace snapshot enrichment
never rewrites live Session domains after the client cursor has been captured.

`AttemptSettled` is the execution outcome and remains exactly once. It does not
promise that response/statistics enrichment has already published. Subsequent
`ReadDomainsUpdated` and `TraceChanged` are independently ordered client events.
Closing projection delivery stops installation and joins semantic delivery only;
it does not join blocked Store reads. A running read retains only its own Store
lease until completion, never a host/runtime/control owner. No shutdown deadline,
Tokio worker count or process-test timeout was changed.

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

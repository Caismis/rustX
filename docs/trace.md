# Native Trace and Web Trajectory

Trace is a read-only presentation projection. It is never canonical history,
execution authority, cancellation/settlement authority, recovery input, or a raw
Event Journal API. Deleting every Trajectory component changes none of those
native semantics.

## Ownership

`runtime_client::trace::TraceProjection` owns the typed interpretation. App Server
only validates/routes reads and serializes its allowlisted DTOs. It does not fold
execution events. There is no Trace table, cache on disk, duplicated snapshot,
duplicated message body, recovery hook, or second subscription.

| Native authority | Trace facts |
| --- | --- |
| Event Journal | Exact identities, ordering, starts and proven terminal boundaries |
| Immutable Request Snapshots | Owning Attempt/logical Step, actual request ordinal, historical model name and request limits |
| Message Ledger | Accepted Assistant text/reasoning, canonical ToolCall proposals, Tool result artifact references |
| Conversation Surface | Remains the current conversation authority; Trace neither alters it nor mistakes retired messages for deleted history |
| Current runtime projections | Positive evidence for current Attempt/request/foreground Tool activity, detached execution, Subagent, Workflow and interaction lifecycle labels |
| Existing artifact carrier | Bounded `artifact/read` for legitimate canonical artifact IDs; no new binary endpoint |

Journal message commits carry IDs; Trace retrieves content from the Ledger. A
provider request completing does **not** prove Assistant acceptance. Interrupted
publication is not promoted into a completed Assistant record. Chat continues to
own the existing publication audit presentation. Canonical ToolCall proposals
prove assembly, not execution: a separate started Tool record proves execution.

Native `TurnId` is the logical model Step inside an Attempt. The UI labels the
Attempt as a Turn/Attempt group and shows its native Steps. `RequestIdentity`
places request #0 and retries/recovery #1, #2, etc. beneath the **same** logical
Step. The exact preceding request's failure class distinguishes transient,
context-overflow and corrective cases when recorded. Neither timestamps nor
repeated output are retry evidence. Tool joins include Attempt, logical Step,
ToolCall ID and Tool ID; parallel physical completion never changes start order.
Detached executions, Subagents and Workflows retain their own native identities.

Native Runtime Client version 43 and App Server version 13 carry this mandatory
summary/detail vocabulary. SQLite schema 42 gates the persisted request terminal vocabulary including
generation evidence. The Event Journal envelope framing is unchanged; this is
request terminal event data, not a new Trace store.

## Server-resolved presentation relationships

Some facts a reader needs are relationships *between* records, not facts about
one record. Trace resolves them from native authority and ships them as
explicit bounded presentation facts. The browser renders these relationships;
it never discovers them from adjacency, names, timestamps, DOM state or the
currently loaded history window. None of them introduces a RuntimeEvent, a
Trace table, or a second history or cache authority.

### System Prompt state is a request-relative native fact

`TraceRequestSummary.system_prompt` carries a closed state — `initial`,
`changed`, `unchanged`, `previous_unavailable` — plus a bounded preview of the
prompt the request introduced. `unchanged` carries no preview, because the
preceding request's row already does.

The predecessor is resolved by **native durable actual-request ordering**: one
bounded, indexed, `limit = 1` seek for the nearest preceding
`ModelRequestStarted` before this request's own anchor, constrained to the same
captured read cut. It is never a Journal scan. The predecessor may belong to
the previous retry, a recovery request, a previous logical Step or a previous
Attempt; `retry_number == 0` is **not** evidence that no previous actual
request exists.

The comparison is exact historical value equality between the two requests'
frozen `RequestSnapshot.effective_system_prompt` values. Current
configuration, the current System Prompt assembly, current Agent state and
`system_sections` names are never consulted, so reconfiguring a Session cannot
rewrite an older row's classification.

`initial` therefore means native authority proved there is no earlier actual
request in this conversation. **Page boundaries cannot change the
classification**: a page that begins in the middle of history reports exactly
what a page containing the predecessor reports. `previous_unavailable` is the
honest third answer for a predecessor whose frozen state the projection cannot
establish at the captured cut; it never hides a durable read failure, which is
propagated as an error under the existing durable contract.

The complete prompt stays in `TraceRequestDetail.effective_system_prompt`
under the existing bounded detail contract. A pageable summary carries only
state and a preview, so a page of 32 requests never carries 32 prompts.

### Context introduction comes from immutable request identity

Canonical Context additions are a request-relative presentation relationship,
not a new execution event. There is no `TraceKind::Context`, no `ContextAdded`
RuntimeEvent and no fake Journal anchor.

`TraceRequestSummary.context_additions` is projected from
`RequestSnapshot.request_context_ids` — the exact ordered request-scoped
canonical context facts committed atomically with that request's start — joined
to the Message Ledger by keyed reads. Each referenced message is validated as a
canonical `InboundKind::Context` User fact; an identity the Ledger does not hold
as one is an invariant violation reported through the repository's ordinary
reconstruction error model, never silently dropped or downgraded into an
ordinary User message. The durable start transition checks structural Context
identity/order, but does not prove Context Assembly provenance/family semantics.
Trace therefore validates that semantic relationship independently.

`TraceContextKind` is a closed presentation family — `goal_status`,
`runtime_tool_observation`, `extension_environment`, `agent_status`. The
internal `ContextKind` payload does not cross: a complete `GoalSnapshot` or
Agent Status generation metadata never enters a summary.

`TraceContextPresentation.source` is a closed typed provenance —
`runtime`, or `certified_extension` carrying the **exact**
`CertifiedExtensionIdentity` the canonical message froze. The family alone
cannot name a producer: every certified extension publishes
`extension_environment`, so two extensions would otherwise collapse into one
indistinguishable provenance. Provenance is copied from the canonical
message's own `UserSource` and from nothing else — not the assembly
generation, the contributor list, the context family, message order, text or
the current extension registry.

Trace projects source and family jointly from the canonical `UserSource` and
`ContextKind`. The complete Context Assembly matrix is:

| Canonical source | Canonical kind | Trace source | Trace kind |
| --- | --- | --- | --- |
| Runtime | GoalStatus | Runtime | GoalStatus |
| Runtime | RuntimeToolObservation | Runtime | RuntimeToolObservation |
| Runtime | AgentStatus | Runtime | AgentStatus |
| Extension { contributor } | ExtensionEnvironment | CertifiedExtension { exact contributor } | ExtensionEnvironment |

Every other pair is `ConversationStoreError::InvalidReference`, including runtime
ExtensionEnvironment and extension GoalStatus, RuntimeToolObservation or
AgentStatus. A contradictory hidden canonical Context can be committed by a
low-level composition; it must never become a contradictory presentation claim.
There is no coercion, new wire variant, or Web-side semantic validation.

The extension identity is bounded by its own contract (128 bytes),
strictly below the Trace identity bound, so exact provenance always fits a
summary row whole and is never shortened into a different identity.

Order is exactly the frozen `request_context_ids` order. Nothing reorders by
timestamp, family, contributor, display name or client preference.

Because the Context Engine admits request context once, at the first successful
request start, and retries and recovery reuse admitted context rather than
admitting duplicates, the request that committed the identities is the one that
exposes them and later requests of the same Step expose none. Trace keeps no
"already displayed Context IDs" state machine; the immutable snapshot
identities are the authority.

`ModelInputMessage::RequestOnly`, including unresolved-output carryover, has no
canonical `MessageId`. It stays request detail with the `request_only` role and
is never labelled canonical Context presentation.

### Tool-owned domains keep their exact originating ToolCall

`TraceRecord.originating_tool_call_id` carries the exact `ToolCallId` copied
from `BackgroundExecutionCommitted`, `SubagentOwnershipCommitted` and
`WorkflowStarted`. One typed field serves all three; other record kinds carry
none.

The relation is never resolved from a Tool name, Agent name, Workflow name,
`native_id`, timestamp, row adjacency, Attempt/Step proximity or browser order,
so reused names cannot cross-correlate records. It remains present and exact
when the parent Tool row is outside the loaded page. It is presentation and
navigation correlation only: it changes no Tool lifecycle, ownership,
settlement or cancellation, and no Background, Subagent or Workflow lifecycle.

rustX has no native nested Tool-call execution path with parent/child call
identities, so there is no `TraceKind::Subtool`, no nested-Tool trait and no
generic Subtool framework. Subagent and Workflow are not Subtool.

### What stays out of lifecycle, and what the browser may not do

`TraceLifecycle` carries mutable lifecycle facts only. It never repeats or
mutates the immutable System Prompt or Context presentation payloads, and it
never carries `originating_tool_call_id`: these cannot have changed.

The projection owns that separation, not just the wire shape. Trace has two
read responsibilities, and neither is allowed to depend on the other:

```text
anchor      native identity, own grouping, own durable terminal    shared
  summary     + bounded preview + the relationships above          session/trace
  lifecycle   + current runtime evidence                           refresh
```

A lifecycle refresh resolves only the facts a `TraceLifecycle` transmits. It
performs no System Prompt predecessor lookup, reads no predecessor
`RequestSnapshot`, joins no canonical Context out of the Message Ledger, and
resolves no recorded Tool name. A refresh may cover up to 512 records, so
this is a correctness rule and not only a cost one: an error reached solely
while resolving a request's immutable Context presentation must not be able
to make that record's lifecycle repair unavailable, and a path that is never
entered cannot fail. Deterministic regressions assert this as an
implementation property — counters around both relationship paths, zero after
a refresh — rather than inferring it from timing.

Web renders these relationships and owns none of them. It must not compare
request details to decide whether the System Prompt changed, must not diff
request messages to decide Context introduction, and must not use adjacency or
names to correlate a domain record with a Tool call. Loaded-window search may
index the bounded semantic labels and identities these DTOs carry, but search
is never relationship authority.

## Summary and detail

`session/trace` returns `TraceRecord` summaries: stable identity, resolved native
hierarchy, lifecycle, preview, usage/timing and safe attachment references.
`session/traceDetail { target, record_id }` returns one `TraceDetail` at an exact
historical read cut. It uses the same represented-prefix rule as historical
paging and never drains observations or advances a subscription cursor.

Adopted User batches expose every retained canonical message, with each message's
identity, role and content preserved in native order.

Request detail reconstructs the provider-neutral ModelRequest through the exact
immutable RequestSnapshot and its historical Surface revision. It includes the
effective system prompt, context, frozen Tool definitions, model and limits,
reasoning configuration and explicitly allowlisted request options. Current
configuration is never a substitute for a missing historical value.

Tool detail joins the canonical Assistant ToolCall, the separate execution-start
fact, the owning request's frozen Tool schema, and the canonical ToolMessage.
Proposal assembly is not execution. Provider completion is not Assistant
acceptance. Result JSON/text, status, exit code, measured duration and artifacts
remain owned by their canonical result; Trace only projects them.

Every bound-driven omission of inspectable content marks its containing
projection partial, including oversized artifact, ToolCall, message, and frozen
Tool-definition identities. Identities are omitted whole, never shortened into
another identity. Semantics outside Trace's vocabulary, such as opaque provider
continuation state, remain intentionally unprojected.

Trace may expose filesystem paths and managed-output locators as recorded
execution facts. These values are presentation only and confer no filesystem,
execution, or recovery authority. Native storage diagnostics retain their
underlying I/O failures and paths; presentation policy does not rewrite them.
Credentials and opaque provider continuation internals remain excluded.

## Durable paging and live lifetime

`session/trace { target, before?, limit }` returns a `TracePage`. Limits are
1–32. Entries are ordered oldest to newest by server-owned durable anchor order.
`next_cursor` is an exclusive boundary for the next older page; null means the
end. The opaque string `TraceCursor` belongs to this conversation's Trace only,
remains valid across reopen while that conversation exists, and is not a live
cursor, transcript cursor, Request ID or artifact ID. Clients must not parse it.
Pages can begin/end inside an Attempt or Step; every row carries its resolved
native grouping. Pagination ends even when encoded-byte bounds reduce page size.

The SQLite seam performs bounded indexed seeks on existing Journal rows, then
exact immutable snapshot/Ledger joins. Each historical page captures its own fixed durable frontier;
all event joins exclude later facts. No page scans the entire Journal, Ledger or
snapshot collection. Expression indexes accelerate existing rows and introduce
no semantic authority. Reads do not advance the Runtime Client subscription
cursor, change revisions, consume inbound work or settle/cancel anything.

The ordinary attach/snapshot response includes a bounded newest Trace window.
Its linearization point is `RuntimeClientProjection::snapshot_cut()` under the
host projection mutex, after draining pending observations. The structured cut
captures the snapshot, Runtime Client cursor and represented Journal prefix.
Trace queries run **after releasing that mutex**, bounded by that captured prefix.
They never obtain an independent latest SQLite frontier.

The store's serialized connection guard stages commit receipts under the SQLite
serialization lock. Staging never publishes a Runtime Client cursor. For every
Trace-affecting semantic fact, the queue retains only its unacknowledged sequence.
The native owner installs hot state, then publishes its existing observation with
that exact receipt: Agent events, canonical messages/Tool commits, Background and
Subagent snapshots, manual compaction, interaction audit observations, and native
Workflow cuts. Workflow delivery carries the retained native revision chain,
not just its latest replacement, so ordinary progress does not manufacture a
revision gap/resync. No owner samples a later durable maximum.

The queue releases one semantic observation batch only after every staged
Trace-affecting receipt has been acknowledged. Thus a later owner's publication
cannot advance the durable prefix past an earlier uninstalled transition. Under
the host mutex the batch folds native state, publishes the ordinary invalidations,
and advances the represented prefix. `snapshot_cut()` then copies all four: native
state, cursor, Trace frontier, and the lifecycle evidence used by `trace_updates`.
Materialization runs outside that mutex. Runtime owners never wait for Trace.
The projection worker owns only read-model state, the pending queue and the native
Workflow read model. It cannot retain the host, runtime or durable storage lease,
even while actively folding; native resource release does not wait for a reader.

The closed classification in `runtime::observation` gates Attempt/Turn/request,
Assistant/Tool message and execution, compaction, Background, Subagent terminal
and ownership, Workflow run lifecycle, and interaction facts. Other Journal facts
(Goal, execution control/progress, workspace and Workflow node/block
and value audits) are not consumed by Trace and never independently advance its
frontier or allocate a cursor. There is no generic `JournalCommitted` publication
and no separate audit-only Trace stream. A later represented Trace prefix may
include those ignored rows, but Trace cannot expose them.

Bootstrap remains under inactive runtime ownership: the coordinator freezes the
semantic seed, installs the store staging observer and captures the initial prefix,
then installs the native observers before activation. Only Runtime Client
composition installs receipt staging; native/headless observation consumers retain
their ordinary semantic delivery and never consume Trace batches. No live transition can enter
between the seed and its first observation. Store errors fence the read model;
Trace remains irrelevant to execution, settlement and recovery.

Historical `session/trace` is an independent read: on a live host it captures
one represented semantic prefix and its native lifecycle projection without
draining observations or moving the live cursor. Thus historical pages cannot
expose an unpublished terminal that the next snapshot repair would retract.
Inactive durable inspection instead captures its own SQLite frontier; it has no
live publication boundary and receives no live lifecycle overlay: a durable start
alone remains incomplete. Paging cursors remain Trace-specific in both cases.
Exact positive lifecycle evidence applies regardless of anchor age. For loaded
records outside the newest tail, `session/snapshot` accepts at most 512 opaque
`trace_records` positions and returns `snapshot.trace_updates`, resolved at the
**same snapshot cut**. These bounded typed patches carry lifecycle/timing, safe
request usage/failure details and canonical artifact references (at most 1 KiB
per update; oversized optional references are omitted with `truncated`); no arbitrary
payload or raw event is exposed. Background execution ID, Subagent ID, structured
Workflow run ID, and the existing exact Attempt/request/Tool/interaction identities
are the only correlations. Durable terminal facts win; absent native evidence
stays incomplete. No in-flight duration is synthesized.

The browser has a separate Trace cache (512 history entries / 4 MiB estimated
encoded UTF-16 size), independent from transcript and live cursors. Its flat list
represents one contiguous server-proven interval. An overlapping newest tail
updates that interval by stable identity. A tail without overlap rebases to its
own interval, cursor and a new epoch; it never concatenates disconnected ranges.
Connection generation, full attachment target and Trace epoch fence older replies.
Paging after rebase starts from the new tail's cursor.

At most one selected entry is retained separately from history. It stays
inspector-visible after a no-overlap rebase and receives server lifecycle patches
through the existing bounded interests request. It is not inserted into the
historical list. Selection receives first priority within the 512-interest bound.
Paging completion requests another repair so settlement while an older read was
pending cannot leave newly loaded entries stale. Reconnect/reattach/resync and
explicit latest replace the domain, including retained selection. Notifications
remain invalidation signals; the browser never folds Journal facts.

Canonical Tool attachments merge Image/File content and result artifact references,
deduplicated by ArtifactId. Canonical typing, never filename or extension, selects
the renderer. Managed-output inspection exposes completeness/availability and
bounded diagnostics, plus the exact recorded continuation locator when present.

## Durable schema contract

SQLite schema **42** retains the indexed Trace presentation seeks. Older stores
are rejected without migration. Reads use existing indexed Journal rows and
immutable native joins, with no Trace persistence table.

## Truthful timing and bounds

Durations use two authoritative timestamps, never receipt/render/reconnect time.
An in-flight or incomplete record has no duration. Request usage comes only from
the exact request's terminal fact; missing usage stays unavailable. Historical
model metadata comes from its immutable snapshot, never current configuration.

The typed inspection allowlist exposes authorized Session/model-visible content:
system prompt, context, Tool schemas/arguments/results, canonical user/assistant
content and reasoning. It excludes credentials, authorization headers, process
or executor secret environment, provider continuation state, internal storage
objects, synchronization objects and arbitrary Debug
dumps. Request options use a closed list of sampling/decoding keys; omitted
options are counted. This is a field contract, not heuristic secret scanning.

Bounds are explicit: previews 512 UTF-8 bytes; detail text 16 KiB; 32 content
blocks per message; 64 request messages and 64 definitions; JSON depth 12,
1,024 visited nodes and 4 KiB string leaves. One request summary carries at
most 16 Context presentation facts, 4 artifact references each, and 4 KiB
encoded for the whole list. Trace owns that encoded bound: an upstream Context
Assembly limit constrains how much context a request may carry and says nothing
about the bytes that context becomes once identities, previews and artifact
references are projected. Truncation retains a deterministic prefix in frozen
order, surfaces `context_truncated`, and releases an entry's content before its
identity, so an adversarial fact can never erase the identity it names. Oversized object keys are omitted
rather than aliased by shortening. Identities over 512 bytes are omitted whole.
Summary records are at most 8 KiB, page records 128 KiB and one detail 512 KiB
encoded JSON. Every omitted or shortened value carries a truncation indication.

### Generation clock contract

Every phase boundary drawn in Trajectory must have native evidence in the same
request timeline domain. The Agent Loop owns that evidence; Trace only projects
it and the browser only renders it.

| Boundary or metric | Exact meaning |
| --- | --- |
| Durable request start | The UTC timestamp supplied to `commit_model_turn_start`, recorded atomically with the immutable Request Snapshot and `ModelRequestStarted` |
| Dispatch frontier | Monotonic reading immediately before entering the actual adapter dispatch, after durable commit and request reconstruction/verification |
| First / last output | First / last non-empty provider-independent normalized text, reasoning, refusal or Tool-call output observed by the execution owner |
| Provider terminal | Observed normalized completion/failure; runtime failures without a provider terminal use the native failure-settlement boundary |
| Canonical Assistant acceptance | Separate later canonical message commit; provider completion never proves acceptance |
| Request duration | Paired durable-start origin → provider terminal, including preparation, measured monotonically |
| TTFT | Adapter dispatch → first output, excluding preparation |
| Generation duration | First output → provider terminal |
| Throughput | Reported output tokens / generation seconds; requires usage, first output, terminal and a positive interval |

Inside cancellation/start arbitration, immediately before the start transaction,
the Agent Loop samples monotonic time and UTC as one deliberate origin pair at
millisecond precision. That exact UTC value is passed to the transaction. The
successful commit linearizes the start fact's existence; its supplied timestamp
is its timing coordinate (not a later transaction-return timestamp). The paired
monotonic origin is retained only for a fresh successful commit, never for an
idempotent historical receipt. No provider dispatch moves before that commit;
cancellation arbitration, snapshot/start identity and atomicity are unchanged.

At dispatch, execution measures `dispatch_after_start_ms` from that retained
origin. `GenerationEvidence` settles this bridge plus first/last/terminal offsets
from dispatch in the exact request terminal event. The provider terminal offset
is captured on observation, so EOF, publication and Journal append delays cannot
extend generation. Each retry owns a new accumulator. There are no per-delta
Journal events, absolute timestamp streams, Trace tables or recovery inputs.

`TraceGeneration.timeline` projects dispatch, first/last output and terminal
as offsets from the paired request start. The duration timeline anchors these
coordinates at `TraceTiming.started_at`; it does **not** rescale them to the
independent Journal UTC start/end span. `TraceTiming.duration_ms` remains that
Journal wall duration, including terminal recording delay and any wall-clock
adjustment, and is labelled separately in Inspector. No current clock participates
in reopening or projection. Equal-width sequence mode does not paint duration
phase boundaries. Missing bridge evidence leaves the wall span unsplit even if
numeric TTFT/generation metrics exist; missing output never acquires a boundary.

For example, preparation 400 ms + dispatch-origin TTFT 320 ms + generation
1280 ms yields request-relative dispatch 400 ms, first output 720 ms and terminal
2000 ms. Multiplying the Journal wall span by `320 / 1600` is prohibited.

**Deliberate Harness deviation:** the pinned Harness `TrajectoryTable.tsx`
derives TTFT as `firstTokenTime - stepStartTime`. rustX retains its native
**adapter dispatch → first provider-independent output** contract because its
durable request-start/reconstruction lifecycle precedes actual dispatch. These
metric definitions are not identical, even though the overview presentation is
adapted from Harness.

The browser retains at most eight detail responses. A server lifecycle repair
invalidates affected payloads and pending detail reads; the selected inspector
refetches against the new cut. Connection generation, attachment target, Trace
epoch and pending-read identity fence asynchronous completion. Reattach/resync
replaces the cache. Selection is presentation state, never native authority.

## Presentation and deliberate exclusions

Trajectory adapts the pinned Harness timing lanes, dense ledger, folding,
selection/inspector, loaded-window search and virtual scrolling patterns to native
Trace props. Attempt and Step folds do not change native state. Fixed-height
virtual rows preserve reader anchors as payloads change; end anchoring follows
new rows only when the reader remains at the tail. Chat/Trajectory is local view
state on the same attachment. Chat transcript paging and the developer raw
JSON-RPC inspector remain separate.

See [Web provenance](../web-console/PROVENANCE.md) and its existing source
inventory for inspected versus rewritten upstream paths and MIT notices.
Harness Session Controller, event assembly, Host/Remote, Cordis lifecycle,
provider/workspace authority and dynamic view ownership are excluded. There is
no TUI Trajectory, server search, telemetry platform, new recovery behavior or
compatibility mode. This view deliberately omits unaccepted publication bodies,
Harness-only semantics lacking native facts. Interaction settlement remains in the existing Chat controls.

See [Session-owned workspace uploads](session-uploads.md) for receipt admission, model paths, fork copies and durable cleanup.

The inspector uses entity-specific Summary/Input/Result/Schema/Usage/Timing and
attachment views, Markdown, the audited Harness JSON tree and code primitives.
Native Bash command content has a shell contract; Write content has source text
but no inferred language. Unknown third-party Tool contracts get structured JSON.
The timing overview supports linked selection, interval focus, zoom and pan.
Two recorded endpoints define spans; a start alone remains a marker.

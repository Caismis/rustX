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

Native Runtime Client version 41 and App Server version 9 carry this mandatory
summary/detail vocabulary. SQLite schema 40 remains the store layout; generation
evidence is request terminal event data, not a new Trace store.

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
bounded diagnostics, excluding its physical continuation locator.

## Durable schema contract

SQLite schema **40** retains the indexed Trace presentation seeks. Older stores
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
or executor secret environment, provider continuation state, storage internals,
physical managed-output locators, synchronization objects and arbitrary Debug
dumps. Request options use a closed list of sampling/decoding keys; omitted
options are counted. This is a field contract, not heuristic secret scanning.

Bounds are explicit: previews 512 UTF-8 bytes; detail text 16 KiB; 32 content
blocks per message; 64 request messages and 64 definitions; JSON depth 12,
1,024 visited nodes and 4 KiB string leaves. Oversized object keys are omitted
rather than aliased by shortening. Identities over 512 bytes are omitted whole.
Summary records are at most 8 KiB, page records 128 KiB and one detail 512 KiB
encoded JSON. Every omitted or shortened value carries a truncation indication.

GenerationTiming observes normalized model output from a request-local monotonic
dispatch origin. Empty deltas, framing and usage updates do not start TTFT.
The request terminal commits first-output, last-output and terminal offsets with
usage; no per-delta Journal events are added. TTFT is first output; generation
is first output through terminal; throughput requires output usage and a positive
generation interval. Missing evidence stays unavailable, including after reopen.

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

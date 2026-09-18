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

The separate native Runtime Client envelopes also advance from version 34 to 35
because their mandatory snapshot and invalidation vocabulary changed. App Server
clients negotiate only App Server version 3. Neither boundary accepts its obsolete
version.

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
(Goal, adoption, execution control/progress, workspace and Workflow node/block
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

Canonical Tool attachments merge Image/File references in `result.content` first,
then `result.artifacts`, deduplicated by ArtifactId. The first canonical occurrence
owns image/file typing; filename and path never determine type. Each source is
visited for at most eight blocks, the combined result contains at most eight
identities, and omitted blocks set `truncated`. File display metadata, raw text,
JSON and physical paths remain withheld. The existing artifact carrier is reused.

## Durable schema contract

SQLite schema **36** retains the Trace index contract introduced in schema 35. Native Trace historical
projection requires the fixed Event Journal presentation indexes. Older stores
are rejected, with no migration, lazy index installation or compatible reader.
Current-version stores missing or redefining any required index are also rejected.
The nine indexes cover kind, Attempt, Step, request ID, scoped ToolCall, execution
ID, Subagent ID, Workflow run ID and interaction ID, each ending in kind/sequence
where applicable. The finite query seam explicitly selects the appropriate index
and performs one bounded equality/range seek per allowlisted event kind; tests
inspect `EXPLAIN QUERY PLAN` for the actual reader SQL in both directions.
SQLite schema 40, native Runtime Client version 38 and App Server version 6 are
independent version domains, despite the first two currently sharing a number.

## Truthful timing and bounds

Durations use two authoritative timestamps, never receipt/render/reconnect time.
An in-flight or incomplete record has no duration. Request usage comes only from
the exact request's terminal fact; missing usage stays unavailable. Historical
model metadata comes from its immutable snapshot, never current configuration.

The projection allowlist excludes provider/MCP credentials, request parameters,
executor environment, storage/workspace paths, synchronization internals, raw
Rust Debug values, and raw snapshot/event structs. Arbitrary system prompts,
context input, Tool schemas, arguments and textual/JSON Tool results are currently
**withheld in full**, marked `redacted`. The UI shows this policy explicitly; it
does not substitute current settings or pretend the input is empty. This is a
field policy, not heuristic secret scanning. Already-authorized canonical
Assistant text/reasoning is bounded; provider reasoning continuation is omitted.
Canonical user-authored/model-authored content retains its existing content
permissions; Trace does not claim to scrub secrets a user explicitly put there.

Text is at most 2,048 UTF-8 bytes, cut on character boundaries. At most eight
canonical content blocks contribute per message. Oversized native identities
(over 512 bytes) are omitted, never shortened into a different identity. Entry
payloads are capped at 32 KiB encoded JSON, pages at 128 KiB of entries. Explicit
`truncated` flags distinguish partial content from complete content. Artifact IDs
reuse the existing read limits, containment checks and object-URL cleanup.

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
raw request/Tool payload inspection and Harness-only semantics lacking native
facts. Interaction settlement remains in the existing Chat controls.

See [Session-owned workspace uploads](session-uploads.md) for receipt admission, model paths, fork copies and durable cleanup.

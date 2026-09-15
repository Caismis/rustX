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
exact immutable snapshot/Ledger joins. Each read captures a durable frontier;
all event joins exclude later facts. No page scans the entire Journal, Ledger or
snapshot collection. Expression indexes accelerate existing rows and introduce
no semantic authority. Reads do not advance the Runtime Client subscription
cursor, change revisions, consume inbound work or settle/cancel anything.

The ordinary attach/snapshot response includes a bounded newest Trace window,
with current runtime labels where the owner positively proves them. The existing
`session/event` notifications invalidate the Web snapshot; request-only changes
use payloadless `trace_changed`. No raw event payload is exposed for this purpose.
The newest snapshot repairs Trace after reconnect/resync just as after continuous
observation. Historical page reads return durable evidence, without pretending
an unresolved historical start proves a live executor.

The browser has a separate Trace cache (512 entries / 4 MiB estimated encoded
UTF-16 size), independent from transcript and live cursors. Connection generation,
full attachment target and Trace epoch fence older responses. Newest snapshot
facts replace overlapping rows; gaps, bounds or unresolved rows outside the new
tail cause deterministic replacement. Reconnect/reattach/resync replaces the
window. Older history can be fetched again. Selection uses stable Trace IDs;
removal reports that the selected record left the loaded window.

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

# App Server protocol v1

The App Server protocol is rustX's public client boundary for the TUI,
Developer Web Console, future Web UI, and SDKs. Rust DTOs in
`src/app_server/protocol.rs` are authoritative. `AppServerConnection` in
`src/app_server/connection.rs` is the transport-neutral semantic endpoint.

```text
one client connection → AppServerConnection (initialize once)
                           ├→ SessionController: durable Sessions and graph
                           └→ SessionRuntimeManager: live incarnations
                                 ├→ Session A: host → ConversationRuntime
                                 └→ Session B: host → ConversationRuntime
```

The host's projection, attachment admission, replay ring and native controls
are reused. Clients do not initialize or wrap an inner Runtime Client protocol.
The existing local TUI stdio endpoint and handwritten `tui/src/protocol/types.ts`
remain tied to the current TUI application until #290; they are not an App
Server schema or a second supported App Server client contract.

## JSON-RPC and initialization

Requests use JSON-RPC 2.0: `jsonrpc: "2.0"`, a string or integer `id`, a `method`,
and typed `params`. Responses contain the same ID and exactly one of `result`
or `error`. Notifications have no ID. IDs correlate requests only; repeating
an ID does not deduplicate a mutation. Integer correlation IDs must fit the
JavaScript safe integer range. String IDs are recommended for arbitrary IDs.

```json
{"jsonrpc":"2.0","id":"init-1","method":"initialize","params":{"protocol_version":1,"client":{"name":"example","version":"1"},"presentation":{"images":true,"questionnaires":true,"reviews":true}}}
```

`APP_SERVER_PROTOCOL_VERSION` is independent of crate, manifest, journal,
event, and local TUI stdio versions. Unsupported versions fail; there is no
downgrade. Initialization stores bounded client diagnostics and presentation
capabilities once per connection. Presentation capabilities do not change
authorization, canonical content, interaction admission, or execution.
Server capabilities advertise multiplexing, single-controller admission,
headless interactions and an explicit experimental-method list (currently empty).

Parse, envelope, method and parameter errors use JSON-RPC codes -32700,
-32600, -32601 and -32602. Domain failures use -32000 with closed typed data.
Internal storage/provider details are not reflected into arbitrary wire errors.
Errors with unknown correlation use a null ID. Client notifications receive
no response and cannot invoke request-only mutations. Batch requests are not
supported in v1; pipeline individual requests instead. This limitation is
explicitly rejected as an invalid request before any action occurs.

## Methods and native owners

| Methods | Owner and semantics |
| --- | --- |
| `initialize`, `server/info` | Connection negotiation and server capabilities |
| `session/list`, `session/read`, `session/name`, `session/tree` | Durable controller; bounded pages; no runtime composition |
| `session/create` | Durable creation from explicit Session selections |
| `session/fork`, `session/branch` | Exact native Surface revision and optional user-message boundary; fork without a boundary clones the revision into an independent Session |
| `session/deletePreview`, `session/delete`, `session/recoverDeletion` | Native revision-confirmed deletion/recovery; no client-supplied cleanup workset |
| `session/attach`, `session/detach` | Load/reuse a runtime and acquire/release its external control attachment |
| `session/unload` | Explicit incarnation-checked native shutdown/unload; waits for settlement, preserves durable Session state |
| `session/snapshot`, `session/subscribe`, `session/transcript` | Authoritative projection, bounded replay and durable transcript pages |
| `turn/start`, `turn/steer`, `turn/cancel` | Native inbound and attempt-cancellation owners; acceptance is not terminal execution |
| `interaction/respond`, `interaction/cancel` | Originating runtime/coordinator, including routed child interactions |
| `settings/read`, `settings/replace` | Explicit durable Session selections with revision CAS; cold composition consumes them |
| `settings/model`, `settings/models`, `settings/setModel`, `settings/setApprovalMode` | Native live settings; admitted work keeps its frozen values |
| `settings/defaults`, `settings/saveDefault` | Bound user-default document owner; revision-checked source writes are distinct from live settings |
| `resources/read`, `resources/reload` | Native capability/resource inspection and generation publication |
| `context/compact`, `goal/control` | Existing maintenance and Goal owners |
| `background/status`, `background/cancel` | Existing background execution registry |
| `subagent/status`, `subagent/cancel`, `subagent/disposeWorkspace` | Existing child/resource owner; no caller-supplied filesystem cleanup paths |

`turn/start` and `turn/steer` both submit native inbound content. The runtime
decides whether it starts a fresh attempt or joins the inbound queue of the
current attempt. Neither method creates a separate execution state machine.
Callers learn accepted message identity and inbound sequence from the result.

The connection admits at most 32 active **plus reserved** attachments. A short
routing-lock transaction reserves capacity before `load` can compose a cold
Session. Success commits that reservation to a route; error or cancellation
drops the reservation. A full connection therefore cannot make a rejected
Session resident. Its map is locked only for local routing changes; no lock
spans composition, provider work, interaction settlement or shutdown. Requests
may run concurrently and finish out of order.
One notification consumer uses bounded fan-in over native subscriptions;
there are no per-Session pump tasks or copied event queues.

## Identity and stale control

Each attached request carries `AttachmentTarget`: Session ID, Conversation ID,
runtime incarnation and attachment ID. Each notification repeats that full
target. A connection cannot use another connection's attachment, or silently
redirect an old target to a replacement runtime.

Session IDs identify durable catalog objects; SessionNode IDs identify graph
nodes; Conversation IDs identify linear durable histories. Attempt IDs identify
runtime executions. InteractionRef includes its originating Conversation and
Interaction ID. Attachment IDs identify one host admission. Incarnation IDs
identify one process-local composition. Request IDs belong only to correlation.
None of these are projection cursors or aliases for each other.

After exact attachment routing, the manager admits every live-runtime read or
control operation under its registry lock, requiring the same Loaded incarnation
and incrementing its in-flight count. Explicit unload compares that incarnation
and claims `Loaded -> Unloading` under **the same lock**. There is one ordering:
an operation admitted first may finish; an unload claim first rejects the
operation as `StaleRuntime`, without invoking its native owner. No control path
reacquires a different runtime on behalf of an old attachment.

An admitted operation runs in a server-owned task with a private, non-cloneable
operation lease. The requesting future receives only a one-shot reply channel;
abandoning or retaining that future cannot retain the lease. Lease destruction
releases strong resident ownership before decrementing the in-flight count.
Unload waits for that count to reach zero, then performs native shutdown,
joins the projection worker, and releases the composition/allocation. No
registry lock is held across any of this asynchronous work. The gate is
subordinate to registry residency, not a second runtime state machine.

Successful `session/unload` removes the exact initiating route and releases its
attachment capacity **before returning success**, even if the requester stops
polling. Its response is the initiating request's authoritative terminal
acknowledgement. Notification consumption is not a resource-release point;
there is no synthetic durable closed-event queue. An already waiting observer
may receive `session/closed` for the old target, but cannot remove a newly
installed route.

## Attachment and observation lifetime

Protocol v1 admits at most one writable external controller per resident
Conversation. A second controller gets a deterministic rejection and cannot
steal the first. Detach and connection destruction release external admission
only. They do not cancel a turn, settle a pending interaction, unload a runtime,
delete a Session, or shut down the process.

`RuntimeAttachment`, `EventSubscription`, and the local single-runtime endpoint
hold weak host references. The resident composition owns the host and live
resource graph. A successful unload can release that graph and native
allocation locks while stale handles remain alive. Only manager-admitted
server operations may temporarily own that graph; unload drains them first.
Notification waiting never takes an operation lease or retains the host across
an await. Those stale handles fail or return
Closed; they cannot resurrect the host or observe the next incarnation.
Unload joins the projection worker after native execution shutdown, so a final
in-flight observation fold cannot outlive successful resource release.
Removing/destroying a projection subscription wakes parked receivers.

Attach drains pending observations, captures the snapshot/cursor, admits the
controller and registers its subscription under the host's single projection
lock. An observation concurrent with this cut is either already reflected or
arrives after that cursor. The connection never reads a snapshot and then
independently starts subscribing. Cursors are monotonic within one incarnation;
the replay ring is bounded and lag is reported as `session/resyncRequired`.
Clients obtain a fresh snapshot and subscribe after its cursor to repair.
Invalid re-subscription leaves the prior registration intact. Transcript cursors
are a separate durable paging domain, not event cursors.

Approval and Questionnaire publication availability follows the bound runtime,
not controller presence. A loaded runtime may publish and retain a pending
interaction with zero external clients. Reattachment observes it through the
authoritative snapshot. Only the originating InteractionCoordinator selects a
terminal response/cancel winner, validates the response and commits its audit.
Runtime unload retains the existing explicit shutdown/cancellation semantics.

## Generation and validation

### Exact integer wire domains

App Server envelopes encode opaque `u64` identities, sequences and revisions as
canonical unsigned decimal **strings**, bounded to `0..=18446744073709551615`.
For example, both an incarnation and a cursor can carry `"9007199254740993"`
without JavaScript rounding. Their generated domains remain separately named:

```ts
type RuntimeIncarnationId = string;
type RuntimeClientCursor = string;
```

This covers runtime incarnation, event and transcript cursors, Surface revision,
settings/approval CAS revisions, capability and resource revisions, Goal
references, Workflow revision and run invocation, candidate version, child
observation revision, compaction generation, inbound sequence, and Todo IDs,
dependencies and allocator position. Natural string identities and default
document/deletion revision strings remain strings. These are distinct domains,
not interchangeable tokens; do not compare decimal strings lexicographically
for numerical order (use `BigInt` locally if necessary, never JSON bigint).

Page offsets/limits, collection and omission counts, option indices, byte and
token counts, durations/retry hints, and Workflow capacity counters remain JSON
numbers, bounded at the App Server codec to the safe integer range. Native
`u16`/`u32` protocol versions, turns, budgets and visit/iteration indices are
intrinsically safe. Signed request IDs are restricted to the safe range; error
and process exit codes are `i32`. Questionnaire exact integers/binary64 retain
their existing lossless string formats. Timestamps remain RFC3339 strings.
Arbitrary user/tool JSON payloads are data, not typed control identity domains.

`src/app_server/wire.rs` applies these rules at the Request/Response/Notification
envelope boundary. Rust/Schemars field types and explicit quantity bounds drive
both serde conversion and the public `schema::protocol_schema()` generator;
there is no second field-name or method-name mapping. Native Rust domain types,
storage and local TUI stdio representations are unchanged. Encode/decode complete
App Server envelopes and use the public schema generator, not a native helper
DTO's standalone serde/schema representation.

### Reproducible generation

Generated client-neutral artifacts are in `protocol/app-server/`:

- `v1.schema.json`: complete JSON Schema generated with Schemars from Rust DTOs.
- `v1.ts`: TypeScript generated from that schema using pinned
  `json-schema-to-typescript` and its committed pnpm lockfile.
- `fixtures.json`: serialized Rust messages, including nulls, string/numeric
  request IDs, timestamps, exact domains above 2^53 and lossless Questionnaire
  numeric encodings.
- `fixtures.ts`: those exact messages consumed with `satisfies ProtocolMessage[]`.

Install once with `pnpm install --frozen-lockfile` in that directory. The single
generation command is:

```bash
cd protocol/app-server
pnpm generate
```

`pnpm check` regenerates and fails on artifact drift; `pnpm typecheck` checks the
Rust-produced client fixtures. CI runs both. Rust conformance tests round-trip
the wire messages and validate them against the generated schema.
Questionnaire `FiniteNumber` uses canonical hexadecimal binary64 strings and
`ExactInteger` uses canonical decimal strings, preserving values beyond JS's
safe integer range. They are not generated as JavaScript numbers.

Deterministic App Server tests live under `tests/scripted/app_server/`. Native
provider gates, projection gates and coordinator terminal gates prove overlap,
snapshot/event ordering and response/cancel races. Manager admission/drain
gates prove both operation/unload orders, including abandoned requests.
Composition gates prove transactional final-slot admission and no composition
on capacity rejection; unload capacity tests never poll notifications.
Every exact numeric schema leaf is tested at 0, above 2^53, and u64::MAX, with
noncanonical/overflow/numeric representations rejected. Weak owner probes and native
deletion preflight prove resource release, rather than inferring it from elapsed
time. Timeouts serve only as liveness guards.

## Follow-up boundaries

#36 supplies listener, framing, authentication and transport backpressure.
#289 supplies the Developer Web Console. #290 migrates the TUI application and
its current local stdio transport to this protocol. #291 supplies residency
policy, quotas, idle eviction and graceful process shutdown. None of those
products or policies is implemented by this semantic endpoint.

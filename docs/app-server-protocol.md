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

The connection retains at most 32 attachments. Its map is locked only for
local routing changes; no lock spans composition, provider work, interaction
settlement or shutdown. Requests may run concurrently and finish out of order.
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

The manager validates that an attached incarnation remains Loaded. Explicit
unload compares the expected incarnation and claims Unloading under the same
registry lock. Stale unload cannot drain a replacement. Native admission still
arbitrates operations racing shutdown; no control path reacquires a different
runtime on behalf of an old attachment.

## Attachment and observation lifetime

Protocol v1 admits at most one writable external controller per resident
Conversation. A second controller gets a deterministic rejection and cannot
steal the first. Detach and connection destruction release external admission
only. They do not cancel a turn, settle a pending interaction, unload a runtime,
delete a Session, or shut down the process.

`RuntimeAttachment`, `EventSubscription`, and the local single-runtime endpoint
hold weak host references. The resident composition owns the host and live
resource graph. A successful unload can release that graph and native
allocation locks while stale handles remain alive. Those handles fail or return
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

Generated client-neutral artifacts are in `protocol/app-server/`:

- `v1.schema.json`: complete JSON Schema generated with Schemars from Rust DTOs.
- `v1.ts`: TypeScript generated from that schema using pinned
  `json-schema-to-typescript` and its committed pnpm lockfile.
- `fixtures.json`: serialized Rust messages, including nulls, string/numeric
  request IDs, timestamps and lossless Questionnaire numeric encodings.
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
snapshot/event ordering and response/cancel races. Weak owner probes and native
deletion preflight prove resource release, rather than inferring it from elapsed
time. Timeouts serve only as liveness guards.

## Follow-up boundaries

#36 supplies listener, framing, authentication and transport backpressure.
#289 supplies the Developer Web Console. #290 migrates the TUI application and
its current local stdio transport to this protocol. #291 supplies residency
policy, quotas, idle eviction and graceful process shutdown. None of those
products or policies is implemented by this semantic endpoint.

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

## One protocol, multiple transports

#284's topology unifies semantics, not transports. #288 owns the versioned
JSON-RPC DTOs, transport-neutral endpoint, generated client schemas and direct
semantic conformance. #36 binds the same `AppServerConnection` endpoint to
concrete stdio JSONL and WebSocket transports. Neither transport defines Session,
execution, interaction, cancellation, history, residency, or replay semantics.

After #290, ordinary local TUI mode uses first-class App Server stdio JSONL to a
TUI-owned child; browser and existing/remote TUI clients use WebSocket to an
externally managed server. The ordinary local TUI does not need a loopback
WebSocket merely to consume the unified App Server protocol. This App Server stdio
binding is not the old Runtime Client wire protocol currently carried by the TUI's
pipes: #290 replaces those temporary semantics with the generated App Server DTOs.

### Connection lifetime versus process ownership

For either transport, connection EOF/close releases that connection's external
attachments. It does not fabricate turn cancellation, interaction settlement,
Session unload/delete, or server shutdown. Explicit detach has the same separation.

In the planned local self-hosted mode, `rustx-tui` owns
`rustx app-server --listen stdio`. Normal TUI exit explicitly shuts down that child
through the process lifecycle seam. This is owner-driven process shutdown,
not transport EOF semantically cancelling work. Persistent execution across TUI
exit requires an externally managed App Server; WebSocket/existing-server TUI
disconnect never shuts down that process. The standalone entry point and explicit SIGINT/SIGTERM transport shutdown seam
are implemented. #291 will add graceful runtime drain/residency governance.

## Standalone startup

One process represents one user environment. Bootstrap binds the canonical user
TOML, validates the selected model catalog, opens one durable `SessionController`,
and creates one `SessionRuntimeManager` before accepting transport traffic. There
is no global active Session and no fixed Conversation composed at launch.

```sh
# Canonical user source: $XDG_CONFIG_HOME/rustx/settings.toml,
# defaulting to ~/.config/rustx/settings.toml. Use rustx init to author it.
rustx app-server --runtime-root /private/user/rustx-state --listen stdio

# Omit --runtime-root to use the user TOML binding, otherwise the default is
# $XDG_STATE_HOME/rustx/app-server (default ~/.local/state/rustx/app-server).
rustx app-server --listen ws://127.0.0.1:8080 --token-file /private/user/socket-token
```

`--models <models.toml>` and `--runtime-root <path>` override source bindings using
the existing configuration resolver. Relative CLI paths resolve at launch; paths
authored in user settings resolve relative to that document. `--config` is
intentionally absent here: in ordinary `rustx` it selects a Session/project override,
not the canonical user source. Use the XDG user configuration binding for the server.
Session cwd and optional project configuration come from `session/create` settings;
launch cwd is never substituted for Session cwd. Sessions retain the existing trust
and configuration admission rules. Neither cwd nor transport authentication is a sandbox.

Exactly one transport is selected. `ws://IP:PORT` accepts numeric IPv4/IPv6 socket
addresses, including port 0 for host-assigned ports. The server advertises the bound
address on stderr only after bootstrap succeeds. Stdio readiness is the response to
`initialize`; no banner is emitted. Stdio requires pipes on stdin/stdout, as supplied
by a child-process launcher. The command does not daemonize or reconnect orphaned pipes.
The existing ordinary `rustx` Runtime Client and internal `--subagent-child` paths
remain in use until #290.

## Transport framing and admission

Stdio reads one UTF-8 JSON-RPC message per LF-terminated record and writes one response
or notification per LF-terminated record. LF is excluded from the payload limit;
a CR before LF counts toward that limit and is ordinary JSON whitespace. EOF at a
record boundary and broken pipe close the connection. Unterminated EOF, invalid UTF-8,
and oversized records fail the transport before dispatch. Malformed JSON and invalid
JSON-RPC instead receive the existing protocol error envelopes, and the connection
remains usable. All diagnostics go to stderr.

WebSocket uses `tokio-tungstenite` 0.30 without TLS or compression features. One
complete text message is one semantic request. The library reassembles fragments
before dispatch, validates UTF-8, enforces frame/message bounds, and handles control
frames. Binary messages, oversized messages and invalid framing terminate that client.
JSON/JSON-RPC errors behave exactly as on stdio. No binary protocol, multiplexed
subprotocol, REST API, or alternative Session API exists.

### Dedicated WebSocket credential

WebSocket requires `--token-file`: 43–128 base64url characters, with at most one
trailing LF (129 file bytes maximum). Generate at least 32 random bytes; keep the
file user-private. For example:

```sh
umask 077
python3 -c 'import secrets; print(secrets.token_urlsafe(32))' > /private/user/socket-token
```

A browser can supply the credential in its handshake without arbitrary headers:

```js
const socket = new WebSocket("ws://127.0.0.1:8080/", [
  "rustx.app-server.v1",
  `rustx-token.${dedicatedTransportToken}`,
]);
```

The server requires both offers on path `/` without a query, rejects failed admission
with HTTP 401, and selects only `rustx.app-server.v1` in its response. It never echoes
the credential. Admission completes before constructing `AppServerConnection`, so
unauthenticated clients cannot initialize or invoke any method. This is a dedicated
single-user transport secret, never a provider key, MCP secret, or runtime credential.
It is captured at startup; rotation requires restarting the process.

The trusted host supplies this token to authorized clients. It owns secure token
delivery/storage, redaction of handshake headers in proxy logs, browser origin/CSP
policy, TLS termination (`wss://` externally), network access, OS identity/isolation,
and process supervision. Use secure termination for non-loopback deployments. All
admitted clients act within the same user environment; rustX adds no accounts,
tenancy, workspace ACLs, or authentication inside `initialize`.

### Finite delivery policy

| Bound | Value | Terminal behavior |
| --- | --- | --- |
| Inbound and outbound JSON payload | 1,048,576 bytes | Close on excess; never dispatch partial input |
| Stdio read buffer | 8 KiB | Incremental bounded record assembly |
| Concurrent request futures per connection | 16 | Close if another message arrives while full |
| Outbound queue per connection | 32 messages | `try_send` failure closes immediately |
| One outbound write including flush | 10 seconds | Drop writer future and close |
| WebSocket frame and assembled text message | 1,048,576 bytes each | Library rejects excess before dispatch |
| WebSocket read buffer | 128 KiB | Fixed library buffer |
| WebSocket write buffer | Flush immediately; maximum 1 MiB + 1 KiB | Fail on excess |
| WebSocket clients, including pending handshakes | 32 | Drop additional accepted sockets |
| WebSocket handshake | 5 seconds | Drop incomplete socket |
| HTTP handshake parsing | Library bounds: 64 KiB / 512 reads / 124 headers | Reject excess; library also rejects pathological tiny reads |
| External Session attachments | Existing endpoint bound: 32 per connection | Existing domain rejection |

The outbound queue holds at most 32 MiB of encoded payload plus one message being
written. Request futures, current decoding/serialization, and library framing buffers
are additional bounded transport work. Native runtime state and result construction
remain under their existing owners; these transport limits are not #291 residency quotas.

Each physical connection has one serialized writer. Requests can overlap across
Sessions, and responses may finish out of order; clients correlate by JSON-RPC ID.
The common serving layer never waits for queue capacity while retaining semantic
leases. A non-reading peer triggers queue overflow or the write deadline. Teardown
drops read/write/request-waiter futures and explicitly closes the semantic connection.
Already admitted runtime operations continue under their existing server owners.
A mutation whose response is lost has an **unknown outcome**: no transport retries it.
Reconnect with initialize/attach/snapshot/resync and inspect authoritative state.

`AppServerConnection::close()` is permanent and idempotent. Its route-table lock
linearizes close against attachment reservation/claim/commit, including pending loads.
It releases exact external claims even if concurrent tasks retain an `Arc`; it never
cancels execution, settles an interaction, unloads a runtime or edits history. Transport
termination has one cleanup path. A stale attachment cannot control its replacement.

SIGINT/SIGTERM stops transport admission and settles connection tasks/attachments.
It does not define a second runtime shutdown state machine or guarantee graceful drain
of in-flight work; #291 owns that follow-up. Stdio EOF ends the standalone child process
after detaching; any subsequent loss of execution is process death, not semantic EOF
cancellation. An owning TUI may explicitly terminate its child. One WebSocket client
closing never shuts down the listener or any other client.

## Transport validation

`tests/support/app_server_conformance.rs` supplies one expectation set for direct,
real-process stdio and real-process WebSocket drivers. Boundary tests additionally
cover handshake admission, fragmented messages, exact size limits, JSON error recovery,
process bootstrap/readiness, correlation, listener survival and process ownership.
Native provider/interaction gates establish disconnect behavior. Controlled byte pipes
exercise actual queue overflow, request admission saturation and a paused-clock write
deadline; timeouts elsewhere are only outer liveness guards.

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
drops the reservation. A full connection therefore cannot make a capacity-rejected
Session resident. Connection reservation is request-scoped; residency loading is
manager-scoped once claimed. Cancelling an admitted cold attach releases its slot
but does not roll back the manager-owned Loading flight: it may reach Loaded with
zero external attachments. A later attach reuses that resident incarnation.
Headless residency quotas/admission/idle eviction belong to #291, not request
cancellation. Its map is locked only for local routing changes; no lock
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

`session/detach` only removes the exact connection-local attachment relationship;
it acquires no runtime operation lease and works even during `Unloading` or after
residency ends. It does not cancel execution, settle interactions, or unload.

After exact local route validation, every terminal `session/unload` manager result
removes the exact initiating route and releases its attachment capacity **before
returning**, even if the requester stops polling. Errors remain errors: native
shutdown failure still leaves residency fail-closed in `Unloading`, and stale
incarnations still fail explicitly. Its response is the initiating request's
authoritative terminal acknowledgement. Notification consumption is not a
resource-release point; there is no synthetic durable closed-event queue. An
already waiting observer may receive `session/closed` for the old target, but
cannot remove a newly installed route.

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
safe integer range. They are not generated as JavaScript numbers. Their custom
Rust schemas describe exactly the serde domains: 16 lowercase hex digits excluding
all NaNs, both infinities and negative zero; and decimal i64 text bounded by
`-9223372036854775808..=9223372036854775807`, with no leading zeros, plus sign, or
`-0`. Every scalar-schema-valid value must deserialize, and every canonical Rust
serialization must validate. Negative tests check both scalar definitions and
nested public Questionnaire requests.

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

#36 supplies both stdio JSONL and WebSocket bindings, the standalone App Server
process entry point, and framing/authentication/backpressure/process/connection
concerns. #289 supplies the WebSocket Developer Web Console. #290 makes local TUI
consume stdio JSONL to its owned child and existing/remote TUI consume WebSocket
to an externally managed server, both using this protocol. #291 supplies residency
policy, quotas, idle eviction and graceful process shutdown. None of those
products or policies is implemented by this semantic endpoint.

### Shared transport conformance

`tests/support/app_server_conformance.rs` defines one small semantic parity scenario
and a test-only `AppServerConformanceDriver`: concurrent typed `request` and
`next_notification` calls. `tests/scripted/app_server/protocol.rs` runs it through
the direct `AppServerConnection` adapter now. #36 can include the same support
module via `#[path = "..."]`, supply a fresh connection and two idle Policy-mode
Sessions, and implement drivers that exchange the same DTOs over stdio or WebSocket.

The scenario checks version rejection/initialization, pipelined correlation,
two-Session attachment routing, authoritative approval-setting changes and routed
events/cursors, snapshots, and detach/reattach without changing incarnation or
runtime settings. It needs no provider timing, framing, retries or network state.
Adapters own transport mechanics, not expected semantic outcomes. This supplements
the detailed owner/race tests; byte framing, limits and process/socket failures
remain #36 tests, not a production transport framework in #288.

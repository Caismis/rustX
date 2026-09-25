# App Server protocol v24

App Server v24 / Runtime Client v50 separate finite Jobs from durable Agents. `jobs`/`job_updated` carry
`job_id` and terminal lifecycle; `agents`/`agent_updated` carry stable `agent_id`,
parent lineage, child ConversationId, current/latest activation identity and explicit
Active/Stopping/Inactive state. Controls route directly to each domain owner. A
resumed activation updates the same Agent row. Only current v24 generated artifacts are retained, following repository policy;
v23 peers are rejected without a compatibility decoder. See
[Jobs and continuable Agents](jobs-and-agents.md).


App Server v23 / Runtime Client v49 complete the reviewed #406 contract. The
#409 follow-up replaces separate successful/terminal process DTOs with native
`TurnProcessView`: exact Attempt ownership on each committed process member,
whole-process message/tool counts, immutable `control_cursor`, native clock and
one semantic owner for running, completed, cancelled, failed, timed-out and
limit-exceeded outcomes. Native evidence seats the control; bounded pages resolve
ownership without browser adjacency inference. Failed/stopped processes remain
open across reconstruction and paging. `CompletedResponseView` retains finalized
answer/TurnTail provenance and actions. Lineage remains selective: finalized
completed-response provenance may cross into children; unsuccessful source
execution outcomes do not. See [the ownership contract](issue-406/terminal-process-ownership.md).


The earlier v22/v48 revision introduced native whole-conversation Turn/Step
totals, measured request timing, the latest exact Attempt clock, and authored
`SessionModelsView::Available.default_model` from the same creation capture as
its catalog. v23/v49 retain these capabilities. A browser product preference can
seed new Session intent; it does not alter authored configuration or existing
Sessions.

The historical v21/v47 revision introduced `completed_process` on transcript
entries, using committed Assistant identities and exact Tool occurrence owners
to identify a successful Attempt and its final response. v23/v49 replace that
field with `turn_process`, extending the semantic owner across outcomes. SQLite
schema 44 retains exact completed-process members in lineage provenance,
remapped by the native copy owner; this selective lineage policy is unchanged.

The v20 Trace predecessor/catalog vocabulary from #394 is retained unchanged.

App Server v19 made resource-diagnostic attribution a native fact
(Issue #392). `ResourceDiagnostic.subject` is a required tagged member:
`{kind:"resource", family, name}` for a diagnostic of exactly one resource
identity, or `{kind:"collection", family}` for a failure of a family's source
document or collection as a whole. Native decides it where the identity is
known — the catalog entry keyed by that identity — and the former `identity`
member, which carried a loader field path such as `mcp_servers.<id>`, is now
`field` and locates the failure only. Before v19 a client had to infer
ownership from that path or from a source file several identities share, and
the second inference attached one MCP definition's failure to every sibling in
the same `mcp.toml`. The same `CapabilityInspection` travels in the Runtime
Client snapshot, which advanced to v45 for the same reason. v18 and every earlier
version are rejected; there is no dual handling and no compatibility shim.
Generated v18 artifacts are removed.

App Server v18 added `ConfigurationApplication.sources`: the ordered authored
source owners one configuration application composes (Issue #391). Its existing
`scope` is an application-scope key — a Session identity for a Session
application, `SourceTarget::application_scope` for a source application — and
carries no source ownership at all. Before v18 a client reading
`session/configuration` had no native fact naming which authored document owns a
failed Session configuration, so owner-specific Settings navigation had to parse
the scope string; that inference is wrong, because `configuration_application`
resolves a Session application by Session identity. `sources` is the native fact
instead: the User source always participates, and a Workspace-rooted capture also
names the exact canonical configuration directory it was taken from. It is a
required member of a type in a strict vocabulary, so v17 and every earlier
version are rejected; there is no dual handling and no compatibility shim.
Generated v17 artifacts are removed.

App Server v17 added `session/summaryInvalidated`: the Session-scoped
post-commit metadata invalidation that lets a live client reread authoritative
Catalog metadata after an asynchronous display-projection publication
(Issue #386). It is a new notification method in a strict vocabulary, so v16
and every earlier version are rejected; there is no dual handling and no
compatibility shim. Generated v16 artifacts are removed.

App Server v24 also carries v16's producer identity on Trace context additions
and typed accepted contributions on request detail. These are historical
RequestSnapshot facts, not live Todo/Goal authority. See
[native contribution lifecycle](native-context-contributions.md)
for atomic startup and same-step reuse. Generated v14 artifacts are removed.

App Server v24 identifies one complete mandatory vocabulary, including exact
`session/summary`, bounded historical Trace detail, and read-only Subagent
transcripts. v12 and all earlier initialization and WebSocket admission versions
are rejected; there is no downgrade or compatibility path.

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
The TUI consumes these generated DTOs through one typed App Server client, with
stdio and WebSocket adapters. See [TUI architecture](tui-app-server.md).

## One protocol, multiple transports

#284's topology unifies semantics, not transports. #288 owns the versioned
JSON-RPC DTOs, transport-neutral endpoint, generated client schemas and direct
semantic conformance. #36 binds the same `AppServerConnection` endpoint to
concrete stdio JSONL and WebSocket transports. Neither transport defines Session,
execution, interaction, cancellation, history, residency, or replay semantics.

Ordinary local TUI mode uses first-class App Server stdio JSONL to a
TUI-owned child; browser and existing/remote TUI clients use WebSocket to an
externally managed server. The ordinary local TUI does not need a loopback
WebSocket merely to consume the unified App Server protocol. This App Server stdio
binding and the remote WebSocket binding carry the same generated App Server DTOs.

### Connection lifetime versus process ownership

For either transport, connection EOF/close releases that connection's external
attachments. It does not fabricate turn cancellation, interaction settlement,
Session unload/delete, or server shutdown. Explicit detach has the same separation.

In local self-hosted mode, `rustx-tui` owns
`rustx app-server --listen stdio`. Normal TUI exit explicitly shuts down that child
through the process lifecycle seam. This is owner-driven process shutdown,
not transport EOF semantically cancelling work. Persistent execution across TUI
exit requires an externally managed App Server; WebSocket/existing-server TUI
disconnect never shuts down that process. SIGINT/SIGTERM enters the shared server
drain contract described below.

## Standalone startup

One process captures one User home and fixed configuration/resource/runtime
bindings. It creates one durable Session controller and residency manager; it
has no global active Session.

```sh
rustx app-server --config /private/user/rustx.toml \
  --runtime-root /private/user/runtime --listen stdio
rustx app-server --config /private/user/rustx.toml \
  --listen ws://127.0.0.1:8080 --token-file /private/user/socket-token
```

Defaults are `~/rustx/rustx.toml`, `~/rustx/.agents`, and `~/rustx/runtime`.
`--config` replaces only the User document. Both explicit path bindings must be
absolute. They are fixed process bindings. Each Session's explicit cwd selects
exactly its Workspace document and `.agents`; no ancestor accumulation or rustX
Workspace trust gate exists. Transport credentials remain a separate owner.

Exactly one transport is selected. Numeric IPv4/IPv6 WebSocket addresses may use
port zero. Bound readiness is reported on stderr; stdio emits no banner. A stdio
connection may use pipes or Unix socketpairs. The internal child process entry
point is separate from the public App Server protocol.


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
  "rustx.app-server.v24",
  `rustx-token.${dedicatedTransportToken}`,
]);
```

The server requires both offers on path `/` without a query, rejects failed admission
with HTTP 401, and selects only `rustx.app-server.v24` in its response. It never echoes
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

The [local Web launcher](../web-console/CONNECTION.md) implements delivery through
a separate browser launch-token exchange and a process-ephemeral browser proof in
origin-scoped sessionStorage (not a Cookie). A dedicated header authenticates
same-origin carrier APIs. Its bootstrap returns the exact native
endpoint/token; the browser then connects directly using the v24 subprotocols above.
The browser launch credential is never a valid substitute for the native credential.
Remote Web attachment is explicit Settings configuration. Neither browser login
nor remote attachment grants Product Host Workspace filesystem authority.

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
| WebSocket clients, including pending handshakes | Configured (default 32) | HTTP 503 where writable, then close |
| WebSocket handshake | 5 seconds | Drop incomplete socket |
| HTTP handshake parsing | Library bounds: 64 KiB / 512 reads / 124 headers | Reject excess; library also rejects pathological tiny reads |
| External Session attachments | 32 per connection; configured process total (default 64) | Typed attachment capacity rejection |

The outbound queue holds at most 32 MiB of encoded payload plus one message being
written. Request futures, current decoding/serialization, and library framing buffers
are additional bounded transport work. Native runtime state and result construction
remain under their existing owners; these transport limits are independent from residency quotas.

Ready observations alternate with protocol progress (a completed response or
incoming-message admission), starting with protocol progress. At most one ready
notification precedes each progress step. Since there are at most 16 pending
requests and only input admission can add another, ready input cannot wait behind
more than 16 response completions plus 17 observations. Conversely, continuous
input/response traffic cannot starve a ready notification. No backlog drain,
sleep, queue expansion, or semantic bypass is needed.

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
termination has one cleanup path. A stale attachment cannot control its replacement. Native operation authority is
captured under the close/admission lock and retained only in the manager-owned
operation task; subsequent detach cannot revoke an already-admitted mutation.
Connection-local subscription delivery still ends with the attachment.

SIGINT/SIGTERM commits server drain, supervises every resident runtime through
native settlement, and then terminates transport tasks. Stdio EOF only detaches;
the child awaits an explicit owner signal. One WebSocket client closing never
shuts down the listener or any other client. A deadline or second signal forces
host termination without claiming semantic settlement.

See [final composition acceptance and dogfooding](app-server-acceptance.md) for
the reference product-host boundary, coverage map and two developer flows.

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
{"jsonrpc":"2.0","id":"init-1","method":"initialize","params":{"protocol_version":16,"client":{"name":"example","version":"1"},"presentation":{"images":true,"questionnaires":true,"reviews":true}}}
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
supported in v24; pipeline individual requests instead. This limitation is
explicitly rejected as an invalid request before any action occurs.

## Methods and native owners

| Methods | Owner and semantics |
| --- | --- |
| `initialize`, `server/info` | Connection negotiation and server capabilities |
| `session/list`, `session/read`, `session/summary`, `session/name`, `session/tree` | Durable controller; bounded pages / exact identity reads; no runtime composition |
| `session/create` | Durable creation from explicit Session selections |
| `session/fork`, `session/branch` | Exact native Surface revision, message boundary, and explicit `side`: `before` restores ordinary User input; `after` includes a durably completed Assistant response with an empty editor. Fork without a boundary clones the revision into an independent Session |
| `session/deletePreview`, `session/delete`, `session/recoverDeletion` | Native revision-confirmed deletion/recovery; no client-supplied cleanup workset |
| `session/attach`, `session/detach` | Load/reuse a runtime and acquire/release its external control attachment |
| `session/switchNode` | Switch to a selected branch; manager owns retirement and composition |
| `session/snapshot`, `session/subscribe`, `session/transcript`, `session/boundaries` | Authoritative projection, bounded replay, durable transcript and revision-bound user-message pages |
| `turn/start`, `turn/steer`, `turn/cancel` | Native inbound and attempt-cancellation owners; acceptance is not terminal execution |
| `interaction/respond`, `interaction/cancel` | Originating runtime/coordinator, including routed child interactions |
| `session/settings` | Durable resolved Session selection; model mutation uses `session/setModel` |
| `session/model`, `session/models`, `session/setModel` | Live attached Session model read/catalog/mutation; native validation and persistence, exact model and advertised profile references |
| `configuration/sourcesRead`, `configuration/sourceWrite` | Native structured User/Workspace documents and exact revisions; bounded semantic-unit CAS writes |
| `session/effectiveConfiguration`, `configuration/reconcile`, `session/adoptConfiguration` | Authoritative application state, native rescan/retry, and fenced explicit Session adoption |
| `context/compact`, `goal/control` | Existing maintenance and Goal owners |
| `job/list`, `job/status`, `job/wait`, `job/cancel` | ConversationBackgroundRegistry: bounded finite Jobs, immediate snapshot, exact terminal wait and cancellation through physical settlement |
| `agent/transcript` | Parent `AttachmentTarget` → current parent Runtime Client authority → exact parent `SubagentRegistry` ownership resolution of caller-supplied `AgentId` (never arbitrary child `ConversationId`) → exact owned child Conversation, which remains history authority → bounded read-only durable transcript projection; grants no execution, control, or HITL authority |
| `agent/list`, `agent/status`, `agent/sendMessage`, `agent/wait`, `agent/interrupt` | Durable Agent owner: atomic message/resume arbitration, captured activation wait and activation-only interruption |
| `subagent/disposeWorkspace` | Activation-specific retained resource owner; no caller-supplied filesystem cleanup paths |

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
Headless residency quotas and idle eviction belong to the runtime manager
policy, independently of request cancellation. Its map is locked only for local routing changes; no lock
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
and incrementing its in-flight count. Internal retirement compares that incarnation
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

Session deletion fences manager admission before coordinating native shutdown.
Every old route becomes unusable at that fence; native `session/closed` observation
retires subscriptions. Other connections cannot publish a replacement writer.
A retirement error does not imply absence: unproven writers remain counted and
fenced. Attachment cleanup does not grant durable deletion authority.

## Attachment and observation lifetime

Protocol v24 admits at most one writable external controller per resident
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

### `session/summaryInvalidated`

`session/summaryInvalidated { session_id }` says exactly one thing: this
Session's durable display metadata changed on the server after the client may
already have read it, so read `session/summary` again. It carries no metadata,
claims no durability beyond the catalog commit that produced it, and is not
canonical history, an Agent event, an Attempt event, or a Conversation runtime
cursor. Session display responsibilities never enter the Agent Loop or the
Conversation runtime.

It is addressed by **Session identity alone** — deliberately not by
`AttachmentTarget`. Durable Session metadata belongs to the Session, not to
whichever Conversation of it a client happens to display, so a branch view
receives it, and a client that merely lists a Session receives it without
attaching a runtime.

Ordering is closed by construction: a connection captures its cursor into the
native invalidation log when the connection is created, before it can serve any
request. A publication older than the connection is already reflected in every
read that connection can make; a newer one is delivered live. Nothing can fall
between the initial metadata observation and live observation.

The log is level-triggered and coalescing, exactly like `configuration/changed`
above: per-Session state plus one connection cursor, with no durable replay, no
background queue and no independent scheduler. Publication is once-only per
Session, so a no-op repair and an already-correct projection produce neither a
catalog write nor a notification. Lag and disconnect reuse the existing resync
discipline: a reconnecting client re-establishes authoritative metadata through
its ordinary bootstrap reads.

The native owner publishes the invalidation only **after** the Catalog
visibility point, including the post-visibility outcome that reports uncertain
durability — that mutation is visible, and losing it would strand a client on
stale metadata. A pre-visibility failure announces nothing. Recording an
invalidation never waits on a client, a socket, or an acknowledgement, and
never happens while the Catalog mutex is held for client delivery.

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
Session model CAS revisions, capability and resource revisions, Goal
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

- `v24.schema.json`: complete JSON Schema generated with Schemars from Rust DTOs.
- `v24.ts`: TypeScript generated from that schema using pinned
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
two-Session attachment routing, authoritative configuration-policy publication and routed
events/cursors, snapshots, and detach/reattach without changing incarnation or
runtime settings. It needs no provider timing, framing, retries or network state.
Adapters own transport mechanics, not expected semantic outcomes. This supplements
the detailed owner/race tests; byte framing, limits and process/socket failures
remain #36 tests, not a production transport framework in #288.

## Process residency and drain policy (#291)

Runtime residency is process-local cache/resource ownership, not durable Session
lifecycle authority. Unload releases a live incarnation; it neither deletes a
Session nor changes its durable selections. A later attach cold-loads through
normal current configuration resolution and native recovery.

The bound User `rustx.toml` accepts this process-only table. Workspace
TOML cannot override it, and it is never persisted in a Session:

```toml
[app_server]
max_resident_runtimes = 8
max_connections = 32
max_external_attachments = 64
idle_grace_ms = 300000
shutdown_deadline_ms = 30000
```

These conservative defaults allow several concurrently focused Sessions while
bounding expensive live compositions. The five-minute grace avoids recomposing
on ordinary focus changes; the thirty-second host deadline bounds operational
shutdown independently of native settlement. They are product defaults, not
throughput or user-per-host claims. One process still represents one user
environment. Zero values are invalid. Upper authoring bounds are respectively
256 runtimes, 1024 connections, 4096 attachments, one day of idle grace, and one
hour of shutdown grace. Invalid policy fails before traffic admission. Settings
changes take effect on the next process start.

Configuration resolves the policy once during App Server composition. The host
owns the full process policy and passes only runtime count and idle grace to
SessionRuntimeManager. The dependency graph is AppServerHost → connection /
transport / SessionRuntimeManager → ConversationRuntime; the manager never imports
App Server types.

`Loading`, `Loaded`, and `Unloading` all consume the same writer quota. The
registry lock checks quota and installs `Loading` atomically; same-Conversation
callers join that flight without consuming another slot. Composition runs
outside that lock. Failure releases a loading reservation; unproven unload
retains its counted `Unloading` slot. Capacity refusal does not queue for a slot.
One native coordinator owns at most one current root attempt, so the resident
slot limit also bounds aggregate root attempts. Pending inbound remains durable
native work; a `turn/start` response does not release an execution permit. No
second root-attempt registry or limiter is introduced. Subagent/Workflow limits
remain with their native domains.

AppServerHost owns process-wide attachment capacity reservations. Each admitted
attachment separately obtains an incarnation-bound RuntimeResidencyPin from the
manager, excluding internal projection/control plumbing. Pins know neither the
global attachment limit nor transport identity. Each connection still permits at most 32 attached
or reserved Session routes; the configured attachment limit applies across the
process. Detach releases only the external relationship. WebSocket close, stdio
EOF, and broken pipe do not cancel attempts or answer interactions. Focusing B
and detaching A does not immediately unload A.

One process reaper checks resident slots once per second using the runtime's
`MonotonicClock`. The grace begins at the first positively verified eligible
scan, never earlier than native settlement. Polling may conservatively add a
scan interval. Timing is reset by external attachment/operation activity or
native admission activity, including work that finishes between scans. Native
eligibility excludes current attempts, compaction, accepted pending inbound,
recovery continuation, pending interactions, background preparation/execution,
unsettled subagents, and counted lifecycle owners (including Workflow,
capability/MCP preparation, interaction callback authority and attempt tasks).
Goal capability presence does not pin residency. A composed Goal that is durably
`Active` with autonomous budget remaining owns authority to admit future
autonomous work and blocks idle eviction, because evicting it would race its own
eligible continuation. An uncomposed extension, an absent Goal, paused, blocked,
completed, and an `Active` Goal whose autonomous budget is exhausted do not
independently block eviction — so an exhausted Goal cannot pin residency forever.
Recovery restores the durable phase; opening the runtime is what lets an Active
Goal continue. Goal create/resume commits through the existing native lifecycle
boundary and changes its epoch, so a stale idle probe cannot cross a later
continuation authorization.

The native admission change token is invalidation only, not a second work-state
registry. The reaper reads native owners outside the registry lock. At eviction
commit it holds the registry lock, requires zero external attachments and
operation leases, validates activity, and claims native `Running -> Draining`
under the coordinator/lifecycle admission boundary before publishing
`Unloading`. The claim uses nonblocking native lock acquisition; contention
conservatively retains the slot without holding up other Sessions. New attach/control/inbound either admitted its lease first and
prevents that claim, or observes a stale/unloading incarnation. External teardown
runs outside the registry lock through the existing unload owner. No unchecked
check-then-unload window exists.

SIGTERM or SIGINT is the normal shutdown request for both externally managed
WebSocket servers and owned stdio children. The first request commits
`Accepting -> Draining` at AppServerHost's admission mutex, shared by semantic
request, connection and attachment capacity admission. Runtime-targeted request
admission also captures the manager operation lease before releasing the host
boundary, so drain cannot reject a previously accepted request in that gap. The manager has no server
lifecycle: already accepted host requests may still claim runtime operations or
loads. Host drain immediately supervises the current residency set while waiting
for those request owners, then drains a final inventory to cover late admitted
loads. Both passes supervise all entries without failing fast. Diagnostics remain readable, but new semantic work receives
`server_draining`. New WebSocket sockets are refused at the transport boundary.
Every reserved/live runtime is supervised concurrently through native shutdown;
operation leases, native settlement, and the existing host projection drain
are awaited before composition release.
Failures are collected without abandoning siblings. Native shutdown may request
cancellation through the existing owners; requesting cancellation is never
reported as proof of physical or durable settlement. Session catalog and runtime
writes already have transactional/synchronous owners, so there is no invented
flush layer. Admitted catalog mutations retain their tasks until completion.

Only proven runtime, accepted-request, attachment and transport settlement yields
`Terminated`. A settlement
failure remains fail-closed, is reported on stderr, and exits nonzero. The host
deadline covers accepted-request settlement, runtime drain and transport exit. Deadline expiration or a
second termination signal takes the explicit forced host exit (code 3), reports
unproven residency resources, and bypasses executor destruction. It writes no
fabricated tool/interaction outcome and does not advertise retained slots as
unloaded. Recovery in a replacement process remains the native durable recovery
contract. Unexpected stdio EOF waits for the owner's explicit signal; EOF alone
is not the shutdown request. #290 will consume this owned-child contract; this
issue does not migrate the TUI.

`server/diagnostics` is a read-only, initialized semantic request. Its
`diagnostics` result contains lifecycle, configured policy, loaded/loading/
unloading counts, native active-root count, external attachment counts, per-
Session resident identities and operation counts, typed refusal counters,
unload/shutdown failures/timeouts, and transport budgets/counts. This diagnostic Session inventory
contains reserved/resident slots; absence only means no process-local residency.
The ordinary `session/list` response contains durable Session information only. `idle_for_ms`
and `idle_remaining_ms` are relative, conservative scan facts, not lifecycle
authority. Aggregation happens upward in AppServerHost: manager diagnostics
contain only residency/native samples, transport diagnostics contain physical
connections/queues/failures, and host diagnostics add lifecycle, global attachment
capacity and host refusal/failure counts. None of these snapshots is authority. Native execution facts are sampled outside the
registry lock; this is not a transaction across all runtime owners. No credentials, environments, or runtime handles
are exposed.

Each physical connection has a 1 MiB encoded-message limit, 32 outbound queued
messages (explicit 32 MiB queue byte budget), one additional message being
written, 16 in-flight protocol requests, and a 10-second physical write deadline.
These transport-owned budgets remain fixed, not Session policy. Encoded messages
are bounded before queue insertion; no byte allocation is inferred from object
sizes. Slow consumers are disconnected and release attachments without semantic
settlement. Handshaking sockets count toward the configured connection quota and
have a five-second handshake deadline. Capacity/drain rejection before JSON-RPC
uses HTTP 503 where the socket can accept the response, then closes. stdio owns
one physical connection. Process catalog request ownership is additionally
bounded by `max_connections * 16` outstanding operations; refusal is
`request_capacity`. Other typed refusals are `residency_capacity`,
`attachment_capacity`, and the existing `stale_runtime` for retired incarnations.

## Session workspace uploads (WEB-02A)

`session/upload { target, files: [{ name, data }] } -> session_uploaded { files }`
commits an ordered batch through the native Session owner. Each result includes
`receipt { session_id, batch_id, token }`, typed `file { batch_id, name }`, and
its intentionally model-visible absolute `path`. The client supplies only a safe
basename and bytes. Standard base64 is the current carrier, not the core domain.

The carrier admits 1–8 files, at most 256 KiB per file and 512 KiB decoded per
batch. Each file has at most 349,528 base64 characters and a 255-byte basename.
The aggregate encoded batch stays below 699,072 bytes; JSON/routing must also fit
the existing 1 MiB request bound. Existing connection/in-flight bounds apply.
There is no streaming, multipart, resumable, or provider Files API.

`turn/start` and `turn/steer` accept `content` containing `{ type: "text", text }`
and `{ type: "upload", session_id, batch_id, token }` receipt blocks. Canonical
`uploaded_file` blocks are server-authored and are not accepted as client input.
Unknown, incomplete and cross-Session receipts fail. Upload creates no User
message; failed turn admission leaves the committed file Session-owned.
A lost upload or turn response is an uncertain mutation, never a replay signal.

See [Session upload architecture](session-uploads.md) for the durable commit,
path safety, exact model projection, fork and deletion contracts.

`artifact/read { target, artifact_id } -> artifact_bytes { data }` remains solely
for presentation of Tool-generated managed artifacts, bounded to 256 KiB. That
conversation ArtifactStore retains its existing capacity (256 identities),
reservation, read, spill and background-output semantics. User uploads never
allocate or read back ArtifactStore identities. There is no artifact upload method.

## Native Trace

`session/trace { target, before?: TraceCursor, limit: 1..32 }` is a mandatory read,
returning `{ type: "trace", page: TracePage }`. Attach/snapshot also carries the
bounded newest `snapshot.trace` window. Payloadless `trace_changed` notifications
use the existing subscription as invalidation signals. Neither historical reads
nor Trace cursors advance a subscription cursor. See [Trace architecture](trace.md)
for source authorities, ordering, read cuts, repair, bounds and unavailable facts.

Generated Rust Schema/TypeScript, Web Console and TUI all negotiate version 13.
Earlier versions are rejected; there are no aliases or dual-version paths.

`session/snapshot` optionally accepts `trace_records: TraceCursor[]` (maximum 512)
for the client's bounded loaded window and separately retained selected record. `snapshot.trace_updates` contains typed
native lifecycle repairs for those identities, including records older than the
newest Trace page. Both the newest page and repairs use the Journal prefix
captured with the returned Runtime Client cursor, never a later SQLite frontier.
Only installed native semantic publications can advance that prefix; a raw SQLite
commit receipt cannot publish it. Historical `session/trace` independently captures
a represented semantic prefix and native lifecycle snapshot on live hosts, without
folding observations or changing the live cursor. Inactive durable inspection
captures its own SQLite frontier and has no live publication boundary.
This remains mandatory protocol v24; no compatibility path is provided.

### Fork editor input

Session transitions return `editor_content` as ordered `UserInputBlock` values,
ready for `turn/start`: exact text plus server-issued upload receipts. Independent
forks copy editor-boundary uploads before publication; same-Session branches share
Session ownership. The canonical destination prefix ends **before** the selected
User message. A client restoring an editor keeps `editor_content` uncommitted; a
product Retry may branch, attach the exact returned node/Conversation, then submit
that content once through `turn/start`. It must stop after any uncertain mutation
response, without repeating the branch or admission. No command interpreter or
additional retry endpoint is involved. These are the merged #319 v4 semantics.

## Goal lifecycle (Issue #351)

`GoalView` is `{ current: GoalSnapshot | null }`. Durable `GoalPhase` is the one
product-visible Goal lifecycle authority:

```text
Active   = continuation is authorized when runtime admission is eligible
Paused   = continuation is not authorized
Blocked  = continuation is not authorized
Complete = terminal
```

The obsolete `GoalView.armed` member and every activation-only observation are
removed, so no snapshot and no `goal_changed` event can represent
`Active + disarmed`. Native Runtime Client version 43 carries this vocabulary;
version 38 clients are rejected by strict negotiation. This remains mandatory
App Server protocol v24, with no compatibility field and no activation mode.

Clients derive presentation from the phase alone: `Active` offers Pause,
`Paused` and `Blocked` offer Resume, and there is no separate Play/arm control
and no "Inactive Goal". `goal/control` `create` and `mutate` are the typed
control path; a client never sends a model request to create, pause, resume,
edit or re-budget a Goal, and never issues a second operation after Resume.

Attach, reconnect and snapshot reads remain observations. None is an
independent start authority: only explicitly opening/activating the runtime
lets a durably Active Goal continue, at its first eligible safe idle admission
boundary.

## Exact pending inbound controls (WEB-06)

Protocol v24 includes `inbound/edit { target, expected, text }` and
`inbound/remove { target, expected }`. `target` is the ordinary exact Session,
Conversation, runtime incarnation and controller attachment authority.
`expected` contains the native `sequence`, `message_id` and `revision` from
`snapshot.inbound.pending`. Exact integers use the existing decimal-string wire
encoding. This is a mandatory vocabulary change; no v4 compatibility mode exists.

Pending Inbound in `ConversationStore` owns mutation. Its SQLite transaction
compares identity and revision and commits the replacement or removal atomically.
Revisions begin at zero and edits increment them without changing occurrence
identity. Removal and canonical adoption permanently invalidate the occurrence.
Old sequences are never reused. A successful response is `inbound_mutation` with
`outcome.status = applied`; known losing outcomes are `not_pending`, `conflict`
and `invalid_item`. Storage acknowledgement/readback failures report
`durability_uncertain`. Routing errors retain the existing typed attachment and
runtime errors. Clients never substitute a newer revision or replay after loss.

Edit currently supports one ordinary Human text block without producer
correlation. Unsupported typed or multipart content is rejected atomically;
QueueDock disables Edit for those shapes. In particular it never flattens or
drops Session-owned upload references. Remove supports ordinary uncorrelated
Human pending messages, including uploads. Runtime-authored work is not a user
queue control target.

Selection is a non-destructive finite watermark, not a payload reservation.
Canonical adoption reads the current rows inside its own transaction and builds
the `InboundTurnAdopted` obligation there. The committed receipt alone supplies
canonical execution content and drain observations. Pre-commit memory checks
validate stable identities; no selected payload is installed after the commit.
If mutation wins, adoption sees edited content or excludes removed work. If
adoption wins, mutation is `not_pending` and cannot change history or execution.
An empty claim creates no answer obligation and admits no fresh turn.

Editing updates the pending message body read through its original transcript
cursor. Removal deletes only its pending transcript index entry, in the same
transaction. Neither operation edits canonical history or cancels attempts,
background executions, Subagents or Workflows. Queue/Steer submission converges
on this same native domain; there is no per-row Steer/reclassification flag.

The mailbox publishes a complete `pending_inbound_changed` projection only after
the durable mutation check and readback. Its publication ordering is coordination,
not mutation authority. The Event Journal remains execution facts and contains
no queue mutation commands. Live observation and snapshot repair consume the
same committed pending state. Web success waits for an authoritative reread;
stale edits preserve drafts for conscious reconciliation. A lost response remains
uncertain, is never replayed, and reconnect replaces transient state from the
native snapshot. Pending mutation invalidates loaded Web transcript windows so
removed entries cannot survive a historical-page merge. Recovery reconstructs
only committed rows and revisions; no browser queue is persisted.

The pending revision column advances the SQLite store schema to 38. As with the
other pre-1.0 schema changes, older stores are refused explicitly; no migration
or compatibility representation is introduced.

Native Runtime Client version 37 carries the mandatory pending revision and
`pending_inbound_changed` event; it also has no compatibility decoder.

Snapshot/attachment reads also reconcile Pending Inbound directly from durable
storage under the mailbox publication cut. This repairs a committed mutation
whose immediate notification/readback was lost, without replaying the mutation.
Repair drains older inbound notifications before folding the durable read and
publishes a replacement only if the pending projection differs. A read concurrent
with native publication retains the preceding complete observation cut rather
than exposing a half-installed transition; subsequent observation/readback
converges on the committed state.


## WEB-07 bounded Workspace navigation projections

Product Host owns Workspace registration, authorization, location resolution and
metadata. App Server does not allocate Workspaces, accept Host ACLs, or persist
Workspace IDs. Browser path strings are not authorization. The local adapter and
its same-origin Host contract are documented in [Web Workspaces](../web-console/WORKSPACES.md).

`session/summary { session_id }` returns `{ type: "session_summary", summary: SessionSummary }`
by exact durable identity. Unknown IDs return native `unknown_session`; deleting IDs
retain the native deletion error. This controller read does not attach, load, change
residency, resolve configuration or admit execution. List and exact read share one
native summary projection, projected from persisted catalog metadata only: neither
`session/list` nor `session/summary` ever opens a conversation store. The row's
`preview` is the persisted display projection — the whitespace-normalized,
120-character-bounded first line of the root lineage's first ordinary user
message — published after the first canonical root user commit, derived from
the frozen seed at clone/fork publication, or backfilled by the explicit
idempotent repair seam at reopen/compose/recovery. Display precedence is
explicit name → preview → identity fallback.

Canonical commitment and projection publication are **two separate commit
points**, in that order, and a client can legally read `session/summary`
between them. `preview: null` therefore means one of three things, and the
protocol does not distinguish them: no ordinary user message exists yet; the
first one has no renderable text (a permanently empty projection); or a
publication gap — the projection has not been published, and may stay
unpublished indefinitely if the publishing process died or its commit failed,
until an explicit repair seam runs. None of the three is repaired at list or
read time, and `null` is never evidence that publication is imminent.

Publication has a defined live-client convergence path:
[`session/summaryInvalidated`](#sessionsummaryinvalidated). A client never polls, retries, or
infers settlement from canonical history.
`session/list` remains bounded searchable/paginated browsing: its query matches ID,
name or preview substrings and must never be used as exact identity resolution.
Web `view.summary` is a replaceable exact observation, not durable/catalog authority.
After canonical first-user-message observation, only successful exact reads complete
preview convergence (including `preview: null`); failed reads remain retryable.

`session/list` now includes `SessionSummary.cwd`, projected by the native catalog
from `SessionPersistentState`. The bounded page contains durable facts only;
residency remains in `server/diagnostics`. Listing never loads a runtime. This avoids a second Session-to-Workspace database.

Workspace registration, rename and order remain Product Host metadata. They do
not rewrite Session state. Switching Workspace does not unload or cancel a
Session, and unregistering a Workspace is not Session deletion.

## Configuration application ownership (Issue #391)

`ConfigurationApplication` carries two distinct identities and they are never
interchangeable:

```text
scope    the application-scope key this application is published and read under
sources  the authored source owners this application composes, lowest first
```

`session/configuration`, `session/attach` and `configuration/changed` resolve a
Session application through `configuration_application(&SessionId)`, which reads
`applications.view(session_id)`. Its `scope` is therefore the **Session
identity**, not `source:user` or `source:workspace:<directory>`. A source
application published for `configuration/sourcesRead`/`sourceWrite` carries
`SourceTarget::application_scope` instead. Neither form is a source owner, and a
client must not parse either one to decide which authoring surface to open.

`sources` answers that question directly. Native records the exact source target
each application scope was captured from at capture time — `capture_source` for a
source scope, `capture`/`register_session_scope` for a Session scope — and
projects it as an ordered `SourceTarget[]`:

```text
source:user                    -> [ {kind:"user"} ]
source:workspace:/w            -> [ {kind:"user"}, {kind:"workspace",directory:"/w"} ]
<session id>                   -> [ {kind:"user"}, {kind:"workspace",directory:"<cwd>"} ]
```

The User document always participates because every capture overlays it; a
Workspace-rooted capture also composes `<directory>/rustx.toml`. Each entry is
exactly the `SourceTarget` a client passes back to `configuration/sourcesRead` or
`configuration/sourceWrite`, so owner navigation needs no directory parsing, no
Session `cwd` inspection and no client-side inheritance model. A Workspace owner
that the Product Host does not register is an explicit client-side error, never a
fallback to User authoring.

Per-unit ownership is deliberately **not** projected. `UnitApplication::Failed`
carries a diagnostic string, and native failure paths do not thread the owning
document through it, so no truthful per-unit owner exists to publish.

## CFG3 configuration authoring and publication

[Configuration](configuration.md) defines the native source model and
[Web Settings](web-settings.md) documents its projection. User and Workspace
read/write operations return exact revisions and redacted structured documents.
Redaction is a property of the projected type, not of a call site: a Provider
credential projects as `CredentialSourceView` (kind, and an environment variable
name), an MCP definition's literal `env`/`headers` are cleared and replaced by
`retained_env`/`retained_headers` identities, and `RuntimeLayer.environment`
projects as a list of authored identities (`string[]`) rather than a map of
literal values. `SourceSettings.user`, `SourceSettings.workspace`,
`SourceSettings.resolved` and `EffectiveConfiguration.document` are all the same
redacted document view, so no source projection can carry a literal Tool
environment value, and an override is authored by supplying a new value rather
than by reading a lower owner's value back.
Protocol v24 uses one `SourceTarget`: `{kind:"user"}` or
`{kind:"workspace",directory:"/canonical/native/context"}`. Source read, write and
reconcile have no Session parameter; mutations carry no second scope authority.
Product Host translates an authorized registered Workspace ID into this native
target. Browser-provided paths are not authorization. Opening the editor discovers
inert definitions without allocating Sessions, runtimes, MCP connections or Python.
Rust validates and serializes whole semantic units. Save transfers responsibility
to native reconciliation. Complete independent units apply automatically;
context-changing or unproven candidates await explicit Session adoption.

`session/effectiveConfiguration` and source projections expose desired input revision,
application identity, per-unit results, current process bindings, complete ready
candidate, and adopted Session binding. Applied, ready, failed and restart states
can coexist. `configuration/changed` carries scope and monotonic version; clients
reject older notifications. Versions are native `u64` counters, comparable only
within one source scope, authority and connection lifetime, and never against a
source revision. Reconnect rereads authority without mutation replay.

A successful `configuration/sourceWrite` acknowledgement is proof of **authoring**
— the committed source revision — and not proof that the application projection it
carries is the latest. Native application publishes on its own schedule, so a
`configuration/changed` notification and the acknowledgement of the write that
caused it arrive in either order. A client must therefore treat a published
application version as a standing observation obligation, discharged only by an
authoritative read whose projection carries at least that version for that exact
scope; an acknowledgement can neither discharge it nor cancel the read it
requires. The obligation is level-triggered: a publication that arrives while a
convergence read is already outstanding survives it, and the client re-reads when
the outstanding read settles below the newest published version. The obligation is
also owned: a publication or acknowledgement obligation observed while a single
convergence owner is running is never discarded, and a superseded read is never
treated as a satisfied obligation. If the owner's own read is preempted by a newer
one-shot read, the owner keeps or deterministically transfers the outstanding
obligation to another bounded pass before releasing ownership. Because an
application version orders application publications only — never authored source
revisions or whole `SourceSettings` snapshots — an acknowledgement is mutation
evidence, not a candidate replacement for the read model: a client that adopted a
causally later authoritative read must not let an acknowledgement carrying an
equal or older application version restore earlier authored state. Status strings
are never ranked: a later legitimate edit republishes as `preparing`, and
`applied` is not a terminal conclusion.

`session/adoptConfiguration` requires the inspected candidate identity and expected
binding revision. `session/configuration` reads the retained binding/candidate and
native advisory eligibility without loading a cold Session. `configuration_adoption` reports Busy, NotReady, Conflict or a
preparation/commit diagnostic. Adoption is ordered against Attempt admission and
preserves canonical history. `configuration/reconcile` rescans external inputs or
starts a new same-revision application attempt. It is never a second Save step.
See [the native contract](configuration.md#save-automatic-application-and-session-adoption).

## Agent read projections (#346)

`RuntimeClientTranscriptEntry.tool_calls` exposes native `ForegroundToolExecution`
records at canonical Assistant positions. Each record now requires `message_id`
(the exact Assistant MessageId) and `block_index` (its content position), alongside
`call_id`, `tool_id`, `name` and `state`. Live `attempt.foreground` carries the same
occurrence identity from native publication frames or canonical bootstrap. A live
record may update only that exact occurrence; a canonical settled result wins.
Provider ToolCallIds are not unique across a Conversation lifetime (the recovery
owner already scopes evidence by Attempt). Consumers must never join historical
calls/results or overlay current state by provider call/Tool identity alone.

`ToolCallId` is an opaque, provider-issued correlation string, not a rustX global
identity. It remains unchanged on OpenAI Chat tool results, Responses
`function_call_output.call_id`, and Anthropic `tool_result.tool_use_id`.
`ToolCallOccurrenceRef { assistant_message_id: MessageId, block_index: ContentBlockIndex }`
is the canonical rustX identity. Every `ToolMessageBlock` requires `occurrence`
alongside its own `id`, provider `tool_call_id`, native `tool_id`, and `result`.
`ToolExecutionId` is a separate runtime-owned UUID for detached execution; it is
neither provider correlation nor canonical occurrence identity.

The Agent supplies occurrence ownership from its committed Assistant blocks before
the result commit. Recovery retains AttemptId + ToolCallId execution evidence and
resolves missing results against exact canonical Assistant blocks; synthesized
results carry that occurrence before commit. Canonical history alone therefore
contains every call/result relationship, without source execution events.

SQLite schema **43** retains `canonical_tool_calls` only as a derived index. Its
primary key is `(assistant_message_id, block_index)`; Assistant/call and result
MessageId uniqueness constraints prevent duplicate provider IDs within one
Assistant and duplicate settlement. Tool commits validate the exact indexed
occurrence, call ID and Tool ID, then insert the canonical result and link its
MessageId atomically. The index is reproducible from canonical messages. No active
Surface search discovers result ownership. Schemas 38/39 are refused without
migration, dual decoding, backfill or fallback JSON scanning.

Clone, fork and tree copies remap canonical MessageIds and each result's
`occurrence.assistant_message_id`, preserving block positions and provider IDs.
These IDs remain historical provider correlation values, not new invocations;
rewriting them has no native ownership purpose. Reuse across Assistant messages is
valid, including within the retained Surface. Fork cuts and compaction boundaries
validate exact occurrence relationships and cannot retain only one side of a pair.

Transcript reads perform one occurrence-index range seek per Assistant page row
and ledger MessageId-index seeks for linked results, even outside that page:
work is bounded by page rows and their associations, not total ledger history.
There is no new event protocol, browser assembler or execution owner.

`SourceSettings.prospective_approval_mode` is the native configuration resolver's
prospective policy (absent when the candidate is invalid). It describes authored
source intent, not loaded/attempt authority. Source writes remain revision-CAS
semantic-unit mutations. Independent policy applies automatically; active Attempts
retain `attempt.execution_settings.approval_mode`. `effective_approval_mode` remains
the loaded runtime fact. Neither field introduces a Session approval override.

`SourceSettings.session_models` is present only for a Workspace target. `available`
carries the `ModelCatalogView` that a Session created in that Workspace now binds,
exactly as its `session/models` then serves it: the Workspace's published creation
capture (or, before one exists, the capture creation would validate and publish),
resolved with the process credentials. `unavailable` carries the native diagnostic
where Session creation would fail. The read publishes nothing and creates no
Session. Clients select pre-Session models only from this catalog; `resolved` is
source resolution and is never a selectable model vocabulary.

### Root metadata authoring (#347)

`configuration/sourceWrite` accepts native `ConfigMutation` units
`agent_identity` (`AgentId | null`) and `description` (`string | null`). They address
independent Root metadata scalars through the same revision/CAS, validation,
serialization and native reconciliation boundary as `instructions`. `null` removes
the authored unit in that scope. Clients never write whole config documents.

## Current Session lifecycle contract

Initialization requires exactly v24 and WebSocket requires `rustx.app-server.v24`.
v23 and all earlier versions are rejected without fallback. Rust DTOs generate
`v24.ts`, `v24.schema.json`, and the serialized fixtures; only the current version is kept.
Manual runtime unload is absent from the public method/result vocabulary.
Session lists have no residency field. Deletion blockers have no current-Session
or ordinary-residency case: external allocation exclusion is `resource_conflict`.
Preview works while attached or executing, without destructive guards. Confirmed
delete owns manager fencing/retirement, destructive exclusion, revision validation,
durable commit, and cleanup. Existing stale-confirmation and durability outcomes
remain authoritative. Close view continues to use `session/detach` only.

## Public deletion recovery ownership

`session/delete` delegates admission, native writer retirement and durable handoff
to SessionRuntimeManager. `session/recoverDeletion` also uses that manager in a
server-owned request task, independent of the initiating connection.

Recovery first obtains the finite durable deletion observation. A live Session
returns Preview or Blocked without cleanup or changing an active deletion fence.
Only an observed committed record permits SessionController::recover_deletion,
which retries that frozen authority. Observed absence receives a catalog durability
barrier before NotFound; it grants no cleanup authority.

Deleted, confirmed NotFound, and CommittedCleanupPending release a retained
manager fence: durable absence or the durably published frozen record excludes
normal Session/allocation admission. CommittedDurabilityUncertain retains the
fence. Recovery never substitutes for unproven native writer retirement. Duplicate
recovery uses existing idempotent cleanup/finalization and idempotent fence release.

A client-side unknown outcome requires authoritative observation, not cleanup
recovery or mutation replay. Only server-confirmed committed outcomes grant the
explicit recovery action. These recovery semantics remain in App Server v24;
native Runtime Client is v44.

## Rich historical Trace inspection (#364)

The current protocol retains bounded TraceRecord summaries in place of TraceEntry
and provides
`session/traceDetail { target, record_id } -> { type: "trace_detail", detail }`.
Detail is nullable when no allowlisted record exists at the captured read cut.
List payloads never carry complete request contexts or Tool results. Inspection
reads do not mutate runtime state or advance live cursors. See [Trace](trace.md)
for native ownership, limits, allowlists and the historical read boundary.

## Server-resolved Trace presentation relationships (#372)

Runtime Client 42 -> 43 and App Server 12 -> 13. `TraceRequestSummary` gains
mandatory `system_prompt` (a closed request-relative System Prompt state plus a
bounded preview) and `context_additions` / `context_truncated` (the canonical
request Context that exact request introduced, in frozen snapshot order).
`TraceRecord` gains `originating_tool_call_id`, the exact outer `ToolCall` of a
Background, Subagent or Workflow record. All three are resolved by native
authority before they reach a client; no client infers them. `TraceLifecycle`
is unchanged and never repeats them. This v24 vocabulary includes v12's
read-only native `agent/transcript` contract and these Trace DTO changes.
Version 19 and earlier clients are rejected without a compatibility decoder or a
dual Trace DTO path.

## Session archive preparation

`session/exportPrepare { session_id }` returns a `session_archive` result with a
short-lived, single-use native download descriptor. All clients consume the same
`rustx-session-archive/v2` stream. The request has no output-path field. Remote
HTTP(S) downloads share the App Server listener; owned stdio children advertise a
loopback stream port. See [Session archive](session-archive.md) for cut semantics,
authentication, resource bounds and cancellation. Durable SQLite remains v41.

Archive preparation failures preserve a closed safe reason through
`archive_preparation_failed`, plus the fixed native diagnostic in `message`.
Unknown Session/capacity use their existing failures. No raw storage/provider
error is projected. See [Session archive safety and errors](session-archive.md).


## Read-only native Agent conversations (v24)

`agent/transcript { target, agent_id, before, limit }` returns the existing
`transcript { page }` result. `target` is the **parent** AttachmentTarget (Session,
Conversation, runtime incarnation, attachment). App Server validates that route;
Runtime Client checks its live runtime; the parent's SubagentRegistry resolves the
exact committed AgentId to its owned child Conversation. No caller-supplied
child ConversationId or filesystem path is accepted. Unknown identity yields
`unknown_agent`; an owned child whose existing history cannot be read yields
`agent_history_unavailable`. Both carry the exact AgentId. Missing stores
are never created by inspection, and storage diagnostics are not exposed.

The registry acquires existing Conversation allocation access and opens the
identity-validated SQLite store read-only. The shared Runtime Client transcript
reader uses the child's durable transcript cursor/projection and response-fact
decorator, exactly as root reads do. App Server only routes/translates. This path
reads committed history, not uncommitted streaming tokens. It works while the
child runs or waits and after success, failure or cancellation while ownership
and durable history remain retained. Workspace retention/disposal is independent
of conversation history. Lifecycle status never supplies transcript settlement.

Pages use the same native limit validation (1–256) and exclusive `before` cursor
as root reads; clients default to 32 entries. Limits outside 1–256 yield
`invalid_params` before child ownership lookup or durable-history access, regardless
of whether the child is unknown or its history is unavailable. A newest read omits
`before`, and `next_cursor` is the explicit older continuation. Viewing does not admit a child
Session, acquire a child controller, activate a runtime, or grant execution,
Composer, steer, cancellation, permission, model, Goal or lifecycle authority.
Live child HITL still reaches the root queue and is answered only through its
exact routed InteractionRef; historical interaction audit is merely transcript.

The TUI reads only one finite child page. On its newest page it polls this read
operation every 1.5 seconds, with at most one refresh in flight. Browsing older
pages pauses polling; Home returns to newest. Parent attachment epochs fence
reads across repair, replacement and release. A disposable child-view generation
also fences selection, page replacement and closing; neither stale successes nor
stale errors update a replacement. Resync/reconnect remembers only the selected
AgentId, reconstructs through the new parent authority, and explicitly shows
unavailability if lookup fails. It does not reuse cached child history or replay
mutations.


## Exact Job and Agent synchronization (v24)

Job status is non-blocking and `job/wait` waits on the registry's exact finite
Job ID through physical settlement. No client poll loop discovers completion;
`job_updated` and canonical inbound publish terminal facts proactively.

`agent/sendMessage` accepts only stable AgentId and message. Active-vs-Inactive
arbitration, resume reservation and guidance/seal ordering belong to the registry.
Stopping returns a deterministic transient rejection. `agent/wait` captures the
current activation once; its result names that activation even if the Agent has
since resumed. Inactive returns a null target. Clients must not automatically
retry a lost Agent wait, since a new operation could capture a later activation.
`agent/interrupt` ends only its captured current activation and preserves identity.

Replay/reconnect reconstructs authoritative Job terminal states and folds all
activation facts under one stable Agent identity. Clients render separate Job and
Agent rows and preserve selection by AgentId. Child transcript reads span the same
canonical conversation across activations; final reports are never reconstructed
from diagnostic/activity snapshots.

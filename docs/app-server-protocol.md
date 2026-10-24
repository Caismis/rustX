# App Server protocol v57

App Server v57 adds durable Scheduled Workflow recovery controls and real
Scheduled Workflow/interaction origins to the exact-node Scheduler surface.
Runtime Client v76 carries these origins and nullable outer ToolCall identities.
It retains the native Workflow `model` node,
its Text/JSON Schema output union, registered Model/Profile authoring choices,
`requires_attempt` dependency diagnostics, separate Model Call accounting and
node-to-Auxiliary CallId observations. Earlier peers are rejected, and generated
bindings describe the one current vocabulary. No older Workflow reader is retained.

The current protocol retains `jsonSchemaOutput` in the closed `ModelCapabilities` wire
object (the native `json_schema_output` capability). Initialization requires v57 and
WebSocket negotiation requires `rustx.app-server.v57`. v56 and earlier peers are rejected
before attachment or configuration/catalog reads; Web and TUI upgrade together.
Rust remains the schema authority, and validators keep rejecting unknown fields.
Runtime Client v76 carries Workflow Model observations. Child IPC v35 carries typed Workflow/interaction origins; its model views and
frozen Hello specifications retain the JSON Schema capability.

App Server v57 and Runtime Client v76 expose the shared Summary/Title model policy,
independent Title model views, generated Session title metadata with provenance,
and bounded auxiliary retry facts. WebSocket negotiation requires
`rustx.app-server.v57`; earlier peers are rejected. Session display precedence is
manual name, generated title, opening preview, then identity fallback.

App Server v57 retains the removal of product count/size caps from generic workspace uploads
and advertises transport flow control only. See [Generic file upload policy](#generic-file-upload-policy-v57).
It retains bounded conversation observation, identity receipts, content range reads
and atomic subscription acceptance introduced in v50 (#467). It also retains
the source-scoped automatic `rustx.toml` repair receipts introduced in v47 (#464):
`SourceSettings.user_repair` / `workspace_repair` carry each source's latest
bounded, redacted `ConfigRepairReceipt`, and the
`configuration/sourceRepairChanged` notification announces a changed receipt
without any Session or attachment target. See
[Automatic source repair receipts](#automatic-source-repair-receipts-v50).
Earlier App Server clients and servers are rejected.

## Bounded observation and acceptance

A native Replay gap and an unavailable observation source are distinct. Healthy
Replay eviction permits `session/resyncRequired` and a fresh checkpoint. A source
failure instead emits bounded `session/observationUnavailable { target,
registration_id, message }` without a cursor. Admission and checkpoint validation
reject the unavailable source; a retained historical cut is not evidence of live
serviceability. The exact registration fences its unaccepted events at the
transport acceptance boundary. Clients preserve their last Applied Cursor and
loaded state, enter SourceUnavailable, and stop automatic checkpoint recovery.
An explicit checkpoint request may wake one retry of the failed derived read on
the existing Projection worker. Concurrent requests at the same fence share that
physical read and retain the checkpoint deadline. Only its successful current-cut
installation restores serviceability; another failure stops with typed unavailable.
After source repair, explicit refresh can obtain a serviceable checkpoint and new
subscription. Neither transition repeats a mutation or settles execution.


App Server v57 and Runtime Client v76 implement separate attachment, observation
and execution lifetimes. `session/attach` returns only `attached { target,
configuration_revision, attachment_kind, observation_source }`. The connection's
`session/attachmentStatus { session_id }` returns its current Target/revision or
an absent Target, without loading or attaching. Use it to reconcile an uncertain
Attach receipt on the same open connection.

Root startup is Attach → Checkpoint(C) → Subscribe(C) → Following.
Compaction display identifies the immutable context checkpoint and its activation.
Derived summaries belong to model-context projection and never become canonical
messages; bounded observation preserves that native ownership.
Projection detail returns the exact captured `read_cut` used to select its items.
Each page retains that cut for deferred reads, including when later pages were
selected at a newer cut. Original provenance preserves its native conversation
scope and digest; oversized source identities are deferred behind canonical
metadata locators. Derived provenance carries an exact bounded prefix and an
explicit truncation indicator. `provenance_source` exposes the complete original
provenance through the existing selected-value range reader. Historical projection
sources use native activation and item coordinates, validated at the retained cut;
clients must not substitute a newly fetched checkpoint for that evidence.
`session/checkpoint { target, trace_records? }` returns `checkpoint { checkpoint }`:
a bounded display shell, exact cursor/read cut, content bindings and collection
frontiers. `conversation/checkpointPage { target, agent_id?, cut, domain, offset,
limit }` loads advancing finite collections at that cut. `conversation/contentRead
{ target, agent_id?, content_ref, offset, max_bytes }` returns at most 65,536 decoded
bytes as Base64, exact end/continuation and readable frontier. A valid reference
read beyond its authorized frontier returns the distinct `invalid_range` error;
the frontier itself is normal EOF. UTF-8 chunks may split code points;
concatenate bytes before decoding. References identify only
CanonicalMessageBlock, DurableTranscriptAudit or LivePublicationPrefix, and grant
no authorization. Runtime/registry and durable/publication owners validate all
source, identity, revision and readable-frontier evidence.

`session/subscribe` returns `subscribed { target, registration_id, after_cursor }`.
Its connection-local registration is PendingReceipt until the serialized receipt
is physically accepted, then Following. Events and Resync carry the exact
registration ID. RecoveryRequired fences every queued but unaccepted ordinary
event, permits its exact Resync, and requires a new checkpoint/subscription.
Resync includes a finite reason, `checkpoint_required` and optional
`failed_at_cursor`; it does not assert what the client applied. A predecessor's
Resync cannot invalidate a successor. Replay remains the native cursor history.

The final registration check, already encoded candidate selection, physical
acceptance attempt and acceptance-state commit hold the same registration mutex.
The acceptance lock order is carrier retirement fence → route table →
registration → native host/projection; native
subscription replacement and App Server installation occur in that boundary.
The WebSocket writer takes the Socket mutex, checks readiness/previous Flush,
then acquires registration only around tungstenite `start_send`. Replacement
acquires neither Socket nor the carrier fence. Teardown marks retirement under
the carrier fence and releases it before closing connection routes. Unaccepted
records are discarded without readiness; physically accepted Stdio records still
finish their remaining bytes and delimiter. Delivery Access retains its existing
revocation/publication boundary. There is no reverse native-to-route callback.
No readiness, Flush, file read or projection materialization awaits inside a
registration critical section.

Writer outcomes are Accepted, Pending, Discarded and Failed. Pending accepted
nothing and is revalidated. Retired notifications are Discarded before readiness;
replacement wakes a Pending writer so a stale queue head advances. Superseded
unaccepted receipts select a pre-encoded bounded `subscription_superseded` error
under the original RPC ID. After Stdio accepts the first positive write it finishes
that exact record and newline, even if registration is subsequently replaced.
WebSocket Flush is not a second semantic acceptance. The client's Applied Cursor
advances only after complete parsing and contiguous fold; TransportAccepted is
not an application ACK.

Request IDs are String or JavaScript-safe Integer, and their serialized JSON fits
256 bytes. Validation precedes method/domain/capacity admission; invalid envelope
IDs return Invalid Request with null ID, while invalid cancellation parameters
return correlated Invalid params. Duplicate in-flight IDs are rejected and exact
admission tokens fence cleanup. IDs remain associated through response disposition
and residual physical work; this is neither durable idempotency nor mutation replay.

Historical Trace detail uses `session/traceHistoryDetail` to capture a bounded
native `CheckpointPage` for the exact selected node. `session/tracePage` and
`session/traceValueRead` continue that immutable source cut; deferred canonical
body references use `session/traceContentRead`, with the same 65,536-byte decoded
limit and indexed source reads as live content. These methods validate the node,
Conversation and source cut on every read. A live node's native Projection bounds
the authorized frontier; an inactive node uses one durable native read cut.
Historical content reads reject LivePublicationPrefix references. They grant no
followable live cursor and cannot activate a node or run a model.

Mutation receipts carry bounded identity/revision outcomes. Configuration writes
return `sources_written { revision, configuration_revision }`; reconciliation and
adoption return `configuration_receipt { scope, version }`; model selection returns
`model_set { settings_revision }`. Independently read current state afterwards.
`configuration/sourcesRead` returns a bounded source projection. A small view is
inline; a larger view has every native field as an inline value or a finite
`SourceFieldRef`, tied to a digest of the exact configuration-owner read cut.
`configuration/sourceValueRead { target, reference, offset, max_bytes }` returns
at most 65,536 decoded bytes of deterministic JSON without allocating the full
encoded field. `configuration/sourcePage { target, revision, field, offset, limit }`
pages native resource revisions, provenance, Agent definitions and MCP inventories
with at most 32 items and 16 KiB inline per item; larger individual items retain an
advancing byte reference. Whole-view, field and row inline values also reject
arrays or maps exceeding 32 entries, even when their encoded bytes are small.
Finite `collections` frontiers describe native totals. Source model selectors
`session_models`, `user_models`, `workspace_models` and `resolved_models` provide
advancing catalog/authoring pages. A changed native source cut returns `read_cut_expired`.
A source write receipt retains the exact candidate digest even when another writer
commits before its response; clients independently read the current source view.

`session/settings` also returns a bounded projection at its exact durable settings
revision. Deferred settings use `session/settingsValueRead { session_id, reference,
offset, max_bytes }`, with the same 65,536-byte chunk maximum, checked byte evidence,
and explicit read-cut expiry. The native Session catalog remains the settings owner.

`session/effectiveConfiguration` returns the native configuration/application
owner's bounded 18-field projection. Its application and process bindings share
the existing native publication fence with the runtime generation.
`session/effectiveConfigurationValueRead` reads exact deferred fields, and
`session/effectiveConfigurationPage` pages source revisions, provenance and
available tools with the same byte and item limits. Cut digests cover every native
field; changed evidence returns `read_cut_expired`.

Fork/Branch return an immutable `editor` locator instead of editor bytes.
`session/editorRead { locator, block_index, offset, max_bytes }` reads one bounded
source canonical block and remints destination-owned upload receipts. Destination
origin must exactly match the locator; source retention failure stays explicit.
Response loss never authorizes repeating a mutation. Serialization overflow becomes
a bounded correlated error, retaining Delivery Access publication ownership and
cancellation/revocation precedence; actual physical I/O failure closes the connection.


App Server v50 adds exact Message-positioned windows and the immutable inherited transcript boundary. Runtime Client v76 carries the boundary independently. App Server v50 also exposes `session/node { session_id, node_id } → node { node: SessionNode }`, an exact native metadata read. v45/v67 peers are rejected before initialization.

App Server v57 / Runtime Client v76 share one Session workspace across Agents
and Workflows (#462). `RuntimeClientAgent.workspace` is the frozen Session
binding path; the isolation, resource-state and handoff object is gone, and
`subagent/disposeWorkspace` with its `workspace_disposed` result is removed.
Workflow views carry no Candidate or handoff fields, Review subjects are Plan
only, the `review_invalidated` settlement is removed, and Session deletion
reports no `workspace` blocker. The five Agent controls (`subagent`,
`list_agents`, `send_message`, `wait_agent`, `interrupt_agent`) are composed
automatically for the Root Agent whenever it admits at least one Agent profile;
capability inspection reports them as `agent_controls`, and authoring them as
ordinary tool selections is refused.


The protocol includes complete child turn directories and cut-bound history positioning in
both the native RuntimeAttachment and App Server. WebUI parent/child views share
the Harness turn navigation surface, including distant unloaded turns, previews,
active marks and return to latest. Runtime Client's internal vocabulary also
exposes the child reads; its version advances independently.

App Server v57 / Runtime Client v76 distinguish adopted runtime, Agent and mixed
inputs with `TraceKind::Context`. Only wholly human message batches are `User`.
This classification is native and independent of preview text or adjacent records.
There is no old-vocabulary fallback.

The protocol also provides child-owned `agent/trace` and
`agent/traceDetail`. The parent AttachmentTarget grants access; `agent_id`
selects the registered child's own durable Conversation, never a client path.
`agent/trace { target, agent_id, before?, limit, records? }` returns the existing
bounded `trace { page }` vocabulary, with independent opaque child cursors and
lifecycle repairs for loaded interests. `agent/traceDetail { target, agent_id,
record_id }` returns `domain_page { page }` for that child only. Its finite
header names exact deferred fields; `conversation/checkpointPage` and
`conversation/checkpointValueRead` retain the returned cut and Agent identity. Page limits,
byte bounds, heavy-detail redaction and missing-record behavior match the root
Trace. These asynchronous read-only methods neither resume nor settle a child,
work after child process exit, and do not depend on its live inspection socket.
`SubagentObservation.attempt_id` is the exact current child Attempt, cleared at
settlement. Live repairs require that identity and exact request/tool identity;
durable terminal facts always win. Clients must upgrade together to v57/v76.


Child statistics remain available through `agent/statistics { target, agent_id }`.
The result is `agent_statistics { metrics }`: child-owned `ConversationStatistics`,
request-owned context occupancy, and cumulative working intervals (`settled_ms`
plus an optional active interval). The finite occupancy view preserves native
counts and breakdown; a large historical model is deferred under its immutable
request, Compaction Journal sequence, or inherited source. `model_source` is read
through checkpoint ValueRead with item offset zero and an empty path, retaining
the exact checkpoint cut across all chunks. It never selects the current model.
Usage, cache hit, request coverage, turns,
steps, model/tool time, TTFT and decode speed share the main conversation's
native definitions. Historical inherited execution is excluded from child usage.
The read uses only the parent's owned child store, never starts or resumes a
child, and is independent of transcript pagination. It reads one finite aggregate
attached to the original native Journal rows, together with the captured occupancy
header. No original event, request or canonical message body is hydrated for
statistics, and no per-Child read cache is retained. Missing required aggregate
evidence returns an observation-source error rather than an older total.
Active clocks carry native
`started_at`, `observed_at` and `running`; non-running clocks freeze at the last
observed fact. `running` is current driver execution evidence, not `AgentState ==
Active`: Stopping may still own work. A durable Attempt terminal ends its interval
before physical cleanup; missing terminal evidence alone never permits a clock
to advance. No wire shape or protocol version changes are required by this
semantic correction. Clients must upgrade together; previous versions are rejected.

The latest Turn clock preserves exact native timestamps even when a retained
Attempt ID is too large to inline. `attempt_id` is then null,
`attempt_id_deferred` is true, and `attempt_id_source` identifies the original
AttemptStarted Journal sequence and top-level `attempt_id` field. Read it through
`conversation/checkpointValueRead` with domain `journal.<sequence>`, item offset
zero and the advertised path. Continuation retains the first captured cut. The
server checks the original finite Journal kind and source ownership; a modified
locator cannot select another field or widen the cut. No substituted Attempt ID
is created.

App Server v50 supports transport-granted committed-delivery access (#454):
`delivery/read`, `delivery/locate`, the `session_file_location` result and the
per-connection `ServerCapabilities.delivery_access` report. Only transport
authentication grants the access. See
[Committed delivery access](#committed-delivery-access-v50).

App Server v57 / Runtime Client v76 refresh retained Trace input ownership at the
current read cut. See [Trace retained-input ownership](#trace-retained-input-ownership-v50).

App Server v57 / Runtime Client v76 separate Session configuration-adoption
eligibility from configuration application state. `ConfigurationApplication` no
longer carries `eligibility`; the live runtime publishes it on its Runtime Client
snapshot (`configuration_adoption_eligibility`) and changes it only through
`configuration_adoption_eligibility_changed`. See
[Session configuration adoption eligibility](#session-configuration-adoption-eligibility-v50).
Only the one current version is accepted.

The preceding v34 separated upload preparation/status from a bounded binary
carrier and added native upload operation correlation (#434).

App Server v33 / Runtime Client v57 added caller correlation to `context/compact`
and bounded native release/result evidence in the context projection (#435).

The preceding v32 generation removed the forgeable public Session-file read method introduced
in this PR's reviewed v31 state. Product Host file access now uses a separate
credential-authenticated native seam, outside the ordinary Method vocabulary.
Runtime Client v57 added typed canonical delivery facts. The preceding main's
App Server v30 introduced cut-bound native turn outlines and transcript windows.

The previous App Server v29 / Runtime Client v57 combined main's Harness semantic ledger with
required configured Tool inventory, capability-gated image admission, and Bash
presentation metadata. Every earlier version is rejected; there is one current
mandatory vocabulary and no compatibility decoder.

App Server is the only external product control protocol (#428): a bare
`rustx` prints help, and no process serves the Runtime Client envelopes to a
client. Runtime Client v57 removed its Session catalog requests, results and
views; App Server v30 replaced the root transcript paging vocabulary for Issue #430.

App Server v28 / Runtime Client v53 introduced bounded canonical Tool argument
previews in Trace summaries. Full structured arguments remain in the bounded
detail path. Current incremental read-domain and Session catalog ownership,
finite Jobs, and durable Agents retain their native authorities.
Agent send-message success acknowledges child canonical acceptance. A Delegate or Guidance
frame proven unwritten returns `agent_not_delivered`; acknowledgement
loss after a write attempt returns `agent_delivery_unknown` and
must never trigger automatic replay. Both identify the durable Agent.
Job wait/cancel returns a bounded native lifecycle receipt or the typed
`job_publication_abandoned { job_id }` failure when its owner has exhausted
publication and relinquished all remaining callbacks. This failure never
turns a retained publication candidate into a canonical terminal result.
`job/list` returns a bounded `domain_page` at one registry cut. The page carries
native positions, exact totals and advancing continuation; growing Job fields
remain deferred sources. `conversation/checkpointPage` loads subsequent pages.

Version 27 / Runtime Client 52 add `read_domains_updated`: native bounded decorated
transcript (including response/TurnProcess/statistics), occupancy and Todo facts.
Tool assembly carries exact native `arguments_json`; `session/trace` accepts bounded
loaded record identities and returns independent `page.updates` lifecycle repairs.
Ordinary publication suffixes remain deltas. Clients require contiguous exact
cursors; snapshots initialize, recover, or explicitly reconcile. The Web reducer
and Chat frame contract are documented in [CHAT.md](../web-console/CHAT.md).

`jobs`/`job_updated` carry
`job_id` and terminal lifecycle; `agents`/`agent_updated` carry stable `agent_id`,
parent lineage, child ConversationId, current/latest activation identity and explicit
Admitting/Active/Stopping/Inactive/Unavailable state. Controls route directly to each domain owner. A
resumed activation updates the same Agent row. Only current v57 generated artifacts are retained, following repository policy;
Older peers are rejected without a compatibility decoder. See
[Jobs and continuable Agents](jobs-and-agents.md).


The reviewed #406 contract introduced App Server v23 / Runtime Client v49. The
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
Only App Server v57 / Runtime Client v76 are supported. Earlier peers are rejected without compatibility paths.

The earlier v22/v49 revision introduced native whole-conversation Turn/Step
totals, measured request timing, the latest exact Attempt clock, and authored
`SessionModelsView::Available.default_model` from the same creation capture as
its catalog. The current v57/v75 pair retains these capabilities. A browser product preference can
seed new Session intent; it does not alter authored configuration or existing
Sessions.

The historical v21/v50 revision introduced `completed_process` on transcript
entries, using committed Assistant identities and exact Tool occurrence owners
to identify a successful Attempt and its final response. v27/v53 replaced that
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

App Server v50 also carries v16's producer identity on Trace context additions
and typed accepted contributions on request detail. These are historical
RequestSnapshot facts, not live Todo/Goal authority. See
[native contribution lifecycle](native-context-contributions.md)
for atomic startup and same-step reuse. Generated v14 artifacts are removed.

App Server v50 identifies one complete mandatory vocabulary, including exact
`session/summary`, bounded historical Trace detail, and read-only Subagent
transcripts. v12 and all earlier initialization and WebSocket admission versions
are rejected; there is no downgrade or compatibility path.

The App Server protocol is rustX's public client boundary for the TUI,
Developer Web Console, future Web UI, and SDKs. Rust DTOs in
`src/app_server/protocol.rs` are authoritative. `AppServerConnection` in
`src/app_server/connection.rs` is the transport-neutral semantic endpoint: it alone
admits requests, reserves and registers attachments and accepts final replies, then
routes each admitted request by an exhaustive request-family classification to the
catalog, history, configuration and runtime handlers under `src/app_server/connection/`.
`src/app_server/rpc_error.rs` is the one native-failure to JSON-RPC error mapping.

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
  "rustx.app-server.v50",
  `rustx-token.${dedicatedTransportToken}`,
]);
```

The server requires both offers on path `/` without a query, rejects failed admission
with HTTP 401, and selects only `rustx.app-server.v57` in its response. It never echoes
the credential. A client the operator trusts with committed-delivery bytes may also
offer `rustx-delivery-access.<separate secret>` (`--delivery-access-token-file`); a
wrong or unconfigured value fails the whole handshake, and it never substitutes for
the transport token. Admission completes before constructing `AppServerConnection`, so
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
endpoint/token; the browser then connects directly using the v50 subprotocols above.
The browser launch credential is never a valid substitute for the native credential.
Remote Web attachment is explicit Settings configuration. Neither browser login
nor remote attachment grants Product Host Workspace filesystem authority.

### Finite delivery policy

| Bound | Value | Terminal behavior |
| --- | --- | --- |
| Complete JSON-RPC record | 1,048,576 encoded UTF-8 bytes | Reject oversized input before dispatch; substitute a bounded correlated response error or exact-registration Resync for oversized output |
| Stdio read buffer | 8 KiB | Incremental bounded record assembly |
| In-flight request-ID associations per connection | 16 | Bounded request-capacity refusal; eight Child observations, six ordinary RPCs and two reserved controls |
| Outbound queue per connection | 32 messages | Pause candidate production; a completed reply waits at most 10 seconds for capacity |
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
Transient saturation pauses native notification consumption before candidate
materialization. Each admitted request may wait for a queue slot under its existing
request association; it acquires the slot before encoding the reply. At most one
immediate, unadmitted-input reply may wait separately, after which input is
backpressured. The serving loop still polls the writer, shutdown and already
admitted requests and controls. No extra encoded queue or semaphore is introduced.
A completed reply has one absolute ten-second capacity deadline; the subsequent
physical write has its own unchanged ten-second deadline. Polling does not reset
either budget. Sustained inability to accept output retires the physical connection;
all attachments share that transport, but their native execution continues.
Teardown drops input/request-waiter futures and explicitly closes the attachment
connection. It retires unaccepted records and joins the physical writer; an accepted
Stdio record retains its remainder and delimiter within the write deadline.
Already admitted runtime operations continue under their existing server owners.
A mutation whose response is lost has an **unknown outcome**: no transport retries it.
Reconnect with initialize/attach/checkpoint/subscribe and inspect authoritative state.

The Web typed request layer gives `agent/wait`, `job/wait` and
`agent/sendMessage` no response deadline: these operations can legitimately await
native admission, durable acceptance or physical settlement. `agent/interrupt`,
`job/cancel` and `turn/cancel` return finite cancellation-intent receipts; they
never wait for physical settlement while occupying reserved control capacity.
Their lifetime is not transport liveness. Ordinary RPCs retain the 30-second
response deadline and existing response-loss disconnect behavior. Actual socket
close/error settles each pending operation once; transmitted mutations remain
uncertain, waits are abandoned without cancellation, and no request is replayed.

Web admission uses the server's eight observation, six ordinary RPC and two
reserved control positions. Ordinary positions include lifecycle waits and
message admissions. Lifecycle controls (`agent/interrupt`, `job/cancel`,
`turn/cancel`) use the reserved positions.
`turn/cancel` consumes neither ordinary RPC capacity nor its 30-second response
deadline; ordinary RPC saturation cannot delay an otherwise admissible exact
cancellation. A full control lane rejects new local control admission before send,
just as other full domain lanes reject new local requests before transmission.
The existing 64-request queue/uncertainty bound remains; cancellation has no browser
queue, retry or replay. Socket loss after transmission still leaves cancellation
uncertain under the response-loss contract above.
Waiting cannot occupy inspection/control slots, and message admission cannot occupy
interrupt/cancel slots. Connection generations and attachment-target checks still
fence result adoption after reconnect, detach, or target replacement.

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
exercise transient and sustained queue saturation, request admission saturation and a paused-clock write
deadline; timeouts elsewhere are only outer liveness guards.

## JSON-RPC and initialization

Requests use JSON-RPC 2.0: `jsonrpc: "2.0"`, a string or integer `id`, a `method`,
and typed `params`. Responses contain the same ID and exactly one of `result`
or `error`. Notifications have no ID. IDs correlate requests only; repeating
an ID does not deduplicate a mutation. Integer correlation IDs must fit the
JavaScript safe integer range. String IDs are recommended for arbitrary IDs.

```json
{"jsonrpc":"2.0","id":"init-1","method":"initialize","params":{"protocol_version":56,"client":{"name":"example","version":"1"},"presentation":{"images":true,"questionnaires":true,"reviews":true}}}
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
supported in v50; pipeline individual requests instead. This limitation is
explicitly rejected as an invalid request before any action occurs.

## Methods and native owners

| Methods | Owner and semantics |
| --- | --- |
| `initialize`, `server/info` | Connection negotiation and server capabilities |
| `session/list`, `session/read`, `session/summary`, `session/name`, `session/tree` | Durable controller; bounded pages / exact identity reads; no runtime composition |
| `session/create` | Durable creation from explicit Session selections |
| `session/fork`, `session/branch` | Selected SurfaceRevision R fixes structure; invocation-time native read cut C fixes temporal facts. A message boundary further narrows structure, with explicit `side`: `before` restores ordinary User input; `after` includes a durably completed Assistant response with an empty editor. Fork without a boundary clones structure through R into an independent Session |
| `session/deletePreview`, `session/delete`, `session/recoverDeletion` | Native revision-confirmed deletion/recovery; no client-supplied cleanup workset |
| `session/attach`, `session/detach` | Load/reuse a runtime and acquire/release its external control attachment |
| `session/switchNode` | Switch to a selected branch; manager owns retirement and composition |
| `session/checkpoint`, `session/subscribe`, `session/transcript`, `session/turns`, `session/boundaries` | Authoritative projection, bounded replay, cut-bound transcript windows/native Attempt outline and lineage-specific user-message pages |
| `turn/start`, `turn/steer`, `turn/cancel` | Native inbound and attempt-cancellation owners; acceptance is not terminal execution |
| `interaction/respond`, `interaction/cancel` | Originating runtime/coordinator, including routed child interactions |
| `session/settings` | Durable resolved Session selection; model mutation uses `session/setModel` |
| `session/model`, `session/models`, `session/setModel`, `session/selectModel` | Bounded attached Session model/catalog reads and finite mutation receipts; complete authored selections or catalog defaults at exact native binding revision and ordinals |
| `configuration/sourcesRead`, `configuration/sourceWrite` | Native structured User/Workspace documents and exact revisions; bounded semantic-unit CAS writes |
| `session/effectiveConfiguration`, `configuration/reconcile`, `session/adoptConfiguration` | Authoritative application state, native rescan/retry, and fenced explicit Session adoption |
| `context/compact`, `goal/control` | Existing maintenance and Goal owners |
| `job/list`, `job/status`, `job/wait`, `job/cancel` | ConversationBackgroundRegistry: bounded finite Jobs, immediate status and cancellation-intent receipt; the separate exact terminal wait reaches physical settlement or typed publication abandonment |
| `agent/statistics` | Parent `AttachmentTarget` → exact parent registry Agent ownership → existing child store → incremental native usage, timing and occupancy; read-only, no child activation |
| `agent/transcript`, `agent/turns` | Parent `AttachmentTarget` → current parent Runtime Client authority → exact parent `SubagentRegistry` ownership resolution of caller-supplied `AgentId` (never arbitrary child `ConversationId`) → exact owned child Conversation, which remains history authority → bounded read-only durable transcript projection; grants no execution, control, or HITL authority |
| `agent/list`, `agent/status`, `agent/sendMessage`, `agent/wait`, `agent/interrupt` | Durable Agent owner: atomic message/resume arbitration, captured activation wait and finite intent receipt for interruption of that exact reserved/current activation |

`turn/start` and `turn/steer` both submit native inbound content. The runtime
decides whether it starts a fresh attempt or joins the inbound queue of the
current attempt. Neither method creates a separate execution state machine.
Callers learn accepted message identity and inbound sequence from the result.

`turn/cancel` asks the native coordinator to select and cancel its current
Attempt under one coordinator guard. A stale or unavailable presentation view
cannot choose the target or refuse cancellation. The receipt identifies that
exact accepted cancellation, independently of later settlement or replacement.
Web and TUI cancellation gestures use current connection and exact attachment
control authority, independently of observation readiness. An unavailable or
resynchronizing display must not disable a fresh cancellation gesture. A retained
gesture still carries its original attachment generation: reconnect, replacement,
detach or loss of control permission fences it before dispatch. Send, edit and
other commands that require an exact display baseline retain their stricter
observation guards. No client guesses a replacement Attempt or retries cancellation
because its response was lost.

`context/compact` returns the five fields of that invocation's successful native
compaction outcome in `context_compacted.compaction`. It never substitutes
`latest_compaction` from observation, and does not await observation catch-up.
A later operation cannot replace an earlier receipt. Transport loss preserves
existing explicit reconciliation and never triggers automatic compaction replay.

`session/setModel` validates a complete authored selection. `session/selectModel`
selects catalog defaults through `{ binding_revision, model_position,
profile_position? }`. Positions are native catalog ordinals from the bounded
catalog pages, including deferred entries; they are never indexes in a filtered
display head. A changed native binding revision rejects the choice before commitment.
Catalog publication advances that existing binding revision independently of
capability resource generation; the two revision domains are not interchangeable.
This permits exact model and profile identities larger than the request envelope
without copying those identities into a mutation request. Choosing catalog defaults
replaces primary-model overrides and output limits with catalog defaults while
preserving the independently configured Summary policy.
Both methods use the same existing capture/resolve/commit owner and return only
the committed settings revision. Neither successful response
replaces the active Attempt's frozen `AttemptModelView`, including when only a
Profile, output budget or request parameter differs. Subsequent independent
Attempts capture the new selection; child work from an older Attempt inherits its
parent's captured authority. General `session/adoptConfiguration` retains its
separate Busy/readiness rules.

Selection errors distinguish invalid complete selections, stale candidates,
unavailable/retired runtimes, persistence failures and committed durability
uncertainty through the existing typed error vocabulary. No error or lost response
authorizes automatic mutation replay. Clients reread Session authority to reconcile
an uncertain result and retain the separately projected running Attempt.
Readback is a current-state observation, not a terminal receipt for an unanswered
request: a native-owned selection may still commit after that observation. A new
deliberate selection uses the existing native revision fences; a true no-op does
not advance them or establish settlement of an earlier request.

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

Protocol v50 admits at most one writable external controller per resident
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

Attach admits the controller and returns a bounded identity receipt. Checkpoint
captures one published native cut independently, without materializing content
under the Projection lock. Subscribe installs a native replay subscription and
connection registration together; its receipt must be TransportAccepted before
ordinary events for that registration. Events concurrent with the checkpoint
remain in the bounded native Replay ring. Cursors are monotonic within one
incarnation; a replay gap is reported as `session/resyncRequired`. Clients load a
new bounded checkpoint and subscribe after its cursor to recover.
Invalid re-subscription leaves the prior registration intact. Transcript cursors
are a separate durable paging domain, not event cursors.

### `session/summaryInvalidated`

`session/summaryInvalidated { session_id, catalog_changed }` is invalidation
evidence only; clients reread native authority. With `catalog_changed=false`,
the named Session's summary/display metadata changed, so reread `session/summary`.
With `catalog_changed=true`, catalog membership may have changed and the named
summary is invalidated; reread `session/list` as well. A deleted Session is no
longer a valid summary target. The native catalog visibility commit emits this
for every membership addition (creation or copy) and removal (deletion), including
visible commits whose directory durability remains uncertain. Deletion announces
removal before physical cleanup; cleanup pending, finish and recovery do not
announce another membership transition when membership is unchanged.

It carries no metadata, claims no durability beyond the catalog commit that
produced it, and is not
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
background queue and no independent scheduler. Display-preview publication is
once-only per Session, so a no-op repair and an already-correct projection
produce neither a catalog write nor a notification. Membership notifications
correspond to additions/removals, not to every catalog write. Lag and disconnect reuse the existing resync
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
references, Workflow revision and run invocation, child
observation revision, compaction generation, inbound sequence, and Todo IDs,
dependencies and allocator position. Natural string identities and default
document/deletion revision strings remain strings. These are distinct domains,
not interchangeable tokens; do not compare decimal strings lexicographically
for numerical order (use `BigInt` locally if necessary, never JSON bigint).

Bounded domain page values and deferred checkpoint/configuration/settings JSON
use the same Rust-schema-derived numeric encoding. The native source serializer
emits exact typed `u64` leaves as decimal text before an opaque page value or
Base64 chunk is constructed. Range reads preserve that encoding across chunk
boundaries. Authored `request_params`, arbitrary JSON Tool results, and raw Tool
arguments retain their source JSON semantics; the encoder does not infer numeric
domains from their property names. `CanonicalJsonV1` content reads remain exact
bytes of their separately defined authoritative structural representation.

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

- `v57.schema.json`: complete JSON Schema generated with Schemars from Rust DTOs.
- `v57.ts`: TypeScript generated from that schema using pinned
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
`next_notification` calls. `tests/scripted/app_server/protocol/admission.rs` runs it through
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

`session/uploadPrepare { target, operation_id, files: [{ name, size }] }` accepts
exactly one file per operation and returns
`upload_prepared { transfer }`. `session/uploadStatus { target, operation_id }`
returns `upload_status { outcome }`: absent, unresolved, failed, or ready with the
original native receipts. Preparation uses exact attachment admission. Bytes use
the dedicated capability socket described in [Session uploads](session-uploads.md),
never JSON. Initialization advertises the finite native upload policy. The previous
byte-bearing upload method is removed; no fallback exists.

Only native ready commits produce usable receipts. `turn/start` and `turn/steer`
accept ordered text and `{ type: "upload", session_id, batch_id, token }` blocks.
Canonical uploaded-file facts are server-authored. Response loss requires exact
operation read repair; it does not permit retransmission.

`artifact/read { target, artifact_id } -> artifact_bytes { data }` remains solely
for presentation of Tool-generated managed artifacts, bounded to 256 KiB. That
conversation ArtifactStore retains its existing capacity (256 identities),
reservation, read, spill and background-output semantics. User uploads never
allocate or read back ArtifactStore identities. There is no artifact upload method.

## Native Trace

`session/trace { target, before?: TraceCursor, limit: 1..32 }` is a mandatory read,
returning `{ type: "trace", page: TracePage }`. Checkpoint also carries the
bounded native `snapshot.trace` window at its exact published cut. Payloadless `trace_changed` notifications
use the existing subscription as invalidation signals. Neither historical reads
nor Trace cursors advance a subscription cursor. See [Trace architecture](trace.md)
for source authorities, ordering, read cuts, repair, bounds and unavailable facts.

Generated Rust Schema/TypeScript, Web Console and TUI negotiate App Server v50
and Runtime Client v76 in their independent version domains.
Earlier versions are rejected; there are no aliases or dual-version paths.

`session/trace` accepts optional `records: TraceCursor[]` (maximum 8) and returns
`page.updates` for ordinary lifecycle refresh without a Session snapshot. Explicit
`session/checkpoint` reconciliation also accepts `trace_records: TraceCursor[]` (maximum 8).
Clients batch their separate bounded cache of 512 records and protected selection
in groups of eight, retaining each response's exact read cut. `snapshot.trace_updates` contains typed
native lifecycle repairs for those identities, including records older than the
newest Trace page. Both the newest page and repairs use the Journal prefix
captured with the returned Runtime Client cursor, never a later SQLite frontier.
Only installed native semantic publications can advance that prefix; a raw SQLite
commit receipt cannot publish it. Historical `session/trace` independently captures
a represented semantic prefix and native lifecycle snapshot on live hosts, without
folding observations or changing the live cursor. Inactive durable inspection
captures its own SQLite frontier and has no live publication boundary.
This remains mandatory protocol v50; no compatibility path is provided.

### Fork editor input

Session transitions return an immutable `editor` source locator. Bounded
`session/editorRead` chunks reconstruct exact text and destination-owned upload
receipts for `turn/start`. Independent
forks copy editor-boundary uploads before publication; same-Session branches share
Session ownership. The canonical destination prefix ends **before** the selected
User message. A client restoring an editor keeps its loaded content uncommitted; a
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
`Active + disarmed`. Native Runtime Client version 43 introduced this vocabulary;
current version 68 retains it and rejects older peers by strict negotiation. This remains mandatory
App Server protocol v50, with no compatibility field and no activation mode.

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

Protocol v50 includes `inbound/edit { target, expected, text }` and
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

Native Runtime Client version 37 introduced the mandatory pending revision and
`pending_inbound_changed` event. Current version 68 retains both with no
compatibility decoder.

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
manual name → generated title → preview → identity fallback.

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
name, generated-title or preview substrings and must never be used as exact identity resolution.
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

`session/configuration` and `configuration/changed` resolve a
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
MCP writes accept environment references and exact-source retained literal key
identities. New literal `env` or `headers` values are rejected by native authoring,
even for direct generic RPC callers. A retained key cannot also be replaced;
omitting it removes it. Native source provisioning remains outside this Web
contract, and general protocol diagnostics omit authoring payloads.

`mcp/probe` accepts `target`, `id`, and `expected_revision`. It checks exactly
that authored MCP source using native credentials and the source's working
directory. It returns `mcp_probe` with `id`, `revision`, and `outcome`:
`reachable`, `connection_failed`, `list_failed`, `timed_out`, `cancelled`, or
`settlement_failed`. Diagnostics never return credential-bearing peer errors.
A changed source revision rejects the check rather than publishing old success.

There are at most four native probes in flight, with no retained native queue;
excess admission is rejected. Initialization and tool discovery share a 15-second
deadline. Cancellation requests shutdown and cleanup is awaited, including when
the RPC waiter disappears. A successful response requires cleanup settlement.
No diagnostic connection is retained or installed in a Session. The WebUI starts
checks from the MCP list and its existing Refresh control; ordinary source reads
and configuration reconciliation remain inert.

There are no `mcp/connect`, `mcp/status` or `mcp/disconnect` settings methods.
Configuration reads never acquire external MCP resources; runtime
connections belong exclusively to the native capability generation owner.

Protocol v50 uses one `SourceTarget`: `{kind:"user"}` or
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

A `configuration/sourceWrite` publishes only into the physical directory it
admitted when it began: a Workspace pathname replaced or redirected before the
publication is refused as `invalid_params` (nothing written), distinct from a
`source_conflict` on changed bytes, and one that stops naming the written
directory after the replacement is `committed_durability_uncertain`.
`configuration/sourcesRead` is bound the same way: it reads each `rustx.toml`
only through the directory it admitted, never through a symlink, and refuses as
`invalid_params` a capture whose directory was replaced or redirected meanwhile.

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
binding revision. `session/configuration` reads the retained binding/candidate
without loading a cold Session. `configuration/changed` publishes its bounded
scope/version and inline-or-deferred application reference. Clients retain the
native version obligation while loading deferred bytes through
`session/configurationValueRead`; the fully reconstructed application is installed
only if its exact scope, version and source ownership remain current.
`configuration_adoption` reports Busy, NotReady, Conflict or a
preparation/commit diagnostic. Adoption is ordered against Attempt admission and
preserves canonical history. `configuration/reconcile` rescans external inputs or
starts a new same-revision application attempt. It is never a second Save step.
See [the native contract](configuration.md#save-automatic-application-and-session-adoption).

A published Model Catalog that no longer admits a Session's configured Model or
Profile leaves it configured and unavailable: `session/model` carries
`unavailable` (the native diagnostic), with the last resolved `effective` and
`summary` as display facts, or neither when the Session was loaded unavailable.
`turn/start`, `turn/steer` and `context/compact` then fail with
`model_unavailable` until `session/setModel` commits a complete selection the
catalog admits; nothing falls back to a default.

## Automatic source repair receipts (v50)

Native automatic repair of the two `rustx.toml` sources is documented in
[configuration](configuration.md#automatic-source-repair-464); this is its wire
contract. Repair runs at App Server startup (User source, before process policy
and transports), before a Session's fresh configuration resolution (User and the
exact selected Workspace), and in `configuration/reconcile`. No other method
writes a source for repair: `configuration/sourcesRead` stays a read.

```ts
type ConfigRepairChange =
  | { kind: "removed_field"; object_path: string } // "" is the document root
  | { kind: "removed_native_tool"; position: number; name: string | null;
      reason: "unknown" | "duplicate" | "agent_control" | "plugin_owned" }
  | { kind: "updated_schema_version"; from: number; to: number };

interface ConfigRepairReceipt {
  id: string;                 // one repair occurrence within this authority
  target: SourceTarget;       // the exact source owner
  status: "repaired" | "unresolved" | "commit_uncertain";
  before_revision: string;    // the exact revision the repair was planned from
  after_revision?: string;    // committed, or observed after an uncertain commit
  changed_count: number;      // every eligible change
  changes: ConfigRepairChange[]; // at most 64
  truncated: boolean;
  backup_available: boolean;  // an exact private copy of the original exists
  diagnostic?: string;        // native category and location only
}
```

A source that never needed repair has no receipt; having no receipt never proves
a source valid (see `receipt: null` below). `repaired` proves only
that authored bytes were committed; whether the configuration was prepared,
applied or adopted stays the `ConfigurationApplication` of `configuration/changed`,
`configuration/reconcile` and `session/configuration`. `unresolved` means the
source is unchanged, either because nothing eligible could fix it — including a
parsable source that violates a source-intrinsic constraint — or because the
candidate failed validation; its `changes` are then the eligible, unapplied ones.
`commit_uncertain` reports the revision observed after a publication that may have
happened; nothing is replayed.

A receipt carries native vocabulary only. Repair edits a document by its exact
authored keys, but reports a separate projection onto the source schema:
`object_path` names the closed schema object an unknown field was removed from,
as schema property names, `*` for an open-map identity (Provider, Model,
Profile, MCP policy, Tool source and similar names) and `[n]` for an array
position — never the removed key itself, which is authored text whatever it
looks like. A removed `agent.tools.builtin` element is reported by its
zero-based `position` and `reason`; `name` is present only when the element
names a native Tool (a duplicate, an Agent control or a Plugin-owned Tool) and
is `null` for `unknown`, since a name no native Tool has is authored text. So
no removed value, credential, `request_params` payload, authored key or
identity, unsupported Tool name, raw parser excerpt, backup path or backup
byte can reach a receipt. A `diagnostic` is assembled from native constants only — a fixed
category, line and column, and a schema location whose open-map identities
(Provider, Model, Profile and similar names) are `*` — so neither an invalid value
nor an identity nor a domain error message can reach it.

`id` is opaque and names one repair occurrence within the App Server authority:
each publication that happened or may have happened gets a new one — a second
commit of byte-identical changes included — while a reread or redelivery of the
same receipt, or an unchanged unresolved observation, keeps it. A client pairs it
with `authority_id`; it never orders anything, which `sequence` does.

`configuration/sourcesRead` returns `user_repair` for the User source and, for a
Workspace target, `workspace_repair` for that Workspace, with `repair_sequence`:
the native ledger sequence both are current at, read with them under one lock.
Receipts live in a finite process-local ledger (one per source, a bounded number
of Workspaces), so a repair committed before any client connected — the startup
repair — is on the first read.

```ts
// notification
{ method: "configuration/sourceRepairChanged",
  params: { target: SourceTarget; receipt: ConfigRepairReceipt | null;
            sequence: string /* exact u64 */ } }
```

The notification is source-scoped: it names a `SourceTarget`, never a Session,
attachment or application scope, and is distinct from `configuration/changed`.
`receipt: null` states only that the native ledger retains no receipt for the
source: it never needed repair, its unresolved receipt was withdrawn — which
happens once the source is healthy under the source-intrinsic contract, never
merely because it parses again — or the finite ledger forgot its entry, or it was
not observed in this process. It is authoritative about the ledger's retained
repair observation, not proof that the document is valid; validity is what the
source read reports (its view `diagnostic`, `prospective_diagnostic`) and native
validation enforces. A client retires any repair notice it presents for the
source and does nothing else because of it: it presents no success, authorizes
no edit, rebases no Settings draft and implies no application or adoption. A
connection observes the User source and each Workspace source it addressed in a
request (`configuration/sourcesRead`, `configuration/sourceWrite`,
`configuration/reconcile`, `mcp/probe`, `session/create`, `session/attach`) —
exactly the sources its own reads could inspect — so no other Workspace identity
reaches it. Notices begin after `initialize`. The interest is finite: the User
and the 64 most recently addressed Workspaces; addressing a forgotten Workspace
again admits it anew. Delivery is level-triggered against the ledger with a
per-connection cursor per source. Each admission of a source — the User at
`initialize`, a Workspace on the request that addresses it — owes the source's
current level once: its latest receipt, or `receipt: null` when it reports no
repair, whether its receipt was withdrawn, never recorded, or forgotten by the
finite ledger. That level carries the ledger sequence it was read at, never
lower than anything recorded of the source; it is a state, not a repair, so a
null level presents nothing. Admitting a source wakes the connection's
notification reader, so the level is delivered without any further ledger
change; every later change of the source follows. A source the connection keeps
observing can still lose its ledger entry, which the finite ledger forgets only
while recording another source and so advancing the sequence. When the level
last delivered to the connection held a receipt, that absence is delivered once,
at the ledger sequence it was read at, with no new admission, so no client keeps
a receipt the ledger forgot; a delivered absence is not repeated however other
sources change, and a later receipt of the source follows at its own higher
sequence. Each cursor therefore records the sequence and whether that level held
a receipt — nothing more per source. The level and its sequence
are read under one ledger lock and become the source's cursor under the
connection's interest lock, so a change committed after that read carries a
higher sequence and is delivered next, and one committed before it is folded
into the level. Addressing an admitted source again delivers nothing.
Notices are not history: a reconnecting client receives each source's level
once more, a receipt id may arrive again at a higher `sequence`, and clients
deduplicate presentation by `id`.

Every change of any source advances the ledger sequence. A notice carries the
`sequence` of its change and a read its `repair_sequence`; within one App Server
authority the higher one is the newer observation of a source, whatever arrives
first — a read answered after a newer notice, or a notice produced before a
fresher read and delivered after it. Receipt ids name occurrences and never
order observations; sequences of different authorities are not comparable.
Two observations of one source at one sequence agree; a client keeps the one it
holds and reads the source again rather than adopt a conflicting one. A higher
sequence always replaces what a client holds of a source, even with the same
receipt id or with no receipt on either side, so a withdrawal or an unchanged
level advances its ordering without presenting anything again. Sequences order
observations of one source, not deliveries across sources: a change notice
carries the sequence of the change that recorded it, which may be lower than
sequences already delivered for other sources. A client that forgets sources
therefore cannot order a change notice of a source it no longer holds by
arrival; it rereads that source (see [TUI and App Server](tui-app-server.md)). Receipts are not persisted, not Event Journal facts and never canonical history.

## Session configuration adoption eligibility (v50)

Whether a Session could adopt a prepared configuration *now* is runtime state,
not configuration state. Native `ConversationRuntime` derives it with the
adoption gate's own idle rule over its in-memory owners — lifecycle and counted
admissions, the foreground attempt and manual compaction, recovery and
durability, live interactions, background work and subagents — and from nothing
about any candidate. The advisory derivation never repeats the gate's two
durable-store probes (pending inbox, Goal continuation authority): whenever
either holds while the runtime is otherwise idle, the admission pass that
follows admits an attempt, which is itself an announced busy owner. The gate
evaluates the complete rule at its commit boundary. Ownership is therefore:

```text
ConfigurationApplication            configuration/application facts (scope, version,
                                    units, candidate); published by configuration/changed
RuntimeClientSnapshot               configuration_adoption_eligibility: eligible | busy |
  .configuration_adoption_eligibility    unavailable — the live runtime's advisory state
session/adoptConfiguration          the final authoritative admission check
```

Activation publishes the first value synchronously; afterwards the runtime's
admission worker re-derives eligibility whenever a lifecycle transition or
counted admission entry/exit is announced, after each admission pass it runs,
and after an absorbing durability failure. The announcement is enabled before
each derivation, so the publication is level-triggered, and derivation and
publication share one lock, so it is published only when the value changes. A
host binds before activation, so every snapshot starts `unavailable`; the first
event of an idle activated runtime is therefore `eligible`, and drain publishes
`unavailable` after `runtime_shutdown` and before its exit boundary.

Invariants:

- `ConfigurationApplication` publication is configuration-domain state; a
  busy/eligible transition never advances an application version.
- Configuration adoption eligibility is runtime-domain advisory state.
- Runtime Client snapshot identity is never a configuration invalidation signal.
- Native adoption revalidates eligibility, candidate identity and expected
  binding at the actual commit boundary; a native refusal is definitive.
- Ordinary streaming Runtime Client deltas publish no eligibility change and
  cause no Session-configuration RPC.

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

Initialization requires exactly v57 and WebSocket requires `rustx.app-server.v57`.
v56 and all earlier versions are rejected without fallback. Rust DTOs generate
`v57.ts`, `v57.schema.json`, and the serialized fixtures; only the current version is kept.
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
explicit recovery action. These recovery semantics remain in App Server v50;
native Runtime Client is v75.

## Rich historical Trace inspection (#364)

The current protocol retains bounded TraceRecord summaries in place of TraceEntry
and provides
`session/traceDetail { target, record_id } -> { type: "domain_page", page }`.
The page has no item when no allowlisted record exists at the captured cut.
Deferred original fields use bounded checkpoint page/value reads at that same cut.
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
is unchanged and never repeats them. This v50 vocabulary includes v12's
read-only native `agent/transcript` contract and these Trace DTO changes.
Version 19 and earlier clients are rejected without a compatibility decoder or a
dual Trace DTO path.

## Session archive preparation

`session/exportPrepare { session_id }` returns a `session_archive` result with a
short-lived, single-use native download descriptor. All clients consume the same
`rustx-session-archive/v9` stream. The request has no output-path field. Remote
HTTP(S) downloads share the App Server listener; owned stdio children advertise a
loopback stream port. See [Session archive](session-archive.md) for cut semantics,
authentication, resource bounds and cancellation.

Archive preparation failures preserve a closed safe reason through
`archive_preparation_failed`, plus the fixed native diagnostic in `message`.
Unknown Session/capacity use their existing failures. No raw storage/provider
error is projected. See [Session archive safety and errors](session-archive.md).


## Read-only native Agent observation (v50)

`agent/observe { target, agent_id, after? }` returns `agent_observe { observation }`.
The three observation classes are `checkpoint`, `events`, and `resync_required`.
A Live checkpoint identifies the exact Agent, child Conversation, Activation and
native cursor. Further requests read that Child's native Replay and wait at most
ten seconds for new events. A DurableOnly checkpoint has no followable cursor;
retained history does not establish an execution outcome. Activation replacement
rejects old cursors. Unavailable source, capacity, read-cut expiry and content
retention are typed read failures, not fabricated execution failure or Resync.

`agent/observeCancel { request_id }` cancels only this connection's exact admitted
observation. Admission installs request ownership, capacity and `(Target, AgentId)`
occupancy before the future is polled. Cancellation before first poll starts no
Child connection. The existing Agent, Tool and activation owners remain intact.
The Child inspection handshake carries read-only identity/capabilities; explicit
bounded checkpoint/event/content/page requests follow. It carries no snapshot.
One absolute fifteen-second budget covers Child observation setup and reads.

Frozen Child canonical message pages keep the `source_cut` supplied by
`agent/transcript`, independently of subsequent live checkpoints. Their
`CheckpointReadCut` carries the exact Child conversation and activation, revision
zero, `through = source_cut.read.journal`, empty domain/collection revisions and
no live-publication or foreground selector. The Registry resolves the owned
Child store for those pages and immutable canonical content bytes even while the
Child is active. Append-only source growth preserves reconstructible history;
foreign conversation/activation identities and inconsistent or mutated cuts fail.

SQLite v50 private Agent authority persists the required `title` independently of its profile name. The
`subagent` tool requires a short title; continuation preserves that title and the
same Agent identity. Workflow children derive titles from their node names.

`agent/sendMessage { target, agent_id, message, attachments }` validates upload
receipts against the parent Session. Active guidance and inactive continuation
carry canonical uploaded-file references through the child IPC protocol. Child
model input resolves uploads with the owning Session, including isolated workspaces.

`agent/artifactRead`, `agent/deliveryRead` and `agent/deliveryLocate` resolve only
an Agent owned by the addressed parent. Managed artifacts use that child's
artifact store. File deliveries require committed successful child Tool output
and use the child's admitted workspace. Private Product Host file reads carry an
optional AgentId and retain the same connection revocation, allocation, bounds
and publication fences as root reads. Browser paths grant no access.


`agent/transcript { target, agent_id, at, limit }` returns a child-owned
`transcript_window { window }`, with the same cut-bound `latest`, `older`, `newer`
and `turn` positions as `session/transcript`. The `at.turn` identity and read cut
must belong to that exact child's store. `agent/turns { target, agent_id, offset?,
limit }` returns `conversation_turns { page }`, counting every native Attempt
across activations while retaining at most 64 directory rows. Both survive
terminal child exit and parent restart, neither starts a child. The former
`before`-only child transcript request is removed; all clients upgrade to v50.

`target` is the **parent** AttachmentTarget (Session,
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
and durable history remain retained. Lifecycle status never supplies transcript settlement.

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


## Exact Job and Agent synchronization (v50)

Job status is non-blocking and `job/wait` waits on the registry's exact finite
Job ID through physical settlement. No client poll loop discovers completion;
`job_updated` and canonical inbound publish terminal facts proactively.

`agent/sendMessage` accepts stable AgentId, message and Session-owned upload receipts. Text may be empty when an attachment is present. Active-vs-Inactive
arbitration, resume reservation and guidance/seal ordering belong to the registry.
Admitting and Stopping return deterministic transient `agent_stopping` rejection
only while a live owner can settle. Unavailable returns typed `agent_settlement`
without retry advice; it denotes failed admission/publication/physical settlement
or poisoned workspace authority. `agent/wait` captures the
current activation once; its result names that activation even if the Agent has
since resumed. Inactive returns a null target. Clients must not automatically
retry a lost Agent wait, since a new operation could capture a later activation.
`agent/interrupt` commits cancellation intent for only its captured reserved or
current activation and preserves Agent identity. It immediately returns
`agent_interruption_accepted { agent_id, activation_id? }`; a null activation
means the Agent was already inactive. `job/cancel` similarly returns
`job_cancellation_accepted { job_id }` for the exact captured Job. Neither receipt
proves a terminal outcome or physical completion. Native lifecycle events and
explicit waits provide settlement evidence.

`agent/wait` returns `agent_wait`, with the captured
`activation_id`, its terminal `outcome`, and the current authoritative `agent`.
A null target means the Agent was already inactive. A non-null reserved target
with null outcome means admission ended without committing an activation. A later
resumed activation may appear in `agent`; it never changes the captured outcome.
Clients report the captured result and reconcile through the native stream or a
fresh checkpoint; they do not overwrite a newer projection with an unversioned reply.

`agent/list` returns newest-created identities first, bounded to 64, with exact
`returned`, `matched`, `limit`, and `truncated` counts. `Admitting` exposes the
reserved current activation; only the owner arbitrates wait, interrupt and message
admission during that phase. A failed or unproven physical rollback retains the
reserved target as Unavailable, including after reconnect; wait and interrupt return
a settlement error instead of reporting completion of the previous activation.

Replay/reconnect reconstructs authoritative Job terminal states and captures
Agent owner state together with its exact latest committed activation under one
registry lock. Finite activation record iteration order never selects the latest
Agent; prior activations belong to Trace history. Clients render separate Job and
Agent rows and preserve selection by AgentId. Child transcript reads span the same
canonical conversation across activations; final reports are never reconstructed
from diagnostic/activity snapshots.

Agent lifecycle publication is one combined owner cut: AgentSnapshot and latest
activation install before their single Event Journal receipt releases. A fresh
`agent/status`, reconnect snapshot and live `agent_updated` after the same cut
agree. Clients must not derive AgentState from finite activation state.
`agent/list` captures all returned Agent/activation pairs in one registry lock.

Permanent Agent unavailability is distinct from finite activation failure:
`agent_settlement` identifies the Agent requiring reconciliation or repair, and
clients do not suggest retry-after-settlement. A later native physical proof can
update an Interrupted activation's Agent to Inactive without changing its logical
outcome or replaying it.
## Session startup commit and catalog invalidation (v50)

`session/create.settings.model` is optional `SessionModelConfig`. Explicit intent
is validated and persisted by native creation; omitted intent captures the native
default at creation, not a client's earlier projection. Successful creation (also
with a durability diagnostic) acknowledges a real Session identity. It does not
acknowledge runtime attachment, input admission or provider output. Clients may
navigate immediately, after transferring pending input to a Session-scoped owner.
A durability diagnostic preserves the identity but stops automatic continuation.

`session/summaryInvalidated.catalog_changed` is mandatory. `true` invalidates
catalog membership as well as the named summary; `false` invalidates only the
summary. Every membership visibility commit publishes invalidation independently
of attachment or deletion cleanup. The sequence coalescer preserves an unobserved membership change even
when a newer preview change arrives for that Session. Catalog reads must not gate
attach or first admission. v57 is mandatory; no earlier-version decoder or compatibility path
is retained. See [ownership and evidence](issue-419/ownership.md).


## Bounded Tool presentation (v50)

Runtime Client v57 and App Server v29 introduced `TraceToolSummary.arguments`, a
`TracePreview` of canonical JSON arguments (at most 512 UTF-8 bytes, with explicit
`truncated`). Null means the exact canonical proposal is unavailable. The same
native same-Step ToolCall resolution supplies name and input; lifecycle refresh
carries neither immutable field. The current `session/traceDetail` returns a
bounded descriptor; selected deferred fields load through the same bounded
checkpoint value-read contract. Original raw Tool-call JSON uses its explicit
`RawJsonUtf8` content representation instead of structural re-encoding.
Generated schemas, fixtures, Web and TUI consume only the current vocabulary.
See [the semantic ledger contract](trajectory-harness-convergence.md).

Foreground and historical Tool presentation identifies an exact canonical
occurrence by its typed native locator and block index. `message_id` is optional
when the original identity cannot fit a bounded display header; its authoritative
metadata remains readable at that locator. Two omitted identities never establish
occurrence equality. A committed result remains a canonical fact independently
of the frozen Journal's Tool-start and Tool-terminal witnesses; an assembled
proposal alone never proves execution.

`committed_result_deferred` describes whether the finite Tool header omits its C
result body; it does not describe execution settlement. The exact C metadata
reference remains present when available even for an inline body. Clients expose
bounded further reading for deferred bodies and retain the captured Journal
evidence on frozen history pages. A current typed header may refine a live cached
row only for the exact canonical occurrence.

## Image admission vocabulary (#425)

App Server v29 combined the v28 Harness semantic ledger with required
`CapabilityView.configured_tools` and image/Bash Tool presentation. The required
field changes the complete mandatory vocabulary, so v28 is rejected. Rust DTOs
remain the sole schema authority; only the v50 generated pair is retained.

### Native process identity for reconnecting presentation

The current mandatory v50 `initialized` result includes `authority_id`, an opaque
UUID allocated once by `AppServerHost`. Cloned hosts/connections share it; a new
native process owner has a new identity even when it listens at the same endpoint.
It is observation identity, never authorization or persistent Session membership.
Web compares it before reconnect resynchronization and uses its existing authority
retirement path on mismatch. Generated schema/TypeScript and strict decoders require
this field; there is no reader for the previous result shape.

## Conversation reading windows

App Server v50 is mandatory and replaces the v29 root transcript request/result;
all consumers negotiate v57 and only its generated schema/TypeScript pair is retained.
Root `session/transcript` uses `ConversationWindowAt` (`latest`, `older`, `newer`, `turn`) and
returns `transcript_window` with one contiguous finite page. `session/turns`
supplies the ordered, paged native Attempt outline, stamped with the
`ConversationReadCut` it was captured at. See
[conversation-reading.md](conversation-reading.md) for normative identity, lineage,
location, bounds and paging semantics.

Transcript pages expose `inherited_through`, the final native cursor in the
immutable lineage seed (zero for an empty prefix). It includes terminal-only
inherited turns and remains unchanged by local appends or compaction. Clients
combine this position with the attached node's `SessionNodeOrigin::Fork` to
place a fork separator; different origin Conversation IDs alone do not prove
a Session fork. Turn navigation locates the destination's own transcript,
while an explicit source action opens the recorded Session/node and locates
`source_message` in that source lineage.

The v50 `ConversationReadCut` fields are `conversation_id`, inclusive `journal`
and `transcript` bounds, and `mutation_revision`. Latest reads capture a new cut;
Turn and continuation reads name the captured cut and reject semantic mutation. Surface revision and pending
population/aggregate revision are removed from this contract. `AttachmentTarget`
retains independent runtime/attachment authority. The current Attempt may be
displayed before its cursor exists; its first native location makes it reachable
without settlement.

`TurnProcessOutcome::IncompleteAtCut` is the immutable presentation of inherited
output whose source Attempt was live at its lineage cut. It grants no execution
ownership, live timer or destination settlement obligation. Bootstrap
`InheritedTurnOutcome` cannot serialize Running. Lineage copy authority is
`(SurfaceRevision R, native read cut C)`. R selects Surface history and its canonical
identity closure; C selects Attempt visibility/outcome, completed responses and
terminal-only settlement. The first SELECT in `read_lineage_cut(R)`'s SQLite read
transaction captures C. A clone repeated with the same R may inherit a different
outcome at a newer C; R alone is not an execution timestamp. Once captured, later
activity cannot change `(R, C)`. Fork/tree message boundaries only narrow the
structural prefix. Message-backed turns require members through C retained in
that structure; terminal-only turns require a terminal through C AND a retained
predecessor. No predecessor can authorize a terminal after C.
Web outline paging records latest versus explicit offset intent independently of
the returned page offset; an omitted offset always means native latest.

## Explicit Session-file deliveries (v50)

`ToolExecutionResult.deliveries` contains runtime-owned `SessionFileReference`
facts, distinct from managed `FileReference` / `ArtifactId`. Only committed
successful Tool results authorize cards. App Server v50 exposes no ordinary
Session-file read Method or caller-supplied root authorization; `delivery/read`
requires transport-granted delivery access (below). Browser actions
use canonical coordinates through the Product Host HTTP carrier; a dedicated
native WebSocket seam authenticates a separate process-scoped Product Host-only
credential before accepting Host-owned current registration roots. Browser
transport credentials and client names do not confer that authority.
The existing exact attachment, original Session mapping, root identity,
descriptor-safe read, finite bounds and revocation fences remain required.
The result/reference and closed `session_file_read` error vocabulary are shared
wire types; their presence does not grant an ordinary request method. See
[file-delivery.md](file-delivery.md) for provisioning, native admission and read
fences, current registration policy, historical scope and resource limits.

### Committed delivery access (v50)

`delivery/read { target, message_id, delivery_index }` returns
`session_file_bytes { file, data }`: the current original bytes (at most 512 KiB,
base64) of one committed delivery. `delivery/locate` with the same parameters
returns `session_file_location { file, path, device, inode }`: the absolute path in
the **server's** filesystem namespace and the verified regular leaf's decimal
device/inode, after the same descriptor walk, without reading bytes or applying
the size limit. Both are pure bounded reads: they start no Agent, Attempt, Tool or
model request, and a lost response is retryable.

Both require *delivery access*, a capability only transport authentication creates:

| Transport | Grant |
| --- | --- |
| stdio | `--stdio-delivery-access`: the process owner that spawned the server and owns its pipes delegates it explicitly (the TUI does for its owned child). Forbidden with WebSocket. |
| WebSocket | `--delivery-access-token-file`: a separate owner-only credential, distinct from the transport token and the Product Host secret, offered as `rustx-delivery-access.<secret>` beside `rustx-token.<token>`. Forbidden with stdio. |

Without it, both Methods fail `session_file_read` / `unauthorized` before any
lookup, whatever the coordinates, root spelling or `initialize` client name. The
target must be one of **this connection's** attachments. `initialize` and
`server/info` report the connection's own `capabilities.delivery_access`; the
report grants nothing. Connection close and native credential removal revoke the
grant; admitted reads then fail at their next fence. The native core, fences,
original-Session mapping, root identity, two shared read permits and closed error
vocabulary are the Product Host lane's, without its registered-root list. A
client interprets a location locally only when it demonstrably shares the
server's filesystem; see [file-delivery.md](file-delivery.md).

**Cancellation.** `delivery/cancel { request_id }` names one of **this
connection's** in-flight `delivery/read` / `delivery/locate` requests by its exact
JSON-RPC id and returns `delivery_cancel { accepted }`. Each such request is
registered under its id before native admission. A duplicate overlapping JSON-RPC
ID is rejected as `duplicate_request_id` before admission, without replacing the
original owner. `accepted: true` means the cancellation won
before that request's publication commit: its native fences fail from then on,
and its one response is the typed failure `delivery_cancelled`, sent only after
its native work physically settled and released its permit. `accepted: false`
means no such request is running here (unknown id, or its response was already
committed) and that response stands as produced. Without delivery access the
method is `unauthorized`; another connection's ids are never visible. Cancelling
one request never affects another request or the connection.

**Cancellation capacity.** Each connection has eight observation positions,
six ordinary RPC positions, and two reserved control positions, for at most
16 associated requests. Exhaustion returns a bounded correlated capacity error
and preserves the connection. Clients reserve the same lanes and retain request
occupancy until terminal response disposition. Agent/Job cancellation intent,
Attempt cancellation, Observe cancellation, Delivery cancellation, interaction
cancellation and Detach use the reserved control lane. These controls do not await
execution terminal settlement. A cancellation cannot recycle a
permit still owned by physical work; exact admission tokens prevent delayed
cleanup from releasing a successor's request identity. Observation cancellation
affects only the wait and never the Child's execution.

**Publication commit.** A delivery response is serialized (and size-checked)
into the ordinary bounded outbound queue together with its publication owner.
It is decided when the transport accepts it (the stdio pipe takes its first
bytes; tungstenite takes its WebSocket frame), in the same synchronous step,
and not before. At that point an accepted cancellation yields
`delivery_cancelled`, and a success (bytes or a native path) additionally
requires the connection's delivery authority and the exact attachment to be
current: credential revocation, connection close or process shutdown yields
`session_file_read` / `unauthorized`, detachment yields `stale_attachment`,
always for the same id. Failures carry nothing sensitive and keep their own
reason. A cancellation or revocation that completes before the acceptance
prevents the success; one that overlaps it waits for it and is ordered after
it. An accepted response is never retracted. See
[file delivery](file-delivery.md#delivery-access-for-app-server-clients) for
the synchronization.

The internal Runtime Client vocabulary is v76. Only App Server v57 clients
are generated; earlier versions are rejected, with no aliases or compatibility
decoder. The current storage/control formats are Event envelope 1, SQLite 81, Session
catalog 18 and subagent IPC v35. Child file and artifact reads use the registered
child Conversation and the parent attachment's native delivery authority.


### Manual compaction correlation boundary

`context/compact.params.request_id` is a `ManualCompactionRequestId`: 1–64 ASCII
letters, digits, underscores or hyphens. UUID strings are accepted. The same native
type validates JSON deserialization before runtime dispatch and describes the
v57 schema's length/pattern constraints. Invalid IDs return invalid params without
admission, lifecycle publication or provider execution. This ID correlates native
read evidence only; it does not authorize execution or provide exactly-once replay.
An exact correlated native release frees Web's gesture guard independently of the
original transport acknowledgement. The unresolved RPC still settles normally,
with its callbacks fenced from any later explicit request.

Generic uploads use the current [v57 upload policy](#generic-file-upload-policy-v57).
Preparation reserves intent; binary handshake admission linearizes against host
drain. Prepared capabilities are revoked; accepted carriers retain a host upload
guard until settlement. See [Session uploads](session-uploads.md).

Upload preparation actors belong to the Host upload subsystem: one tracked
supervisor owns expiry and any private loopback listener. Normal settlement cancels
expiry; detach revokes only unconsumed preparations. Drain revokes prepared intent
and waits for admitted native work and physical supervisor exit. Terminal settlement
requires no transfers, upload guards or upload actors. Status on a revoked,
unallocated operation is Absent; consumed work survives control disconnect.

### Trace retained-input ownership (v50)

App Server v57 / Runtime Client v76 require `TraceLifecycle.location` in each
retained-record refresh. An idle `InboundTurnAdopted` initially has no Attempt;
its answering Attempt is resolved natively at the read cut after that Attempt
starts. Summary pages and retained-record refreshes use the same resolution.
The browser replaces the cached location, including an inspected record outside
the latest page, without guessing ownership from adjacent rows. This places the
initial System Prompt before the first Turn and each input under its answering
Turn. In-Attempt adoptions keep their exact recorded Attempt. Earlier peers are
rejected; no compatibility decoder or client-side ownership inference is added.

### Quiet opening and durable history (v50)

`session/history { session_id, node_id?, at, limit }` returns `session_history`
with the native `conversation_id` and a decorated durable transcript `window`.
The page limit is 1..=64. The read retains allocation access through its bounded
SQLite read, validates the read cut, and neither composes a runtime nor acquires
control. Unknown/deleting identities and invalid nodes fail through the existing
Session errors. Connection preparation may run concurrently. Clients display this
as read-only history until attachment, and ignore late reads after attachment or
navigation replacement. Only a submitted local prompt displays connecting feedback.

`session/traceHistory { session_id, node_id?, before?, limit }` and
`session/traceHistoryDetail { session_id, node_id?, record_id }` provide the same
cold-read path for trajectory pages and inspection details. They return
`session_trace_history { conversation_id, page }` and
`session_trace_history_detail { conversation_id, page }`. Deferred detail fields
use `session/tracePage` and `session/traceValueRead` with the captured cut. Pages are bounded to
1..=32 records; detail identities to 256 bytes. Both retain allocation access and
reuse the native durable TraceProjection without loading configuration or tools.
The WebUI fences preview reads by navigation, connection and attachment epoch,
and replaces them with the bounded checkpoint after attachment. Prompts
submitted during initialization stay locally owned until attachment, display
connecting feedback immediately, and are admitted once; connection failure keeps
the unsent draft recoverable.

`session/statistics { session_id, node_id? }` independently reads durable
whole-conversation statistics and the last measured context occupancy. Its
`session_statistics` result includes `conversation_id`, `statistics` and
finite `occupancy`, together with `read_cut`, at one native journal frontier.
Deferred model and clock fields use `session/traceValueRead` or
`session/tracePage` at that returned cut and exact Node. The selected occupancy
and Journal domains share the live reader's source checks and 65,536-byte chunk
bound. These reads do not attach or activate the conversation. Missing derived
aggregate evidence is an observation-source error, not an older total or an
execution outcome.
It does not initialize a runtime, resolve tools or probe a provider. The WebUI
loads it alongside history, displays persisted model settings before attachment,
and retires late metadata replies when attachment or navigation changes.

### Bounded history windows and Host execution retirement (v50)

`ConversationWindowAt` is `latest`, `older {before, cut?}`,
`newer {after, cut}`, `turn {id, cut}`, or `message {id, cut?}`.
`ConversationWindow` includes its
`cut`, bounded `page`, optional `newer_cursor`, and exact `target` /
`target_cursor` for Turn navigation. Native SQLite resolves the identity and
rejects foreign, future or mutated cuts. Appends preserve frozen reads. A distant
jump needs one at-most-64-entry read; the browser retains a finite historical
window independently of the native live tail. See [conversation reading](conversation-reading.md).

`session/summary` includes `ownership_generation`, an exact decimal integer
assigned by the native catalog. Session creation and active-node replacement
persist the new catalog commit generation with the active-node selection at the
catalog rename visibility point. Metadata changes and repeated selection retain
the generation. A → B → A therefore has three distinct ownership generations.
Catalog schema 15 requires this field; older development catalogs are refused.
`session/switchNode` retires the old resident runtime, commits the selected node
and its ownership, then composes the successor runtime. Failed composition does
not roll back the committed selection. Cold attachment and history reads do not
select a node.

`session/ownershipRetired {session_id, retired_through}` publishes an inclusive
ownership-generation watermark after committed Session deletion or active-node
replacement, including uncertain directory durability. Pre-rename failures
publish nothing. The log coalesces by retaining the greatest retired generation;
its delivery sequence is only an observer cursor, never execution authority.
A delayed or reordered watermark cannot retire a successor generation.

The Product Host subscribes before native ownership reads. Each request registers
an admission before its read starts, retaining the greatest matching retirement
watermark until the request ends. After the read, the Host validates native
Session/node identity, workspace authority and generation. Before synchronous PTY
reservation/spawn, and after any capacity-eviction wait, it rechecks that admission.
The notification handler synchronously fences pending admissions and starts
settlement of registered PTYs at or below its watermark. A notification during
PTY readiness therefore finds an already registered owner; a notification during
the native read fences its later completion. Unrelated Sessions and successor
generations remain usable. No permanent Host watermark cache is required.

A read authorizes execution under its exact generation, not an indefinitely
current Session. Native replacement may precede delayed notification delivery;
any already admitted execution remains owned and settles when retirement is
observed. No browser-supplied generation authorizes execution. Input and resize
reread native authority and require the terminal's exact retained generation;
close only settles an existing matching Session/node/terminal identity and does
not readmit execution. Native connection loss permanently closes this Host's
terminal admission; a replacement Host requires its own observer lifetime.

Retirement, explicit close and shutdown share the supervisor settlement promise.
Capacity is released only after native child-reaping proof and successful outer
supervisor exit. Failures retain capacity and remain observable. This publication
does not replace process containment or transfer authority to a browser holding
a terminal ID.

### v50 message-positioned transcript windows

The session/transcript Message selector is { type: "message", id: MessageId,
cut: ConversationReadCut | null }. It is a read-only operation on the attached
Conversation. Native resolves the canonical message index and returns a forward
window of at most 64 entries, with the exact target_cursor and no fabricated
Turn target. Unknown, foreign, future-cut and retired message locations fail.
No older protocol files, alternate subprotocol or historical-page scan remain.

### Exact Tool occurrence navigation

App Server exposes `session/traceLocateTool` and `agent/traceLocateTool`.
Both take the current parent `target` and a `locator` containing
`occurrence: { assistant_message_id, block_index }`, `call_id`, and `tool_id`.
The Agent method also takes the registered `agent_id`. Runtime Client v76 carries
`agent_trace_locate_tool` through the child inspection protocol.

The native reader validates the canonical assistant block, then searches indexed
execution-start facts through one finite Journal cut. Provider call IDs are not
globally unique: the canonical message, block and exact Tool identity disambiguate
repeated IDs. The lookup covers the complete local execution history independently
of loaded browser pages and runs on the blocking read pool. No current Tool
configuration or output text is used to infer an execution.

The `trace_tool_location` result contains `location: { record_id, page }` or
`location: null` when that canonical block has no local execution record. `page`
is a bounded, contiguous native window ending at the selected execution; its
opaque `next_cursor` supports ordinary older paging. Existing `traceDetail` reads
load the heavy result by the returned record ID. Child lookups use the registered
child store and remain available after the process exits or the Session reopens.
A parent's occurrence never selects a child execution merely because IDs coincide.

The WebUI's Inspect action selects this native landing and opens the correct
Conversation's Trajectory inspector. A historical landing remains in place during
live refreshes; lifecycle repairs still apply, and Return to latest explicitly
replaces the interval. Attachment, navigation and visible-Conversation ownership
fence late results. Neither missing execution evidence nor lost transport responses
cause neighbour matching or replay a Tool operation.

### Exact Session Node metadata (v50)

`session/node` requires both Session and Node identities. It reads the existing native catalog node map under the catalog lock, returns immutable origin and conversation identity, and fails with `unknown_session`, `unknown_node`, or `deleting_session` as appropriate. It never substitutes the active Node, opens a Conversation store, acquires an Agent execution attachment, or enumerates the tree. `session/tree` remains the paged browsing operation. Fork discovery and source attachment preparation use the exact read; Web navigation and attachment observation fences retire late metadata replies.

Tool associations in frozen request detail carry nullable `wire_id`. Null means
both occurrences are covered by native continuation and this request transmits no
correlation field for them. Stored Responses results retain the actual accepted
output's native ID; stateless replay and full explicit history retain their encoded
request-local IDs. This observation contract is App Server v57 / Runtime Client v76;
only the current schema generation is shipped.


## Generic file upload policy (v57)

App Server v57 retains the removal of generic file count, per-file bytes and aggregate bytes
from `ServerCapabilities.upload_policy`. It now advertises transport flow control
only: `max_concurrent_transfers` and `max_chunk_bytes`. The native Session accepts
ordinary workspace attachments without a product count/size cap, matching DSH.
The carrier receives one chunk at a time into anonymous scratch storage and
hands complete, verified file bodies to the existing native allocation owner.

Each `session/uploadPrepare` operation admits exactly one file. Multi-file selections
are queued as independent operations by the Composer; there is no selection count
limit. The native Session upload owner still controls allocation, commit, and
receipts. One bounded filename and one receipt per operation keep every committed
wire upload recoverable through repeatable `session/uploadStatus` reads, including
after a lost carrier acknowledgment.

Web and TUI negotiate only `rustx.app-server.v57`; v55 and older peers are
rejected explicitly. The current native contracts are Runtime Client v76,
Session catalog schema 18 and SQLite schema 80. There are no old policy fields, alternate upload paths
or compatibility handlers. See [Session-owned workspace uploads](session-uploads.md).

## Workflow file authoring (Issue #491)

Three read-only methods expose native Workflow files of one exact scope; the
semantics are specified in [Workflow authoring](workflow-authoring.md#file-authoring-app-server-and-web).

| Method | Params | Result |
| --- | --- | --- |
| `workflow/list` | `target` | `workflow_library`: `target`, `directory`, `absent_revision`, bounded `files[]` and an optional directory `diagnostic` |
| `workflow/read` | `target`, `id` | `workflow_source`: `revision`, `present`, the strictly parsed `document` (absent for a malformed file), `diagnostic` and the editor `vocabulary` |
| `workflow/check` | `target`, `id`, `document`, optional `context`, `explain` | `workflow_check`: separate `structure` and `dependencies` facts and an optional compiled `explanation`; a valid structure carries the `revision` a `replace` with this candidate commits as |

Writes add no method: `configuration/sourceWrite` accepts the mutation
`{kind: "workflow", id, edit}` with `edit.operation` one of `create`,
`replace`, `copy` or `delete`, under the ordinary `expected_revision` fence. The
acknowledgement is the existing `sources_written` receipt (file `revision` and
`configuration_revision`). A rejected candidate returns the error data
`{kind: "workflow_invalid", diagnostic}` and leaves the file unchanged; a
changed revision is the ordinary revision conflict.

All three reads are side-effect free (`may_commit` is false): they start no
node, connect to no provider or MCP endpoint and read no credential. Diagnostic
paths are bounded to 512 bytes and details to 1 KiB, and each listing entry to a
7 KiB serialized share, so a scope's at most 128 files always list within the
message bound. Document numbers follow the Workflow number contract: a file or
candidate holding a number a browser would round, retype or re-sign (an integer beyond ±(2^53 − 1), an integral float such as `1.0` below 1e21, or negative zero) is `unsupported_number`; see [the number contract](workflow-authoring.md#number-contract). Numbers
travel in the existing exact wire domains, so `u64` fields such as `timeout_ms`
are decimal strings in TypeScript. Documents are bounded by the native 512 KiB
Workflow limit and the 1 MiB message limit.

A `create` or `copy` that would pass a discovery bound (1024 authored entries or 128
`.yaml` entries in the directory, or 64 composed Workflow IDs: User alone for a
User target, User with the Workspace for a Workspace target) is refused before publication with
`workflow_invalid`, diagnostic category `capacity`; see
[Capacity](workflow-authoring.md#capacity).

Workspace targets reach the App Server through the Product Host, which maps a
registration ID to its directory on its per-registration lane
(`/product-host/workflow`); browsers never send Workspace paths. Each request
carries the Product Host authority scope the browser transaction was bound to,
validated in the lane immediately before native dispatch, and, separately, the
native App Server identity (`native`, the `authority_id` the browser observed at
`initialize`), which the Host compares with the connection it initialized before
sending any Workflow request (`native_replaced` otherwise). User-scope
calls use the browser's App Server connection directly. A lost write
acknowledgement on either path is reported as uncertain and reconciled by
rereading, never by replaying the write.

## Named auxiliary consumers

Protocol v57 exposes `auxiliary/classify`, `auxiliary/btw`, `auxiliary/status`, `auxiliary/result`, `auxiliary/submissionLookup`, `auxiliary/cancel` and `auxiliary/discard`. Starts return bounded native receipts; notifications use `auxiliary/segment` and the current registration slot. Every carrier uses the existing DeliveryAccess/Writer publication fence. Cancellation retains reserved control capacity. [Semantics, exact correlation, limits and ownership](auxiliary-consumers.md) apply to generated direct, Stdio and WebSocket clients.

## Scheduler exact-node controls

These methods require the existing initialized product-control connection and
explicit `session_id`/`node_id`. Native catalog authority resolves the exact
conversation; no runtime attachment, UI focus change or Agent activation occurs.

| Method | Additional params | Result |
| --- | --- | --- |
| `schedule/list` | `after_ordinal` (exclusive u64 decimal string), `limit` (1–100) | `schedules`: bounded headers excluding reminder text, plus optional `next_ordinal` |
| `schedule/get` | `id`: exact conversation/ordinal | `schedule`: optional full native snapshot |
| `schedule/command` | `command`: stable command ID and typed create/update/enable/disable/delete mutation | `schedule_command`: native committed receipt, no-op, stale conflict or capacity result |
| `schedule/recoverStorage` | none | `schedule_storage_recovered`: reconciled store generation |
| `workflow/scheduled/getRecovery` | `start_id`, `after_effect_ordinal` (u64 decimal string), `limit` (1–100) | `scheduled_workflow_recovery`: exact native header, bounded effects, committed result and next cursor |
| `workflow/scheduled/cancel` | `start_id` | `scheduled_workflow_recovery`: durable cancellation intent or pre-Claim cancellation |
| `workflow/scheduled/reconcile` | same bounded page parameters as getRecovery | `scheduled_workflow_recovery`: native proof reconciliation, never graph replay |

List pages use keyset order. A full last page can supply a cursor whose next
page is empty. A full page of maximum-size definitions remains within the public
frame budget because reminder bodies require individual Get reads. All native
u64 counters remain lossless decimal strings; clients must not use Number.
Completed means no future occurrence, and a trigger receipt proves admission.
Reads and receipt display never imply target execution success.

Commands use the native trusted `RuntimeControl` namespace. Clients cannot supply
a Human grant or authority field. Mutation/recovery retains server operation
ownership through settlement if the response waiter disconnects. Retry the same
command identity and authored input after uncertain acknowledgment; stale CAS
requires rereading rather than automatic overwrite. Storage recovery reopens and
reconciles an existing native store under manager ownership; it is not Workflow
force recovery and cannot bypass unproven writers. Workflow authoring uses native
preparation at the composed exact node; admission/recovery can remain cold.
Recovery/cancel/reconcile reject force/proof fields. Missing physical proof keeps
capacity and deletion blocked. Scheduled Workflow and interaction DTOs carry a
real Scheduled origin, with nullable outer tool-call identity and no fabricated
Attempt or model turn. Root model Tools receive a separate native current-Human
grant and eight-mutation budget; transport input cannot provide either. See
[Scheduler](scheduler.md).

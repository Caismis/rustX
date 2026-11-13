# Shared App Server control and application callbacks

This is the implementation contract for issue #532. The implementation base is
`4ecd28b8834d91566404ccc24a601ba7cf2e9b7d`. The reference is OpenAI Codex
`rust-v0.166.0`, resolved to immutable commit
`6ff1e1ca3b30fc8b914720681fcf5fe62e95f856`.
The public protocol is App Server v78, with Runtime Client v85, Session catalog
v19, SQLite v87 and Runtime Manifest v6. Earlier protocols and storage layouts
are rejected; there is no compatibility reader or alternate gateway.

## Identity, methods and native owners

A Thread ID is an exact native Conversation ID. Its Session and node are native
catalog metadata. Switching the Session's selected node does not change what an
old Thread ID means. The Session remains the group used for Workspace settings,
lineage operations, deletion and residency. The native same-Session residency
restriction still applies: multiple clients do not enable parallel writers on
different nodes of one Session.

A Turn ID is the native Attempt ID. A model request, tool batch or retry is not a
new Turn. Items are existing canonical transcript entries, message descriptors,
publication facts and tool results. The RPC layer owns no history or execution
journal.

| Pinned Codex capability | rustX public contract | Native owner and intentional difference | Validation |
| --- | --- | --- | --- |
| `initialize` | `initialize { protocol_version:78, client, presentation }` | Connection negotiation; rustX retains explicit protocol and finite presentation capabilities | Schema fixtures; transport conformance |
| `thread/start` | `thread/start { settings, dynamicTools, submission? }` -> `session_transition { thread, session, ... }` | SessionController allocates exact native lineage; rustX retains its existing first-submission and editor receipts | Catalog and product first-submit suites |
| `thread/resume` | `thread/resume { threadId }` -> exact AttachmentTarget | SessionRuntimeManager loads the catalog's exact Session/node; attachment identity includes runtime incarnation | Shared-control conformance; attachment and node-switch tests |
| Thread read/list/status | `thread/read`, `thread/list { offset, limit, archived }`, `thread/loaded/list { offset, limit }`; `thread/status/changed`, `thread/status/resyncRequired` | Native catalog identities plus SessionRuntimeManager residency and ConversationRuntime activity; 64 rows per Thread page, instance notifications without transcript attachments | Native status, Interaction, physical-tail, shared-control and Web/TUI status regressions |
| Thread name/archive | `thread/name/set`, `thread/archive`, `thread/unarchive` | Native node metadata. Archive probes outside Registry, then validates the exact resident and native idle epoch before retirement; active work is refused | Thread catalog, cross-Thread archive isolation and resident/epoch race regressions |
| Thread history/Items | `session/history`, `session/historyPage`, `session/node`, `session/turns`, transcript/ContentRef readers | Existing bounded native history, turn and content authorities. rustX keeps chunked bodies and exact integer text | History, reading, root checkpoint and large-content suites |
| Fork/branch | `thread/fork`, `session/branch`, `session/switchNode` | Existing catalog/lineage transactions. Explicit forks copy declarations; switching preserves exact-target admission and native retirement | Catalog switch/fork and projection tests |
| Deletion | `session/deletePreview`, `session/delete`, `session/recoverDeletion` | Intentional native extension: revision-fenced deletion of the Session lineage group, with native recovery receipts; no separate synonymous Thread-delete RPC | Deletion membership, storage and product lifecycle suites |
| `turn/start` | `turn/start { target, inputId, content }` -> `turn_input_accepted { turnId, disposition, message_id, inbound_sequence }` | Conversation coordinator and existing mailbox: start idle work or steer an open active Attempt; one Agent Loop | Real provider `shared_start_steer_sparse_queue...` |
| `turn/steer` | Required `expectedTurnId`, plus `target`, `inputId`, `content` | Same native coordinator/steering boundary; stale or closed Attempts refuse before durable acceptance | Shared-control conformance; native cancellation tests |
| `turn/interrupt` | Required exact `turnId` and `target` | Native cancel/start arbitration; an old Turn never selects its successor | Shared-control conformance; product cancellation tests |
| Explicit queued input | `inbound/queue`, `inbound/edit`, `inbound/remove`, `inbound/submissionLookup` | One existing durable mailbox. Queue entries wait for the next Attempt, remain distinct from steering and retain native CAS revisions | Sparse-adoption SQLite regression; real provider and lost-receipt client tests |
| Approval and user input | `item/interaction/request`, a JSON-RPC client result/error, `serverRequest/resolved` | Native Interaction coordinator, including child routing and Workflow Review. rustX preserves typed finite headers, chunked bodies, exact questionnaire numbers and Review subject identity | Native shared-interaction and product prompt tests |
| Dynamic tools | `dynamicTools` on Thread creation; `item/tool/call`; client result/error | Native ToolOrigin::Host and common Tool Runtime, described below | Real provider broadcast/error/domain-invalid conformance; actual-process recovery |
| Event subscription | `session/checkpoint`, `session/subscribe`, native `session/event` and invalidations | Independent native replay/registration fences. No controller acquisition or handoff event | Physical receipt/registration tests; product reconnect suites |
| Configuration edits | Existing revision-fenced configuration RPCs | Existing source CAS and automatic safe-boundary application; admitted capabilities/model stay frozen | Configuration and model-catalog suites |

Reference source anchors at the immutable revision:
[authentication](https://github.com/openai/codex/blob/6ff1e1ca3b30fc8b914720681fcf5fe62e95f856/codex-rs/websocket-auth/src/lib.rs),
[Turn admission](https://github.com/openai/codex/blob/6ff1e1ca3b30fc8b914720681fcf5fe62e95f856/codex-rs/app-server/src/request_processors/turn_processor.rs),
[broadcast arbitration](https://github.com/openai/codex/blob/6ff1e1ca3b30fc8b914720681fcf5fe62e95f856/codex-rs/app-server/src/outgoing_message.rs),
[Thread lifecycle](https://github.com/openai/codex/blob/6ff1e1ca3b30fc8b914720681fcf5fe62e95f856/codex-rs/app-server/src/request_processors/thread_lifecycle.rs).
This is semantic alignment, not zero-modification Codex client compatibility.
The [implementation evidence](validation.md) maps invariants to executable tests,
records local checks and independent review, and describes measurement limits.

## Authentication and deployment

Every WebSocket listener, including loopback, requires exactly one configured
credential mode. Stdio trusts OS ownership of the spawning process and its pipes;
network credential flags are rejected for stdio. Initialize does not authenticate.

* Capability mode: `--token-file /absolute/path` or `--token-sha256 <64 hex digits>`.
  The file contains 1–4096 valid bearer characters, optionally followed by one LF.
  Generate at least 32 random bytes for deployment. The verifier retains a SHA-256
  digest and compares fixed-size digests in constant time.
* Signed bearer mode: `--shared-secret-file /absolute/path`, optionally `--issuer`,
  `--audience` and `--max-clock-skew-seconds`. The UTF-8 HS256 secret is 32–4096
  bytes; JWT input is bounded at 16 KiB. The signature algorithm is fixed to HS256.
  `exp` is required; `nbf`, configured issuer and configured audience are checked.
  Boundaries match the pinned contract: `now <= exp + skew` and
  `now >= nbf - skew`. Additional role/scope/Thread claims grant no permission tier.

A native client sends `Authorization: Bearer <credential>`. A browser offers
`rustx.app-server.v78` and `rustx-token.<base64url-no-pad(credential bytes)>`. Encode the ASCII bearer bytes
with RFC 4648 URL-safe Base64 and omit padding; native Authorization sends the
original bytes. There is no raw-token subprotocol alternative. Both reach the same verifier.
The server selects only the normal protocol, never echoes the secret, and rejects
multiple Authorization fields, mixed header/subprotocol credentials, duplicate or
unknown credential offers, malformed carriers, and any query string. Native bearer
clients need not offer a browser subprotocol. Origin-bearing upgrades use the same bearer validation; the listener does not
provide an Origin allowlist. The trusted deploying host enforces Origin/CSP policy. rustX intentionally differs from the
pinned upstream's default local admission/browser restrictions.

Credentials are loaded at process startup. Updating a file does not hot-reload it.
JWT checks occur at upgrade: expiry does not disconnect established sockets or
cancel accepted Agent work. Restart/reconnect loads and checks fresh credentials.
Immediate invalidation requires the trusted operator to close existing connections
or restart the instance. There is no token revocation database.

All authenticated, initialized ordinary clients have the same instance-wide API
eligibility. Client names, browser/native transport, credential mode, creator
identity and Thread focus are not roles or ACLs. Subscription controls callback
and event delivery, not ordinary API eligibility. Native model credentials,
Workspace/OS rules, resource policy, exact targets, approvals and parameter/state
checks still apply. The private Product Host carrier remains private and separately
owns registered-root proof; it is not an extra ordinary-client permission tier.
The ordinary delivery capability is common to every initialized connection.

The trusted host/operator owns token issuance/distribution, file permissions,
proxy log redaction, TLS termination, browser Origin/CSP rules and per-user process
isolation. Never put credentials in RPC bodies, query parameters, canonical history
or logs. Model-provider keys cannot authenticate an App Server connection.

## Admission, replies and cancellation

Every writable attachment is independently closable. One native Conversation
coordinator remains the writer. Native coordinator/mailbox locks order input
acceptance and Attempt admission; capability freeze and configuration CAS remain
at their existing boundaries. No global lock spans a model call or tool execution.
Accepted execution survives ordinary client disconnect. A closed connection fences
queued admission, late callback claims and connection-owned reads/publications.

Archive refusal is a metadata-dependent execution admission result, checked after
Loading registration against fresh node metadata. It does not retain a failed
initialization flight. Unarchive therefore permits an immediate ordinary resume;
accepted scheduled receipts remain durable and their existing Native owner adopts
them exactly once. Actual initialization failures retain their explicit retry path.

Every input mutation carries a new `inputId` (1–64 ASCII letters, digits, `_`, `-`).
The acceptance transaction records correlation in the existing native inbox store.
Duplicate IDs refuse; equal text with different IDs remains separate input. A lost
receipt is uncertain. Read `inbound/submissionLookup` against the same Conversation
and ID; a positive receipt proves acceptance, not execution or success. Absence is
not a license to blindly replay. Browser reconnect performs this read repair; TUI
uncertainty retains the exact correlation for the same typed reader. Neither client
retransmits an uncertain mutation.

Explicit queue entries may lie between steering entries. Adoption removes only
selected native rows, and projection updates remove exact message IDs rather than
an entire sequence prefix. Native transcript positions and acceptance correlations
survive adoption. Queue edits/removals use existing pending-item CAS.

The App Server callback broker owns only ephemeral transport correlation. Its
server incarnation and monotonic counter identify pending requests. Capacity is
128 pending requests instance-wide, with a 256-message callback queue per client
and the ordinary 1 MiB frame limit. Slow callback consumers close independently.
No subscriber or business handler owns a provider lease.

Eligible live subscribers receive the same request ID and native invocation
identity. Live pending requests replay with that identity on subscription; solved
requests never replay. A different subscription incarnation, disconnected client,
unknown ID, or canceled/retired native actor cannot claim. Native validity and
request/subscription validity are checked at the response claim and physical
outbound handoff boundaries. Each subscription retains its exact native Runtime
lifecycle. Physical publication and response claims hold that lifecycle's short
commit gate before native pending validity and broker correlation; retirement
fences old subscribers before any delayed `Closed` notification is consumed.

The first valid correlatable result **or JSON-RPC error** atomically removes the
pending request. Domain validation happens after that claim. An invalid domain
answer settles as denial/cancellation or unknown result under its native owner;
it never reopens arbitration to seek a better answer. `serverRequest/resolved`
clears every recipient's prompt/handler. Sending a client reply is not a server
acknowledgement or native settlement.

Cancellation before physical callback dispatch prevents that external start.
After dispatch, cancellation or connection loss cannot prove external work stopped.
A response claimed before cancellation retains its result evidence; native Tool
Runtime still decides cancellation/result commit order and terminal uniqueness.
Native deadlines use the same cancellation path. No transport callback commits an
independent canonical result or bypasses native cleanup/durability.

## Host Tool declarations and execution

`dynamicTools` contains at most 64 declarations with optional namespace, name,
description and canonical `inputSchema`. Nonempty identity parts are bounded to
128 ASCII letters/digits/underscores/hyphens; descriptions are at most 16 KiB,
schemas 64 KiB each and all declarations 256 KiB. Duplicate effective identities,
reserved helpers and collisions with effective Builtin/MCP/managed Python tools
refuse before model execution. Distinct namespaces may use the same short name.

The native catalog persists declarations on the exact node. Runtime composition
registers them before capabilities freeze. A provider-neutral `host_` plus the first
56 hexadecimal SHA-256 characters of the JSON `[namespace-or-empty,name]` tuple
is the bounded model-facing key; native Host origin and callbacks retain full
namespace/name. Provider adapters remain responsible for wire rendering.

Calls use the ordinary Tool Runtime's schema validation, policy/approval,
foreground execution, output limits/spill, cancellation and canonical settlement.
There is no new tool execution engine. Callback results are
`{ success, contentItems:[{type:"inputText",text}] }`; a correlatable error or invalid
domain result becomes `OutcomeUnknown`, never a fabricated success. Generic clients
ignore unhandled Host Tools. Explicit handlers are selected by full namespace/name.

Two handlers can both perform external effects before either replies. Only one
selected result and continuation become authoritative; this is not external
exactly-once execution. Applications must implement their own business idempotency
when needed. Host capabilities are not implicitly inherited by Subagents, Workflow
or Fleet scopes. Explicit lineage forks retain native declarations. Process death
retires RPC correlation; native recovery records interrupted external effects as
unknown, without replaying a Host invocation or automatically resending a model
request. Restart recovery and live pending-request replay are different operations.

## Existing clients and standalone example

WebUI and TUI retain their existing controls, draft/focus ownership and layouts.
Their internal protocol adapters use exact Thread/Turn IDs, explicit mailbox
queueing and generic server-request replies. To preserve the requested UI surface,
the existing browser Delivery Access input remains visible but supplies no extra
credential; the ordinary bearer token is the sole connection credential.

One standalone Node example uses only built-in file and WebSocket APIs:

```sh
umask 077
python3 -c 'import secrets; print(secrets.token_urlsafe(32))' > /absolute/socket-token
cargo run --all-features --bin rustx -- app-server --listen ws://127.0.0.1:8787 --token-file /absolute/socket-token
node examples/app-server/connect.mjs ws://127.0.0.1:8787/ /absolute/socket-token
```

It connects, initializes v78 and prints the read-only `server/info` result. It is
not an SDK or a second demo application. Rust-derived `v78.schema.json`, `v78.ts`
and fixtures are generated together; product clients validate server output apart
from the generic client-response envelope so arbitrary callback results cannot
weaken native server-response validation.

A subscriber's detach sends its exact outstanding `serverRequest/resolved`
correlations on the same ordered callback channel before any later replay. This
retires only that subscriber's ability to answer and aborts its local Host Tool
handlers. Other subscribers and native requests remain live.

Input receipt recovery retains bounded application settlement ownership under the
original server authority, Conversation and inputId. Both a lost transport response
and a native `outcome_unknown` response retain that correlation. A positive lookup
is handed to the original acknowledgement owner; Web's revision-sensitive composer
consumes only that submission's text/files, and TUI settles the retained operation.
Absent proofs and failed reads preserve uncertainty. Recovery never resends input
or upgrades durable acceptance into execution completion.

## Thread runtime status (v78 amendment)

The public vocabulary follows the pinned Codex
[Thread status source](https://github.com/openai/codex/blob/6ff1e1ca3b30fc8b914720681fcf5fe62e95f856/codex-rs/app-server/src/thread_status.rs).
rustX derives it from its existing native ownership boundaries rather than
introducing another execution state machine or journal. Catalog archive/deletion,
Turn outcomes, connection state and current runtime status are independent.

| Public state | Native derivation |
| --- | --- |
| `notLoaded` | No execution composition owns the exact Conversation. A storage-only catalog/store entry is also cold. |
| `active` | Initialization or retirement owns the residency slot; or an admitted Attempt, scheduled invocation, manual compaction, physical task tail, background execution/preparation, live Child/recovery/resume, auxiliary call, or logical human Interaction remains owned. |
| `activeFlags` | All logical root and routed Child Interactions: approvals set `waitingOnApproval`; questionnaires and Workflow reviews set `waitingOnUserInput`. Both may coexist, ordered approval then user input, once each. |
| `systemError` | Native durability admission is closed by its absorbing committed or requested failure, MCP settlement is unproven, Child terminal physical settlement or admission rollback is unproven, Child reconciliation has exited with unresolved recovery ownership, or manager retirement is unproven. Fault visibility does not release any execution owner. |
| `idle` | Loaded composition, no native work or logical human wait, and no native system fault. This says nothing about the previous Turn's outcome. |

The coordinator and joint Interaction → Background → Child → Auxiliary leaf
snapshot prevent mixed-owner false idle during handoffs. Leaf and Gate reads use
nonblocking lock acquisition while Coordinator is held. Coordinator, composition
and per-Thread capture locks also use try acquisition. A busy native source
makes the observation unavailable, without holding Coordinator or the per-Thread
capture lock across the source's long operation. The manager emits one resync
notice per unavailable resident. Its separate recovery worker awaits native mutex
availability outside all other locks, including holders that emit no activity
change, then retries capture; recovery publishes a fresh revision even if the
public level is unchanged. The absorbing durability
frontier is reread after releasing the leaf locks, while the coordinator remains
held: auxiliary reservation takes Gate → Auxiliary, so status must not invert
that order. Observational RAII leases live with actual task/admission tails;
they do not authorize work. Background and Child activity indexes contain live
ownership records, avoiding a scan of historical completed executions.

A callback transport claim, cancellation receipt or dialog closure cannot settle
a native Interaction. Native terminal arbitration and audit commitment remove the
logical wait. Three subscriber copies remain one native request. Host Tool
callbacks contribute ordinary active Tool execution, with no human flag. Native
OutcomeUnknown terminal settlement ends the required native owner; status never
waits indefinitely for unknowable application side effects. Provider retries and
ordinary failed/cancelled Tool or Turn outcomes alone are not system faults.

A fault cannot be cleared by a status read, reconnect or successful previous Turn.
Durability failure is absorbing for that runtime allocation; MCP/recovery and
retirement failures retain their native recovery requirements. Only native
reconciliation and proven retirement/recomposition, or process restart with its
existing recovery, can establish a healthy new incarnation. Initializing and
stopping remain active until native proof permits a different state; unproven
retirement remains systemError. An unloaded/restarted Thread never inherits an
old process's running bit or pending dialog.

Thread reads capture only addressed identities, at most 64, without load,
MCP initialization, model calls, history hydration or lifecycle admission. Session
list/summary envelopes carry the active node's exact `thread_id` and a nullable
`thread_status` proof in one bounded response (Session pages remain 32). A busy
Native source clears only its proof; the metadata page and other Threads remain
readable. Strong Thread reads return an explicit observation-unavailable error
instead of inventing a status. Lightweight recovery notifications restore a
failed proof using the retained exact identity. The durable catalog stays pure
metadata. `thread/loaded/list` excludes storage-only entries and includes owned
initializing/stopping slots. Its sorted offset pages reflect current residency;
membership can change between pages, so consumers reread from offset zero after
an invalidation instead of treating offsets as a frozen cursor.

Each meaningful status/incarnation change publishes one process-local exact u64
revision, encoded as decimal text, with exact `threadId` and nullable
`runtimeIncarnation`. Snapshot revisions are current publication cuts, including
cold reads. Per-Thread captures serialize before native observation; registry
identity validation fences replacement before publication. Resident observers
hold only weak runtime/registry references and scalar watches across waits.
Loaded publication never invents active while its native observer is pending.
No token stream or identical snapshot generates a status notification, and retired
identities leave no permanent tombstone cache.

Initialized instance connections receive these lightweight facts without any
transcript attachment; transcript and callback routing remain exact-Thread scoped.
The broadcast is bounded at 256. A lagging receiver discards its backlog and emits
`thread/status/resyncRequired`, requiring authoritative current reads. A connection
close fences delivery and never cancels server-owned work.

Web and TUI fold revisions under their negotiated process authority and retain
128 early notification proofs so a first snapshot cannot overwrite a newer
unknown-Thread event. Eviction records one revision floor; an ambiguous older
snapshot becomes unavailable and triggers bounded pure-read repair. Repeated
overflow advances a repair epoch and requires another read before synchronization
is restored. Web repairs the current catalog page and all retained exact view
identities; TUI repairs its bounded retained/attached interest set. Disconnect
shows unavailable rather than idle; reconnect reads fresh proofs. Status consumers
use the row's or focused view's exact Thread, preserving drafts, controls and focus.

A nullable metadata proof revokes status cached before that metadata request.
Clients retain a successful higher-revision Native notification received since
its start, so a recovery notice arriving before the nullable response is not
lost. Duplicate or delayed revisions do not certify recovery. Thread identity
remains available for later notifications even while its status is unknown.
Failed `thread/read` responses use that same notification cut, connection and
repair epoch: they revoke starting evidence while retaining a newer recovery
proof received during the request. Revocation retains the previous revision
fence, so delayed older notifications cannot restore discarded evidence.
Notification receipts follow existing proof objects across early-cache eviction
and replacement by newer snapshots. Snapshots inherit the original receipt cut
without advancing it; exact revocation invalidates inherited receipts, and a
connection/repair reset discards them. These weak references retain no Thread
or transcript and cannot turn an old receipt into a new Native observation.

TUI retains proofs and pending read interests for live attachments throughout
their existing bounded lifetime (at most 32 per connection). Its proof cache and
read-interest set each retain at most another 128 catalog identities; LRU eviction
applies only to identities without a live attachment. Detach, native retirement and node switching restore the ordinary
catalog bound. Browsing other Sessions cannot discard an attached Thread's proof
or recovery ownership.

TUI keeps pending exact reads under one owner, bounded by those attached and
catalog interests. Catalog rows crossing a status gap and attachment reads enter
the same sequential drain. The existing two-request metadata lane remains
unchanged; the owner waits on a shared capacity wake and rechecks before
dispatch. Other metadata operations can delay repair without discarding it.
Another gap queues fresh reads at the new epoch; closing the connection wakes
any capacity wait and ends the drain. No input or mutation is replayed.

# Architecture

## 1. Architectural objective

rustX is an execution kernel, not an agent application framework and not a control plane. Its responsibility is to execute an immutable runtime manifest, produce durable execution facts, and expose stable runtime-owned contracts to higher-level systems.

The architecture is layered so that external SDKs, storage backends, process managers, and UI protocols can change without rewriting the agent kernel.

## 1.1 Native durable conversation authority (M8 / Issue #11)

The conversation has one backend-independent durability owner,
`ConversationStore`, implemented locally by `SqliteConversationStore`. One
SQLite database is physical colocation only; semantic ownership remains
separate:

| Domain | Durable authority |
| --- | --- |
| Pending Inbound Inbox | Accepted, not-yet-adopted deliveries, one shared `InboundSequence`, and correlation/idempotency state. |
| Message Ledger | Append-only canonical `MessageBlock` bodies, stable `MessageId`, and commit order. |
| Conversation Surface | Immutable `SurfaceOp` history plus current `SurfaceRevision`, active identity order, and compaction generation. |
| Request Snapshot | Immutable non-history inputs for one actual request, bound to one historical Surface revision and one `RequestId`. |
| Event Journal | Append-only typed execution facts with `EventId`, per-conversation `EventSequence`, schema version, reference checks, and terminal constraints. |
| Checkpoint/index metadata | Current Surface head and structural/context checkpoint copies used for bounded bootstrap validation; never a transcript. |
| `ConversationInboundMailbox` | Process-local acceptance coordination and wakeup only; it is not durable authority. |
| Runtime Client | Projection, cursor, and control adapter only; reconnect never supplies recovery facts. |

There is no `ConversationRecord`, full transcript table, request-message
copy, generic repository, or second durable history. Canonical message bodies
are stored once in the Ledger. A Surface revision stores identity/order
transitions, and a historical request combines that revision with its frozen
snapshot on demand.

The SQLite schema is development schema version 2. An incompatible database
fails explicitly; there is no migration chain, legacy reader, compatibility
fallback, dual write, or old storage mode. File-backed stores use WAL,
`synchronous=FULL`, foreign-key enforcement, and a busy timeout. A successful
SQLite commit is the local durability linearization point documented here.

The version-2 physical tables are deliberately semantic rather than generic:

| Table | Purpose and constraints |
| --- | --- |
| `rustx_store` | One-row conversation binding, schema version, and durable next `InboundSequence` / Event Journal sequence counters. |
| `pending_inbound` | Pending deliveries keyed by `InboundSequence`, with unique `MessageId`, serialized User body, and optional correlation. |
| `inbound_correlation` | Exactly-once correlation mapping to the accepted sequence and unique `MessageId`. |
| `message_ledger` | Append-only canonical bodies keyed by commit `position` and unique `MessageId`. |
| `bootstrap_identity` | Immutable initial-history count and digest, including an explicit empty bootstrap. |
| `surface_ops` | One immutable `Append` or `Replace` operation per `SurfaceRevision`, with compaction generation. |
| `surface_head` | Current Surface revision, active identity order, and compaction generation. |
| `context_checkpoints` | Current structural/index checkpoint matching `surface_head`; it is not message history. |
| `request_snapshots` | One immutable non-history snapshot per `RequestId`, its Surface revision, and committed start sequence. |
| `events` | Append-only typed envelopes keyed by per-conversation Event Journal sequence and unique `EventId`. |
| `lifecycle_state` | Durable terminal markers enforcing zero-or-one terminal event and terminal absorption for attempt, turn, and background-execution lifecycles. |

`MessageId`, `InboundSequence`, `SurfaceRevision`, `RequestId`, `EventId`,
Event Journal sequence, `AttemptId`, `TurnId`, `ToolCallId`,
`ToolExecutionId`, `CapabilityRevision`, and `ContextGeneration` remain
distinct identity domains even where SQLite stores their serialized values as
integers or text. The unique constraints prevent identity reuse; semantic
reference checks prevent an Event Journal fact or Surface operation from
pointing at an unavailable authority.

The semantic write pattern is always:

```text
prepare and validate all fallible state
        ↓
one ConversationStore SQLite transaction
        ↓ COMMIT = durable authority linearization
install the already-validated hot result, or reload it from the authority
```

Inbound acceptance commits sequence allocation, pending delivery, and
correlation state together. Adoption commits the selected finite pending
watermark, canonical User Ledger rows, Surface Append revisions, checkpoint
metadata, and pending deletion together. Ordinary canonical appends and
ToolResult sibling batches append Ledger bodies and Surface revisions in one
transaction; committed-message events share that transaction. Compaction
commits the summary body, Surface Replace revision, generation/checkpoint
metadata, and `CompactionCompleted` reference together. Request start commits
the immutable Request Snapshot and `ModelRequestStarted` fact together before
the provider adapter is called. Background terminal publication commits its
terminal inbound row and reference fact together.

The durable request-start flow is:

```text
assemble provider-neutral ModelRequest
→ freeze RequestSnapshot / RequestId
→ commit snapshot + ModelRequestStarted
→ independently reconstruct and compare exact ModelRequest
→ invoke provider adapter
```

Historical reconstruction reads only the immutable snapshot, the referenced
Surface operation history, and keyed Ledger bodies. It never reruns
contributors, Skill discovery, extension/DSH logic, current status sampling,
workspace inspection, or current model/tool/capability configuration.

The Event Journal lifecycle table rejects a second terminal attempt, turn, or
background-execution fact and rejects every later fact in that lifecycle. An
attempt terminal is an attempt-level fact; it is not treated as a late event
inside a turn that already emitted `TurnCompleted`.

Runtime bootstrap loads only the current Surface head, active IDs, active
message bodies, structural checkpoint metadata, pending items, and bounded
projection state. Event pages, old requests, retired Ledger rows, and
historical Surface revisions are read lazily. M8 stores the evidence; M9a
(Issue #12) adds the startup recovery that classifies and reconciles it — see
[Recovery model](#7-recovery-model). Model-turn cancellation redesign (M9b)
and runtime supervision/quiescence (M9c) are delivered below; replay/resend
policy and retry orchestration remain intentionally outside this architecture.

## 1.2 Runtime supervision and quiescence (M9c / Issue #12)

`ConversationRuntime` owns one lifecycle authority:

```text
Inactive --activate--> Running --shutdown linearization--> Draining
                                                               |
                         all owned work durably/native settled  v
                                                           Quiescent
```

The `Running -> Draining` transition is performed under the coordinator
state lock. It is the total-order point against inbound acceptance, model
updates, and attempt admission. Background ownership transfer and capability
revision commit use the same lifecycle at their native registry/coordinator
boundaries; they cannot commit new semantic work after drain wins.

`ConversationRuntime::shutdown()` is the one public semantic shutdown
operation. It is asynchronous and idempotent: concurrent and repeated calls
join one drain completion, and success means `Quiescent`, not merely that a
cancellation signal was set. `Inactive` shutdown is refused. `Draining` still
permits only required settlement mutations; `Quiescent` refuses even stale
settlement callbacks.

The ownership graph is concrete:

```text
ConversationRuntime
├─ admission worker (explicit exit boundary)
├─ current AgentExecution
│  ├─ M9b model-start arbitration and provider settlement
│  └─ Agent Loop foreground tool-batch structural settlement
├─ ConversationBackgroundRegistry
│  └─ conversation-owned runners and terminal Pending Inbound publication
├─ CapabilityCoordinator
│  ├─ counted capability/environment preparation
│  ├─ runtime-owned capability revision commit boundary
│  └─ retained MCP runtimes and notification subscriptions, closed/joined
│     through their existing physical settlement contract during drain
└─ native process composition
   └─ existing TERM → grace → KILL → group terminality → reap/containment proof
```

Cancellation requested, operation settled, and runtime quiescent are
distinct. A started model request is awaited after cancellation; started
foreground tools are awaited by the Agent Loop; committed background work is
cancelled and awaited through `wait_until_settled`; and a background record
does not become terminal until its exactly-once terminal Pending Inbound fact
has durably committed. An accepted Pending Inbound item is never adopted into
a new attempt after drain and remains durable at quiescence.

### Supervision does not stop at the first failure

Drain is a supervisor, not a short-circuiting pipeline:

```text
close admission
  -> request cancellation/closure of every concrete owner
  -> supervise EACH owner to its own native terminal boundary
  -> collect settlement/durability failures
  -> decide: Quiescent, or one aggregated settlement failure
```

A failure in one participant is an error **fact**; it is never permission to
abandon a sibling that can still produce an external effect. Every wait has a
native boundary that cannot be starved: a background record either settles
terminally or explicitly abandons its bounded durable terminal publication
(neither fact leaves any callback authority behind), an MCP runtime's close
proves or disproves physical settlement, and a counted lifecycle admission is
released only by its owner. No drain wait is conditioned on a global health flag, so
one owner's failure can never be read as another owner's settlement. The
collected failures are rendered as one bounded deterministic diagnostic —
a diagnostic aggregation, not an error framework. `Ok(())` therefore still
means exactly `Quiescent`; `Err(RuntimeOwnedSettlement)` means admission is
closed and every settleable owner was supervised to its strongest available
boundary while some ownership/physical/durable terminal condition stayed
unproven. An unresolved `PublishingTerminal` record remains explicitly
non-terminal and is never reinterpreted as success.

### A settlement fact never precedes the owner's last callback

`publication_abandoned` is the fact drain consumes as one background
execution's settlement, so publishing it is a linearization point, not a
bookkeeping detail:

```text
durable terminal publication attempts #1 and #2 exhausted
  -> failure sink callback begins (coordinator lock, durability health,
     possible `DurabilityFailed` observation)
  -> failure sink callback completes
  -> `publication_abandoned` commit
  -> waiters notified
  -> zero remaining conversation callback authority
  -> drain may treat this owner as settled-with-failure
```

Reporting the failure is real semantic runtime work, so the abandoned fact
must not become observable while that callback can still run; otherwise drain
could aggregate the abandoned evidence and cache a failed shutdown before the
runner finished calling back into the conversation. The continuation is held
inside one counted settlement admission across both steps, because a *failed*
drain leaves the lifecycle `Draining` — where settlement callbacks remain
intentionally legal — and so cannot rely on the admission refusal that
protects a successful `Quiescent`. The contract is logical callback
settlement, not the runner task's syntactic return: once
`publication_abandoned` is observable, that execution owns no failure-sink
callback, observer callback, Pending Inbound attempt, durability-health
mutation, progress callback, terminal retry, or semantic registry mutation.
`shutdown().await` — `Ok` or `Err` — therefore always returns after every
runtime-owned operation reached its strongest honest settlement boundary.

### Waiter lifetime is not ownership lifetime

A caller's future is never the owner of a physical resource. MCP connection
establishment makes this explicit, because a stdio process exists before the
handshake completes:

```text
no physical owner
  -> conversation-counted preparation owner (own task, own counted lifecycle
     admission, own ownership cancellation signal)
  -> physical MCP process ownership established
  -> either  A. transferred into the coordinator's retained `mcp_runtimes`
     or      B. cancelled/failed and driven to physical settlement
```

The counted admission is released only after A or B, so aborting or dropping
the `prepare_candidate` caller cannot remove the physical owner from the
quiescence proof. Runtime drain **cancels** those owners; it never drops
their futures, because `Drop` merely requesting a shutdown is a cancellation
signal, not proof of settlement.

### Attempt slot settlement and attempt task lifetime

Clearing the current-attempt slot hands the conversation state back to the
coordinator. It does not end the attempt task, which still owes the
coordinator its final admission callback. The attempt task therefore holds
its own counted lifecycle admission from the publication that fills the slot
until the task body has fully returned, so quiescence covers the task's
callback authority and not only the slot. `AgentExecution` remains the
execution and terminal semantic authority; `ConversationRuntime` remains the
task/runtime lifetime composition authority.

Foreground tool results retain one slot per model call and canonical model
call order. A committed background execution survives attempt cancellation
but not conversation drain. The process runner's existing physical proof is
composed transitively; no global process registry is introduced. Capability
preparation is counted until its owner returns, so shared `EnvironmentStore`
materialization is not cancelled globally by one conversation but cannot
create a late runtime callback or revision. Retained conversation-owned MCP
stdio runtimes are explicitly closed and physically settled before
quiescence; an unproven process or notification-task settlement is reported
as a runtime failure, not as successful shutdown. All runtime capability
commit points refuse after drain.

Runtime Client remains projection, control, and attachment state only. Client
detach, stdio EOF, TUI exit, and attachment drop do not cancel or drain a
conversation. The async client shutdown request awaits the runtime operation;
the `RuntimeShutdown` projection event marks admission closure, while the
`shutdown_completed` response marks successful quiescence.

## 1.3 Native interaction and approval coordination (M9.2 / Issue #64)

rustX has one provider-independent human-interaction plane. It is a
conversation-owned rendezvous, not a second execution engine:

```text
ToolRegistry preflight
  -> canonical Assistant ToolCall commit
  -> AttemptLifecycle::pre_tool (immutable PreToolView)
       Allow ------------------------------┐
       Deny -> one typed denied result      │
       Ask -> InteractionCoordinator       │
                   -> Runtime Client       │
                   <- typed response       │
       -> existing cancellation/start frontier
       -> exact original PreparedInvocation, or one result slot
```

`InteractionCoordinator` is the sole owner of interaction identity,
pending state, terminal response/cancellation coordination, and the waiter
rendezvous. `AgentExecution` remains the owner of tool scheduling and
execution. The Runtime Client and TUI only project and transport the
request; neither can mutate canonical history, execute a tool, or rewrite
arguments.

The ownership table is:

| Concern | Owner |
| --- | --- |
| Interaction identity and pending registry | `InteractionCoordinator` |
| Pre-tool decision | the attempt's required `PreToolPolicy` |
| Tool scheduling and start | Agent Loop |
| Tool identity and validated arguments | original `ToolCall` + `PreparedInvocation` |
| Tool result settlement | Tool Plane / Agent Loop |
| Response transport | Runtime Client |
| Rendering and input | TUI projection |
| Attempt cancellation | `AgentCancellation` |
| Runtime drain and quiescence | `ConversationRuntime` / `ConversationLifecycle` |
| Crash recovery | existing M9 recovery owner |

The pre-tool seam is total and typed: every `AttemptLifecycle` carries one
`PreToolPolicy`, while a runtime-created attempt receives one concrete native
binding to its owning `InteractionCoordinator`. The binding is not a
replaceable production rendezvous strategy; a standalone inert execution has
no interaction provider and therefore fails an `Ask` closed. `AlwaysAllow` is
the identity policy because the current product has no native rule saying
that a specific tool requires approval. This issue does not add a permission
language, risk-classification engine, allowlist, routing layer, or form
framework.
`PreToolPolicy` runs only after registry identity resolution, reserved metadata
stripping, and business-argument validation, and after the Assistant
`ToolCall` is canonical. It cannot resolve a tool, dispatch it, or alter the
prepared invocation.

An approval request contains only immutable, decision-relevant facts:
conversation/attempt/turn identity, `ToolCallId`, resolved `ToolId`, safe tool
name, origin, mode, validated arguments, and the policy reason. Conversation
identity is injected by the coordinator and attempt identity is supplied by
the owning execution at the narrow request boundary; neither is caller-
reported through approval facts. The response vocabulary is finite (`Allow`
or `Deny { reason }`) and contains no replacement arguments. Allow therefore
resumes the exact invocation that was already prepared; the Agent Loop checks
cancellation again at the existing start frontier before creating an executor
future.

The asynchronous policy boundary has one cancellation rule: after
`PreToolPolicy::evaluate()` settles, the Agent Loop checks cancellation before
consuming `Allow`, `Deny`, `Ask`, or a policy error. If cancellation is
observable, the decision is not consumed and the call receives the normal
cancelled result slot. An `Ask` response is subject to a second checkpoint;
`Answered(Allow)` is a rendezvous outcome, never tool-start authority.

The coordinator's pending state has one mutex-protected terminal transition:

```text
Pending --response--> Answered
Pending --owner cancellation/runtime drain--> Cancelled
```

The losing operation receives `not_pending` and cannot wake or resume the
owner. A terminal transition removes the live map entry, but its waiter keeps
a counted `LifecycleAdmission` until the owner consumes or drops the outcome.
The Runtime Client settled observation is published only after that waiter
authority is released and while a second counted settlement admission covers
the leaf observation callback. Thus an empty pending map is not quiescence,
and no interaction callback can begin after `Quiescent`.

`AgentCancellation` remains the sole cause authority for an attempt-owned
interaction. The coordinator retains the owning handle to consume its
already-selected first-winner reason at this boundary, but performs no cause
arbitration of its own. A response that arrives after cancellation is
observable cannot publish `Answered`; it is rejected as `not_pending` after
the matching `Cancelled { reason }` transition. During drain,
`ConversationRuntime` requests `RuntimeShutdown`, reads the active attempt's
winner, and propagates that reason to every live pending interaction. A prior
`UserRequested` cause therefore remains `UserRequested`; absent an earlier
winner, all interaction, tool, and attempt cancellation facts report
`RuntimeShutdown`.

The runtime has one active `CurrentAttempt` slot. A runtime-created native
interaction is published only by that attempt's AgentExecution, so every live
pending interaction at drain belongs to the one cancellation authority being
propagated. `finish_attempt` clears the slot only after the attempt's semantic
settlement and final callback, while interaction waiter admissions keep
quiescence behind the same boundary.

Interaction IDs are derived as `{AttemptId}-interaction-{ordinal}`. Attempt
identities are recovered from durable history and never reused, so a process
restart cannot make a delayed pre-crash response name post-restart work. Live
pending interactions are process-owned observations, not durable workflow
records. Recovery does not replay or reconstruct an old approval request; a
new runtime starts with no phantom pending interaction.

Runtime Client v1 carries the same semantic plane through
`interaction_respond`, typed acceptance/errors, `interaction_pending` and
`interaction_settled` events, and `snapshot.pending_interactions`. Snapshot
plus cursor and subscribe-after-cursor retain the existing repair invariant.
No capable attachment at publication fails approval closed as `Unavailable`.
Detaching an attachment only closes admission for future interactions; it
does not answer, deny, or cancel an already-published request. A later
attachment can answer a still-live request from the authoritative runtime
projection.

For a parallel tool batch, every call resolves its own pre-tool decision in
canonical call order before any executor starts. A denied or cancelled call
gets exactly one normal Tool Plane result slot and no `ToolExecutionStarted`
fact or executor future. Canonical ToolCall/result order is independent of
response timing. A denied result is typed `ToolExecutionStatus::Denied`, not
executor `Failed`.

The real `ConversationRuntime::shutdown()` path is covered by a deterministic
regression: it observes the Runtime Client pending event, linearizes
`Running -> Draining`, cancels the interaction through runtime-owned
`AgentCancellation`, and remains incomplete while a test gate holds the
waiter handoff after pending-map removal. Only after the interaction waiter,
AgentExecution, attempt task, and projection settlement release their counted
authority may the lifecycle publish `Quiescent`.

## 2. Layer model

### Layer 0: Domain and protocol types

This layer contains runtime-owned data contracts only:

- Message blocks and content blocks
- Model requests and model events
- Tool definitions, calls, and results
- Runtime events
- Runtime manifest
- Attempt, turn, and capability identifiers

It must not depend on provider SDKs, MCP SDKs, databases, HTTP frameworks, or process implementations.

## 2.1 Implemented Layer 0 contracts (M1)

The canonical contracts defined in M1 live in the `src` module tree as follows:

```text
runtime/identity.rs        strong IDs (ConversationId, MessageId, AgentId,
                           AgentVersionId, AttemptId, TurnId, EventId, ToolId,
                           InteractionId, ToolCallId, ToolExecutionId, ToolVersionId,
                           McpServerId, SkillId, SkillVersionId, ArtifactId)
                           and CapabilityRevision
runtime/interaction.rs     provider-independent native Approval request,
                           response/outcome, coordinator pending registry,
                           terminal rendezvous, and Runtime Client observation
runtime/cancellation.rs   CancellationSignal: the one runtime-owned
                           cancellation primitive shared by model adapters,
                           compaction, foreground tool execution, and
                           background work
runtime/types.rs           TokenMeasurement, TokenMeasurementSource,
                           CancellationReason, RuntimeError, RuntimeClock
runtime/inbound.rs         ConversationInboundMailbox (per-conversation
                           process-local coordination contract over the
                           durable Pending Inbound Inbox): InboundSequence,
                           InboundItem, InboundBatch, MailboxError
durable/inbox.rs           ConversationStore trait + domain types (InboundDraft,
                           AcceptedInbound, PendingInboundItem, PendingBatch):
                           the backend-independent acceptance/selection/adoption
                           operations, plus the fused `commit_model_turn_start`
                           contract (request-scoped context + RequestSnapshot +
                           ModelRequestStarted in one transaction)
durable/sqlite.rs          SqliteConversationStore: the M8 SQLite backend
                           (one semantic authority over Pending Inbound,
                           Message Ledger, Surface revisions, Request
                           Snapshots, Event Journal, and checkpoint metadata)
runtime/continuation.rs   ProviderContinuationState boundary (OpenAI Responses
                           stored/stateless, Anthropic opaque state)
message/content.rs         TextBlock, ImageReference, FileReference
message/types.rs           MessageBlock (System/User/Assistant/Tool), provenance
                           (SystemAuthority, UserSource, InboundKind),
                           UserMessageBlock.timestamp (persisted inbound
                           instant; absent for derived compaction summaries),
                           ContentBlockIndex, content enums per role
tools/types.rs             ToolDefinition (id, name, description, canonical
                           input schema, ToolInvocationPolicy,
                           ToolReplayPolicy, ToolOrigin), ModelToolDefinition
                           (the compiled model-facing definition), ToolCall,
                           ToolCallStart, ToolInvocation (stripped/validated
                           canonical invocation), ToolExecutionResult,
                           ToolExecutionStatus, ToolProgress, TruncationState
agent/lifecycle.rs          required PreToolPolicy / PreToolView seam and
                           AttemptLifecycle interaction rendezvous binding
tools/executor.rs          ToolExecutor boundary, ToolExecutionContext,
                           ProgressReporter, ToolRegistry (validating
                           definition/executor registry), PreflightOutcome
                           (both variants carry the registry-resolved ToolId
                           and ToolOrigin)
tools/schema.rs            JSON Schema validation, the reserved __rustx_
                           namespace, the model-facing schema compiler, and
                           reserved invocation metadata extraction
tools/workspace.rs         Workspace: the canonical runtime-owned workspace
                           boundary (canonicalized root, relative paths only,
                           no escape, symlink containment)
tools/artifacts.rs         ArtifactStore: conversation-owned opaque monotonic
                           artifact ids with streaming spooling
tools/environment.rs       ToolEnvironment: the explicit authorized child
                           environment (no wholesale parent inheritance)
tools/background.rs        ConversationBackgroundRegistry: conversation-owned
                           background executions (lifecycle state machine,
                           dispatch ownership commit, cancel-vs-complete
                           linearization, terminal inbound publication,
                           bounded progress snapshots)
runtime/subagent/          SubagentRegistry (conversation-owned one-shot
                           child runtimes: two-stage prepare/commit, driver
                           task as sole process owner, cancel/escalation,
                           exactly-once terminal publication), the bounded
                           framed control IPC, and process supervision
tools/runtime.rs           ConversationToolRuntime: the per-conversation
                           bundle of workspace, artifacts, environment, and
                           background registry handed to AgentExecution
tools/native/             the native tool plane: one module per native
                           capability (read/, write/, edit/, glob/, grep/,
                           bash/, background_task/), each owning its name,
                           description, typed input contract, generated
                           schema, executor, and private helpers;
                           registration.rs owns the NativeToolRegistration
                           pair and schema generation, input.rs the typed
                           input boundary, support.rs the shared failed/
                           success results and the one atomic file commit,
                           and mod.rs only composes the known native tools
tools/native/search/      the private native-search substrate shared by
                           Glob and Grep: the one workspace file-universe
                           policy (containment, traversal, hidden-file
                           visibility, ignore-file behavior, symlink
                           policy, normalized relative paths, deterministic
                           enumeration) — not a tool, never registered,
                           never a generic search-provider framework
tools/native/bash/        the Bash subsystem: registration (mod.rs), the
                           invocation lifecycle executor (executor.rs), the
                           output capture half (capture.rs), and the
                           per-invocation process supervisor
                           (supervisor.rs) — the supervisor is not a
                           separate tool
tools/mcp/                MCP adapter: protocol-revision negotiation,
                           configured server runtime, paginated discovery,
                           list-change invalidation, canonical calls,
                           progress, cancellation
tools/python.rs           immutable ToolVersion discovery/publication,
                           PythonToolEnvironment materialization, and the
                           canonical Python executor
model/types.rs             ModelRequest, ModelUsage, ModelProtocol, and the
                           provider-neutral request boundary. Model-visible
                           runtime context is canonical UserMessageBlock
                           history plus the frozen Effective System Prompt;
                           no semantic context attachment type crosses this
                           layer.
model/catalog.rs           the validated models.json catalog: explicit
                           provider endpoints and credential sources,
                           redacted credentials, model definitions,
                           capabilities, reasoning profiles, bounded compat
model/invocation.rs        opaque requestParams and their shallow-overlay
                           contract, per-protocol protected wire keys,
                           effective-capability intersection,
                           ResolvedModelInvocation, ModelBindingRegistry
model/session.rs           SessionModelConfig (the session's authoritative
                           mutable model state), the summary policy, and the
                           immutable AttemptModelSnapshot
model/fixture.rs           fixture construction for tests over the public
                           catalog path (no runtime behaviour of its own)
local_runtime/             the local conversation runtime process: bounded
                           session configuration, the one composition owner,
                           the startup argument contract, and the stdio
                           serving lifecycle
model/finish.rs            ModelFinishReason
model/error.rs             ModelError, ModelErrorKind
model/event.rs             ModelEvent (adapter-to-kernel streaming protocol)
events/types.rs            RuntimeEventEnvelope, RuntimeEvent, AttemptOutcome,
                           AttemptFailure
protocol/manifest.rs       RuntimeManifest and capability/context/limit sections
model/adapter/traits.rs    ModelAdapter runtime-owned interface, ModelEventStream
(cancellation lives in runtime/cancellation.rs; model adapters receive
                           the shared CancellationSignal)
model/adapter/validation.rs    deterministic local capability validation
model/adapter/block_index.rs   provider-key to ContentBlockIndex allocator
model/adapter/openai/     OpenAI Chat Completions and Responses adapters
                           (async-openai, custom no-retry HTTP service)
model/adapter/anthropic/  Anthropic Messages adapter (direct HTTP/SSE,
                           no Anthropic SDK)
```

Dependency direction between the modules points inward toward the shared
runtime-owned types:

```text
protocol → model, runtime
events   → message, model, tools, runtime
model    → message, tools, runtime
message  → tools, runtime
tools    → runtime (and the tool plane reuses runtime-owned coordination)
```

Serialization conventions for persistence-facing types:

- Enums use explicit discriminators with stable snake_case values
  (`"role"` for `MessageBlock`, `"type"` for events and content blocks).
- Strong IDs serialize as transparent JSON strings; `CapabilityRevision`
  serializes as a plain JSON number.
- Timestamps are UTC RFC 3339 strings (`chrono::DateTime<Utc>`).
- Durations are integer milliseconds (`duration_ms`, `retry_after_ms`).
- `serde_json::Value` is used only for genuinely arbitrary JSON: JSON
  Schema, tool-call arguments, structured tool output, and opaque provider
  continuation payloads.
- Persistence-facing structures never use `HashMap`; ordering is explicit.

The three execution layers each consume these contracts: the agent kernel
operates on them, the context engine assembles them into provider context,
and the model plane translates them to and from provider protocols.

#### M1 contract corrections discovered by later milestones

`ModelRequest.max_output_tokens` is a required `u32`, not an
`Option<u32>`. Real Anthropic integration proved that an adapter cannot
faithfully represent "no runtime output limit" when the provider requires an
explicit generation maximum (`max_tokens`), and hiding an arbitrary
adapter-local default behind `None` was rejected as hidden runtime policy.
The runtime must therefore resolve an effective output-token limit before
entering the adapter boundary; no adapter-local default exists. This is a
deliberate pre-1.0 canonical correction, not a compatibility shim.

`ContextManifest` gained `context_window_tokens` in M4, and Issue #42 moved
its *ownership*: the context window belongs to the selected catalog model,
not to the process. A session owns a static `SessionContextPolicy` (reserve
tokens, keep-recent target, summary output cap) and each attempt derives its
`ContextConfig` from that policy plus **its own** immutable model snapshot.
The soft input limit is still
`context_window_tokens - reserve_tokens - max_output_tokens` (checked,
impossible configurations rejected), but an attempt on a 32k model never
plans compaction with a previously selected 128k window.

Issue #42 also retired the universal `ReasoningEffort` enum. Reasoning is a
model-declared *named profile* whose wire behaviour is exactly its configured
`requestParams`; the runtime assigns no meaning to a profile name and
synthesizes no reasoning field. `ModelManifest` therefore carries a catalog
`ModelRef`, the selected `ReasoningProfileId`, and the semantic
reasoning-enabled state.

### 2.2 Attempt settlement invariant

Normally exactly one terminal runtime event is durably committed for an
attempt, and each terminal event carries only the data valid for that state:

```text
AttemptCompleted      finish reason
AttemptCancelled      cancellation reason
AttemptTimedOut       -
AttemptLimitExceeded  exceeded limit
AttemptFailed         normalized AttemptFailure
```

`AttemptCompleted` never carries a failure outcome, and unknown event
payload fields are rejected on deserialization, so contradictory terminal
encodings are impossible by construction. The platform-level `AttemptOutcome`
type maps one-to-one with these terminal events via
`AttemptOutcome::from_terminal_event`. When an attempt fails because a model
request exhausted its retry policy, `AttemptFailure::Model` preserves the
normalized `ModelError` without degrading it to a runtime error string.

The Agent Loop settles its execution state before attempting the terminal
Event Journal append. If that required append fails, no terminal event is
published or fabricated; the typed execution result carries the settlement
candidate and the durable failure, and the owning runtime enters
`DurabilityFailed`.

### 2.3 Streaming assembly identity

`ModelEvent` (and the corresponding `RuntimeEvent` deltas) target content
blocks by the rustX-owned `ContentBlockIndex`: the position of the block
within the ordered `AssistantContentBlock[]` of the message being assembled.
Interleaved text, reasoning, refusal, tool-call, and provider
continuation-state streaming therefore assembles unambiguously without
exposing any provider block id type. Refusal streams as refusal
(`RefusalDelta` / `AssistantRefusalDelta`) and assembles into
`AssistantContentBlock::Refusal`, never into plain text. `ToolCallStarted`
carries only the data known at start (`ToolCallStart`: call id, tool id,
name); raw argument fragments stream via `ToolCallArgumentsDelta`, and the
fully assembled `ToolCall` is emitted only at `ToolCallCompleted`.

### 2.4 Tool execution event identity

Every tool execution event carries the executing tool call identity:
`ToolExecutionStarted`, `ToolExecutionProgress`, `ToolExecutionCompleted`,
and `ToolExecutionFailed` all carry `tool_call_id` and `tool_id`. With
parallel execution, completion order may differ from call order, and each
completion remains attributable to its originating call. `ToolExecutionResult`
itself stays reusable and carries no call identity; identity is attached at
the event and message boundary only.

### 2.5 Message content single source of truth

The durable Message Ledger (M8) is the only authoritative store for canonical
message content. `AssistantMessageCommitted` and `ToolMessageCommitted` are
execution facts that reference the committed message by its stable
`MessageId` and never embed the message body, so the Event Journal never
holds a competing copy.

A committed-message event is inserted in the same `ConversationStore`
transaction as the Ledger body it references. The same rule applies to
`CompactionCompleted` (summary plus exact Surface revision) and
`ModelRequestStarted` (snapshot plus referenced Surface revision). This makes
an orphan reference structurally impossible in the SQLite backend; a failed
transaction exposes neither side. Persist-before-publish then appends the
committed event before any observer or external projection sees it.

### 2.6 Durable Pending Inbound Inbox (Issue #63)

The durable authority split for inbound work:

```text
Pending Inbound Inbox     = accepted / not-yet-adopted inbound durability
                            (the one per-conversation InboundSequence
                            allocator; the acceptance linearization point)
ConversationInboundMailbox = process-local coordination / wakeup only
Message Ledger            = adopted canonical conversational facts
Conversation Surface      = current model-visible ordering/projection
ConversationRuntime       = admission + safe-boundary adoption owner
Event Journal             = execution facts
```

Two linearization points are defined exactly:

1. **Acceptance** ([`ConversationStore::accept_inbound`]): the durable
   per-conversation sequence allocation, the pending record, and any
   producer correlation/idempotency state commit in **one** transaction.
   Producer success is returned only after that commit. The process-local
   wake fires strictly after it and is a liveness optimization — a crash
   between the commit and the wake loses nothing. A successful acceptance
   and the coordinator's `shutdown` have one total ordering: the coordinator
   holds its one state lock across the lifecycle/shutdown decision **and**
   the durable acceptance, so shutdown linearizes either entirely before the
   acceptance (the acceptance then fails with `Shutdown` and commits nothing)
   or entirely after it.
2. **Adoption** ([`ConversationStore::adopt_pending_batch`]): the selected
   finite watermark batch is appended to the durable canonical Message
   Ledger, advanced through Surface Append operations, updates the current
   checkpoint, and removes its pending records in **one** transaction. Crash
   before the commit leaves the items pending; crash after it makes them
   canonical and Surface-visible exactly once, never independently
   re-adoptable.

`select_pending_batch` is a non-destructive finite-watermark snapshot: an
item accepted after the snapshot belongs to the next batch. The durable
store persists the **complete** canonical Message Ledger — adopted inbound
`User` messages, `Assistant` messages, `ToolResult`s, context facts, and
compaction summaries in canonical order — through the prepare →
durable-append → infallible-install seam. A complete `ToolResult` sibling
batch commits atomically (one durable transaction), so a partial tool-result
group can never become canonical. Background terminal notifications converge
on the same acceptance seam with a deterministic producer correlation, so a
retry with the same committed correlation can never publish a duplicate
notification; the durable terminal inbound commits **before** the background
record is exposed as terminal, and a durable publication failure retains the
terminal candidate in an explicit `PublishingTerminal` state rather than
faking `Running`. Retaining the candidate is not the settlement ownership
itself: the background runner drives the production settlement continuation
— publication attempt #1 inside `finish`, then exactly one registry-owned
retry under the same deterministic correlation (exactly-once even when
attempt #1 committed durably but observed an error). When that bounded
budget is exhausted, the candidate stays retained and the failure is
reported to the owning `ConversationRuntime` through the narrow
`BackgroundDurabilityFailureSink` seam, which places the owning runtime
into its explicit `DurabilityFailed` state; no runtime-owned execution can
leave its production settlement path without a guaranteed terminal
publication or that explicit degraded outcome. A standalone never-claimed
registry may retain an observable `PublishingTerminal` candidate when its
bounded budget is exhausted because it has no owning `ConversationRuntime`
durability-health sink.

A durable Ledger append is **not** by itself a resumable runtime safe
boundary. The durable store's complete Ledger ordering is the canonical
truth of committed message facts; normal live runtime resumption additionally
requires a structurally complete current Surface boundary. The conversation
domain's `recovery_safety` predicate answers that question fail-closed for an
incomplete tool turn (an `Assistant` tool call without its committed
`ToolResult` sibling). A compaction summary can no longer be durable without
its exact Surface Replace and checkpoint because M8 commits those facts in
one transaction.

M9a supersedes the M8 restart *gate* with a restart *contract*: an incomplete
tool turn is now repaired from durable evidence rather than refused, and
`recovery_safety` becomes the checked **post-condition** of reconciliation
instead of a construction-time veto. It remains the live admission guard, so a
failed tool-result batch during normal execution still fails closed. See
[Recovery model](#7-recovery-model).

### Layer 1: Agent kernel

The kernel owns deterministic execution semantics:

- Attempt state machine
- Turn lifecycle
- Model -> tool -> model loop
- Tool batch ordering
- Turn-boundary inbound message draining
- Attempt termination rules
- Retry and compaction decision points
- Typed lifecycle interception coordination (`PreStepPolicy`,
  `ToolResultObserver`), the deferred context buffer, and the split between
  lifecycle *timing* and semantic *ownership*

The kernel operates only on rustX canonical types and interfaces.

The runtime inbound boundary (`src/runtime/inbound.rs`, Layer 0) is
coordination only. Since Issue #63 the [`ConversationInboundMailbox`] is the
narrow acceptance/publisher seam over the **durable Pending Inbound Inbox**
(`src/durable`): it validates eligibility and lifecycle, durably accepts
through the [`ConversationStore`], then publishes the process-local wake and
observation. It owns **no** sequence allocator and **no** payload queue. The
kernel's `AgentExecution` selects and adopts exactly one finite batch per
safe turn boundary. Canonical history is the durable Message Ledger and the
Event Journal records execution facts; the mailbox is not a scheduler,
supervisor, or persistent service layer.

#### M3 implementation (agent loop)

The M3 implementation freezes the agent-loop boundary in `src/agent` and
the tool execution contract in `src/tools/executor.rs` (the provisional M3
`Tool` trait was replaced by the canonical M5 [`ToolExecutor`] boundary):

```text
canonical input state
        |
ModelAdapter (canonical ModelRequest in, ModelEvent stream out)
        |
ExecutionStateMachine: Idle -> RunningModel -> WaitingForTool -> RunningModel -> Completed
        |
ModelEventAssembler: stream validation + ordered AssistantMessageBlock assembly
        |
ToolRegistry preflight: resolve -> extract -> strip -> validate -> dispatch
        |
deterministic scheduling phases (sequential barriers, parallel groups)
        |
        durable Event Journal facts, ending in one terminal event when its durable append succeeds
```

The loop owns execution semantics, message assembly, tool execution,
continuation state, cancellation observation, and runtime-event emission.
The durable Event Journal owns historical execution facts; the observer is
only the live projection seam. Adapters own provider protocol translation only; the validating
[`ToolRegistry`] pairs canonical [`ToolDefinition`] values with
[`ToolExecutor`] implementations and never falls back id-first.
Continuation state propagates losslessly without fabrication, cancellation
always settles as a terminal cancellation candidate, and a normally settled
attempt commits exactly one terminal `RuntimeEvent`. A failed terminal append
is an explicit durable failure, not a fabricated event. See
`docs/agent-loop.md` for the full boundary description.

The M3 test suite drives the loop with scripted fixture models and tools
(`tests/common/fake.rs`), asserts behavior through the recorded
`RuntimeEvent` trace and the platform `AttemptOutcome`, and reconstructs
execution phases from traces (`tests/common/mod.rs`).

### Test seams are not published API

Substituting a runtime-owned dependency is a `#[cfg(test)] pub(crate)`
seam, never a published item:

```text
ModelBindingRegistry::new         one binding path; builds the three
                                  supported protocol adapters directly
ContextRuntime::for_attempt       one context-runtime constructor; derives
                                  the summarizer from the frozen snapshot
```

An external test binary can only reach `pub` items, so a seam usable from
`tests/*.rs` is necessarily a seam a consumer can call; `#[doc(hidden)]`
hides it from documentation without removing it. The suites that need a
scripted `ModelAdapter` or a scripted `ContextSummarizer` therefore compile
into the crate's own test build through `src/lib.rs`, with their sources
under `tests/scripted/` so `src/` carries production code only. The
remaining `tests/*.rs` binaries use published API exclusively; fixtures
shared by both live in `tests/common/`, and fixtures that need a seam live
in `tests/scripted/support/`.

### Three provider fixtures, three bounded purposes

```text
tests/scripted/support/model.rs    a scripted injected `ModelAdapter` behind
                                   a validated catalog binding. Internal
                                   state machines and units that need no
                                   network and no provider boundary.

tests/common/mod.rs FixtureServer  a raw Rust HTTP/1.1 fixture. One adapter
                                   in isolation: request serialization,
                                   stream parsing, error normalization,
                                   one-attempt/no-retry. No Agent Loop.

test-support/fake-provider         the canonical external provider-emulation
                                   boundary. Composed Agent Loop conformance
                                   across the real runtime and a real
                                   external provider process.
```

The third is the one that decides what "conformance" means. It is an
external Python 3.12 process (managed by uv, never a production runtime
dependency) that speaks the real HTTP/SSE provider protocols, and
`tests/issue47_conformance.rs` composes the real `LocalConversationRuntime`
against it:

```text
test driver -> real catalog, binding, adapter, HTTP client, stream parser,
               Agent Loop, context engine, tool runtime, capability plane,
               Runtime Client projection
            -> real HTTP + SSE
            -> the scripted external provider
```

Nothing in rustX is substituted there — no fake adapter, no fake tool, no
fake Skill runtime, no second Agent Loop. Scenarios are strict ordered
scripts: request *N* meets step *N*, an unexpected or extra request fails by
default, and an unconsumed step fails the process. Race-sensitive tests are
ordered by named provider-side gates and an observation barrier rather than
by sleeps: a driver waits until the provider provably reached a point,
performs its runtime action, and releases. `test-support/fake-provider/README.md`
documents the process, control, and scenario contracts.

The lower two fixtures are retained deliberately. Routing an adapter
translation test through an external process, a Python toolchain, and a
scenario definition to assert one JSON field would add cost without adding
truth. They are not, however, an implementation of composed conformance: a
test that exercises the Agent Loop, the context engine, the tool runtime, or
the capability plane belongs on the external boundary.

The Issue #22 inbound batching integration is canonical:
`ConversationToolRuntime` owns the one conversation inbound mailbox, and at
every safe turn boundary the loop performs exactly one finite
watermark-bounded drain of `tool_runtime.mailbox()` and appends every
drained message as its own canonical `UserMessageBlock` before the next
model request. The loop and the background runtime provably share one
mailbox: an `AgentExecution` over a tool runtime of a different
conversation is rejected structurally at construction. Mailbox draining
adds the safe-boundary cancellation-before-selection rule; observable
cancellation before every model turn is a generic Agent Loop invariant for
all executions. See `docs/agent-loop.md` section 9 for the full boundary
description.

### Layer 2: Context engine

Issue #54 fixes the conversation boundary used by the context engine:

```text
System / User / Assistant / Tool
        ↓ canonical roles
Message Ledger
  append-only immutable facts
        ↓ current active MessageIds only
Conversation Surface @ SurfaceRevision
  sole authority for active identity, order, and visibility
        ↓ keyed reads of the finite current Surface
Context Engine
  projection, token pressure, retention, and compaction planning
```

The Context Engine owns only the finite projection of canonical conversation
and its token/retention/compaction behavior. Context Assembly, request
admission, RequestSnapshot creation, and provider translation are owned by
the Agent Loop and model plane respectively.

Compaction appends one canonical `User` message with
`UserSource::Runtime` and `InboundKind::CompactionSummary`, then applies one
complete-message Surface `Replace`. It never deletes or mutates Ledger facts.

#### Current context implementation

The Issue #55 implementation freezes the boundaries in src/context and
src/agent/execution.rs:

```text
claimed inbound
    ↓ finite ContributorInputSnapshot
ContextAssembly (native + certified extension proposals)
    ↓ validated AcceptedContext
Agent Loop staging (scratch validation, prepared canonical commits;
                    no durable effect)
    ↓
cancellation-vs-start arbitration (attempt start gate held; M9b)
    ↓ commit_model_turn_start: one transaction
canonical User context + Surface + RequestSnapshot + ModelRequestStarted
    ↓
ModelAdapter → provider
```

The engine is a deterministic pure function of the current Surface, keyed
Ledger results, tool definitions, the exact Effective System Prompt, and
observed provider usage: the same inputs always produce the same projection,
plan, and estimate. It owns no provider knowledge — token estimation is
pluggable (`TokenEstimator`, with a default
`ceil(bytes / 4)` formula), and the engine holds no model catalog.

Its configuration is split by ownership. The session owns the static
`SessionContextPolicy`; the *window* comes from the attempt's immutable model
snapshot. `ContextRuntime::for_attempt` derives one engine per attempt from
those two inputs, so a session model change between attempts changes the next
attempt's compaction arithmetic and never the running one's.

Key contracts:

- `ContextProjection` contains only complete canonical messages in current
  Surface order; it never creates a partial Assistant projection.
- The canonical roles are exactly `System`, `User`, `Assistant`, and `Tool`.
  `Assistant` owns `ToolCall` identity and arguments; `Tool` owns the result
  and references `ToolCallId`. A runtime compaction summary remains a `User`
  message with `UserSource::Runtime` and
  `InboundKind::CompactionSummary`.
- The interim system rule is bounded: a `Replace` span cannot contain
  `System`, but a later `System` does not pin all earlier content or resurrect
  retired Surface history. Issue #55 owns the Effective System Prompt and
  Request Snapshot architecture.
- Token measurements carry explicit provenance
  (`ProviderReported`/`Estimated`); provider-reported `input_tokens` apply
  only to the exact measured projection (deterministic fingerprint), and
  estimates never become provider usage.
- `TokenMeasurement` and `TokenMeasurementSource` are Layer 0 value contracts
  owned by `runtime/types.rs`. The Context Engine owns the estimator,
  provider-observation validity, provenance application, and compaction
  accounting behavior in `context/tokens.rs`; it does not own the shared
  measurement data type.
- Cut selection is structural: a deterministic index of tool-call/result
  edges rejects orphan tool messages and never separates a call from its
  result. A candidate is always a whole-message span.
- `SurfaceRevision` is a stable reconstruction reference within one live
  conversation lineage. Historical Surface operations are retained for exact
  replay; later Appends and Replaces never change an earlier revision.
- Normal projection, planning, and compaction read only current Surface
  identities and keyed Ledger bodies. They do not enumerate the Ledger or
  scan the historical Surface operation log; current replacement generation
  is O(1) head metadata.
- The `ContextSummarizer` service is provider-neutral; the production
  `ModelBackedSummarizer` issues a canonical one-off `ModelRequest` (no
  tools, no Agent Status, no Skill catalog, no continuation) through the
  `SummaryRequest::model_input()` assembly shared with the planner. The
  summary input limit therefore covers the fixed instruction, serialized
  request, and canonical User wrapper, using the summary invocation's own
  effective context window and output budget, never the primary model's
  window, through the
  existing `ModelAdapter` boundary. It is constructed from the attempt's
  *frozen summary policy*, never from an independently injected summarizer:
  in `session` mode that is the attempt's own primary invocation, in
  `explicit` mode a separately resolved catalog model. The context plane's
  summary output safety cap is applied through the runtime-owned protected
  max-output field and never by mutating a reasoning profile or a
  request-parameter object.
- The mandatory progress rule (coverage advances and projected estimate
  strictly decreases) is the anti-loop invariant; successful compaction
  invalidates the pending provider continuation, and
  `ContextWindowExceeded` is recovered through exactly one bounded
  compact-and-retry
  (`MAX_CONTEXT_OVERFLOW_RETRIES_PER_MODEL_TURN = 1`).
- Agent Status is sampled from authoritative runtime facts and admitted as a
  canonical `UserSource::Runtime` context message with
  `InboundKind::Context(ContextKind::AgentStatus)`. It is rendered by the
  native composer before assembly, participates in normal history,
  projection, token accounting, and Surface revisioning, and is never
  reinjected by an adapter. Identical rendered bytes at distinct admitted
  steps receive distinct canonical identities.
- The initial-turn trigger is an explicit execution mode, never an `Option`
  used as a status switch: `AgentExecutionRequest` carries one
  `InitialTurnTrigger` — `FreshInbound(FreshInboundTurn)` makes validation,
  Agent Status, and fresh-inbound compaction protection mandatory and keeps
  the trigger pending until one successful model invocation observes it;
  `Continuation` expresses an intentional pure continuation with no new
  inbound turn and therefore no Agent Status on the first request. There is
  no `disable_status`, no optional status mode, and no legacy no-context
  execution path.
- A `FreshInboundTurn` is ordered according to canonical history/inbound
  sequence: `validate_against` requires the referenced messages to occur in
  strictly increasing canonical position in `message_ids` order
  (`OutOfCanonicalOrder` otherwise); the runtime never sorts or reinterprets
  a caller-supplied turn order.
- A provider's section identity is captured at registration and frozen as
  runtime-owned metadata: `section_id()` is called exactly once, validated
  against reserved and duplicate ids, and never queried again; composition,
  ordering, diagnostics, and provider listing all use the stored identity, so
  a stateful provider can never shadow a reserved id or mutate into a
  duplicate.
- Extension providers contribute structured runtime facts
  (`AgentStatusFact` label/value pairs) only: the provider result type
  cannot express built-in section variants, so built-in section semantics
  are runtime-owned and can only be constructed by the Agent Status
  composer/runtime. The canonical context renderer owns labels, separators,
  and layout, and providers never hand over pre-rendered footer lines.
- Context failure semantics are separated at the attempt boundary: failures
  that occur while preparing model context **before compaction starts**
  (invalid pending fresh-inbound state, a failing status provider, a
  projection preparation failure) classify as
  `RuntimeError::ContextPreparationFailed`, while an actual proactive
  compaction pipeline failure keeps `RuntimeError::ContextCompactionFailed`.
  An overflow whose recovery compaction fails still preserves the normalized
  `ContextWindowExceeded` as the final model failure with the compaction
  diagnostic carried by `CompactionFailed`.
- A fresh inbound turn that has not been observed may never be compacted
  away; when preserving it makes the projection impossible, planning fails
  with `CannotFit` rather than summarizing the unobserved instruction.

#### Native Skill context (Issue #55)

The Skill catalog is rendered once from the attempt's immutable capability
snapshot and enters the same Context Assembly path as every other
model-visible context fact:

- Skill guidance is admitted as canonical
  `UserSource::Runtime`/`InboundKind::Context(ContextKind::SkillGuidance)`
  context. It is not a special request attachment and is never copied into
  canonical tool definitions. Tool definitions remain immutable capability
  state and are frozen independently in `RequestSnapshot`.
- The accepted Skill fact is an ordinary committed User message and is
  therefore included in Ledger, Surface, compaction, and historical request
  reconstruction. The capability snapshot itself remains immutable for the
  attempt.
- It participates in normal canonical-message token accounting and
  complete-message compaction.
- Adapters receive it only through the final canonical User projection and
  perform protocol translation; they do not discover or inject it.

The context path is **mandatory**: every `AgentExecution` is constructed
with a `ContextRuntime`, a `ConversationToolRuntime`, and an attempt
capability lease
(`AgentExecution::new(request, adapter, capability, cancellation,
context_runtime, tool_runtime)`); the no-context compatibility path,
`with_context_runtime`, and any capability-free constructor are gone, and
there is no Agent Status disable flag.
See `docs/context-engine.md` for the full boundary description.

#### Issue #55 request reconstruction contract

The Agent Loop is the single coordination owner for Context Assembly and
request admission. `ContextContributor` receives only a finite
`ContributorInputSnapshot` and returns an awaited boxed future of transient
`ContextProposal` values; `ContextAssembly::assemble` is the one async typed
assembly boundary. Bounded contributor work settles before the start
arbitration.
RustX owns contributor identity, trusted provenance, semantic lanes,
canonical `MessageId` allocation, Ledger/Surface mutation, cancellation,
and provider dispatch. Native context and certified extensions therefore
share one validation path.

The generic model-turn start commit point is the cancellation-vs-start
arbitration in `src/agent/execution.rs` (Issue #12, M9b): the attempt's
start gate is held across the cancellation check and the fused
`ConversationStore::commit_model_turn_start` transaction. Cancellation
that linearizes first leaves no dynamic context, Surface advancement,
RequestSnapshot, start fact, or provider request. Once the commit
linearizes first, accepted context and its Surface revision are historical
and are not rolled back by provider failure or cancellation.

`RequestSnapshot` freezes every non-history input needed by one
provider-neutral request: `RequestIdentity`, exact `SurfaceRevision`, the
rendered Effective System Prompt, effective `ModelInvocationConfig`, model
window, reasoning values, tool definitions, capability revision,
`ContextGeneration`, and opaque continuation state. The Surface revision is
an exact historical reference; request-time rendered/configuration values
are stored by value. `RequestSnapshot::reconstruct` hydrates only that
historical Surface revision and the frozen values, and the Agent Loop checks
structural equality with the actual `ModelRequest` before adapter
translation. Current contributors, Skills, configuration, filesystem, and
runtime status are never consulted.

During execution, `AgentExecution` keeps only the current attempt's bounded
request references. At request start the ConversationStore durably commits
the immutable snapshot and its `ModelRequestStarted` fact. The runtime's
`RequestHistory` is now a durable read handle, not a `Vec<RequestSnapshot>`
and not a second transcript. Historical inspection either loads one snapshot
by key or reads a bounded, fallible page with an exclusive sequence cursor;
it never retains every request in process memory.

The same rule applies to execution facts: `AgentExecution` owns bounded active
state and the current conversation working set, not a complete attempt
Event Journal. Each event is durably appended first, then delivered to the
live observer when one is attached, and its full body is released from the
loop. `AgentExecutionResult` transfers the settlement candidate, durability
status, and current conversation only; historical events are inspected from
`ConversationStore::read_events` through bounded pages.

An overflow retry reuses the staged ContextGeneration and canonical
context facts. `ContextWindowExceeded` does not prove that fresh inbound was
observed, so compaction still protects the pending `FreshInboundTurn`. Only
compaction-dependent Surface/request fields may change; contributors are not
reinvoked, the pre-step policy is not re-evaluated, tool-result observations
are not replayed, and duplicate context is never committed.

#### Issue #56 typed lifecycle interception

The Agent Loop remains the lifecycle owner. Issue #56 adds exactly two
phase-specific typed seams, carried by one required immutable
`AttemptLifecycle` value (`src/agent/lifecycle.rs`) per attempt:

```text
Context Assembly (deferred + native + extension proposals)
        |
PreStepPolicy               Enter | Reject(reason)
        |
staging (scratch validation, no durable effect)
        |
cancellation-vs-start arbitration   <- the one linearization point
        |                             (start gate held across check + commit)
commit_model_turn_start -> context + Ledger/Surface + RequestSnapshot
                           + ModelRequestStarted, in one transaction

Assistant(ToolCall A, ToolCall B) committed
        |
execute, settle every CallSlot, commit ToolResult A then ToolResult B
        |                                <- batch structural settlement point
cancellation checkpoint    <- before each observer, and again once it settles
        |
ToolResultObserver pass, in (canonical ToolCall order, producer order)
        |
validate count + content                 <- observer transaction boundary
        |
stamp the observer's bound producer reference
        |
Agent-Loop-owned deferred buffer (transient, not history)
        |
next Context Assembly -> resolve producer -> lane + provenance
        |
PreStepPolicy -> admission -> canonical User context, owned by its producer
```

`AttemptLifecycle::inert()` is the identity configuration, so no execution
path branches on whether a seam is attached. The `ConversationRuntime`
currently constructs the inert configuration, exactly as it constructs
`ContextRuntime::for_attempt` without certified contributors — a configured
owner arrives with the consumer that needs it, not as speculative plumbing.

**Lifecycle timing and semantic ownership are separate concerns.** The Agent
Loop owns *when* a proposal becomes eligible: "post-tool" means its owning
tool batch settled, so it enters the next primary step rather than this one.
Context Assembly owns *who* the fact belongs to: every staged proposal carries
the `DeferredContextProducer` the loop stamped from its observer's binding —
never from anything the observer returned — and assembly resolves that
reference before deriving lane, `UserSource`, and `ContextKind`, through the
same table it applies to that owner's request-time proposals. There is no rule
turning post-tool proposals into native runtime context: a certified extension
(#58) producing deferred post-tool context keeps its extension identity,
provenance, and lane.

**Binding is not admission.** `ContextAssembly::register_extension` is the one
semantic identity/provenance/attestation authority. The lifecycle seam exposes
only `with_native_tool_result_observer` and
`with_extension_tool_result_observer`, and the latter takes a logical key that
any caller can construct — a reference, not a credential. At assembly time the
native producer resolves to the rustX-owned runtime observation owner, and an
extension producer resolves to the matching **registered** extension, using
that registration's own generation and attestation. An unregistered key fails
the assembly with `ContextAssemblyError::UnregisteredContributor` before
admission: no lane, no `UserSource::Extension`, no synthesized generation. A
certified extension that only defers still resolves to its authoritative
generation. The lifecycle seam therefore cannot become a second registry.

`PreStepPolicy` observes the final immutable `AcceptedContext` and returns
`Enter` or `Reject`. It has one owner per attempt rather than a chain — a
chain would require a second ordering model on top of the Issue #55
lane/identity order, and no consumer needs several independent admission
decisions. It is the single downstream authority every proposal converges on,
so a rejection proves no proposed dynamic context committed, no Surface
revision advanced because of it, no `RequestSnapshot` was frozen, and no
provider request started. It owns no cancellation: a pending bounded
evaluation settles and the generic checkpoint still decides admission.

`ToolResultObserver` receives an immutable `ToolResultObservation` of one
finalized result — canonical batch position, `ToolCallId`, registry-resolved
`ToolId`, typed `ToolOrigin`, the committed `ToolExecutionResult`, and an
`ObservedToolInvocation` carrying the resolved `ToolInvocationMode` and the
**validated business arguments** of the call. The arguments are needed because
a result under-determines the fact it describes: native Read returns content,
while the path lives only in the invocation, and re-deriving it from history
would build a second drifting authority. They are read-only, metadata-stripped
and provider-payload-free, and absent entirely for a preflight-rejected call
that never resolved an invocation. The model-facing tool name is deliberately
absent, so recognizing the native rustX Read capability is a typed identity
question (`tool-read` + `ToolOrigin::Builtin`) rather than a name comparison;
an MCP or Python tool publicly named `read` can never be confused with it.
Both `PreflightOutcome` variants carry the registry-resolved identity and
origin from the same resolved `ToolDefinition`.

Observers are bound to a `DeferredContextProducer`, at most one per semantic
owner, so a native runtime owner and one or more certified extensions can each
own deferred context about the same settled call. They are invoked and ordered
by logical producer, giving the deferred order key `(ToolCall batch position,
producer identity, proposal FIFO)` with no registration-order term and no new
ordering model.

An observer returns bounded `UserMessageProposal` values only — not the full
`ContextProposal` vocabulary. A settled tool batch is a conversational fact,
and the only concrete requirement (including #58's `PostToolUse
additionalContext`) is deferred conversational context, so this seam cannot
change the Effective System Prompt of the following turn. That is enforced by
the return type, not by a runtime check.

The bounded return value is checked at the **observer transaction boundary** —
per-observation count against the established `MAX_PROPOSALS_PER_CONTRIBUTOR`
limit, running attempt total, and per-proposal content — before a single
proposal is staged, so an unbounded observation is rejected where it happens
rather than one step later.

Cancellation ownership stays with the Agent Loop: it is checked before each
observer starts and again once that observer settles, before its return value
is consumed. An in-flight bounded observation is allowed to settle, but once
cancellation is observable no later observer starts and neither an observer's
success nor its failure can decide the terminal outcome.

Any failure or cancellation in the pass discards every proposal of that pass
and clears the buffer, leaving no partial deferred state. The buffer is not a
second transcript, ledger, or Surface, and the observer is not a privileged
committer: a later pre-step rejection or cancellation prevents the deferred
context from ever becoming canonical.

Tool-execution wrappers/middleware, post-tool result replacement, pre-tool
argument or identity rewriting, question/form frameworks, generalized
permission/risk policy, subagent lifecycle observation (#60), and
turn-stopping/forced continuation are intentionally absent. The bounded
native `PreToolPolicy`/Approval seam is implemented by M9.2 above; it does not
expand into those frameworks.
`docs/agent-loop.md` section 4.3 carries the full authority matrix.

#### M5 implementation (native tool plane)

The M5 implementation freezes the canonical tool plane boundary in
`src/tools` and replaces the provisional M3 `Tool` trait:

```text
canonical ToolDefinition (tool-owned schema + two policy axes)
        |
validating ToolRegistry (definition + Arc<dyn ToolExecutor>)
        |
preflight: resolve -> extract reserved metadata -> strip -> JSON Schema validate
        |
ToolInvocation (stripped/validated business arguments + resolved mode)
        |
ToolExecutor::execute(ToolInvocation, ToolExecutionContext)
        |
ToolExecutionResult
```

The two policy axes are independent:

- [`ToolExecutionPolicy`] (`ForegroundOnly` / `BackgroundOnly` /
  `ModelSelectable`) decides ownership and settlement: foreground work is
  attempt-owned and physically cancellable, background work is
  conversation-owned and detached after accepted dispatch.
- [`ToolConcurrencyPolicy`] (`Sequential` / `Parallel`) decides scheduling
  within one tool-call batch: a `Sequential` invocation is an exclusive
  barrier, adjacent `Parallel` invocations run as one group.

The canonical input schema is tool-owned and never mutated. For
`ModelSelectable` tools the model-facing compiler decorates a clone with the
required reserved `__rustx_execution` field
(`{"type": "string", "enum": ["foreground", "background"]}`) inside the
reserved `__rustx_` top-level property namespace. The runtime extracts the
field, resolves the canonical mode, strips it, and validates the remaining
business arguments against the original schema before dispatch; reserved
fields are never forwarded to executors. `ModelRequest.tools` carries the
compiled [`ModelToolDefinition`] values only — provider adapters translate
them verbatim and never decide execution semantics.

The registry is a correctness boundary: duplicate `ToolId`s, duplicate
model-facing names, empty identities, invalid or non-root JSON Schema,
reserved `__rustx_*` collisions, invalid policy combinations, and
background-capable `background_task` registrations are rejected; a
canonical call whose id and name disagree is a contract violation. Tool
definitions reach the model in deterministic registration order, and the
context engine accounts the exact compiled definitions.

One conversation owns one `ConversationToolRuntime`, constructed exactly
once from a bounded `ConversationRuntimeConfig` that binds one
`ConversationStoreBinding` (the mailbox capability is derived from it),
the clock, the event sink, the environment, the workspace, and the
artifact store; after construction the conversation background registry
identity and its execution records are stable and can never be replaced
or reset by a configuration change. The runtime owns the canonical
`Workspace` boundary (canonicalized root; relative paths only; no `..`
escape; symlinks contained) and the `ArtifactStore` (opaque monotonic
`artifact_N` ids, streaming spooling). The artifact root and the
workspace root must be disjoint filesystem regions: equal roots, nested
roots, and symlink-resolved overlap are rejected at construction, so
runtime-private output files are never observable through Glob/Grep/Bash.
The explicit `ToolEnvironment` and the authoritative
`ConversationBackgroundRegistry` complete the bundle. Background
executions own a deterministic `exec_N` `ToolExecutionId`, a lifecycle
state machine (`Starting -> Running -> Cancelling -> terminal`), a
two-stage dispatch with an explicit ownership commit linearization
point, a cancel-vs-completion linearization rule, bounded latest progress
snapshots, and exactly-once terminal inbound mailbox publication
(`background-exec_N-terminal`). The `background_task` intrinsic
(foreground-only, sequential) provides `status` and idempotent `cancel`.

The dispatch ownership commit is the background linearization point: the
registry synchronization boundary is acquired first and the final
attempt-cancellation observation happens at that same protected boundary.
Cancellation observable there rolls the prepared dispatch back completely
(no published record, no accepted result, the runner never begins);
ownership wins commits exactly once and a later attempt cancellation can
never reclaim the detached execution. Since M9a the commit also writes the
durable `BackgroundExecutionCommitted` fact — the execution identity, its
owning `ToolCall`/tool, and the frozen tool name — **before** releasing the
runner's start gate, so no detached external side effect can begin without
durable evidence a restart can classify. A durable failure there rolls the
dispatch back completely and returns `BackgroundDispatchError::Durable`;
nothing is detached. The fact opens the `background:{execution_id}` durable
lifecycle that the one terminal publication closes. Cancellation intent that commits
first retains its reason and canonicalizes the final terminal result, so
the registry winner and the stored result always agree (only an explicit
process-control failure after cancellation intent settles as `Failed`).
All progress entering runtime state and events passes through one shared
UTF-8-safe bound (`bound_tool_progress`) used by both foreground and
background paths. Foreground progress is additionally cardinality-bounded
per active call (`MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL`): one invocation
retains at most that many normalized observations before structural
settlement — the first `MAX - 1` observations pinned, the final slot
tracking the newest observation — and only the retained observations become
durable `ToolExecutionProgress` Event Journal facts at batch commit;
coalesced observations never cross the durable commit point. Background
progress retains only the latest bounded snapshot per execution record.

Agent Status owns the runtime `background_execution` built-in section: the
executing attempt samples a read-only active snapshot from the background
registry, the composer builds the section (never an extension provider),
and the renderer shows active executions only in allocation order.

The native tool plane implements Read, Write, Edit, Glob, Grep, and Bash as
ordinary registrations under the concrete bounded `NativeToolPolicies`
configuration: each ordinary native tool independently selects its
execution and concurrency policy (foreground-only sequential by default,
with `BackgroundOnly` and `ModelSelectable` as legal per-tool choices).
The only intentionally fixed policy remains the runtime intrinsic
`background_task`.

One native capability owns one module boundary. A native tool module owns
its name, description, typed input contract, generated schema, executor,
and private helpers, and constructs itself through its own
`registration(policy)` function returning a `NativeToolRegistration`
(definition + executor); `tools/native/mod.rs` only composes the known
native tools. Composition stays explicit and deterministic: no discovery,
no plugin loading, no registration macros, no generic tool factory.

##### Model-facing ordinary native tool contracts

The model-facing schemas of the six *ordinary* native tools follow
established Pi coding-agent conventions rather than rustX-specific
parameter vocabulary, so a model trained around modern coding agents
recognizes the surface immediately:

```text
read   { path, offset?, limit? }              offset is 1-based (default 1),
                                              limit defaults to 200 lines
write  { path, content }
edit   { path, edits: [{ oldText, newText }] }
glob   { pattern, path? }
grep   { pattern, path?, glob?, ignoreCase?, literal?, context?, limit? }
bash   { command, timeout? }                  timeout is in seconds
```

Adopting those conventions is a *schema* decision only. It does not import
Pi's runtime, subprocess model, permission system, ignore behavior, result
ordering, or remote-operations abstractions: execution semantics stay
explicitly rustX-owned, and where a rustX contract and an external
implementation disagree, the rustX contract wins.

Four consequences are load-bearing:

- **Write is intentionally unchanged.** `path` + `content` was already the
  right contract, so it does not churn for symmetry. In particular `path` is
  never renamed to `file_path` anywhere in the plane.
- **Edit is an atomic multi-edit against one original file snapshot.** One
  invocation reads one snapshot, resolves *every* `oldText` against that
  same snapshot (never against the result of an earlier edit in the same
  call), requires each to identify exactly one range, computes the whole
  replacement range set before mutating anything, rejects intersecting,
  nested, and coinciding ranges, orders the validated disjoint ranges by
  position, and commits one final snapshot through the plane's single
  atomic file commit. Input edit ordering therefore cannot change the
  result, and any validation failure leaves the file byte-for-byte
  unchanged. There is no sequential-application mode and no replace-all
  mode.
- **Glob and Grep share one search substrate.** `tools/native/search/` owns
  the single workspace file-universe policy both observe: search-root
  containment through the `Workspace` boundary, hidden files visible,
  ignore files (`.gitignore`, `.ignore`, git global excludes,
  `.git/info/exclude`) deliberately *not* applied, symlinks never followed
  (so neither a directory symlink recursion nor a file symlink target can
  enter the universe), normalized root-relative paths, and deterministic
  lexical enumeration. A caller filter — Glob's `pattern`, Grep's optional
  `glob` — only ever narrows that shared set. The `ignore`, `globset`,
  `grep-regex`, and `grep-searcher` crates are implementation dependencies
  of that substrate and of the Grep engine; no `rg` executable is ever
  spawned, none of those crates' defaults are part of the tool contract, and
  `grep-searcher` never owns workspace traversal.
- **Bash converts its unit at the tool boundary.** The model-facing
  `timeout` is measured in seconds and is converted to the internal
  `Duration` in the Bash input contract. Nothing below that boundary — the
  executor, the supervisor, process-group lifecycle, cancellation, timeout
  settlement, descendant termination, output capture — changes, and no unit
  conversion spreads into the process plane.

Deterministic ordering and bounded output remain rustX-owned semantics in
all cases: Glob returns lexically ordered paths (never mtime-ranked), Grep
returns matches ordered by path, then line, then column, both enforce a
result count cap and a hard payload byte cap with explicit truncation
state, and neither ever drops results silently.

The payload cap bounds the **actually serialized** model-facing JSON, not an
estimate of it. Each candidate entry is charged its own serialization plus
its array separator on top of a measured envelope, and an entry is admitted
only if the whole document still fits, so JSON escaping inside a path or a
matched line cannot push the delivered payload past
`MAX_MODEL_TOOL_RESULT_BYTES`. Grep's `matches` and `context` arrays share
that one budget. The count cap is evaluated before the byte budget, so which
entries survive truncation stays a function of the deterministic ordering
alone rather than of how expensive a particular entry is to encode.

`background_task` is a runtime intrinsic that happens to participate in the
common tool execution plane. It is not an ordinary native tool, its contract
and runtime semantics are outside this alignment, and it is never moved,
renamed, or re-schema'd to make `tools/native/` look uniform.

Native tool input schemas are generated from tool-owned Rust input types,
so the typed contract is the single source of truth for the model-facing
arguments:

```text
native:   Rust input type -> generated schema -> ToolDefinition
MCP:      MCP schema                          -> ToolDefinition
Python:   package schema                      -> ToolDefinition
```

All three converge at the same registry boundary, and the runtime keeps
validating every invocation against the stored canonical schema before
dispatch. An optional native property means an *absent* property: the
native schema-generation boundary collapses the nullable union that
`Option<T>` would otherwise produce for a field that only expresses
omission, so `{"timeout": null}` is a business argument violation
rejected at preflight rather than a second spelling of omission. This is a
rule about implicit nullability, not a restriction on the schema language:
a native contract that genuinely needs a composite or nullable model-facing
shape states it explicitly.

The executor ABI is unchanged by this: `ToolRegistry` validates
model-issued business arguments against the generated canonical schema
before dispatch, and a native executor receives the validated canonical
`ToolInvocation` — whose `arguments` remain canonical JSON — and
immediately decodes them into its tool-owned typed input before any
tool-specific filesystem, process, or other business work begins.
`ToolExecutor` never carries typed generics.

```text
model JSON
    -> ToolRegistry schema preflight
    -> validated ToolInvocation
    -> native executor boundary
    -> typed input decode
    -> tool-specific semantic validation
    -> actual tool work
```

Required fields, type correctness, and schema constraints belong to the
input contract, while workspace permission, filesystem existence, pattern
compilation, and process lifecycle rules remain execution concerns.
Outputs are deliberately untyped: the canonical `ToolExecutionResult` stays
the only tool result contract, so the agent loop never learns tool-specific
result types.

Bash treats one invocation as one complete lifecycle:
spawn one per-invocation supervisor, capture stdout/stderr/combined, let
the supervisor own the invocation's process group to its kernel-mediated
terminal state, and settle only when the shell's terminal status is
known, the owned group's terminal report arrived, AND the output capture
is settled — shell-parent exit is not by itself the Bash settlement
boundary, so a descendant that remains in the owned group after the shell
exits (holding the pipes or having redirected them away) can never escape
the timeout/cancellation contract. The child runs with an explicit
`env_clear()`-based environment, bounded head/tail previews per stream
with full raw output spooled to artifacts, `TERM -> BASH_TERM_GRACE ->
KILL` cancellation driven by the supervisor, typed result semantics (zero
exit success, non-zero exit failed with the code preserved, timeout as
`TimedOut`, cancellation as `Cancelled`), explicit artifact-capture
failures, and explicit process-control failures (supervisor setup, shell
spawning, waiting/reaping, signaling, and IPC failures settle as `Failed`,
never as a silent `Success`, `Cancelled`, or `TimedOut`) — never a silent
success that lost the retained output.

**The Bash invocation ownership boundary is its dedicated process group.**
On both supported platforms the inner supervisor creates a fresh session and
process group, and `TERM`/`KILL` are issued with `killpg` while the retained
inner pid proves that the numeric group id is still allocated to the
invocation. The outer supervisor reports the canonical `AllChildrenReaped`
event only after its group-scoped `waitid(Id::PGid)` gate reaches `ECHILD`.

Linux strengthens that base lifecycle with an inherited seccomp policy that
rejects descendant `setsid(2)`/`setpgid(2)` calls, plus child-subreaper
adoption for orphaned descendants. Those two primitives make the group wait
a complete whole-group terminal proof, including shell-backgrounding and
supervisor-loss fallback; the filter uses syscall numbers from the compiled
Linux ABI and rejects x32 execution on x86-64.

macOS has the same real process-group and `waitid` lifecycle, using the
platform libc adapter because `nix` does not expose `waitid` on Apple
targets. It has no seccomp or child-subreaper equivalent, so a descendant
that outlives the shell is reparented to launchd and becomes invisible to
the supervisor's group-scoped wait. macOS therefore does **not** treat a
group-scoped `ECHILD` as a whole-group terminal proof. Instead:

- Bash is wrapped with an EXIT `wait` as a **best-effort convenience** so
  ordinary background jobs finish naturally; it is not an ownership
  boundary and the user command may legally replace it;
- when the shell is reaped, the inner supervisor escalates to the outer's
  fallback containment (`SIGKILL` to the retained group), and the outer
  reports terminality only after issuing that containment signal and then
  proving the group absent with a `killpg(pgid, 0)` probe reaching `ESRCH`;
- a containment signal whose result is `EPERM` is never itself terminal:
  `EPERM` proves only that the signal operation was not authorized, so the
  group's absence is proven independently by the `killpg(pgid, 0)` probe
  rather than inferred from `EPERM`. (On macOS the kernel also reports a
  zombie-only group as `EPERM`, which is indistinguishable from an
  unauthorized live member, so neither is ever treated as a terminal fact.)

A command that deliberately creates a new session leaves the macOS
process group and thereby exits rustX's ownership domain: rustX does not
track, contain, reap, or wait for such a descendant, and settlement of the
owned group does not imply it terminated. A lost outer supervisor that
leaves no waitable anchor is reported as unproven rather than converted
into a false terminal proof. macOS terminal settlement therefore proves the
owned process group was actively terminated — not that every descendant
was reaped, which rustX cannot prove on macOS. `/proc` is never the source
of truth for ownership or quiescence on either platform.

**The inner supervisor pid is an ownership anchor with exactly one
reaping owner.** The outer supervisor's dedicated anchor path is the only
code allowed to observe the anchor's terminal state (`waitid` with
`WNOWAIT`: observation only, never consumption) and the only code allowed
to reap it (the group-scoped gate, strictly after any fallback
containment signal). The outer therefore has **no generic `waitpid(-1)`
reaping loop**: every child of the outer is either the anchor or an
in-group adopted descendant, so the gate reaps the whole child domain and
a generic loop could only ever consume the anchor and lose the
abnormal-exit fallback-containment decision. An `ECHILD` from the
dedicated anchor observation is an ownership invariant violation, never a
terminal observation: the outer reports it and fails safely — it never
derives owned-group terminality from an anchor `ECHILD`, never signals a
numeric group id without the retained anchor, and never reports the
canonical terminal event. The inner supervisor's own reaping hygiene
consumes only its own children (bash and adopted in-group descendants),
never an anchor of another owner.

The OS ownership commit is the successful `/bin/bash` spawn after the inner
has created the invocation session/group and installed the platform's
membership policy (seccomp on Linux; an explicit no-op on macOS). Protocol
state makes this explicit: the inner reports `AnchorReady`, rustX retains the
possible ownership identity and replies `Start`, then the inner reports
`OwnershipEstablished` after spawning Bash. If communication fails after
`Start`, rustX conservatively assumes ownership may exist. Pre-gate setup
failure reports `NoOwnership` and may settle without a Bash domain. The
`Start` gate is a recognition point, not a reader boundary: the inner's
rustX-facing control direction owns one `FrameReader` for the whole
invocation, shared by the gate and the owned control loop, so a `Terminate`
that the kernel delivered in the same `read()` as `Start` still drives the
ordinary `TERM` -> grace -> `KILL` path (the shared control-frame ownership
invariant, identical to the interactive unit's gates).
On Linux, catastrophic fallback authority is a pre-ownership prerequisite:
the runtime child-subreaper primitive is consulted (once per process,
idempotently) before the supervisor unit spawns, so `START` — which
authorizes the Bash spawn — is never sent before rustX can own catastrophic
containment. macOS has no equivalent orphan-adoption primitive; its normal
path uses direct-child and process-group ownership, and a lost outer without
a waitable anchor remains explicitly unproven.

Control-channel EOF is never a post-ownership process-terminal event. Normal
terminality linearizes at the outer's group-scoped `ECHILD` and its
`AllChildrenReaped` frame. On Linux, catastrophic loss of both supervisors
uses the runtime's **process-level kernel coordination primitive** — the
child-subreaper capability owned by `src/runtime/process_supervision.rs`,
with lazy one-time, idempotent, sticky activation — to retain the adopted
inner anchor, contain its group, and reach a second group-scoped `ECHILD`.
Kernel adoption does not assign arbitrary children to Bash lifecycle
ownership, and rustX implements no generic unknown-child reaper. On macOS,
the outer's descendants are not adopted by rustX; if the anchor is not
waitable, emergency containment reports `AnchorUnavailable` and remains
unproven rather than committing a result. Thus EOF changes communication
state and failure intent, while process lifecycle remains independently
`PreOwnership`, `OwnershipPossible`/`Owned`, or `Terminal`.

Every Bash result status — `Success`, `Failed`, `Cancelled`, and
`TimedOut` — is terminal with respect to the invocation-owned process
group: no invocation-owned Bash process remains capable of executing work
before any result is returned. A detected process-control/runtime failure
determines the eventual result status but does not itself settle the
invocation lifecycle: failures before any Bash tree was established may
return `Failed` immediately, while failures after ownership exists (signal
failure, wait/reap failure, IPC failure, control-channel abandonment)
follow the containment lifecycle — the failure is remembered, the outer
supervisor becomes the active containment authority (it observes the
inner's terminal state via `waitid(WNOWAIT)` without releasing the
structural anchor, sends one fallback `SIGKILL` to the still-proven-owned
group, and releases the anchor only through the group-scoped wait), the
capture is finalized, and only then is `Failed` returned.
`BASH_TERMINATION_CONFIRMATION` is a process-confirmation watchdog: expiry
records `QuiescenceTimeout` failure intent but does not authorize result
commit. After process terminality, a separate capture deadline may force-
finalize wedged readers and return `Failed(CaptureTimeout)`. The
outer supervisor also un-wedges a `SIGSTOP`-frozen inner anchor with
`SIGKILL`, so a stopped containment chain cannot strand the owned group;
the only residual state in which rustX cannot prove owned-group
terminality from outside the unit (a unit frozen beyond the outer
supervisor's reach) cannot be truthfully converted into a terminal result.
Control-channel abandonment remains fail-safe through the normal supervisor
unit. If the unit itself is lost, the rustX-held subreaper authority above is
the independent fallback. The anchor is released only by the final reap
after the last signal, so a numeric group id whose allocation has ended is
never signaled.

### Layer 3: Model plane

The model plane implements protocol adapters:

- OpenAI Chat Completions
- OpenAI Responses
- Anthropic Messages

The M2 implementation freezes the model-plane boundary:

```text
Provider HTTP / SDK
        |
adapter-private provider representation
        |
ModelAdapter
        |
ModelEvent
        |
M3 Agent Loop
```

#### Model selection ownership (Issue #42)

Selection is upstream of the adapters and entirely catalog-driven:

```text
models.json
    -> ModelCatalog                  validated: explicit baseUrl, explicit
                                     apiKey source, protocol, limits,
                                     capabilities, reasoning profiles, compat
    -> ResolvedModelCatalog          credentials bound at startup
    -> ModelBindingRegistry          one adapter per provider x protocol
    -> ResolvedModelInvocation       immutable: binding, model, protocol,
                                     window, output budget, selected profile,
                                     effective requestParams, effective
                                     capabilities
    -> AttemptModelSnapshot          frozen at attempt admission
```

Governing rules:

- **No implicit endpoint.** `OpenAiAdapterConfig::new` and
  `AnthropicAdapterConfig::new` both require an explicit base URL. No
  provider *name* selects an official endpoint, and there is no path from
  `"openai"` or `"anthropic"` to a network address.
- **Credentials are redacted by type.** A resolved credential lives in
  `ResolvedCredential`, which has no `Serialize`, redacted `Debug`/`Display`,
  and exactly one read boundary (`expose`) used only to construct a provider
  client. Client-facing views carry at most the credential *source kind* and
  the environment variable *name*.
- **`requestParams` is opaque.** rustX normalizes no provider sampling or
  routing parameter. Effective parameters resolve as model defaults →
  selected reasoning profile → session overrides, each a **top-level shallow
  overlay**: nested objects and arrays are replaced atomically, never
  deep-merged. The selected profile *owns* every top-level key it declares, so
  a session override that also declares one fails deterministically rather
  than being resolved by merge order.
- **Protected wire keys.** Each protocol declares the runtime-owned fields
  opaque parameters may never replace — Chat Completions: `model`,
  `messages`, `tools`, `stream`, `stream_options`, and *both* max-token
  spellings; Responses: `model`, `input`, `instructions`, `tools`, `stream`,
  `max_output_tokens`, `store`, `previous_response_id`, `include`; Anthropic:
  `model`, `messages`, `system`, `tools`, `stream`, `max_tokens`. A collision
  fails at configuration time *and* again at final request construction.
  Provider-owned reasoning/sampling fields (`thinking`, `reasoning`,
  `output_config`, `temperature`, …) are deliberately **not** protected: a
  reasoning profile is expected to own them.
- **Final wire placement.** Each adapter translates canonically, serializes
  to a JSON object, validates protected-key ownership, and shallow-overlays
  the effective parameters at the **top level** of the real provider body.
  There is no invented `extra_body` nesting level and no second HTTP path.
- **Model identity.** A canonical model reference is `provider/model-id`.
  Only the first slash separates the provider; the model ID keeps the entire
  remainder, so `gateway/Qwen/Qwen3` reaches the provider as `Qwen/Qwen3`.
  Provider IDs and reasoning-profile IDs cannot contain `/`, and model IDs
  may contain slash-separated non-empty segments (`a//b` is invalid).
- **Bounded compat.** `compat` configures only structural translation the
  adapters actually branch on — the legal Chat max-token spelling, whether
  streaming usage options are supported, whether previous assistant reasoning
  is replayed as `reasoning`, `reasoning_content`, or omitted, and the
  Responses storage / continuation mode. For `openai_chat_completions`,
  `chatReasoningReplay` is required and has no universal default; it is a
  provider/model wire contract, not a reasoning-generation switch. It is not
  a strategy framework, and nothing is ever inferred from a hostname.
- **Effective capabilities.** The client-visible capability is
  `model-declared ∩ adapter/protocol ∩ current runtime`. Because no adapter
  can transmit canonical image or file references yet, a catalog claiming
  image input never causes image input to be advertised, and unsupported
  content is rejected at the invocation boundary before a provider request is
  opened. A model without effective tool-call capability stays usable as a
  text model and simply never receives tool definitions.

Provider SDK and wire types terminate inside the adapter modules
(`src/model/adapter/openai`, `src/model/adapter/anthropic`); the agent kernel
operates only on the runtime-owned `ModelAdapter` interface and the
`ModelEvent` stream.

#### OpenAI adapters (async-openai)

Both OpenAI adapters use the `async-openai` crate for typed request types,
the SDK client plumbing, and SSE stream consumption. Two properties are
enforced by construction:

- Automatic retry is bypassed. The SDK's default executor wraps the plain
  transport in `OpenAIRetryLayer`; the adapters install a rustX-owned custom
  HTTP service (`NoRetryService`) that executes exactly one `reqwest` request
  per call and performs no retry. One adapter invocation is exactly one
  provider request attempt.
- The Chat Completions response stream and the entire Responses protocol use
  the SDK's BYOT (bring-your-own-type) facility as raw JSON, so unknown
  finish reasons, unknown event fields, and future item shapes are tolerated,
  and preserved Responses continuation items round-trip losslessly.

The no-retry service also captures the provider HTTP status, `Retry-After`
header, and error payload at the transport boundary, because the SDK's typed
error drops response headers.

#### Anthropic Messages (direct HTTP/SSE)

Anthropic has no official Rust SDK, and the evaluated community SDK
(`anthropic-sdk-rust` 0.1.x) has stale typed stop-reason coverage relative to
the current Messages API. The Anthropic adapter therefore talks to
`/v1/messages` directly with `reqwest` and `eventsource-stream`:

- correct current streaming semantics (incremental `text_delta`,
  `thinking_delta`, `signature_delta`, and `input_json_delta` deltas emit
  canonical events as they arrive; cumulative `message_delta` usage;
  `pause_turn`; `model_context_window_exceeded`);
- explicit rejection of server-side fallback: a provider `fallback` block is
  `Unsupported` (never silently discarded), because its position carries
  replay semantics rustX cannot preserve losslessly with the current
  canonical continuation model;
- current request semantics (`redacted_thinking.data` preserved losslessly
  as opaque provider state; `thinking` and `output_config` are provider-owned
  fields the *selected reasoning profile* declares — the adapter synthesizes
  neither);
- current refusal semantics (`stop_reason = refusal` with top-level
  `stop_details`; a human-readable `explanation` streams as `RefusalDelta`
  before `Completed(Refusal)`, never as plain text);
- current stop-reason coverage (`end_turn`, `stop_sequence`, `tool_use`,
  `max_tokens`, `model_context_window_exceeded`, `refusal`, `pause_turn`);
- forward-compatible event parsing (unknown top-level events never crash the
  parser; content-block events with a missing or invalid `index` are hard
  provider protocol errors, never reinterpreted as `0`);
- transparent retry ownership (the transport performs exactly one HTTP
  request per invocation; no retry, no reconnect, no failover);
- no SDK type leakage (there is no Anthropic SDK dependency at all).

The Anthropic wire representation is private to
`src/model/adapter/anthropic/wire.rs`; no alternative canonical Anthropic
model exists.

Canonical deltas are provisional adapter output: M2 reports what the provider
actually streamed, including partial output that a later refusal may
invalidate. Whether provisional content becomes a completed canonical
`AssistantMessageBlock` is owned by the future Agent Loop, never by M2, so no
adapter-local terminal buffering exists for Anthropic text or thinking.

#### Normalization rules

- `ContentBlockIndex` is assigned by rustX, never by the provider. A
  provider-index-to-canonical-index allocator maps provider block identity to
  canonical positions in first-appearance order, so provider tool indexes and
  different content-part layers never shift canonical indexes. Anthropic
  server-side fallback blocks are rejected as `Unsupported` before any
  canonical allocation: their provider positional/replay semantics cannot be
  preserved losslessly with the current canonical continuation model, so
  they are never silently dropped.
- Tool names resolve deterministically to canonical `ToolId` values before a
  request is sent; duplicate model-facing names are rejected before any
  provider request. Provider call ids remain `ToolCallId`; they are never
  synthesized from `ToolId` or from array position.
- Tool argument fragments stream raw (`ToolCallArgumentsDelta`) and the
  complete JSON is parsed exactly once at completion. Malformed completed
  JSON terminates the invocation with a normalized failure.
- When a provider emits both incremental deltas and a cumulative snapshot for
  the same semantic text, reasoning, or refusal value, the adapter accumulates
  the exact streamed value and requires the snapshot to match it. Matching
  snapshots are deduplicated; snapshot-only values are recovered; a
  contradiction fails the invocation rather than being repaired heuristically.
- Continuation state is emitted through the canonical
  `ProviderContinuationState` boundary, never kept in hidden adapter memory.
  OpenAI Responses supports both `Stored` (provider storage,
  `previous_response_id`) and `Stateless` (`store: false`, preserved output
  items including opaque encrypted reasoning). Anthropic thinking signatures
  and `redacted_thinking.data` are preserved as rustX-owned opaque JSON on
  the reasoning block and replayed verbatim; canonical reasoning text alone
  is never sufficient to reconstruct a provider reasoning item (OpenAI
  Responses fails with `Unsupported` instead of fabricating one).
- Usage is normalized without inventing counts: Anthropic effective input is
  `input_tokens + cache_creation_input_tokens + cache_read_input_tokens`
  from the latest cumulative snapshot (never summed over time), reported
  `output_tokens_details.thinking_tokens` maps to
  `UsageDetails::reasoning_tokens`, and `cache_read_input_tokens` maps to
  `UsageDetails::cached_input_tokens` without double counting.
- Cancellation is the runtime-owned `CancellationSignal` flowing through
  the common interface; the network-opening await itself is
  cancellation-aware (a cancellation while waiting for response headers
  aborts the in-flight request), an in-flight invocation stops consuming the
  provider stream, no retry ever occurs, and the invocation terminates with
  `Failed(Cancelled)`.
- Live integration tests are opt-in (`#[ignore]`): ordinary CI runs
  `cargo test --all-targets --all-features` without credentials or network.
  A developer CLI smoke tool (`examples/model_smoke.rs`) streams one response
  per protocol against production credentials.

### Layer 4: Tool plane

The tool plane exposes a single runtime-owned execution contract and multiple executor implementations:

- Native tools
- MCP tools
- Custom Python tools
- Platform communication tools such as durable message sending

Execution implementations may depend on `rmcp`, process APIs, `uv`, or other libraries. The agent kernel may not.

#### M7 implementation (one external-capability tool plane)

Every model-visible tool is one canonical `ToolDefinition` paired with one
`Arc<dyn ToolExecutor>`. Native, MCP, and custom Python tools use the same
registry preflight, reserved `__rustx_*` stripping, JSON Schema validation,
execution policy, concurrency policy, progress event, cancellation signal,
foreground/background ownership, and result types. The Agent Loop has no
origin-specific dispatch path.

The base/native/runtime registry is immutable input to capability preparation.
Each candidate composes it with MCP definitions in `McpServerId` order (the
configured server set is a `BTreeMap` keyed by that identity, so the order is
structural rather than a sorting pass) and then remote name, then Python definitions sorted by canonical model-facing name.
The candidate owns a new `ToolRegistry`; a committed `CapabilitySnapshot`
owns that exact registry. A duplicate model-facing name rejects the complete
candidate.

`McpServerRuntime` owns one configured server's rmcp peer, transport, progress
dispatcher, list-change invalidation mechanism, and supervised stdio owner
when used. It is constructed from an `McpServerId` and an identity-free
`McpServerBinding`: the coordinator's `BTreeMap<McpServerId,
McpServerBinding>` key is the one authoritative server identity, so duplicate
identity is structurally impossible and no ordering pass exists.

Connection setup negotiates a protocol revision rather than requiring one.
rustX offers the resolved rmcp build's complete `ProtocolVersion::
KNOWN_VERSIONS`, newest first, through `ClientLifecycleMode::Auto`: rmcp
probes the MCP 2026-07-28 inline `server/discover` lifecycle, walks the
offered list down whenever the peer answers `UNSUPPORTED_PROTOCOL_VERSION`,
and falls back to the legacy `initialize` handshake only when the peer proves
it does not know `server/discover`. rustX then validates the negotiated
revision against its own offered set — the legacy handshake lets a server
echo any revision — and a peer with no shared revision fails with a bounded
`McpError::ProtocolCompatibility` naming both sides. The negotiated revision
also selects the invalidation mechanism: `subscriptions/listen` from
2026-07-28 onwards, the plain `notifications/tools/list_changed` client
callback before it. At most one invalidation mechanism is installed per
connection; when the server advertises `tools.listChanged`, exactly one
revision-appropriate mechanism is installed.
Executors capture an `Arc` to that runtime. The observed remote tool surface
and binding are immutable for a capability revision, but rustX does not claim
to snapshot the implementation behavior of the independent remote server.
`tools/list_changed` epoch mutation and capability snapshot activation share
exactly one synchronization boundary — the mutex-protected MCP invalidation
state: notification epoch advancement, preparation epoch snapshots, and the
commit's final epoch validation plus snapshot swap all serialize through the
same guard. If the notification wins first, the prepared candidate cannot
commit and the active snapshot is unchanged; if the commit wins first, the
notification belongs to a future refresh and can never retroactively
invalidate the already-committed snapshot. Lock ordering is explicit and
documented (capability state lock -> MCP invalidation guard; the notification
path holds only the guard), so no cycle exists. Preparation rejects a catalog
that changes during discovery, and commit rejects a candidate whose epoch
changed before the protected snapshot swap.

MCP stdio uses the runtime-owned interactive supervisor unit, whose control
sockets are separate from the server's stdin/stdout protocol pair. The unit
is the M5 Bash supervisor shape applied to a long-lived server, composed
from the same shared structural ownership core
(`src/runtime/supervised_unit`): an inner supervisor calls `setsid()`,
applies the fixed-membership restriction on Linux (macOS has no seccomp
equivalent), and issues `TERM -> grace -> KILL` with `killpg` against its
own process group; an outer supervisor is the reaper of last resort with the
single-owner anchor discipline and the authoritative terminal report. The
kernel-mediated terminal proof is the group-scoped wait
(`waitid(Id::PGid)` returning `ECHILD`) — never a `/proc` scan and never a
`killpg(0)` probe **on Linux, where child-subreaper adoption plus the
fixed-membership restriction make that a complete whole-group proof**. On
macOS that `ECHILD` only proves the waiting supervisor has no waitable group
child left, so macOS instead escalates to the outer's fallback containment
`SIGKILL` and proves the group absent with a bounded `killpg(pgid, 0)` probe
reaching `ESRCH` — never a fabricated whole-group emptiness claim. rustX's
detached driver task owns physical settlement
from the moment the supervisor spawn succeeds, drains the server's stderr
until EOF (bounded preview), reaps the direct supervisor child before
publishing settlement, and runs Linux's adopted-anchor emergency containment
when the unit is lost; macOS reports the lost-anchor case as unproven.
Startup is
ownership-gated in both directions: the outer supervisor may create the unit
hierarchy only after rustX accepted and retained its control connection
(`MSG_OWNER_ATTACHED`), and the outer attaches its inner supervisor with a
bounded pre-ownership state machine (inner connection, inner exit, upstream
loss) instead of a blocking accept. `MSG_ANCHOR_READY` is then the anchor
commit point: only a unique, positive, pid-matching announcement gives the
inner pid its second meaning as the owned process-group id. Before it the
outer owns the inner strictly as a direct child pid — no group-scoped wait
may run against that pid — and reports `NoOwnership` only after proving
that direct child reaped; an unprovable pre-anchor reap reports a
process-control failure and no `NoOwnership`. After the startup gate has
opened, bare pre-ownership plus control loss is therefore never a terminal
proof. Both gates are pure recognition points, not reader boundaries: each
stream direction has exactly one buffered control-frame reader for the whole
connection lifetime (rustX -> outer across the startup gate, the pre-inner
drain, inner attachment, the pre-anchor phase and the anchored relay loop;
outer -> inner across the START gate and the owned inner control loop), and
every phase drains what is already buffered before it waits for another
read. `NoOwnership` itself is parsed by one strict, fail-closed grammar
(empty payload, or a four-byte positive reaped pid; anything else is a
protocol error). Physical settlement is published only
with proven terminality; an unproven terminal state is returned as an
explicit error from `wait_for_settlement`/`McpServerRuntime::close`, never as
a successful settlement. Streamable HTTP
uses the current rmcp client transport with explicit static headers and no
`Mcp-Session-Id` compatibility state.

Custom Python packages are discovered only from
`<workspace>/.agents/tools/<tool-name>/`. Candidate preparation reads a finite
package snapshot, computes `ToolVersionId`, publishes it immutably as
`tool-versions/<ToolVersionId>/source/` plus a version marker (every uv
preparation command uses exactly the `source/` root; reuse validates the
published source content digest against the claimed identity), validates
the existing `uv.lock`, and materializes a distinct immutable
`PythonToolEnvironmentDigest` environment whose ready marker locks every
deterministic identity input (format, OS, architecture, digest, lock digest,
Python runtime identity, uv identity). ToolVersion identity and environment
identity are separate: source/description/schema changes can change the
former without changing the latter, and each ToolVersion -> environment
binding is recorded deterministically outside the environment's immutable
dependency identity. The environment isolates dependencies, not filesystem,
network, or security permissions. Execution never uses the published
source as a working directory: each invocation materializes a private copy
of it under `python-invocations/execution-N/`, runs the harness with that
copy as module root and working directory, and deletes the copy when the
invocation settles, so ordinary tool writes (relative paths, `__file__`)
can never mutate the canonical ToolVersion bytes. The interpreter whose
identity enters the digest is pinned to uv via `UV_PYTHON`, managed Python
downloads stay disabled, and every preparation command has a finite
deadline (a timeout is an explicit preparation failure). A harness uses a private input file and
one bounded JSON result envelope; the Python subprocess uses the shared
supervised short-lived runner. Same-digest in-flight builds coalesce behind
one store-owned owner task (callers only wait; owner failure publishes a
terminal error and removes the in-flight entry).

### Layer 5: Skill plane

Skills are filesystem/workflow packages. A skill may include:

- `SKILL.md`
- scripts
- references
- assets
- Python dependency declarations
- Node dependency declarations

All active skills in one conversation share one Python environment and one Node environment. Skills use the same native Bash execution capability as the agent.

#### M6 implementation (skills and shared environments)

The M6 implementation (`src/skills`) freezes the Skill plane boundary:

- **Skill root.** There is exactly one Skill root,
  `<workspace>/.agents/skills/`, anchored to the canonical Workspace root
  and never to Bash's mutable working directory. Discovery is one level
  only; a missing root is an empty Skill set; hidden root entries and
  unrelated files are ignored; results are deterministically ordered by
  validated Skill name; any malformed candidate fails the whole discovery
  transaction; symlinked package roots and package-internal symlinks are
  rejected (Skill-package validation only — the general Workspace symlink
  contract for ordinary tools is unchanged).
- **Format.** `SKILL.md` is standard Agent Skills YAML frontmatter plus
  Markdown: `name` (validated against the standard naming rules and the
  parent directory), `description` (non-empty, standard length bound),
  `metadata` (string-to-string map), and the preserved optional
  `license`, `compatibility`, and `allowed-tools` fields — no runtime
  policy is invented for them.
- **Version identity.** `SkillVersionId` is a deterministic content-derived
  `sha256:<hex>` digest over the complete accepted package state (sorted
  workspace-relative paths, lengths, and raw bytes), independent of host
  paths, mtimes, permissions, enumeration order, and wall-clock time.
- **Dependency declarations.** The standard `metadata` extension point
  carries exactly the rustX keys `rustx.python-dependencies` and
  `rustx.node-dependencies`, each a JSON object of package name to exact
  version string. Python distribution names normalize deterministically
  (lowercase, `-`/`_`/`.` equivalence); Node names may be scoped
  (`@scope/pkg`). Ranges, tags, extras, markers, URLs, VCS, local paths,
  editable installs, and workspace references are rejected; rustX never
  builds a semver solver. Merging across active Skills coalesces identical
  declarations and reports deterministic conflicts (ecosystem, normalized
  package, every responsible Skill, every declared version) before any
  package-manager subprocess runs.
- **Shared environments.** All declared Python dependencies materialize
  into one shared Python environment and all declared Node dependencies
  into one shared Node environment per capability set — never one per
  Skill. An ecosystem with no dependencies needs no runtime and no
  environment. `PythonEnvironmentDigest` and `NodeEnvironmentDigest` are
  distinct SHA-256 identities over (format/version domain, OS,
  architecture, resolved runtime identity, resolved package-manager
  identity, sorted normalized dependency map), never including
  workspace/store/staging paths, time, or random values. Environments live
  in a caller-configured runtime-private store disjoint from the Workspace
  using canonical, symlink-safe prospective-path validation before creation.
  Python is built directly at its final digest directory and becomes
  reusable only when its exact deterministic manifest is atomically committed
  as the ready marker; Node uses private staging followed by atomic rename.
  Both are reused only when the committed manifest matches the expected
  digest inputs, and neither is installed into again. Same-process
  preparations of one ecosystem/digest coalesce behind one
  `EnvironmentStore`-owned build task: candidate callers only wait on its
  result, so caller cancellation cannot cancel the physical materialization
  or release the in-flight entry. The owner publishes the terminal result and
  releases that entry only after materialization, validation, and publication
  return.
- **Catalog.** The model-visible catalog is rendered compactly from the
  attempt's immutable Skill snapshot: the common `.agents/skills/` root
  once, then `- <name>: <description>` per validated Skill in
  deterministic order. `SKILL.md` bodies, supporting resources, dependency
  metadata, and host absolute paths never appear.
- **Execution.** Skills remain workflow/instruction packages: no
  `skill_search`/`activate_skill`/`skill_view`/`run_skill`/
  `run_skill_script` abstractions exist. The model reads
  `.agents/skills/<name>/SKILL.md` and supporting files through native
  Read and runs scripts through native Bash against the Workspace.

**Workspace-file limitation.** M6 freezes discovered identities, version
identities, catalog metadata, dependency declarations, environment
identities, and the effective ToolEnvironment. Skill source files
(`SKILL.md`, scripts, references, assets) remain ordinary Workspace files
accessed through normal Read/Bash semantics: M6 does not snapshot-mount
them, and an external rewrite of `.agents/skills/...` after preparation is
observed only at the next quiescent re-discovery.

### Layer 6: Runtime services

This layer owns execution infrastructure:

- Cancellation hierarchy
- Runtime event writer
- Message store interface
- ConversationStore integration for Ledger, Surface, Request Snapshot, and
  Event Journal durability (the in-memory model is only a bounded hot read
  model)
- Capability revision management
- Capability mutation guard
- Process supervision
- Background shell session management
- Durable recovery evidence, the (M9a) startup recovery pipeline that
  classifies and reconciles it, the M9b model-start arbitration, and the M9c
  runtime supervision/quiescence drain contract

#### M6 implementation (capability coordination)

M6 implements the concrete capability snapshot/mutation semantics required
for Skills in a narrow coordination layer (`src/capabilities`), not a
generic runtime supervisor:

- **Immutable attempt capability snapshot.** One `CapabilitySnapshot`
  holds the monotonic `CapabilityRevision`, the immutable `ToolRegistry`
  handle, the immutable Skill snapshot/catalog with its
  `SkillId` + `SkillVersionId` bindings, the Python/Node environment
  identity and path when present, and the effective `ToolEnvironment`
  (base authorized environment plus the deterministic Skill environment
  overlay).
- **Capability owner identity.** A `CapabilityCoordinator` is explicitly
  conversation-owned and records the canonical Workspace root with its
  `ConversationId`. An attempt lease can only be passed to a
  `ConversationToolRuntime` with the same conversation/workspace ownership
  domain; construction rejects a mismatch before model or tool execution.
- **Attempt capability lease.** An `AgentExecution` structurally holds one
  RAII lease pinning one immutable snapshot for its complete lifetime; no
  model turn re-discovers Skills or re-queries the conversation capability
  pointer. There is no capability-free constructor.
- **Quiescent commit.** Candidate preparation (discovery, dependency
  merge, environment materialization) runs independently; activation is a
  quiescent atomic commit legal only when zero attempt leases are active
  for the conversation. Acquisition and commit serialize through one
  synchronization boundary; an identical candidate is a no-op that never
  fabricates a revision; a stale candidate (prepared from an obsolete base
  revision) is rejected; failed preparation/commit leaves the active
  revision authoritative. Conversation-owned detached background
  executions do not hold attempt leases and never block a capability
  commit.
- **Background environment capture.** The effective environment is
  captured at background dispatch prepare time (before the ownership
  commit) and retained by the detached execution; later revision
  activations never mutate it.

#### M7 capability additions

Capability preparation now owns the full composition transaction:

```text
base/native/runtime tools + prepared MCP + prepared Python
    -> candidate ToolRegistry -> candidate CapabilitySnapshot -> commit
```

There is no mutable process-global active registry. An attempt lease captures
one snapshot and its exact registry for all turns. Detached background work
captures the exact executor before ownership transfer; an old MCP call keeps
its `McpServerRuntime`, and an old Python call keeps its published source and
environment, across later capability revisions. Environment GC metadata is
written deterministically for future ownership, but M7 implements no GC.

The shared supervised process runner (`src/runtime/process_runner`) is the
M5 Bash process-group lifecycle extracted so native Bash and Skill
environment materialization share one owned supervisor/process-group
domain: same child-subreaper contract, same cancellation/timeout physical
settlement, same catastrophic containment, explicit cwd and child
environment, finite timeout, bounded diagnostics, and no generic
`waitpid(-1)` reaper.

### Layer 7: Interfaces and projections

The outermost layer exposes the runtime to humans and other systems:

- Runtime Client Protocol v1 (semantic client boundary)
- Local interactive CLI
- Runtime command interface
- HTTP control interface
- Runtime event streaming
- AG-UI projection

AG-UI is an output projection, not the internal durable event model.

#### Runtime Client Protocol v1 implementation (Issue #37)

Issue #37 implements the one external semantic normalization boundary in
`src/runtime_client`:

```text
canonical runtime state / internal RuntimeEvent
                |
                v
 deterministic Runtime Client projection
                |
                v
 RuntimeClientEvent / RuntimeClientSnapshot
                |
                v
      Runtime Client Protocol v1
```

The governing invariant is that all authoritative execution and
conversation state originates from rustX Runtime; external clients observe
deterministic projections and never become a second authority. The
internal `RuntimeEvent` vocabulary is an execution-fact vocabulary, **not**
the wire contract: `RuntimeClientEvent` and `RuntimeClientSnapshot` are
explicit runtime-owned projection types with their own versioning
(`RUNTIME_CLIENT_PROTOCOL_VERSION_V1`, independent from
`EVENT_SCHEMA_VERSION`, the manifest schema version, and the crate
version), lifecycle semantics, and cursor domain
(`RuntimeClientCursor`). Later transports (Issue #38 stdio JSONL,
Issue #36 WebSocket) wrap this semantic layer without redefining it, and a

future AG-UI adapter consumes this projection as its only source — there
is no second AG-UI interpretation path directly from internal runtime
events. The existing `src/protocol` boundary remains the compiled
`RuntimeManifest` protocol; the two protocols are not mixed.

Module ownership:

```text
runtime/                      the semantic conversation runtime
conversation_runtime.rs       ConversationRuntime: the conversation
                              coordinator (Issue #61) — session model
                              authority, attempt-id allocation, the
                              current-attempt slot, attempt admission,
                              between-attempt ConversationState,
                              RequestHistory, the mailbox/admission
                              relationship, the lifecycle/drain authority,
                              settlement handoff, the
                              inactive/running/draining/quiescent lifecycle
                              boundary, and the adapter
                              bootstrap handshake; publishes semantic
                              observations
runtime/observation.rs        the runtime-owned semantic observation
                              contract (Issue #61): ConversationObservation
                              (semantic source types only) and the leaf
                              PendingObservations queue. The runtime keeps
                              no second fold of this vocabulary: the
                              Runtime Client projection is the one fold
runtime/request_history.rs    append-only in-memory owner of frozen
                              settled RequestSnapshots and reconstruction
                              lookup (owned by ConversationRuntime);
                              never a message transcript
runtime/inbound.rs            ConversationInboundMailbox: inbound ordering
                              and finite batching authority, with the
                              shared admission wake handle
runtime_client/types.rs        protocol version, cursor, attachment/request
                               ids, the typed request/response/event
                               envelope, method results, typed errors
runtime_client/event.rs        RuntimeClientEvent (external vocabulary)
runtime_client/snapshot.rs     RuntimeClientSnapshot read model
runtime_client/projection.rs   RuntimeClientProjection: the client read
                               model linearization owner (fold, cursor
                               allocation, bounded replay, subscribers)
                               and the translation of semantic
                               observations into the client vocabulary
runtime_client/host.rs         RuntimeClientHost: the projection + control
                               + attachment adapter over ConversationRuntime
                               — attachment admission, snapshot/cursor
                               reads, event subscriptions, protocol
                               adaptation; it owns no canonical
                               conversation/session/admission state
runtime_client/attachment.rs   RuntimeAttachment: at-most-one attachment,
                               RAII/explicit detach, request dispatch,
                               event subscription delivery
runtime_client/endpoint.rs     RuntimeClientEndpoint: the transport-neutral
                               semantic entry point that dispatches every
                               v1 request, `initialize` included
runtime_client/transport/      byte-stream adapters beneath the semantic
                               layer (Issue #38); `stdio.rs` is the strict
                               stdio/JSONL transport
```

Issue #61 extracted the conversation runtime coordinator from this
boundary. The layering is:

```text
ConversationRuntime semantic facts
        |
        v
ConversationObservation (runtime-owned vocabulary)
        |
        v
shared leaf observation queue (PendingObservations)
        |
        v
RuntimeClientProjection (translation, fold, cursor, replay, subscribers)
        |
        v
RuntimeClientHost (attachment / control adapter)
        |
        v
RuntimeClientEndpoint -> transports -> TUI
```

The runtime never emits Runtime Client projection types: the observation
vocabulary carries runtime-owned source types, and the projection owns the
translation into `RuntimeClientEvent`/`RuntimeClientSnapshot`.

A conversation runs the exact same admission/execution path with zero
Runtime Client attachments: the coordinator is the semantic owner, and the
Runtime Client is a projection/control/attachment adapter over it.

- **The semantic endpoint owns `initialize`.** `RuntimeClientEndpoint` is
  the boundary a transport wraps. It starts unattached and accepts every
  v1 request; `initialize` performs version negotiation, single-attachment
  admission, `AttachmentId` allocation, and the linearized initial
  snapshot, storing the resulting attachment. Non-`initialize` requests
  before that are `not_attached`; a successful `detach` (or dropping the
  endpoint) returns it to the unattached state. `RuntimeClientHost::attach`
  remains an internal primitive that the endpoint invokes — it is not the
  protocol entry point. Issue #38 therefore reduces to framing:

  ```text
  JSONL line -> RuntimeClientRequest -> endpoint.handle_request
             -> RuntimeClientResponse -> JSONL line
  endpoint.next_event -> RuntimeClientProtocolEvent -> JSONL line
  ```

  No transport implements negotiation, admission, identity creation, or
  replacement/rejection semantics, and none needs an out-of-band attach
  operation.

- **Two synchronization boundaries, one per authority.** The conversation
  coordinator guards its admission state (session model, between-attempt
  canonical state, current-attempt slot, lifecycle/drain authority, inbound/attempt
  identity counters) with one lock; the Runtime Client host guards its
  projection state (snapshot read model, cursor allocation, bounded
  replay, subscribers, attachment admission/detach) with a second lock.
  The coordinator publishes every semantic transition as a
  runtime-owned `ConversationObservation` into the shared leaf queue,
  and every host lock acquisition drains that queue first, so the
  projection folds observations in the coordinator's commit order.
  Snapshot/cursor, cancel-current, terminal settlement, and admission
  therefore still linearize by synchronization, never by timing — the
  coordinator's admission linearization is one documented point, and the
  projection's snapshot/cursor linearization is another.
- **Native interactions use the same semantic boundary.** A pending approval
  is folded from `ConversationObservation::InteractionPending` into
  `RuntimeClientSnapshot.pending_interactions`; its terminal outcome folds
  through `InteractionSettled` and removes the live entry. Clients answer
  with the typed `interaction_respond` request, which contains only an
  `Allow` or `Deny` decision and never replacement tool arguments. A stale,
  duplicate, pre-crash, or post-quiescent response is the typed
  `interaction_not_pending` error and has no semantic effect.
- **Attachment availability is bounded and non-semantic.** The one active
  attachment represents the 0.1 interaction provider. If none is present at
  publication, approval fails closed as `Unavailable`; detaching later only
  closes admission for future requests. It does not answer or cancel a live
  interaction, and a reattached client repairs from the runtime snapshot and
  cursor rather than from local prompt state.
- **The Runtime Client host binds before activation.** A conversation
  runtime has four lifecycle states and one explicit admission/drain
  authority:
  them:

  ```text
  ConversationRuntime::new(..)         -> runtime-owned / inactive
      [optional] RuntimeClientHost::new(..)     bind the client adapter
  ConversationRuntime::activate()      -> Running: execution may begin
  ConversationRuntime::shutdown()      -> Draining -> Quiescent
  ```

  An **inactive** runtime is inert, and this is enforced, not merely
  documented: its mailbox refuses `enqueue` with
  `MailboxError::ConversationInactive`, `submit_inbound` fails with
  `InboundAdmissionError::Inactive`, `model_set` fails with the typed
  `ModelUpdateError::Inactive`, `shutdown` fails with the typed
  `ShutdownError::Inactive`, the background registry refuses
  `commit_dispatch` with `BackgroundDispatchError::ConversationInactive`,
  and the capability coordinator refuses a runtime-owned `commit` with
  `CapabilityCommitError::ConversationInactive`. No admission worker
  exists, `admit_next_attempt` is a no-op, and an inactive runtime
  therefore publishes no observation at all.

  Capability candidate preparation is a composition/readiness operation and
  may run while inactive. It is counted through activation and drain, while
  the revision swap remains refused until a live commit observes `Running`.

  There is exactly **one authoritative lifecycle state**: the shared
  `ConversationLifecycle` token composed by the runtime and read by every
  runtime-owned semantic boundary. The mailbox keeps no lifecycle flag
  (runtime ownership is the lifecycle handle itself), the capability
  coordinator keeps no lifecycle flag (the handle is attached at its
  claim), the coordinator keeps no copy, and the background registry reads
  the same gate through its mailbox. `activate` performs the single
  `Inactive -> Running` transition and `shutdown` performs the single
  `Running -> Draining` transition of that one token. Activation's worker
  spawn and initial admission kick are the one-time post-transition work of
  its winning caller; drain's settlement and `Quiescent` publication are the
  one shared runtime-owned completion. Because there is no subsystem-specific
  intermediate lifecycle state, background and capability commits can never
  observe contradictory lifecycle states in one real-time history.

  Binding a client host is a **composition decision, not a hot
  operation**. A bind after activation is refused with the typed
  `HostConstructionError::RuntimeAlreadyActivated`; rustX does not
  promise that a first host installed after semantic execution has begun
  would reconstruct the read state a continuously attached client would
  have. A headless runtime (Issue #60 subagents, every zero-client
  regression) simply never constructs a host.

  Runtime Client **attachments** stay fully dynamic after activation —
  attach, detach mid-attempt, reattach — because attachment lifetime and
  host-binding lifetime are different axes.

- **Adapter bootstrap is one global cut.**
  `ConversationRuntime::install_observation_bridge` is the one fallible
  step after the binding claim. It runs entirely under the one
  coordinator lock — the same lock `activate` takes, which is what makes
  the lifecycle rejection atomic — and captures the seed in this order:

  ```text
  T0  coordinator lock; reject if activated; install the observation queue;
      capture shutting_down / canonical messages / session model
  T1  background registry: install observer + capture snapshots
  T2  mailbox:             install observer + capture pending
  R   capability:          install observer + capture snapshot   <- the cut
      coordinator lock released
  ```

  > **Invariant.** The bootstrap cut `R` is a real global state of the
  > runtime: the initial snapshot contains every projected runtime fact
  > committed through `R`, every projected transition after `R` is
  > delivered exactly once through the live observation stream in
  > semantic publication order, and no transition before `R` is
  > published as a post-`R` event.

  This is a proof, not four independent cuts glued together. Every
  captured value is still its authority's live value at `R`:

  - coordinator facts cannot move — every mutator (`model_set`,
    `shutdown`, `submit_inbound`, admission, settlement) takes the
    coordinator lock, held across `[T0, R]`;
  - the background plane is pristine by construction — the
    `ConversationToolRuntime -> ConversationRuntime` ownership transfer
    requires no prepared dispatch and no committed record, and the
    registry then refuses `commit_dispatch` while its mailbox is bound
    inactive — so no background record exists across `[T0, R]` and none
    can be created;
  - the mailbox refuses `enqueue` while its bound runtime is inactive,
    so the pending queue is frozen across `[T0, R]`;
  - the capability coordinator refuses a runtime-owned `commit` before
    activation, and the capability snapshot is captured *at* `R`.

  And because each authority installs its observer in the same lock
  section that captures its seed, no transition can be both seeded and
  queued, and none can be neither.

  **Bootstrap state never fabricates a live event.** The projection
  installs every seeded fact — canonical history, session model,
  capability snapshot, and pending inbound — as snapshot state through
  `RuntimeClientProjection::bootstrap`. Nothing is routed through
  `apply`, so bootstrap publishes no `RuntimeClientEvent` and allocates
  no `RuntimeClientCursor`: `{ snapshot, cursor 0 }` is the state at `R`,
  and the first cursor belongs to a real post-activation transition (the
  background seed is provably empty by the ownership-transfer invariant).
  The bootstrap cut `R` **precedes** the activation transition: the
  handshake completes over the inert runtime and the shared
  `ConversationLifecycle` `Inactive -> Running` CAS happens afterwards.
  Because the runtime remains semantically inert from `R` until that
  transition — mailbox, background, capability, and coordinator mutations
  are all inactive-gated — no projected semantic fact can appear in the
  interval `[R, activation)`, so the live stream carries every
  observation the runtime ever emits.

  There is deliberately **no** runtime-side mirror of the client attempt
  view. The runtime does not fold `ConversationObservation` a second
  time; the client projection is the single fold.
- **One conversation runtime per identity, one host per runtime.** One
  `ConversationToolRuntime` identity is bound to at most one
  `ConversationRuntime` and at most one `RuntimeClientHost` for that
  identity's lifetime. `ConversationRuntime::new` performs one
  **tool-runtime ownership transfer** and claims the capability
  coordinator binding; `RuntimeClientHost::new` claims a second, client
  binding on the same handles; both are `Clone` and every clone shares one
  binding, so a cloned runtime bundle is not a second bindable identity. A
  second coordinator is rejected with
  `ConversationRuntimeError::RuntimeAlreadyBound` and a second host with
  `HostConstructionError::RuntimeClientAlreadyBound`.

  The ownership transfer is one real synchronization contract, not three
  independent steps. Under the background registry lock — the same
  boundary the dispatch ownership commit linearizes at — it requires a
  pristine background plane (no prepared dispatch, no committed record),
  claims the coordinator binding, and binds the canonical mailbox
  runtime-owned with a fresh `Inactive` shared lifecycle, all at one
  point:

  ```text
  standalone ConversationToolRuntime
      |
      |  ownership transfer (one registry critical section)
      |    1. require pristine background (no prepared, no committed)
      |    2. claim the coordinator binding
      |    3. bind the mailbox runtime-owned + shared Inactive lifecycle
      v
  ConversationRuntime-owned / inactive
      |
      |  background commit -> BackgroundDispatchError::ConversationInactive
      v
  ConversationRuntime::activate()   (the shared lifecycle Inactive -> Running)
  ```

  Either a standalone background commit wins the section first — the
  transfer is refused typed with
  `ConversationRuntimeError::ToolRuntimeNotQuiescent` and consumes
  nothing — or the transfer wins first and every later background commit
  fails `ConversationInactive`. A `ConversationRuntime` can therefore
  never be constructed over a tool runtime that already contains staged
  or committed background work, and the inactive phase can never inherit
  a detached semantic transition that would later advance the Runtime
  Client cursor before activation. Construction is transactional: if the
  capability claim fails after the transfer, the mailbox is unbound and
  the coordinator claim released again, restoring the exact previous
  standalone state.

  The ownership transfer (`standalone -> runtime-owned/inactive`) and
  activation (`Inactive -> Running`) is a distinct commit point after the
  transfer establishes runtime ownership plus the `Inactive` lifecycle
  relationship, and `activate` later performs the one lifecycle
  transition.

  This is a runtime ownership invariant, not a caller convention. Two
  coordinators over one authoritative runtime would each admit attempts
  from the same mailbox over competing canonical state, and each
  subsystem carries exactly one observer slot, so the second construction
  would silently unhook the first. The headless conversation runtime
  (zero hosts) is fully supported: it installs no observation seams and
  admits asynchronous inbound through the mailbox's shared wake handle.

  Every fallible validation runs before the claim, the binding claim is
  the ownership-commit boundary, and the only fallible step after it is
  the bridge handshake — on whose failure the claim is released again. A
  rejected construction therefore has no semantic side effect: no
  observer is replaced, no worker starts, no mailbox, background, or
  capability state moves, and no claimed-but-invalid binding remains.
- **Conversation runtime activation is explicit.** `ConversationRuntime::new`
  requires a Tokio execution runtime and rejects construction outside
  one with the typed `ConversationRuntimeError::NoExecutionRuntime`
  error, so `activate` can always spawn the admission worker. Activation
  is the composition's own explicit step — never a side effect of
  constructing a Runtime Client host — so the admission worker exists at
  exactly the same lifecycle point for a headless runtime and an
  interactive one, and native producers never depend on a Runtime Client
  call to activate admission.
- **One conversation authority.** The `ConversationToolRuntime` owns the
  `ConversationId`, the canonical mailbox, the authoritative background
  registry, and both binding identities; the conversation runtime
  *derives* its identity from it, and the Runtime Client host derives
  everything it reports from the conversation runtime.
  `RuntimeConversationConfig` and `RuntimeClientHostConfig` therefore
  carry no conversation id field of their own — a second configured
  identity could disagree with the runtime, and a coordinator that runs
  one runtime while naming another conversation would issue
  `AgentExecutionRequest`s the runtime rejects, after having already
  admitted the attempt. Structural absence removes that state instead of
  checking for it. The capability coordinator remains a separate
  authoritative identity, so it is still validated explicitly against the
  runtime before the coordinator binding claim.

  **Host lifetime is not attachment lifetime.** Reconnect replaces the
  attachment on the same host (detach, then a fresh `RuntimeClientEndpoint`
  `initialize` yielding a new `AttachmentId`); it never reconstructs the
  host. The binding is deliberately not released when the bound host is
  dropped: rebinding a surviving runtime bundle would require a recovery
  model for canonical history, pending mailbox projection, and cursor
  continuity that the Runtime Client projection does not own. Recreating a host over the same
  runtime bundle is **not** supported v1 recovery — a new host requires a
  new `ConversationToolRuntime` identity. Observer installation on the
  mailbox, background registry, and capability coordinator is crate-private
  for the same reason: it is a runtime coordination seam, not a public
  extension point.
- **Ownership: observation edges are non-owning.** The graph is:

  ```text
  semantic owner ─────────► Arc<RuntimeInner>
  (ConversationRuntime and clones, the host adapter, a running attempt
   task — the task is a bounded owner that releases at settlement)

  RuntimeInner ──► authoritative subsystems (tool runtime, mailbox,
                   capability coordinator)
  RuntimeInner ──► shared leaf observation queue (PendingObservations)

  RuntimeClientHost ──► Arc<ClientInner>
  ClientInner ──► Arc<ConversationRuntime> (control + bootstrap reads)
             ──► projection state
             ──► Arc<PendingObservations>

  authoritative subsystem ──► Arc<RuntimeObserver>
  RuntimeObserver ─────────► Weak<RuntimeInner>

  admission worker ────────► Weak<RuntimeInner> + Arc<WakeGate>
  projection worker ───────► Weak<ClientInner> + Arc<PendingObservations>
  ```

  Subsystem observer slots keep owning `Arc<dyn InboundObserver>` and
  friends; the concrete `RuntimeObserver` is non-owning, so installing a
  seam cannot create the cycle
  `RuntimeInner -> subsystem -> Arc<RuntimeObserver> -> RuntimeInner`. Each
  callback upgrades the weak handle and returns without publishing when the
  upgrade fails — the conversation runtime no longer exists, which is never
  an error for the subsystem. The admission worker holds only a weak
  runtime handle plus the wake gate it waits on, and the projection worker
  holds only a weak client handle plus the queue it waits on; neither holds
  a strong handle across an await.

  `RuntimeInner` is therefore destroyed when its last semantic owner is
  released, not at process exit. `RuntimeInner::drop` closes the wake gate
  (the admission worker's terminal condition) and the observation queue;
  `ClientInner::drop` closes the same queue (the projection worker's
  terminal condition); both closes are idempotent. Teardown takes no lock,
  joins nothing, and publishes nothing. A running attempt task is a
  deliberate *bounded* strong owner — an admitted attempt must reach
  settlement, and the task releases the runtime when it does. Attachment
  detach remains unrelated to runtime or host lifetime.
- **Lock order.** The graph is acyclic by construction:

  ```text
  CoordinatorState ──► ConversationInboundMailbox ──► PendingObservations
  CoordinatorState ──► PendingObservations
  ClientState ──────► PendingObservations
  ConversationBackgroundRegistry ───► PendingObservations
  SubagentRegistry ────────────────► PendingObservations (observer only)
  CapabilityCoordinator ───────────► PendingObservations
  AgentExecution (attempt task, holds no lock) ──► PendingObservations

  bootstrap (one section, coordinator lock held throughout):
    CoordinatorState ──► ConversationBackgroundRegistry
                    ──► ConversationInboundMailbox
                    ──► CapabilityCoordinator

  mailbox wake / WakeGate ─────────► (leaf Notify only)
  ```

  `PendingObservations` is the single leaf (one mutex plus a `Notify`; it
  calls nothing). Every authoritative subsystem fires its observer *while
  holding its own lock*, and every such observer does exactly one thing:
  append an immutable observation to that leaf. No subsystem ever
  acquires `CoordinatorState` or `ClientState`. Since Issue #61's
  revision there is no runtime semantic record in the graph at all — the
  runtime performs no fold, so there is no second intermediate lock.

  The subagent terminal-durability sink is a separate rule: the registry
  copies the sink while its mutex is held, releases that mutex, and only then
  calls the owning `ConversationRuntime`, which may acquire `CoordinatorState`
  and publish `DurabilityFailed`. Thus the normal ownership direction is
  `CoordinatorState -> SubagentRegistry`; there is no held-lock reverse edge.
  The registry never waits for a process or performs async work while its
  mutex is held. Runtime Client projection callbacks remain leaf queue
  writes, and the driver task owns the process handle independently of both
  logical locks.

  All downward edges out of `CoordinatorState` point the same way. The
  `CoordinatorState -> mailbox` edge exists in `admit_next_attempt`,
  which drains under the coordinator lock so the drain fact, the history
  commits, and the attempt publication linearize together. The bootstrap
  handshake adds `CoordinatorState -> {background, mailbox, capability}`
  in that same direction, held as one section, which is what makes the
  bootstrap cut global. No reverse edge exists, so the graph stays
  acyclic.

  The mailbox's shared wake handle notifies the admission worker at every
  enqueue publication — a leaf signal, never a lock — so idle
  asynchronous inbound is admitted without any client request.
  Consequently an authoritative commit never waits on the client lock,
  and subscriber notification can never block authoritative runtime
  state. The `AgentExecutionObserver` callbacks append to the leaf queue;
  that adds no incoming edge because `AgentExecution` is owned by its
  attempt task and holds no lock when it observes. Every client lock
  acquisition drains the pending queue first, so queued observations fold
  in the coordinator's commit order.
- **Snapshot/cursor invariant.** `snapshot_get` returns `{ snapshot,
  cursor }` where the snapshot describes all Runtime Client state through
  cursor C, and a subscription after C observes every subsequently
  published event or fails explicitly with `resync_required`.   This holds
  by construction (one boundary), not by luck. At bootstrap the same
  invariant holds at cursor 0: the seed is installed as snapshot state,
  never replayed through `apply`, so no pre-existing runtime fact
  allocates a cursor or publishes an event — and, by the ownership-transfer
  invariant, no background execution can even exist at bootstrap (the
  registry is pristine at construction and refuses dispatch commits while
  its mailbox is bound inactive).
- **RuntimeEvent mapping policy.** Every internal event is classified
  PROJECT / FOLD INTO CLIENT STATE ONLY / INTERNAL in the projection
  owner: attempt lifecycle/settlement, streaming output, tool-call
  assembly, and foreground/background tool lifecycle project; turn
  counting and final usage fold; model request mechanics and compaction
  mechanics stay internal. Internal `RuntimeEvent` evolution therefore
  cannot silently break Runtime Client Protocol v1.
- **Streaming repair.** The snapshot carries an in-flight Assistant output
  view (accumulated blocks) and foreground tool views keyed by the
  logical tool-call identity, so a client repairing after `resync`
  reconstructs every client-visible effect without duplicated or missing
  semantic output. Parallel physical completion never corrupts logical
  identities.
- **The client replay ring is a projection cache, not durability.** A finite
  in-memory ring (`RUNTIME_CLIENT_REPLAY_LIMIT_DEFAULT = 4096`,
  configurable) holds recent projected events; expired or ahead-of-stream
  cursors fail with `resync_required`. Reconnect/bootstrap rebuilds the
  projection from the ConversationStore's current head and paged Event
  Journal when historical inspection is requested. The ring never supplies
  recovery facts and is not the Event Journal.
- **Cursor-driven subscriptions (no second backlog).** A subscription is
  a consumed `RuntimeClientCursor` into that one ring plus an
  edge-triggered, payload-free wakeup. Publication pushes into the ring,
  evicts beyond the retention limit, and wakes subscribers; a consumer
  pulls the next retained entry after its cursor, one per poll. A stalled
  consumer therefore costs one cursor rather than a queue, total retained
  memory stays bounded by `replay_limit` no matter how far behind any
  consumer is, and the publisher never blocks on a slow consumer. A
  consumer that falls behind retention receives the explicit
  `EventDelivery::ResyncRequired` — a stable, terminal verdict — instead
  of a silently non-contiguous stream, so cursor contiguity within a
  subscription is guaranteed. `EventDelivery` distinguishes `Event`,
  `Pending`, `Closed`, `ResyncRequired`, and `Exhausted`, which is what
  lets Issue #38 implement deterministic bounded transport backpressure
  with no unbounded queue hidden underneath it.
- **Explicit projection failure.** Cursor allocation uses a checked add:
  overflow sets an exhausted flag rather than wrapping, publication
  stops, and every read (`snapshot_get`, `capability_get`, `initialize`,
  subscribe, and subscription polls) then fails with
  `projection_exhausted`. A read never hands back a model that silently
  stopped folding authoritative transitions.
- **Attachment lifecycle.** Protocol v1 admits at most one active
  attachment: the first attach succeeds, a second fails with
  `attachment_in_use` and never evicts the first, detach (explicit or
  RAII drop) releases ownership, reconnects receive a fresh attachment
  identity, and request ids are attachment-scoped. Detach changes only
  attachment state: it never cancels the attempt, never cancels
  conversation-owned background work, never drains the mailbox, and
  never mutates canonical history or capability state.
- **Current-attempt coordination.** The conversation runtime owns the
  current-attempt slot and the exact `AgentCancellation` the attempt task
  runs against; `cancel_current_attempt` requests cancellation through the
  coordinator, which verifies under its own lock that the named attempt is
  still the current one, so a settlement/admission race can never cancel a
  newer attempt. The acceptance response is never terminal settlement (the
  Agent Loop owns settlement, observed asynchronously). Neither the
  coordinator nor the host owns a second attempt state machine.
- **ConversationState: one owner at a time.** Ownership transfers by move; it
  is never cloned or shared as a second mutable authority:

  ```text
  idle        ConversationRuntime owns ConversationState
  admission   ConversationState moves into AgentExecution, which is the
              sole authority while the attempt runs
  running     the runtime never mutates a competing copy; asynchronous
              inbound stays mailbox-owned until the loop commits it, and
              RuntimeClientSnapshot.messages is projection only
  settlement  AgentExecutionResult moves ConversationState back to the
              ConversationRuntime for the next idle/admission boundary
  ```

  The settlement-path equivalence between the projection mirror and the
  authoritative ledger is covered deterministically by regression tests;
  there is only ever one mutable authority.

  This move-based runtime ↔ AgentExecution boundary is the bounded #54
  design; Issue #61 extracted the enclosing `ConversationRuntime`.
- **Admission: one authority.** `ConversationRuntime` is the one
  next-attempt admission owner. Every ordinary inbound producer — the
  Runtime Client human submit path, runtime/agent inbound, background
  terminal notifications, future subagent/fleet/external producers —
  publishes into the authoritative mailbox; the mailbox's shared wake
  handle notifies the coordinator's admission worker; and
  `admit_next_attempt` observes idle + gate, performs one finite
  watermark-bounded drain, commits the drained messages into canonical
  history, allocates the attempt id, freezes the model snapshot, and
  publishes the current attempt — all under the one coordinator lock.
  While an attempt is running, enqueued messages wait for the loop's
  safe-boundary drain inside the same attempt, and the settlement handoff
  admits the next attempt exactly once. Success of `submit_inbound` means
  accepted/published, never assistant-finished. No producer ever starts an
  `AgentExecution` itself.
- **Mailbox diagnostics.** The projection mirrors enqueue/drain facts
  (pending items in `InboundSequence` order, latest drain watermark and
  count) from an observation seam fired at the mailbox linearization
  points; the conversation runtime's observer queues observations (the
  coordinator drains the mailbox under its own lock) and a worker task
  plus every projection lock acquisition applies them in total order.
  `RuntimeClientCursor` remains a distinct domain from `InboundSequence`;
  clients can never drain or mutate the mailbox. Background terminal
  notifications enqueue through the same semantic path as every other
  mailbox state.
- **Background projection.** The authoritative
  `ConversationBackgroundRegistry` is projected through a read-only
  observation seam: `BackgroundExecutionUpdated` events and the snapshot
  background section carry execution identity, tool identity/name,
  lifecycle, latest bounded progress, and terminal result. Detached work
  survives attempt termination and client detach; protocol
  `background_status`/`background_cancel` use the registry authority, and
  cancel acceptance is distinct from terminal settlement.
- **Capability/tool/Skill inspection.** One semantic projection derives
  from the active `CapabilitySnapshot`: the revision, a deterministic
  tool catalog (id, name, description, input schema, execution/
  concurrency/replay policies, origin builtin/MCP/Python), and a
  deterministic Skill catalog (identity, version, name, description).
  Executors, environment paths, package-manager state, and `SKILL.md`
  bodies never appear; ordering is deterministic; inspection never
  mutates the capability set.
- **Agent Status projection: composed exactly once.** One request
  preparation calls `AgentStatusComposer::compose` exactly once
  (`AgentExecution::compose_status`), sampling the clock once and
  invoking each registered provider once. That one structured `AgentStatus`
  value fans out to two core-owned destinations: `render_agent_status`
  produces the canonical Runtime context UserMessageBlock for Context
  Assembly, and the same structured value is handed to `observe_status` for
  the Runtime Client projection. The client path never calls `compose` again
  — not even through a cloned composer sharing the same clock and providers
  — and never parses the rendered prompt text to recover structure.
- **Protocol envelope.** A transport-neutral JSON-RPC-style envelope:
  `request(id, method + typed params)`, `response(id, result | error)`,
  and `event(cursor + typed payload)` with no request ids on
  notifications. Every v1 method is client-initiated
  (`initialize`, `submit_inbound`, `cancel_current_attempt`,
  `snapshot_get`, `subscribe_events`, `capability_get`,
  `background_status`, `background_cancel`, `detach`, `shutdown`).
  Typed errors distinguish `unsupported_protocol_version`,
  `attachment_in_use`, `not_attached`, `invalid_request`,
  `no_current_attempt`, `unknown_background_execution`,
  `resync_required`, `runtime_shutdown`, `invalid_state`,
  `projection_exhausted`, and `runtime_failure` — provider SDK errors and
  internal synchronization failures are never exposed.
- **Shutdown vs detach.** `shutdown` starts the one runtime drain, cancels
  the current attempt and conversation-owned work, and resolves only after
  quiescence. It is not detach. Detach and transport loss leave semantic
  runtime work running.

#### Runtime Client transports: stdio JSONL (Issue #38)

Transports live beneath the semantic layer, in their own namespace:

```text
rustX Runtime
      |
      v
Runtime Client projection
      |
      v
Runtime Client Protocol v1        semantic; Issue #37
      |
      v
transport adapters                framing only; src/runtime_client/transport
      |
      +-- stdio / strict JSONL    Issue #38
      |
      +-- WebSocket               Issue #36, later
      |
      v
clients
```

Issue #38 adds `src/runtime_client/transport/stdio.rs`. Adding Issue #36
means adding a sibling module there; no semantic module moves.

- **The endpoint remains the semantic owner.** A transport calls
  `RuntimeClientEndpoint::handle_request` and forwards
  `EventSubscription` deliveries. It implements no protocol-version
  negotiation, no attachment admission, no `AttachmentId` allocation, and
  no snapshot, cancellation, replay, or shutdown semantics. The governing
  transport invariant is that only a complete, valid, in-bound-size
  framed request may cross into `handle_request`.
- **One session owns framing and I/O.** `serve_stdio_jsonl_with_io` is
  one async loop owning the endpoint, the bounded reader, the writer, and
  the framing state; `serve_stdio_jsonl` is the process-stdio composition
  of it over `tokio::io::stdin()`/`stdout()`. There are no transport
  tasks, no channels, and no ownership cycle back into the host. Dropping
  the endpoint on return is the RAII detach.
- **Record limit.** `STDIO_JSONL_MAX_RECORD_BYTES` is 8 MiB and applies
  in both directions. It bounds one record's JSON payload: the
  terminating LF is not counted, and a trailing CR is counted when CRLF
  was used on input. Inbound records are accumulated out of a fixed
  `STDIO_JSONL_READ_CHUNK_BYTES` chunk with the bound checked before
  every append; outbound records are serialized into a size-limited sink
  so an oversized record is refused mid-serialization rather than built
  and then measured. The bound is on logical record retention: each
  transport buffer holds at most one record's bytes and no reservation
  above the limit is ever requested. Allocator rounding of such a
  request is outside this contract, so `Vec::capacity()` itself is not
  claimed to be bounded by the limit.
- **LF, and accepted CRLF.** LF is the sole record delimiter. One
  physical LF terminates one record, so an escaped `\n` inside a JSON
  string stays in one record and multiline pretty-printed JSON is not
  supported. CRLF input is accepted by removing exactly one `\r` before
  the terminating LF; no other whitespace is touched.
- **Malformed and oversized input is transport-fatal.** Protocol v1 has
  no uncorrelated error envelope, and a malformed frame may not even
  carry a request id, so the transport invents none. Any complete
  in-bound-size record that does not deserialize to the exact v1 request
  type — malformed JSON, unknown method, unknown field, wrong parameter
  type, empty or whitespace-only record — ends the session with a
  framing error, applies nothing, and writes no protocol record. An
  oversized record is session-fatal immediately: no further buffering, no
  discard/recovery state machine, and never a partially applied request.
- **Zero outbound backlog.** The transport queues no protocol records. At
  most one outbound record is being serialized and written at a time, and
  the next input record or event is selected only after that write
  completed. The projection's bounded replay ring stays the one retained
  Runtime Client event backlog: there is no second transport history and
  no reconnect log.
- **A slow consumer stalls the transport, not the runtime.** A blocked
  output parks the transport's current write and stops it consuming
  input. Attempt execution, event publication, mailbox activity,
  background execution, and capability state continue under their own
  owners, and no projection lock is held across any transport await.
- **Active-subscription lag closes the transport.** After a stall the
  subscription may fall behind the bounded replay ring. Protocol v1 has
  no uncorrelated stream-error record, so the session ends with a typed
  local `SubscriptionLagged` error carrying the cursor information and
  the client repairs from an authoritative snapshot after reconnecting.
  The semantic `subscribe_events` → `resync_required` path is unchanged.
- **EOF and broken pipe detach only.** Clean EOF at a record boundary and
  an output `BrokenPipe` are normal session ends; a partial record at EOF
  is a typed truncation error. All of them drop the endpoint and detach,
  and none cancels the current attempt, settles anything, drains the
  mailbox, mutates canonical history, or shuts the runtime down. A failed
  write is never retried, because it may have partially reached the peer.
- **Semantic shutdown does not close the transport.** A successful
  `shutdown` is answered like any other request and the session keeps
  serving: reads still work, further inbound gets the typed
  `runtime_shutdown` error, and only a later EOF or detach ends the byte
  stream.
- **Transport errors are not protocol errors.** `StdioTransportError` and
  `StdioSessionEnd` are local to the transport; nothing transport-shaped
  enters `RuntimeClientError`, and the transport writes no human or
  operator logging to its output sink — failures are returned to the
  caller for a process-composition layer to report.
- **Conformance is transport-independent.** The Issue #38 scenario suite
  (`tests/scripted/support/runtime_client_conformance.rs`) drives one set of
  semantic scenarios through a direct-endpoint driver and the stdio
  driver. Issue #36 adds a WebSocket driver and inherits every scenario
  unchanged; byte-level framing tests stay transport-specific.

#### Runtime Client model semantics (Issue #42)

The protocol distinguishes two model facts that a client must never
conflate:

```text
snapshot.model            the session's *desired* configuration and its
                          resolution — mutable, client-settable
snapshot.attempt.model    the immutable snapshot an already-admitted attempt
                          froze — never changes for that attempt
```

While an attempt admitted with model A is running and the session has been
switched to B, the snapshot truthfully reports both at once. No client has to
infer this from event ordering.

The same guarantee holds for a client that only follows the incremental
stream, because `attempt_started` is **self-contained**:

```text
attempt_started { attempt_id, model }   model = the frozen AttemptModelView,
                                        identical to snapshot.attempt.model
```

The value is runtime-owned and published by the projection under the same
coordinator lock that admitted the attempt; a client never supplies it and never
derives it. So a continuously subscribed client answers "which model is this
attempt actually using" from the start event alone — no `snapshot_get` round
trip and no inference:

```text
session = A ; attempt admitted -> attempt_started(model = A)
model_set(B) accepted mid-attempt
                               -> session_model_changed(B)
                                  (no second attempt_started; A keeps running)
next attempt admitted          -> attempt_started(model = B)
```

Three methods complete the contract:

- `model_catalog_get` — the bounded public catalog view: model reference,
  protocol, context window, configured max output, declared *and* effective
  capabilities, reasoning profile identities with their semantic enabled
  state, the default profile, and the redacted credential *source*. This is
  why #39 never reads `models.json`. No endpoint, no credential, no adapter
  internal, and no compat object appears.
- `model_get` — the authoritative session model state.
- `model_set` — a **whole-state replacement**, never a JSON patch.
  Validation is transactional: a rejected update changes nothing, allocates
  no cursor, and publishes no event. A valid update may occur while an
  attempt is running and affects future admissions only.

One event, `session_model_changed`, is published on the existing observation
stream by the existing projection owner, under the same coordinator lock that
owns attempt admission. There is no second event stream and no second cursor
domain.

### Layer 8: The local conversation runtime process (Issue #42, Issue #61)

```text
explicit startup arguments (--models --session --workspace --runtime-root)
        |
ModelCatalog + LocalSessionConfig
        |
        +--> SessionModelState (authoritative session model)
        +--> ConversationToolRuntime  (workspace, artifacts, mailbox,
        |                              background registry, base environment)
        +--> base ToolRegistry + register_native_tools(...)
        +--> CapabilityCoordinator    (same conversation and workspace)
        +--> prepare_candidate() -> commit()   <-- before serving
        +--> context policy / Surface / status pieces
        |
LocalConversationCore  (the one shared semantic composition, inactive)
        |
        +-- into_interactive(): RuntimeClientHost (projection/control/
        |                       attachment adapter), then activate
        |       -> LocalConversationRuntime -> RuntimeClientEndpoint
        |          -> stdio JSONL (Issue #38)
        |
        +-- into_headless(): activate, no Runtime Client host
                -> HeadlessConversationRuntime (Issue #60 subagents)
```

`LocalConversationCore::compose` is the one Rust-side semantic composition
owner; `LocalConversationRuntime::compose` and
`HeadlessConversationRuntime::compose` are the two final paths over it, both
returning already-active runtimes and both activating through the one
`ConversationRuntime::activate` boundary. The startup capability commit
happens *before* the conversation runtime is constructed, so it is not
subject to the runtime's lifecycle gate.

Startup failure ownership (Issue #81): failures that prove the core runtime
itself cannot be constructed — startup files, model catalog/credentials/
bindings, session configuration, workspace/private-store ownership, native
tool plane construction, or the base capability plane (environment-store
layout, malformed Skills, dependency conflicts, shared environment
materialization) — remain fatal composition errors. Failures of **optional
external capability sources** — the custom Python tool plane and each
configured MCP server independently — are isolated by the capability plane
into typed availability state (`CapabilitySourceState::Unavailable { reason }`
keyed by `CapabilitySourceId`), and composition continues: the base/native
capability set is never conditional on an optional source, one MCP server's
failure never suppresses another, and only successfully prepared capability
objects enter the committed active snapshot. Opening/creating the
Python-private store itself (`<environment store>/m7-tools`) is part of the
optional Python preparation — the coordinator constructor owns only the
store location — so a broken Python store degrades Python availability and
can never fail core construction, and the base-only subagent capability
path (`prepare_base_only_candidate`) never touches Python storage at all.
Each `reason` is normalized at the capability-owning boundary before it
enters the authoritative state: valid UTF-8, deterministic, at most
1024 bytes (`CAPABILITY_FAILURE_REASON_MAX_BYTES`, truncation marked with
`…[truncated]`), so an external peer can never make the committed state
unbounded and the Runtime Client projects the already-bounded value
verbatim. The Runtime Client capability
projection (`CapabilityView.sources`) carries the typed state, so a client
observes *why* a source is unavailable instead of inferring failure from a
dead transport. `CapabilityRevision` advances only when the effective
committed executable capability set changes; an availability-only change
never fabricates a revision but is still observed: both kinds of commit
publish the one `CapabilityUpdated` Runtime Client event carrying the
complete folded `CapabilityView`, whose `revision` tells the client
whether the executable capability identity changed.

The governing invariant:

> One local runtime process owns one conversation session. That session owns
> one authoritative mutable session-model configuration, one
> `ConversationToolRuntime` identity, one `CapabilityCoordinator`, one context
> policy/Surface domain, and one `ConversationRuntime`. Runtime Client
> attachments may come and go without replacing those semantic owners, and
> the conversation executes identically with zero attachments (Issue #61:
> headless composition is the same coordinator, admission, `AgentExecution`,
> Context Assembly, tool, and provider path).

A client — including the Issue #39 TUI — owns the child-process lifecycle and
nothing else. It never assembles provider adapters, model parameters, context
engines, tool registries, capability coordinators, or summary models.

There is deliberately no process-global registry, no second background
manager, no client-owned tool registration, no tool plugin factory, no second
coordinator, and no second host.

Configuration is explicit paths only. M10 (#13) owns discovery, precedence,
profiles, and manifest UX; none of that exists here. Unknown fields are
rejected everywhere, so a typo fails startup loudly rather than silently
changing semantics.

#### Native async subagents (Issue #60 / M9.25)

The `subagent` intrinsic is conversation-owned detached work, not a second
agent loop or a generic task framework:

```text
ConversationRuntime/coordinator
        |
        v
SubagentRegistry          logical identity, lifecycle, capacity, durability
        |
        v
subagent process driver   sole committed OS-process handle owner
        |
        v
real --subagent-child     ConversationRuntime + Agent Loop + Context + Tools
```

`prepare` validates the bounded task/context and stages a real child through
its typed Hello/Ready handshake. The one ownership commit freezes one start
timestamp, durably writes `SubagentOwnershipCommitted`, and creates the
logical Running record. Start-vs-cancel has exactly one arbitration
boundary: the registry mutex covers the command-handle install, the
lifecycle read, and the synchronous start-gate release in one critical
section. Cancellation committed first resolves the gate cancelled — the
driver sends `Cancel` before `Delegate`, so no child semantic work ever
begins; gate release committed first defines an already-started child whose
later cancellation is in-flight cancellation. The registry retains no
`tokio::process::Child`; rollback and the committed driver are the only
physical teardown owners at their respective phases.

`ConversationRuntime::new` validates the registry's typed ownership domain
before anything is claimed — the same `ConversationId`, the same parent
`AgentId`, the exact same canonical mailbox (structural identity, never a
file-path comparison). The ownership transfer then binds the mailbox to the
runtime's `Inactive` lifecycle and performs the **authoritative** pristine
check under the registry mutex: a standalone child commit that won before
the binding makes the constructor reject (rolling back every claim it
acquired); a runtime claim that wins first makes later standalone child
commits fail. The runtime never silently adopts a registry with a live
child started outside its ownership transfer.

The child accepts `Delegate` through its ordinary durable inbound path as
`UserSource::Agent(parent)`. A child-side `Cancel` commits directly into the
runtime-owned one-shot cancellation intent
(`cancel_current_or_next_attempt`): a current attempt's `AgentCancellation`
is requested immediately, and a still-unadmitted attempt starts
already-cancelled when admission consumes the intent. `AttemptAdmitted`
observation is evidence, never a control dependency, and the existing
durable model-request-start frontier (M9b) decides whether a model request
may start — zero requests before it, in-flight cancellation after it. The
child result is only a candidate on IPC;
the parent driver reaps first, then the registry freezes a UTF-8-safe
byte-bounded candidate and asks the parent mailbox to atomically accept the
terminal inbound plus `SubagentTerminalPublished`. A normal terminal state is
not observable before that compound commit. `PublishingTerminal` remains
capacity-owning while the candidate is unresolved. After bounded retry
exhaustion, the failure sink (called outside the registry mutex) places the
owning `ConversationRuntime` in `DurabilityFailed`; no false terminal success
or healthy state is reported.

`DurabilityFailed` is the **fail-closed frontier for new ownership**, shared
with the background plane: `ConversationRuntime` owns the durability
*policy*, and the runtime-owned `DurabilityGate` is the **single
authoritative storage of the absorbing durability-failure fact**
(`DurabilityFailure { operation, diagnostic }`, committed through its one
mutation API) as well as the synchronization frontier shared with both
conversation-owned registries. The coordinator keeps only transient
admission-cycle retry bookkeeping — there is no second failed-state
authority. Every new subagent or background ownership
commit holds that gate across its durable ownership write and record
publication, and the `DurabilityFailed` commit acquires the same gate, so
the two have one deterministic total order — a failure that wins first makes
the new ownership commit refuse (the staged child/runner rolls back
conclusively, no durable fact, no record, no Delegate), and an ownership
that wins first is already durably owned before the failure can be
published. `DurabilityFailed` is deliberately distinct from `Draining`: it
closes new semantic mutation only, while already-owned work (cancel,
escalation, reap, terminal publication, drain, failure reporting) retains its
settlement authority and never acquires the gate.

Terminal validation resolves the child identity from the durable
`SubagentOwnershipCommitted` fact — through its canonical event identity
(`subagent-committed-event:{id}`, derived from the embedded `SubagentId`,
which the durable authority enforces at write time and revalidates at
read time) and the unique `event_id` index in bounded time, never a journal
scan — so a repeated or caller-controlled `child_agent_id` in a terminal
event is not authority. Success is
`UserSource::Agent(child)`; failure, cancellation, and recovery interruption
are `UserSource::Runtime`. The Explore child capability snapshot contains
exactly Read/Glob/Grep, with no write, shell, background, MCP, or recursive
subagent capability. Parent hard death closes the control channel; the child
exits, and restart classifies the old nonterminal ownership as Interrupted
without reattach, replay, PID adoption, or relaunch.

Representative `models.json` (no real credential ever appears in a catalog
checked into a repository — `$ENV_VAR` is the reason the literal form exists
only for local development):

```json
{
  "providers": {
    "gateway": {
      "baseUrl": "https://gateway.example/v1",
      "apiKey": "$RUSTX_MODEL_API_KEY",
      "models": [
        {
          "id": "reasoner",
          "protocol": "anthropic_messages",
          "contextWindow": 200000,
          "maxOutputTokens": 32000,
          "capabilities": {
            "inputModalities": ["text", "image"],
            "outputModalities": ["text"],
            "toolCalls": true,
            "reasoning": true
          },
          "requestParams": { "temperature": 0.7, "top_k": 40 },
          "reasoning": {
            "defaultProfile": "on",
            "profiles": {
              "off": {
                "enabled": false,
                "requestParams": {
                  "thinking": { "type": "disabled" },
                  "temperature": 0.7
                }
              },
              "on": {
                "enabled": true,
                "requestParams": {
                  "thinking": { "type": "enabled", "budget_tokens": 32000 },
                  "temperature": 1.0
                }
              }
            }
          },
          "compat": {}
        }
      ]
    },
    "compat-service": {
      "baseUrl": "http://127.0.0.1:8080/v1",
      "apiKey": "local-development-only",
      "models": [
        {
          "id": "small",
          "protocol": "openai_chat_completions",
          "contextWindow": 32768,
          "maxOutputTokens": 4096,
          "capabilities": {
            "inputModalities": ["text"],
            "outputModalities": ["text"],
            "toolCalls": false,
            "reasoning": false
          },
          "requestParams": { "min_p": 0.05, "repetition_penalty": 1.1 },
          "compat": {
            "chatMaxTokensField": "max_tokens",
            "chatReasoningReplay": "omit",
            "chatStreamUsage": "unsupported"
          }
        }
      ]
    }
  }
}
```

Note that `capabilities.inputModalities` claims `image` for `reasoner`, but
the *effective* capability the Runtime Client advertises is text-only until an
adapter can actually transmit an image reference. The claim is preserved so a
client can explain why.

Representative local session configuration:

```json
{
  "conversationId": "conv-local-1",
  "agentId": "agent-default",
  "model": {
    "model": "gateway/reasoner",
    "reasoningProfile": "on",
    "requestParams": { "top_p": 0.95 },
    "maxOutputTokens": 8000,
    "summaryModel": {
      "mode": "explicit",
      "model": "compat-service/small",
      "requestParams": { "temperature": 0.1 }
    }
  },
  "timezone": "Europe/Paris",
  "context": {
    "reserveTokens": 16384,
    "keepRecentTokens": 20000,
    "summaryOutputCap": 2048
  },
  "mcpServers": {
    "exa": {
      "type": "http",
      "url": "https://mcp.exa.ai/mcp",
      "headers": { "x-api-key": "YOUR_EXA_API_KEY" }
    },
    "exa-local": {
      "type": "stdio",
      "command": "npx",
      "args": ["-y", "exa-mcp-server"],
      "env": { "EXA_API_KEY": "YOUR_EXA_API_KEY" }
    }
  },
  "mcpToolPolicies": {
    "exa": { "execution": "foreground_only", "concurrency": "parallel" }
  },
  "nativeTools": {
    "bash": { "execution": "model_selectable", "concurrency": "sequential" }
  },
  "environment": { "RUSTX_PROJECT": "demo" }
}
```

`mcpServers` is an ecosystem-compatible named map keyed by MCP server
identity: an entry is the same object an MCP server's own documentation
publishes, so it stays copy-pasteable. The map key is the only identity;
there is no `serverId` field, no nested `transport` object, and no embedded
rustX policy. The canonical spellings are `type: "http"` with `url`
(plus optional `headers`) and `type: "stdio"` with `command` (plus optional
`args`, `env`, and a workspace-relative `cwd`). The two shorthand forms the
ecosystem's own READMEs use are accepted and normalize identically: a bare
`url` infers HTTP, a bare `command` infers stdio. Nothing else is accepted —
no `streamable-http`/`streamable_http` alias, no `sse`, no `ws` — and every
ambiguous or contradictory entry (`url` with `command`, `type: "http"` with
`args`, an unknown field, a blank `url`/`command`) fails startup.

`mcpToolPolicies` is the separate rustX-owned overlay keyed by the same
identity. It carries the invocation policy rustX applies to every tool of one
server; a server without an entry gets the deterministic default
(`foreground_only` + `sequential`), and an entry naming a server `mcpServers`
does not declare fails startup. Keeping it outside the connection object is
what keeps `mcpServers` recognizable as ordinary MCP configuration.

Normalization happens exactly once, at this boundary: `LocalSessionConfig`
validates each entry and turns it into a typed
`BTreeMap<McpServerId, McpServerBinding>`. The shorthand spellings never
reach `McpServerRuntime`, the `CapabilityCoordinator`, the Agent Loop, or the
TUI.

A session override may not declare a key the selected reasoning profile owns
(`temperature` above belongs to the `on` profile, so `requestParams` declares
`top_p` instead) and may not declare a runtime-protected wire key.

**Process output contract.**

```text
before serving : stdout is exactly empty
while serving  : stdout is Runtime Client JSONL records only
diagnostics    : stderr, always
```

Composition — including the initial capability commit — finishes entirely
before the transport is created, so a startup configuration failure writes a
bounded stderr diagnostic, exits non-zero, and leaves **zero bytes** on
stdout. `println!` is never used for diagnostics anywhere in the process.

**Exit semantics.** A clean input EOF at a record boundary or a peer broken
pipe ends this one-session process successfully. Malformed framing or any
other transport error reports to stderr and exits non-zero. Semantic
`shutdown` responds only after the conversation runtime reaches quiescence,
but does **not** close the transport — a controlling client closes it
according to its own lifecycle policy. Transport EOF remains a detach, never
an Agent Loop cancellation primitive; M9 recovery and quiescence remain
semantic runtime concerns, not transport concerns.

### Layer 9: The TypeScript reference terminal client (Issue #39)

`tui/` is a private pnpm package holding the client half of the Runtime
Client boundary. It exists to validate rustX end to end, and its whole design
follows from one rule:

> Pi TUI is only the terminal input/output projection of rustX.

`@earendil-works/pi-tui` sits **below** the rustX semantic boundary, never
beside it:

```text
rustX Runtime semantics
        |
        v
Runtime Client Protocol v1
        |
        v
rustX TypeScript projection
        |
        v
rustX TUI presentation
        |
        v
@earendil-works/pi-tui primitives
```

Pi supplies terminal mechanics — differential rendering, a multiline editor,
Markdown layout, overlays, a spinner. rustX supplies every semantic above it.
No Pi class holds authoritative state, and nothing resembling Pi's
`AgentSession`, `SessionManager`, model runtime, provider registry, tool
registry, compaction state, session tree, extension runtime, Skill runtime,
or `InteractiveMode` exists in the package. Pi is imported by one file.

**Owners.** Each responsibility has exactly one owner:

```text
ChildRuntimeProcess     OS process lifecycle only: spawn with the explicit
                        --models/--session/--workspace/--runtime-root
                        contract, stdio, a bounded stderr tail, stdin close,
                        wait, bounded fallback termination. It never reads a
                        byte of stdout and never interprets a startup path.

RuntimeClientConnection the single owner of JSONL framing, request-id
                        allocation, the pending RPC map, response
                        correlation, event delivery, and ordered writes.
                        Every pending request settles exactly once; after
                        terminal failure new requests fail immediately.

RuntimeClientSession    attach, snapshot/cursor installation, subscribe,
                        resync repair, shutdown sequencing. No agent
                        semantics.

PresentationProjection  the ephemeral render cache.

CommandDispatcher       UI intent -> one canonical Runtime Client operation.

RustxTuiApp             the Pi components, and the only owner of the client's
                        own display preferences.
```

**The semantic component model (Issue #79).** The presentation layer is a
grammar of semantic components rather than a log of protocol records:

```text
PresentationState
        |
        +-- presentation/tools.ts        ToolCallId correlation
        |
        +-- transcript components        UserMessage, AssistantText, Reasoning,
        |                                Refusal, system/compaction, ToolCard
        |
        +-- activity components          background, interactions, orphaned
        |                                executions, WorkingStatus
        |
        +-- model/session components     ModelSelector, footer/status
                |
                v
          Pi primitives
```

Three rules hold the layer together.

*One tool call is one visual entity.* rustX publishes three different facts
about one logical call — the assistant `tool_call` block, the attempt's
foreground execution lifecycle, and the committed canonical `role: "tool"`
result. Their semantic ownership stays separate; `presentation/tools.ts` joins
them for display only, keyed by `ToolCallId`. Correlation uses no tool name,
argument equality, list position, timing, or adjacency, so two concurrent
identical calls remain two cards. The card renders at the assistant block that
requested it, which is why the same call never appears simultaneously as
transcript JSON, a running card, and a separate result block.

*A folded result never reorders canonical content.* rustX's canonical model
permits a `tool_call` that is not the last block of its assistant message, and
does not require a result message to immediately follow the message that
requested it, so a committed result is folded into its call's card **only if
every canonical fact it would be moved across belongs to the same foldable
batch**. Fold eligibility is a property of the complete canonical interval
between the call anchor and the result position, not of the owning assistant
message's block tail: a batch (the message's trailing unbroken run of
`tool_call` blocks) folds only when nothing but `tool_call` blocks follow its
first call *and* every transcript entry from the anchor through the batch's
last committed result is a result of that same batch. An intervening `User`,
`System`, or unrelated `Assistant` message unfolds the whole batch, all-or-
nothing, and each of its calls is drawn as two fragments of one entity: the
call at its `tool_call` block and its terminal continuation at the canonical
result message. The plan is a pure derivation of the ordered transcript, so a
fresh snapshot reaches the same decision as an incrementally folded state.

*Tool identity chooses presentation; runtime facts choose semantics.* A stable
`ToolId` selects a specialized renderer — Bash, Read, Grep, Glob, Edit, Write
today — so a shell call reads as `$ cargo test --all` instead of argument
JSON. A renderer formats already-authoritative facts and is never handed the
lifecycle: running, success, failure, denial, cancellation, timeout,
interruption, progress, duration, exit code, and truncation all come from the
Runtime Client and are rendered by the card shell. A renderer that does not
recognise a shape returns nothing and the generic renderer takes over, so
unknown, MCP, and Python tools are always fully usable.

*Display preferences are not runtime state.* Reasoning visibility and which
cards are expanded live in the app, not in `PresentationState`. Every
externally-derived band of a collapsed card — subject, both detail bands, the
failure/denial reason, and the runtime-published summary — is finite in *both*
line count and content length, because one dimension is not a bound: a 100 kB
JSON string is three pretty-printed lines and a 50 kB path is one. The two
detail bands keep separate budgets, and the card shell owns the bound so no
renderer can forget it or bypass it through `summary`. The status header names
the settlement (`failed`, `denied`) while the runtime's prose explaining it
lives once in the bounded reason band; only the runtime's own
`TruncationState` and the typed `CancellationReason` are unbounded header
facts. Expanding a card re-renders facts the client already holds — it never
re-executes a tool or fetches anything — and it is unrelated to the runtime's
own `TruncationState`, which is always reported.

Client collapse is therefore **finite and reversible**, while runtime
truncation is authoritative and irreversible. One expansion state per entity
governs every expandable band of that entity, so a background settlement never
expands its body while leaving its failure reason clipped, and a pending
approval's runtime-published reason and validated arguments are inspectable in
full — from already-held state, before the user answers allow or deny. That is
disclosure, not a second approval gate: nothing requires a card to be opened
before it can be answered, and expanding never edits the arguments the runtime
validated.

Expansion state is kept in three sets, one per runtime identity domain
(`ToolCallId` for foreground cards, `ToolExecutionId` for background ones,
`InteractionId` for pending approvals), so equal wire strings in different
domains cannot alias. `/expand all` and `/expand none` cover all three;
`latest` stays scoped to the `ToolCallId` domain. Reasoning *visibility* is
likewise unrelated to the `reasoningProfile` / `reasoningEnabled` request
configuration, which only `model_set` changes — and neither is the catalog's
`defaultReasoningProfile`, which describes what a model offers rather than what
this session configured. `configured`, `effective`, and the attempt's frozen
model stay three separately displayed facts.

**The projection is not a second runtime state machine.** Two total functions
define the whole model — `replaceFromSnapshot(snapshot, cursor)` and
`reduce(state, event)` — and every field they write is copied from a
runtime-published value. The reducer decides where a fact goes in the render
tree, never what the fact is. Given a fresh authoritative snapshot the
complete meaningful UI state is reconstructable without any hidden local
conversation log and without client-side history inference.

**Resync** is loss of trust in the incremental projection, never a gap to
fill:

```text
resync_required -> snapshot_get -> replace the projection -> subscribe after
                                   the new cursor -> continue
```

**What the client must never do**, and does not: construct ModelAdapters or
provider HTTP clients, parse `models.json`, resolve credentials or endpoints,
build context engines or summarizers, register tools, read `SKILL.md`,
compose an Agent Status, infer a mailbox drain, execute a tool, or let a tool's
name or origin decide an execution semantic. Tool identity may choose a
*renderer*; it may never choose a lifecycle. Several of those are reachable
only through the canonical operations this client calls.

**Validation.** Most correctness is proven without a terminal, without
credentials, and without sleep-based races: scripted byte and record
sequences drive framing, RPC correlation and terminal settlement, projection
folding, the A -> B model invariant, and resync repair. A bounded integration
suite then drives the **real** `rustx` binary over the real stdio/JSONL
transport against the shared external provider emulator, exercising spawn,
initialize, subscribe, model and capability inspection, inbound submission,
streaming and commit, attempt settlement, resync, shutdown, stdin EOF, and
clean exit. The TUI owns no provider protocol: it launches the same
`test-support/fake-provider` process the Rust conformance suite uses, through
a launcher that knows only about process mechanics and the control API.

The layering is checkable rather than asserted: `@earendil-works/pi-tui` is
imported by four files — the app, the autocomplete provider, the model selector
component, and the transcript module's one type import — and every suite below
`ui/` runs without a terminal. Framing, RPC, presentation projection, session
lifecycle, the model invariant, tool correlation, the process owner, and the
real-binary integration never reach Pi at all. Replacing the terminal library
would leave every one of them valid.

Both jobs install Python 3.12 and uv for the provider emulator; the Rust job
additionally sets `RUSTX_REQUIRE_PROVIDER_EMULATOR=1`, so a missing toolchain
fails the pipeline instead of silently skipping the conformance suite.

CI runs the TUI as a separate job on the nvm LTS line, so the Rust suites
never depend on Node being present.

## 3. Dependency rule

Dependencies point inward.

```text
Interfaces / projections
        |
Runtime services
        |
Model / Tool / Skill implementations
        |
Context engine
        |
Agent kernel
        |
Domain and protocol types
```

Forbidden dependencies include:

```text
Agent kernel -> OpenAI SDK
Agent kernel -> Anthropic SDK
Agent kernel -> rmcp
Agent kernel -> database client
Agent kernel -> HTTP framework
Agent kernel -> control-plane schema
```

## 4. Message model

The canonical conversation model contains four message roles:

```text
SystemMessageBlock
UserMessageBlock
AssistantMessageBlock
ToolMessageBlock
```

Semantics:

- `SystemMessageBlock`: trusted instructions or runtime context.
- `UserMessageBlock`: inbound information supplied to the current agent. The source may be a human, another agent, the control plane, or an external system.
- `AssistantMessageBlock`: model output produced by the current agent.
- `ToolMessageBlock`: result of a tool call produced by the current agent.

Identity and provenance are metadata. Message role does not encode real-world identity.

The Message Ledger is append-only immutable canonical fact storage. The
Conversation Surface is the sole authority for active identity, order, and
visibility. `SurfaceRevision` is a stable reconstruction reference within
the single conversation lineage; later mutations never alter an earlier
revision. Normal projection uses current Surface identities and keyed Ledger
reads only, never full Ledger or Surface-history scans.

Provenance is implemented as typed runtime-owned metadata: `UserSource`
distinguishes human, agent, fleet, external-system, and runtime sources;
`SystemAuthority` distinguishes platform, agent, runtime, skill, and fleet
authority for system blocks. A runtime compaction summary is represented as a
`UserMessageBlock` with runtime provenance and `InboundKind::CompactionSummary`;
no fifth message role exists. Ordinary inbound messages carry their
persisted UTC instant on `UserMessageBlock.timestamp` (supplied by the
producer, never fabricated); derived compaction summaries carry `None`.

Agent-to-agent communication uses a durable mailbox model. A `send_message` tool result reports only whether delivery was durably accepted or rejected. The recipient later receives the content as a `UserMessageBlock`.

## 5. Turn model

A turn is:

```text
one model response
+ all tool calls emitted by that response
+ all corresponding tool results
```

Tool execution may be parallel or sequential. Runtime execution events may follow actual completion order, while canonical tool-result ordering follows the original tool-call order for deterministic context construction.

Inbound mailbox messages may arrive at any time but are injected only at safe turn boundaries.

## 6. Durability model

The runtime distinguishes execution events from conversation messages:

```text
RuntimeEvent = execution fact
MessageBlock = model-context fact
```

Runtime events are append-only. In production, a successful Event Journal
append must commit before the event is published to external subscribers. A
failed required terminal append publishes neither the terminal event nor a
synthetic replacement; the owning runtime reports the durable failure.

Partial model deltas are execution facts. A canonical `AssistantMessageBlock` is committed only when a complete model response has been assembled. The model plane communicates through the normalized `ModelEvent` streaming protocol, which is an adapter-to-kernel fact stream and is never inserted into the canonical conversation history; the agent kernel assembles one `AssistantMessageBlock` from it.

Normally exactly one terminal runtime event settles an attempt (see section
2.2). If its required append fails, the attempt has a typed settlement
candidate but no terminal Journal fact. Committed-message events reference
the message by identity only: canonical message content exists solely in the
Message Ledger, and the Event Journal records the commit fact (see section
2.5).

Persist-before-publish is the frozen event-publication invariant:

```text
generate RuntimeEvent
→ durably append / commit sequence
→ publish externally
```

It applies to every externally published runtime fact. Facts that reference a
Ledger body, Surface revision, or Request Snapshot use the shared
ConversationStore transaction so publication never outruns its authority.
Successful publication does not add the committed event to an attempt-local
trace: the observer is a live projection seam, while the durable Event
Journal remains the historical authority.

## 7. Recovery model

Runtime process memory is disposable.

> **Durability says what happened. Recovery classification says what can
> safely happen next.**

M8 answered *what durably happened*. M9a (Issue #12) answers *given exactly
what durably happened, what state is this conversation in after restart, and
what is safe to do next*. The governing invariant is:

> Recovery reconstructs what durably happened; it never invents success, never
> silently replays an ambiguous external side effect, and never regenerates
> historical request/context from current configuration.

and, three times over:

```text
exact historical reconstruction  !=  safe replay permission
started + outcome unknown        !=  safe retry
started + outcome known          !=  never externally started
```

The recovery evidence model keeps the **external execution lifecycle** and
the **canonical structure lifecycle** on separate axes. Only an attempt with
zero durable external-start evidence — no `ModelRequestStarted`, no
`ToolExecutionStarted`, ever — may be classified as the safe Class-B
continuation case; a crash/restart/recovery cycle never turns historical
external-start evidence into a later claim that no external work started.

### 7.1 Owner

Recovery **policy** is owned by `ConversationRuntime` (`src/runtime/recovery.rs`,
driven from `ConversationRuntime::new`) and consumes `ConversationStore`
evidence. The store exposes durable facts and semantic transactions; it never
decides whether an ambiguous request is replayable. No recovery policy lives in
the SQLite backend, the Runtime Client, a provider adapter, the mailbox, the
TUI, or a background producer.

Recovery runs **after** the tool-runtime ownership transfer and **before** the
runtime object exists. Both halves matter: a construction that loses the
ownership race must leave no trace, so it must never have reconciled anything;
and the claim's pristine-background-plane precondition is what proves a
durably-owned-but-unpublished background execution has no live in-process
record. Because no coordinator exists yet, no recovery SQLite work can ever run
under the admission mutex, and activation/admission cannot race an unfinished
reconciliation.

### 7.2 The four phases

```text
durable facts
    -> reconstruct   RecoveryEvidence::reconstruct  (read only)
    -> classify      RecoveryPlan::classify         (pure)
    -> reconcile     RecoveryPlan::reconcile        (atomic durable commits)
    -> recovered runtime state
    -> resume        ResumeDisposition              (a permission, not a replay)
```

- **Reconstruct** reads the durable Surface head and its active bodies,
  Pending Inbound, and a paged fold of the Event Journal. It commits nothing,
  invokes no provider or tool, fabricates no observation, and executes no
  context contributor.
- **Classify** is a pure function of that evidence. It never depends on
  wall-clock timing, current provider availability, current plugin/config
  state, whether a Runtime Client is attached, or a random retry decision.
- **Reconcile** commits the new recovery facts, each atomically.
- **Resume** is a typed permission the runtime consumes at activation.

### 7.3 Evidence sources

Startup may consume only rustX-owned durable authority:

| Source | Used for |
| --- | --- |
| Conversation Surface head + checkpoint metadata | the recovered active working set |
| Message Ledger (keyed reads) | active message bodies, canonical structure |
| Pending Inbound Inbox | accepted-but-unadopted work |
| Request Snapshots | exact historical request reconstruction |
| Event Journal (paged fold) | attempt/turn/model/tool/background lifecycle |

Historical truth is never reconstructed from a Runtime Client snapshot or
cache, TUI cards, current DSH state, current Skill discovery, current Agent
Status, current filesystem state, current `models.json`, a live
`ContextContributor` run, regenerated dynamic context, or old process-memory
registry contents. Current configuration configures **future** work only.

### 7.4 Classification matrix

| Class | Durable evidence | Recovery action | Resume |
| --- | --- | --- | --- |
| **A — not started** | no attempt fact at all; accepted Pending Inbound may exist | none | ordinary Pending Inbound admission |
| **B — admitted, no external start** | `AttemptStarted`, **no `ModelRequestStarted` ever**, **no `ToolExecutionStarted` ever** | one interrupted attempt terminal | the already-canonical turn may continue through a **new** attempt |
| **C — external start committed, outcome unknown** | `ModelRequestStarted` with no durable outcome, and/or `ToolExecutionStarted` with no durable outcome | canonical tool-turn repair, then one interrupted attempt terminal | blocked: recovery starts nothing |
| **D — durable terminal exists** | one terminal attempt fact | none (absorbing) | ordinary Pending Inbound admission |
| **E — external start committed, outcome durably known, settlement incomplete** | a `ModelRequestStarted` followed by `ModelRequestCompleted`/`ModelRequestFailed`, and/or `ToolExecutionStarted` followed by a durable outcome — with no attempt terminal | canonical tool-turn repair (exact durable result), then one interrupted attempt terminal | ordinary Pending Inbound admission; **no** automatic continuation, resend, or replay |

Class B is the **only** state whose meaning is "no external work started": it
requires durable proof that **zero** external-start commits ever occurred for
this attempt. A resolved outcome is not "never started" — the two facts live
on separate axes and never collapse:

```text
started + outcome known   !=  never started
canonical ToolResult committed  !=  historical ToolExecutionStarted erased
```

Per plane:

- **Model.** The request lifecycle is monotonic: `NeverStarted` →
  `StartedOutcomeUnknown` → `StartedOutcomeKnown`. `ModelRequestCompleted` or
  `ModelRequestFailed` never moves an attempt back to "no request started".
  `ModelRequestStarted` + no outcome means the provider may have received and
  executed the request: recovery reconstructs the exact provider-neutral
  request for diagnosis, classification, and audit — and performs **zero**
  automatic resend. A durably known request outcome (Class E) is preserved as
  a durable fact: the attempt settles honestly, but the canonical Assistant
  message never committed, so **no response body is fabricated** from
  `ModelRequestCompleted`, and **nothing is resent**. A durably **failed**
  request is never converted into a silent retry: M9a has no generic retry
  engine, and the historical failure stays durable. Request ambiguity and
  attempt settlement are different facts: the request outcome stays unknown
  while the attempt settles.
- **Foreground tools.** External execution history and canonical repair
  evidence are separate axes with separate owners. Each unanswered call on
  the current Surface is answered from durable evidence only: a durably
  known outcome is used verbatim; a started call with no outcome becomes
  `ToolExecutionStatus::Interrupted`; a call with no start evidence at all
  becomes `Cancelled { ParentCancelled }` because nothing external happened.
  A committed canonical `ToolResult` releases the call's detailed per-call
  repair evidence; the owning attempt's **bounded external summary**
  independently keeps proving the historical `ToolExecutionStarted`, so a
  crash between the repair commit and the attempt terminal can never
  reclassify an indeterminate attempt as Class B. Tool repair evidence is
  keyed by owning attempt **and** call id: the durable authority does not
  guarantee `ToolCallId` uniqueness across the conversation lifetime
  (providers mint call ids; only the active Surface rejects duplicates), so
  historical attempts can never alias the current unresolved call. No tool
  is re-executed. The missing siblings of one Assistant turn commit as one
  atomic batch in canonical model-call order, so no durable prefix of a
  sibling batch is ever observable.
- **Background.** A committed async background execution survives the starting
  *attempt*, not the *process*. A durably owned, never-published execution is
  terminalized as `BackgroundTerminalState::Interrupted` — never `Failed`,
  never relaunched — and its model-visible notification is published through
  the one Pending Inbound authority in the same atomic transition as the
  `BackgroundTerminalPublished` fact.

### 7.5 Recovery-generated durable transitions

| Transition | Before the commit | After the commit |
| --- | --- | --- |
| `append_canonical_batch_with_events` (tool-turn repair) | the turn is structurally incomplete; no recovered result exists | every issued call owns exactly one committed `ToolResult`; the turn can form a valid later model request |
| `append_event(AttemptFailed { RestartInterrupted })` | the attempt is durably non-terminal | the attempt is absorbing; a second reconciliation is refused by the durable lifecycle |
| `accept_inbound_with_event(terminal notification, BackgroundTerminalPublished)` | no model-visible terminal exists; recovery owns publication | the notification and the terminal fact both exist, exactly once |

Recovery-generated canonical facts carry **no** attempt or turn identity: they
are facts of the startup recovery phase, never retroactive claims about what
the dead attempt did. The one exception is the attempt terminal itself, which
must name the attempt whose lifecycle it closes.

If a reconciliation transaction fails, recovery fails closed: no fabricated
success is published, no runtime is constructed, and nothing is admitted as
though recovery had completed. The same applies to a durable authority that
holds two concurrently non-terminal attempts: that contradicts the
one-active-attempt admission invariant, so recovery reports it instead of
settling whichever attempt sorted first and silently leaving the other
unresolved.

### 7.6 Terminal uniqueness and repeated-restart idempotence

Terminal uniqueness is owned by the durable `lifecycle_state` table, never by
an in-memory flag. `attempt:{id}` and `background:{execution_id}` accept
exactly one terminal fact; a second is a typed `TerminalViolation`. After the
first successful recovery the classification is Class D with no unpublished
background work, so every later restart commits nothing and durable state stops
changing.

### 7.7 Identity recovery

Two process-local ordinals could otherwise collide with durable history after a
restart:

- `AttemptId` — allocated by `ConversationRuntime` as
  `AttemptId::for_conversation(conversation, n)`, an explicit bijection with a
  conversation-scoped ordinal. Recovery folds durable attempt facts back
  through `AttemptId::conversation_ordinal` and reseeds the allocator past
  every ordinal in durable authority. Independently, the Event Journal refuses
  a second `AttemptStarted` for one identity.
- `ToolExecutionId` (`exec_N`) — reseeded from the durable
  `BackgroundExecutionCommitted` facts before the runtime activates, while the
  background plane is provably pristine.

### 7.8 Pending Inbound across a restart

Still-pending stays pending with its exact `InboundSequence`, `MessageId`,
provenance, content, timestamp, and correlation. Already-adopted is a canonical
Ledger fact, is not pending, and is never re-adopted — identity, not content
equality, is the idempotency key. Finite watermark semantics are unchanged.
There is deliberately no separate "recovery queue": the durable Pending Inbound
Inbox *is* the queue of accepted-but-unadopted work, and an idle recovered
runtime admits it at activation with **zero** Runtime Client attachments.

### 7.9 Durability health after recovery

A successful recovery starts a fresh admission cycle. A previous process's
crash never poisons a runtime whose classification and reconciliation
succeeded, and an unresolved durable inconsistency is never silently converted
into a healthy state: it fails construction instead. Recovery failure and the
bounded live admission retry are distinct concepts; neither is overloaded into
the other.

### 7.10 Bounded working set

The evidence fold pages the Event Journal and retains only the *unresolved*
state. Reads are O(history); hot memory is O(unresolved work):

```text
recovery hot memory =
    O(nonterminal attempt summaries)      (at most one by the admission invariant)
  + O(canonical tool repairs outstanding) (only while a ToolResult is missing)
  + O(unpublished background executions)  (bounded by background policy)
  + O(active Surface attribution)         (bounded by the active working set)
```

A resolved entry is dropped the moment its resolving fact is read, so
complete Event Journal, Request Snapshot, and Ledger history are never
materialized as recovery state.

The tool plane is split across two owners on purpose:

- **Attempt-level external summary.** `AttemptEvidence` owns a bounded
  summary of the attempt's foreground-tool external history — did external
  execution happen, is any external outcome unknown, is any known. It
  survives the release of every detailed entry and is removed only by the
  attempt's own terminal.
- **Per-call repair evidence.** The repair map holds the exact
  `ToolExecutionResult` (or the honest unknown) needed to rebuild a missing
  canonical `ToolResult`, and only while that repair is outstanding. A
  committed `ToolMessageCommitted` releases the entry **whatever the owning
  attempt's terminal state**; absence from the map means "this call needs no
  further canonical repair". An attempt terminal alone never destroys a
  still-needed entry: the terminal-before-repair shape (Class D) keeps its
  per-call evidence until the canonical result commits.

So the retention rule is: detailed per-tool recovery evidence exists only
while that tool call may still require canonical repair; durable historical
external-start knowledge needed for attempt classification is represented
independently in bounded attempt-level state. A long attempt with 10,000
previously settled/canonicalized foreground tools retains zero detailed tool
results while its one bounded summary keeps classifying honestly.

### 7.11 Recovery-prefix invariant

> Every successfully committed prefix of recovery reconciliation is itself a
> valid, truth-preserving input to a subsequent recovery.

Reconciliation commits tool-turn repair, the attempt recovery terminal, and
background terminal publication as **separate** atomic transitions on
purpose; each is a useful semantic commit point. A crash between any two of
them must leave a durable state that the next startup classifies exactly as
truthfully as the first did. In particular, a `ToolMessageCommitted`
committed by a repair — with the attempt terminal still absent — keeps the
attempt's external-start evidence intact, so the next recovery still sees an
indeterminate (or known-outcome) attempt and never reclassifies it as
Class B.

### 7.12 Replay policy

`ToolReplayPolicy::Idempotent` remains metadata. M9a implements no replay
engine, no retry framework, no configurable recovery strategy, and no
user-selectable replay mode. The rule is unconditional in this slice: an
ambiguous tool/process side effect is never automatically replayed. The safe
default is to commit an interrupted/unknown tool result and let the model
decide what to do next.

## 8. Compatibility policy

Before 1.0, rustX intentionally does not preserve compatibility with previous runtimes or flawed abstractions. Breaking changes are preferred when they materially improve correctness, separation of concerns, or long-term maintainability.

# Architecture

## 1. Architectural objective

rustX is an execution kernel, not an agent application framework and not a control plane. Its responsibility is to execute an immutable runtime manifest, produce durable execution facts, and expose stable runtime-owned contracts to higher-level systems.

The architecture is layered so that external SDKs, storage backends, process managers, and UI protocols can change without rewriting the agent kernel.

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
                           ToolCallId, ToolExecutionId, ToolVersionId,
                           McpServerId, SkillId, SkillVersionId, ArtifactId)
                           and CapabilityRevision
runtime/cancellation.rs   CancellationSignal: the one runtime-owned
                           cancellation primitive shared by model adapters,
                           compaction, foreground tool execution, and
                           background work
runtime/types.rs           TokenMeasurement, TokenMeasurementSource,
                           CancellationReason, RuntimeError, RuntimeClock
runtime/inbound.rs         ConversationInboundMailbox (per-conversation
                           in-memory coordination contract): InboundSequence,
                           InboundItem, InboundBatch, MailboxError
runtime/continuation.rs   ProviderContinuationState boundary (OpenAI Responses
                           stored/stateless, Anthropic opaque state)
message/content.rs         TextBlock, ImageReference, FileReference
message/types.rs           MessageBlock (System/User/Agent/Tool), provenance
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
tools/executor.rs          ToolExecutor boundary, ToolExecutionContext,
                           ProgressReporter, ToolRegistry (validating
                           definition/executor registry), PreflightOutcome
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
tools/mcp/                MCP 2026-07-28 adapter: configured server runtime,
                           paginated discovery, list-change invalidation,
                           canonical calls, progress, cancellation
tools/python.rs           immutable ToolVersion discovery/publication,
                           PythonToolEnvironment materialization, and the
                           canonical Python executor
model/types.rs             ModelRequest, ModelUsage, ModelProtocol,
                           AgentStatusAttachment (the cross-layer
                           model-request attachment contract of the Agent
                           Status projection: the context plane produces it,
                           `ModelRequest` and adapters consume it, and model
                           contracts never depend on context implementation
                           modules)
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

Exactly one terminal runtime event settles an attempt, and each terminal
event carries only the data valid for that state:

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

### 2.3 Streaming assembly identity

`ModelEvent` (and the corresponding `RuntimeEvent` deltas) target content
blocks by the rustX-owned `ContentBlockIndex`: the position of the block
within the ordered `AgentContentBlock[]` of the message being assembled.
Interleaved text, reasoning, refusal, tool-call, and provider
continuation-state streaming therefore assembles unambiguously without
exposing any provider block id type. Refusal streams as refusal
(`RefusalDelta` / `AgentRefusalDelta`) and assembles into
`AgentContentBlock::Refusal`, never into plain text. `ToolCallStarted`
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
message content. `AgentMessageCommitted` and `ToolMessageCommitted` are
execution facts that reference the committed message by its stable
`MessageId` and never embed the message body, so the Event Journal never
holds a competing copy.

A committed-message event must not be emitted before the corresponding
`MessageBlock` has been durably committed to the Message Ledger. Message
Ledger persistence and Event Journal persistence are separate durable
operations unless a backend provides a shared atomic transaction; M8 owns
the atomicity or crash-reconciliation boundary between these stores. If a
crash occurs after the `MessageBlock` is durably committed but before the
corresponding committed-message event is appended, recovery must recognize
and reconcile that state rather than treating the message as absent or
duplicating its content.

Persist-before-publish applies to `RuntimeEvent` publication only: append
the event durably before publishing it externally. It does not by itself
provide a transaction with the Message Ledger.

### Layer 1: Agent kernel

The kernel owns deterministic execution semantics:

- Attempt state machine
- Turn lifecycle
- Model -> tool -> model loop
- Tool batch ordering
- Turn-boundary inbound message draining
- Attempt termination rules
- Retry and compaction decision points

The kernel operates only on rustX canonical types and interfaces.

The runtime inbound coordination contract (`src/runtime/inbound.rs`, Layer
0) is deliberately not part of the kernel: the conversation mailbox is a
conversation-owned in-memory queue shared by concurrent producers while the
kernel's `AgentExecution` consumes exactly one finite batch per safe turn
boundary. The mailbox is coordination only — canonical history is the
durable conversation truth and the Event Journal records execution facts —
and it is not a scheduler, supervisor, or persistent service layer.

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
ModelEventAssembler: stream validation + ordered AgentMessageBlock assembly
        |
ToolRegistry preflight: resolve -> extract -> strip -> validate -> dispatch
        |
deterministic scheduling phases (sequential barriers, parallel groups)
        |
RuntimeEvent trace, ending in exactly one terminal event
```

The loop owns execution semantics, message assembly, tool execution,
continuation state, cancellation observation, and the runtime event
trace. Adapters own provider protocol translation only; the validating
[`ToolRegistry`] pairs canonical [`ToolDefinition`] values with
[`ToolExecutor`] implementations and never falls back id-first.
Continuation state propagates losslessly without fabrication, cancellation
always settles as a terminal cancellation, and every attempt emits exactly
one terminal `RuntimeEvent`. See `docs/agent-loop.md` for the full
boundary description.

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

The context engine owns what the model sees:

- Context assembly
- Token accounting
- Context checkpoints
- Pi-style compaction
- Valid compaction cut-point selection
- Split-turn summaries
- Provider-context compilation

Compaction is a projection of durable conversation history. It must never delete or rewrite the canonical history.

#### M4 implementation (context engine)

The M4 implementation freezes the context-plane boundary in `src/context`
and its integration point in `src/agent/execution.rs`:

```text
canonical history
    ↓
ContextEngine (build_projection, plan_compaction, apply_compaction)
    ↓
ContextProjection { items, estimated_input, checkpoint_generation }
    ↓
compile_projection → canonical ModelRequest.messages
    ↓
ModelAdapter → provider
```

The engine is a deterministic pure function of (canonical history, latest
checkpoint, tool definitions, observed provider usage): the same inputs
always produce the same projection, plan, and estimate. It owns no provider
knowledge — token estimation is pluggable (`TokenEstimator`, with a default
`ceil(bytes / 4)` formula), and the engine holds no model catalog.

Its configuration is split by ownership. The session owns the static
`SessionContextPolicy`; the *window* comes from the attempt's immutable model
snapshot. `ContextRuntime::for_attempt` derives one engine per attempt from
those two inputs, so a session model change between attempts changes the next
attempt's compaction arithmetic and never the running one's.

Key contracts:

- `ContextProjection` is the model-visible projection; a projection-only
  `AgentSlice` (split-turn content) is materialized transiently under its
  source `MessageId` when compiled and is never persisted, never emitted as
  `AgentMessageCommitted`, and never returned in
  `AgentExecutionResult.messages`.
- `SystemMessageBlock` values are pinned: everything through the last
  system message stays literal and is outside summary coverage; summaries
  are `UserMessageBlock` values with `UserSource::Runtime` and
  `InboundKind::CompactionSummary` — no fifth message role exists.
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
  result; whole-turn boundaries are preferred, and oversized turns are
  split at complete content-block boundaries.
- Checkpoints (`ContextCheckpoint`) carry stable `MessageId`-based
  boundaries and deterministic summary ids; the `ContextCheckpointStore`
  abstraction (with an in-memory development/test implementation) is the M4
  persistence contract, M8 owns the durable backend.
- The `ContextSummarizer` service is provider-neutral; the production
  `ModelBackedSummarizer` issues a canonical one-off `ModelRequest` (no
  tools, no Agent Status, no Skill catalog, no continuation) through the
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
- Agent Status is the mandatory, provider-neutral, ephemeral projection of
  current runtime facts (temporal section with a narrow clock/timezone
  boundary, structured section providers with stable reserved ids, and a
  canonical deterministic renderer). It exists only while a
  `FreshInboundTurn` is pending, participates in the full token estimate
  and the projection fingerprint, is excluded from recent-conversation
  retention, and is protected from compaction until a successful model
  invocation observes it. Adapters own its wire placement.
- The cross-layer `AgentStatusAttachment` is a Layer 0 contract owned by
  `model/types.rs`: it holds only the request-level data adapters need (the
  target canonical `MessageId` and the rendered status text). The context
  plane (`src/context/status.rs`) *produces* the attachment, but `ModelRequest`
  uses only Layer 0 runtime-owned attachment data — `model` never depends on
  `context`. `ContextProjection`, `CompiledContext`, `ModelRequest`, token
  accounting, and every provider adapter refer to the Layer 0 type.
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

#### M6 implementation (Skill catalog projection)

The Skill catalog follows the Agent Status attachment pattern:

- The cross-layer `SkillCatalogAttachment` is a Layer 0 contract owned by
  `model/types.rs`: it holds only the rendered catalog text. The capability
  plane produces it from the attempt's immutable Skill snapshot;
  `ContextProjection`, `CompiledContext`, `ModelRequest`, fingerprinting,
  token accounting, and every provider adapter refer to the Layer 0 type.
- It is projection-only capability context: never canonical history,
  never checkpoint history, never returned in `AgentExecutionResult.messages`,
  and never emitted as a committed-message event. The existing
  `SystemAuthority::Skill` canonical variant does not justify durable
  Skill-catalog history and is not used for the catalog.
- It participates in the projection fingerprint, the full request token
  estimate, the hard-fit calculation, the soft compaction threshold, and
  the before/after compaction progress comparison (the exact attachment is
  carried by `CompactionPlan` and reused on both sides); it never counts
  toward `keep_recent_tokens`. A large catalog can therefore contribute to
  `CannotFit`.
- Provider adapters own wire placement: OpenAI Responses combines it
  deterministically with the canonical system instructions in the trusted
  `instructions` channel on every request (a continuation never loses it),
  Anthropic Messages places it in the top-level `system` content, and
  OpenAI Chat Completions translates it through a system-level message —
  never a user message. Agent Status remains a separate user-targeted
  ephemeral attachment with its existing semantics.

The M4 context path is **mandatory**: every `AgentExecution` is constructed
with a `ContextRuntime`, a `ConversationToolRuntime`, and an attempt
capability lease
(`AgentExecution::new(request, adapter, capability, cancellation,
context_runtime, tool_runtime)`); the no-context compatibility path,
`with_context_runtime`, and any capability-free constructor are gone, and
there is no Agent Status disable flag.
See `docs/context-engine.md` for the full boundary description.

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
once from a bounded `ConversationRuntimeConfig` that binds the mailbox,
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
never reclaim the detached execution. Cancellation intent that commits
first retains its reason and canonicalizes the final terminal result, so
the registry winner and the stored result always agree (only an explicit
process-control failure after cancellation intent settles as `Failed`).
All progress entering runtime state and events passes through one shared
UTF-8-safe bound (`bound_tool_progress`) used by both foreground and
background paths.

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

**The Bash invocation ownership boundary is its dedicated process group,
and membership is immutable for Bash descendants.** A Bash invocation
executes inside one fixed rustX-owned process group. Process-group/session
mutation from Bash descendants is rejected so the ownership boundary
cannot be escaped or partially hidden: the inner supervisor installs a
narrow inherited seccomp policy between its own `setsid()` setup and the
`/bin/bash` spawn that rejects `setsid(2)` and `setpgid(2)` with `EPERM`
(the only syscalls that can change process-group/session membership on
Linux; seccomp filters are inherited across `fork`/`exec` and can only
become more restrictive). A command such as `setsid sleep 30` fails
deterministically and nothing leaves the invocation group. The filter uses
syscall numbers defined by the compiled Linux target ABI. On x86-64 it
rejects x32 syscall execution because x32 shares the x86-64 audit
architecture while using a distinct syscall-number namespace. This
restriction is what makes the supervisor's kernel child-wait terminal
proof complete: an in-domain descendant cannot remain hidden behind an
ancestor that left the domain. Subreaper adoption is a reaping
implementation detail, not an ownership claim.

Bash process ownership is kernel-mediated and reuse-safe by construction:
each invocation owns a small supervisor process unit — an outer
supervisor (rustX child, subreaper, final containment and reaping
authority) plus an inner supervisor (session and group leader via
`setsid`, subreaper, `/bin/bash` parent) — and the shell's in-group
descendants live in exactly the invocation's own session/process group.
`TERM`/`KILL` are issued by the inner supervisor with `killpg` against its
own group, whose numeric id is its own pid — provably allocated while it
lives, so the numeric group id can never name a foreign process group
while signals remain legal. Shell descendants that outlive the shell are
reparented into the supervisor's child domain (`PR_SET_CHILD_SUBREAPER`)
rather than rediscovered from `/proc`; the terminal ownership point is the
kernel's **group-scoped wait** with one authoritative reporter — the outer
supervisor's `waitid(Id::PGid)` returning `ECHILD` (no child of the outer
remains in the invocation group, the inner anchor itself released by that
same wait strictly after any fallback containment signal), reported to
rustX as the canonical `AllChildrenReaped` over a `UnixStream` control
channel. `P_PGID` alone observes only the waiting process's children, not
arbitrary group members; its `ECHILD` is a complete whole-group terminal
proof only because membership is immutable — every in-group process other
than the inner supervisor is a bash descendant that can never leave the
group, and when the shell (or any in-group ancestor) exits, the kernel
reparents its in-group children directly into the nearest subreaper's
child domain (the inner supervisor while it lives, the outer supervisor
after it). A live group member keeps the group-scoped wait from returning
`ECHILD` in every reachable topology; there is no hidden-grandchild state.
`/proc` is never the source of truth for process ownership or quiescence,
and `killpg(..., 0)` probes are never the terminal point (an un-reaped
leader zombie keeps the numeric group observable).

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
has created the invocation session/group and installed seccomp. Protocol
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
Catastrophic fallback authority is a pre-ownership prerequisite: the runtime
child-subreaper primitive is consulted (once per process, idempotently)
before the supervisor unit spawns, so `START` — which authorizes the Bash
spawn — is never sent before rustX can own catastrophic containment.

Control-channel EOF is never a post-ownership process-terminal event. Normal
terminality linearizes at the outer's group-scoped `ECHILD` and its
`AllChildrenReaped` frame. For catastrophic loss of both supervisors, the
runtime process activates its child-subreaper capability — a
**process-level kernel coordination primitive** owned by the runtime
coordination layer (`src/runtime/process_supervision.rs`; lazy one-time,
idempotent, sticky activation; a failed activation fails every Bash
invocation as a pre-ownership setup failure; never toggled per
invocation). Enabling it changes process-wide orphan reparenting, but
kernel adoption does not by itself assign arbitrary adopted children to
Bash lifecycle ownership: in M5, Bash supervisor units are the only
production subprocess hierarchy relying on orphan adoption, and rustX
implements no generic unknown-child reaper. Catastrophic Bash
containment remains invocation-scoped — after reaping its
direct outer child, rustX retains the adopted inner zombie using
`waitid(WNOWAIT)`, issues `SIGKILL` to the still-anchored group, and
linearizes emergency terminality only at its own group-scoped `ECHILD`.
If the adopted anchor is unavailable (`ECHILD`) without a prior
authoritative terminal event, emergency containment reports
`AnchorUnavailable` — never terminal — and no `ToolExecutionResult`
commits. The anchor is retained, contained, and released only as one
coherent state machine: identity ownership, reaping ownership, signaling
authority, and terminal settlement are the same ownership. Thus EOF changes
communication state and failure intent, while process lifecycle remains
independently `PreOwnership`, `OwnershipPossible`/`Owned`, or `Terminal`.

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
`AgentMessageBlock` is owned by the future Agent Loop, never by M2, so no
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
Each candidate composes it with MCP definitions sorted by `McpServerId` and
remote name, then Python definitions sorted by canonical model-facing name.
The candidate owns a new `ToolRegistry`; a committed `CapabilitySnapshot`
owns that exact registry. A duplicate model-facing name rejects the complete
candidate.

`McpServerRuntime` owns one configured server's rmcp peer, transport, progress
dispatcher, list-change subscription, and supervised stdio owner when used.
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
installs the fixed-membership seccomp restriction, and issues
`TERM -> grace -> KILL` with `killpg` against its own process group; an
outer supervisor is the reaper of last resort with the single-owner anchor
discipline and the authoritative terminal report. The kernel-mediated
terminal proof is the group-scoped wait (`waitid(Id::PGid)` returning
`ECHILD`) — never a `/proc` scan or a `killpg(0)` probe. rustX's detached
driver task owns physical settlement from the moment the supervisor spawn
succeeds, drains the server's stderr until EOF (bounded preview), reaps the
direct supervisor child before publishing settlement, and runs the shared
adopted-anchor emergency containment when the unit is lost. Startup is
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
`tool-versions/<ToolVersionId>/source/` plus a version marker (the executor
and every uv command use exactly the `source/` root; reuse validates the
published source content digest against the claimed identity), validates
the existing `uv.lock`, and materializes a distinct immutable
`PythonToolEnvironmentDigest` environment whose ready marker locks every
deterministic identity input (format, OS, architecture, digest, lock digest,
Python runtime identity, uv identity). ToolVersion identity and environment
identity are separate: source/description/schema changes can change the
former without changing the latter, and each ToolVersion -> environment
binding is recorded deterministically outside the environment's immutable
dependency identity. The environment isolates dependencies, not filesystem,
network, or security permissions. The interpreter whose identity enters the
digest is pinned to uv via `UV_PYTHON`, managed Python downloads stay
disabled, and every preparation command has a finite deadline (a timeout is
an explicit preparation failure). A harness uses a private input file and
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
- Context checkpoint store interface
- Capability revision management
- Capability mutation guard
- Process supervision
- Background shell session management
- Crash reconciliation

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
(`RuntimeClientCursor`). Later transports (Issue #38 stdio JSONL, Issue
#36 WebSocket) wrap this semantic layer without redefining it, and a
future AG-UI adapter consumes this projection as its only source — there
is no second AG-UI interpretation path directly from internal runtime
events. The existing `src/protocol` boundary remains the compiled
`RuntimeManifest` protocol; the two protocols are not mixed.

Module ownership:

```text
runtime_client/types.rs        protocol version, cursor, attachment/request
                               ids, the typed request/response/event
                               envelope, method results, typed errors
runtime_client/event.rs        RuntimeClientEvent (external vocabulary)
runtime_client/snapshot.rs     RuntimeClientSnapshot read model
runtime_client/projection.rs   RuntimeClientProjection: the one
                               linearization owner (fold, cursor
                               allocation, bounded replay, subscribers)
runtime_client/host.rs         RuntimeClientHost: conversation
                               coordinator, canonical history between
                               attempts, current-attempt handle,
                               observer wiring, admission, shutdown
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

- **One linearization owner.** The host guards one state instance with
  one lock. Fold, cursor allocation, event publication, bounded replay
  retention, subscriber delivery, snapshot reads, attachment
  admission/detach, the current-attempt slot, canonical-history swap, and
  shutdown decisions all serialize through it, so snapshot/cursor,
  cancel-current, terminal settlement, and admission linearize by
  synchronization, never by timing.
- **One host per runtime identity.** One `ConversationToolRuntime` identity
  is bound to at most one `RuntimeClientHost` for that identity's lifetime.
  `RuntimeClientHost::new` claims a one-time binding on the tool runtime and
  on the capability coordinator; both are `Clone` and every clone shares one
  binding, so a cloned runtime bundle is not a second bindable identity. A
  second construction is rejected with
  `HostConstructionError::RuntimeClientAlreadyBound`.

  This is a runtime ownership invariant, not a caller convention. A host is
  the conversation coordinator over its runtime identity — canonical
  history, the current-attempt slot, the projection and its cursor domain,
  attachment state, and the inbound and attempt identity counters all live
  in one host — so two hosts would be two coordinators over one
  authoritative runtime. Each subsystem also carries exactly one observer
  slot, so the second host would silently unhook the first.

  Every fallible validation runs before the claim and every step after it is
  infallible, so the claim is the ownership-commit boundary and a rejected
  construction has no semantic side effect: no observer is replaced, no
  worker starts, and no mailbox, background, or capability state moves.
- **One conversation authority.** The `ConversationToolRuntime` owns the
  `ConversationId`, the canonical mailbox, the authoritative background
  registry, and the Runtime Client binding identity; the host *derives* its
  conversation identity from it. `RuntimeClientHostConfig` therefore has no
  conversation id field of its own — a second configured identity could
  disagree with the runtime, and a host that coordinates one runtime while
  naming another conversation would issue `AgentExecutionRequest`s the
  runtime rejects, after having already admitted the attempt. Structural
  absence removes that state instead of checking for it. The capability
  coordinator remains a separate authoritative identity, so it is still
  validated explicitly against the runtime before the binding claim.

  **Host lifetime is not attachment lifetime.** Reconnect replaces the
  attachment on the same host (detach, then a fresh `RuntimeClientEndpoint`
  `initialize` yielding a new `AttachmentId`); it never reconstructs the
  host. The binding is deliberately not released when the bound host is
  dropped: rebinding a surviving runtime bundle would require a recovery
  model for canonical history, pending mailbox projection, and cursor
  continuity that pre-M8 does not own. Recreating a host over the same
  runtime bundle is **not** supported v1 recovery — a new host requires a
  new `ConversationToolRuntime` identity. Observer installation on the
  mailbox, background registry, and capability coordinator is crate-private
  for the same reason: it is a runtime coordination seam, not a public
  extension point.
- **Ownership: observation edges are non-owning.** The graph is:

  ```text
  semantic owner ─────────► Arc<HostInner>
  (RuntimeClientHost and clones, RuntimeAttachment,
   RuntimeClientEndpoint, EventSubscription, a running attempt task)

  HostInner ──► authoritative subsystems (tool runtime, mailbox,
                capability coordinator)
            ──► projection state
            ──► Arc<PendingObservations>

  authoritative subsystem ──► Arc<HostObserver>
  HostObserver ─────────────► Weak<HostInner>

  observation worker ───────► Weak<HostInner>
                       ────► Arc<PendingObservations>
  ```

  Subsystem observer slots keep owning `Arc<dyn InboundObserver>` and
  friends; the concrete `HostObserver` is what became non-owning, so
  installing a seam cannot create the cycle
  `HostInner -> subsystem -> Arc<HostObserver> -> HostInner`. Each callback
  upgrades the weak handle and returns without publishing when the upgrade
  fails — the projection no longer exists, which is never an error for the
  subsystem. The observation worker likewise holds only a weak host handle
  plus the queue it waits on, and never a strong handle across an await.

  `HostInner` is therefore destroyed when its last semantic owner is
  released, not at process exit. `HostInner::drop` closes
  `PendingObservations`, which is the worker's terminal condition; teardown
  takes no host lock, joins nothing, and publishes nothing. A running
  attempt task is a deliberate *bounded* strong owner — an admitted attempt
  must reach settlement, and the task releases the host when it does.
  Attachment detach remains unrelated to host lifetime.
- **Lock order.** The graph is acyclic by construction:

  ```text
  HostState ──► ConversationInboundMailbox ──► PendingObservations
      └──────────────────────────────────────► PendingObservations
  ConversationBackgroundRegistry ────────────► PendingObservations
  CapabilityCoordinator ─────────────────────► PendingObservations
  ```

  `PendingObservations` is a leaf (one mutex over a `VecDeque` plus a
  `Notify`; it calls nothing). Every authoritative subsystem fires its
  observer *while holding its own lock*, so every such observer only
  appends an immutable observation to that leaf and wakes the host
  worker — no subsystem ever acquires `HostState`. The single downward
  edge `HostState -> mailbox` exists only in `admit_next_attempt`, which
  drains under the host lock so the drain fact, the history commits, and
  the attempt publication linearize together. Consequently an
  authoritative commit never waits on the host lock, and subscriber
  notification can never block authoritative runtime state. The
  `AgentExecutionObserver` callbacks apply directly under `HostState`;
  that adds no incoming edge because `AgentExecution` is owned by its
  attempt task and holds no lock when it observes. Every host lock
  acquisition drains the pending queue first, so queued observations fold
  in enqueue order.
- **Snapshot/cursor invariant.** `snapshot_get` returns `{ snapshot,
  cursor }` where the snapshot describes all Runtime Client state through
  cursor C, and a subscription after C observes every subsequently
  published event or fails explicitly with `resync_required`. This holds
  by construction (one boundary), not by luck.
- **RuntimeEvent mapping policy.** Every internal event is classified
  PROJECT / FOLD INTO CLIENT STATE ONLY / INTERNAL in the projection
  owner: attempt lifecycle/settlement, streaming output, tool-call
  assembly, and foreground/background tool lifecycle project; turn
  counting and final usage fold; model request mechanics and compaction
  mechanics stay internal. Internal `RuntimeEvent` evolution therefore
  cannot silently break Runtime Client Protocol v1.
- **Streaming repair.** The snapshot carries an in-flight agent output
  view (accumulated blocks) and foreground tool views keyed by the
  logical tool-call identity, so a client repairing after `resync`
  reconstructs every client-visible effect without duplicated or missing
  semantic output. Parallel physical completion never corrupts logical
  identities.
- **Bounded pre-M8 replay is the only retained backlog.** A finite
  in-memory ring (`RUNTIME_CLIENT_REPLAY_LIMIT_DEFAULT = 4096`,
  configurable) holds every retained event; expired or ahead-of-stream
  cursors fail with `resync_required`. There is no Event Journal, no
  persistence, and no crash-safe replay claim (M8 owns durability).
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
- **Current-attempt coordination.** The host owns the current-attempt
  slot and the exact `AgentCancellation` the attempt task runs against;
  `cancel_current_attempt` requests cancellation under the one boundary
  and its acceptance response is never terminal settlement (the Agent
  Loop owns settlement, observed asynchronously). The host does not own a
  second attempt state machine.
- **Canonical history: one owner at a time.** Ownership transfers, it is
  never shared:

  ```text
  idle        Host owns canonical history
  admission   the history is moved into the attempt's AgentExecution,
              which is the sole authority for committed history while the
              attempt runs (including safe-boundary mailbox drains)
  running     the Host never mutates a competing copy; asynchronous
              inbound stays mailbox-owned until the loop commits it, and
              RuntimeClientSnapshot.messages is projection only
  settlement  the execution's final `AgentExecutionResult.messages`
              becomes the Host's canonical history for the next
              idle/admission boundary
  ```

  The `debug_assert_eq!` at settlement is a sanity assertion on the
  projection mirror, not the mechanism that keeps two authorities
  coherent — there is only ever one.
- **Admission.** `submit_inbound` stamps runtime-owned metadata
  (identity, mailbox sequence, timestamp, provenance), enqueues into the
  authoritative mailbox, and starts an attempt when idle; while busy the
  message waits for the loop's safe-boundary drain. Success means
  accepted/admitted, never assistant-finished.
- **Mailbox diagnostics.** The projection mirrors enqueue/drain facts
  (pending items in `InboundSequence` order, latest drain watermark and
  count) from an observation seam fired at the mailbox linearization
  points; the mailbox observer queues observations (the host drains the
  mailbox under its own lock) and a worker task plus every lock
  acquisition applies them in total order. `RuntimeClientCursor` remains
  a distinct domain from `InboundSequence`; clients can never drain or
  mutate the mailbox. Background terminal notifications enqueue through
  the same semantic path as every other mailbox state.
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
  invoking each registered provider once. That one `AgentStatus` value
  fans out to both destinations: `render_agent_status` produces the
  canonical model-facing attachment, and the same value is handed to
  `observe_status` for the Runtime Client projection. The client path
  never calls `compose` again — not even through a cloned composer
  sharing the same clock and providers — and never parses the rendered
  prompt text to recover structure.
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
- **Shutdown vs detach.** `shutdown` accepts the narrow local-runtime
  shutdown (no further inbound admission, current attempt continues to
  settlement); it is not detach and not cancellation.

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
  owners, and no host lock is held across any transport await.
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
host lock that admitted the attempt; a client never supplies it and never
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
stream by the existing projection owner, under the same host lock that owns
attempt admission. There is no second event stream and no second cursor
domain.

### Layer 8: The local conversation runtime process (Issue #42)

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
        +--> context policy / checkpoint / status pieces
        |
RuntimeClientHost -> RuntimeClientEndpoint -> stdio JSONL (Issue #38)
```

`LocalConversationRuntime::compose` is the one Rust-side composition owner.
The governing invariant:

> One local runtime process owns one conversation session. That session owns
> one authoritative mutable session-model configuration, one
> `ConversationToolRuntime` identity, one `CapabilityCoordinator`, one context
> policy/checkpoint domain, and one `RuntimeClientHost`. Runtime Client
> attachments may come and go without replacing those semantic owners.

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
  "mcpServers": [],
  "nativeTools": {
    "bash": { "execution": "model_selectable", "concurrency": "sequential" }
  },
  "environment": { "RUSTX_PROJECT": "demo" }
}
```

This empty `mcpServers` field is intentionally startup-safe and is not a
canonical MCP connection-configuration example. The user-facing MCP shape is
being revised under [issue #46](https://github.com/Caismis/rustX/issues/46);
that issue owns the named-map connection contract and protocol negotiation.

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
`shutdown` keeps its Issue #38 behaviour: it responds, stops admitting
inbound work, lets the current attempt settle, and does **not** close the
transport — a controlling client closes it according to its own lifecycle
policy. Transport EOF remains a detach, never an Agent Loop cancellation
primitive, and no M9 recovery or quiescence exists here.

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

RustxTuiApp             the Pi components.
```

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
compose an Agent Status, infer a mailbox drain, execute a tool, or branch on a
tool's name or origin for anything but a label. Several of those are reachable
only through the canonical operations this client calls.

**Validation.** Most correctness is proven without a terminal, without
credentials, and without sleep-based races: scripted byte and record
sequences drive framing, RPC correlation and terminal settlement, projection
folding, the A -> B model invariant, and resync repair. A bounded integration
suite then drives the **real** `rustx` binary over the real stdio/JSONL
transport against a local SSE provider fixture, exercising spawn, initialize,
subscribe, model and capability inspection, inbound submission, streaming and
commit, attempt settlement, resync, shutdown, stdin EOF, and clean exit.

The layering is checkable rather than asserted: `@earendil-works/pi-tui` is
imported by exactly two files, and eight of the nine client suites — framing,
RPC, presentation projection, session lifecycle, the model invariant,
rendering, the process owner, and the real-binary integration — never reach it
directly or transitively. Replacing the terminal library would leave every one
of them valid.

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

The canonical conversation model contains four message classes:

```text
SystemMessageBlock
UserMessageBlock
AgentMessageBlock
ToolMessageBlock
```

Semantics:

- `SystemMessageBlock`: trusted instructions or runtime context.
- `UserMessageBlock`: inbound information supplied to the current agent. The source may be a human, another agent, the control plane, or an external system.
- `AgentMessageBlock`: model output produced by the current agent.
- `ToolMessageBlock`: result of a tool call produced by the current agent.

Identity and provenance are metadata. Message role does not encode real-world identity.

Provenance is implemented as typed runtime-owned metadata: `UserSource`
distinguishes human, agent, fleet, external-system, and runtime sources;
`SystemAuthority` distinguishes platform, agent, runtime, skill, and fleet
authority for system blocks. A future compaction summary is represented as a
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

Runtime events are append-only. In production, events must be persisted before being published to external subscribers.

Partial model deltas are execution facts. A canonical `AgentMessageBlock` is committed only when a complete model response has been assembled. The model plane communicates through the normalized `ModelEvent` streaming protocol, which is an adapter-to-kernel fact stream and is never inserted into the canonical conversation history; the agent kernel assembles one `AgentMessageBlock` from it.

Exactly one terminal runtime event settles an attempt (see section 2.2). Committed-message events reference the message by identity only: canonical message content exists solely in the Message Ledger, and the Event Journal records the commit fact (see section 2.5).

Persist-before-publish is the frozen event-publication invariant:

```text
generate RuntimeEvent
→ durably append / commit sequence
→ publish externally
```

It applies to `RuntimeEvent` publication only and does not by itself provide
a transaction with the Message Ledger; cross-store atomicity or crash
reconciliation between the Message Ledger and Event Journal is owned by M8.

## 7. Recovery model

Runtime process memory is disposable.

Recovery uses durable state:

- committed message blocks
- runtime events
- capability revision
- context checkpoints
- workspace state

An unresolved tool call after a crash is never automatically replayed unless the tool explicitly declares an idempotent replay policy. The safe default is to commit an interrupted/unknown tool result and allow the model to decide what to do next.

## 8. Compatibility policy

Before 1.0, rustX intentionally does not preserve compatibility with previous runtimes or flawed abstractions. Breaking changes are preferred when they materially improve correctness, separation of concerns, or long-term maintainability.

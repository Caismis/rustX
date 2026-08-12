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
runtime/types.rs           CancellationReason, RuntimeError, RuntimeClock
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
                           input boundary, and mod.rs only composes the
                           known native tools
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
                           ReasoningEffort, AgentStatusAttachment (the
                           cross-layer model-request attachment contract of
                           the Agent Status projection: the context plane
                           produces it, `ModelRequest` and adapters consume
                           it, and model contracts never depend on context
                           implementation modules)
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

`ContextManifest` gained `context_window_tokens` in M4. The model context
window is runtime-owned configuration, never a hard-coded per-model catalog
in the context engine; the soft input limit is
`context_window_tokens - reserve_tokens - max_output_tokens` (checked,
impossible configurations rejected). This is an additive pre-1.0 contract
change: the M1 manifest fixture and round-trip tests were updated
accordingly.

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
knowledge — the window/reserve/recent-token configuration is runtime-owned
(`ContextConfig`, mirrored additively into the M1 `ContextManifest`), token
estimation is pluggable (`TokenEstimator`, with a default
`ceil(bytes / 4)` formula), and no model name catalog exists.

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
  tools, no continuation) through the existing `ModelAdapter` boundary.
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
omission, so `{"timeout_ms": null}` is a business argument violation
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
- current request semantics (adaptive thinking via `thinking.type`,
  effort via `output_config.effort` — never `thinking.display`;
  `redacted_thinking.data` preserved losslessly as opaque provider state);
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
```

- **One linearization owner.** The host guards one state instance with
  one lock. Fold, cursor allocation, event publication, bounded replay
  retention, subscriber delivery, snapshot reads, attachment
  admission/detach, the current-attempt slot, canonical-history swap, and
  shutdown decisions all serialize through it, so snapshot/cursor,
  cancel-current, terminal settlement, and admission linearize by
  synchronization, never by timing.
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
- **Bounded pre-M8 replay.** A finite in-memory ring
  (`RUNTIME_CLIENT_REPLAY_LIMIT_DEFAULT = 4096`, configurable) serves
  resume after retained cursors; expired or ahead-of-stream cursors fail
  with `resync_required`. There is no Event Journal, no persistence, and
  no crash-safe replay claim (M8 owns durability).
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
- **Canonical history.** Between attempts the host owns canonical
  conversation history; the loop owns its working copy during an
  attempt; at settlement the host replaces its history with the
  authoritative `AgentExecutionResult.messages` under the one boundary
  and verifies the projection mirror. The projection mirror is never an
  independently mutable history.
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
- **Agent Status projection.** The loop observes the exact composed
  `AgentStatus` (one clock sample, one provider invocation set) at
  composition time; the client view carries the structured sections plus
  the canonical rendered representation derived from the same
  composition. The client never triggers a second composition and never
  parses the rendered prompt text to recover structure.
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

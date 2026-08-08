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
                           ToolCallId, ToolVersionId, McpServerId, SkillId,
                           SkillVersionId, ArtifactId) and CapabilityRevision
runtime/types.rs           CancellationReason, RuntimeError
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
tools/types.rs             ToolDefinition, ToolCall, ToolCallStart,
                           ToolExecutionResult, ToolExecutionStatus,
                           ToolExecutionMode, ToolReplayPolicy, ToolOrigin,
                           TruncationState
model/types.rs             ModelRequest, ModelUsage, ModelProtocol,
                           ReasoningEffort
model/finish.rs            ModelFinishReason
model/error.rs             ModelError, ModelErrorKind
model/event.rs             ModelEvent (adapter-to-kernel streaming protocol)
events/types.rs            RuntimeEventEnvelope, RuntimeEvent, AttemptOutcome,
                           AttemptFailure
protocol/manifest.rs       RuntimeManifest and capability/context/limit sections
model/adapter/traits.rs    ModelAdapter runtime-owned interface, ModelEventStream
model/adapter/cancellation.rs  ModelCancellation (rustX-owned cancellation)
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
tools    → runtime
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
the tool execution contract in `src/tools/executor.rs`:

```text
canonical input state
        |
ModelAdapter (canonical ModelRequest in, ModelEvent stream out)
        |
ExecutionStateMachine: Idle -> RunningModel -> WaitingForTool -> RunningModel -> Completed
        |
ModelEventAssembler: stream validation + ordered AgentMessageBlock assembly
        |
ToolRegistry: deterministic call resolution and Tool::execute
        |
RuntimeEvent trace, ending in exactly one terminal event
```

The loop owns execution semantics, message assembly, tool execution,
continuation state, cancellation observation, and the runtime event
trace. Adapters own provider protocol translation only, and tools own
their definitions and single-call execution. Tool calls of one turn
execute in deterministic block order with no hidden concurrency or retry,
continuation state propagates losslessly without fabrication, cancellation
always settles as a terminal cancellation, and every attempt emits exactly
one terminal `RuntimeEvent`. See `docs/agent-loop.md` for the full
boundary description.

The M3 test suite drives the loop with scripted fixture models and tools
(`tests/common/fake.rs`), asserts behavior through the recorded
`RuntimeEvent` trace and the platform `AttemptOutcome`, and reconstructs
execution phases from traces (`tests/common/mod.rs`).

The Issue #22 inbound batching integration is additive:
`AgentExecution::with_inbound_mailbox` attaches the conversation mailbox
(rejecting a mismatched conversation), and at every safe turn boundary the
loop performs exactly one finite watermark-bounded drain and appends every
drained message as its own canonical `UserMessageBlock` before the next
model request. Mailbox attachment adds the safe-boundary
cancellation-before-selection rule; observable cancellation before every
model turn is a generic Agent Loop invariant for all executions. See
`docs/agent-loop.md` section 9 for the full boundary description.

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
- A fresh inbound turn that has not been observed may never be compacted
  away; when preserving it makes the projection impossible, planning fails
  with `CannotFit` rather than summarizing the unobserved instruction.

The M4 context path is **mandatory**: every `AgentExecution` is constructed
with a `ContextRuntime` (`AgentExecution::new(request, adapter, tools,
cancellation, context_runtime)`); the no-context compatibility path and
`with_context_runtime` are gone, and there is no Agent Status disable flag.
See `docs/context-engine.md` for the full boundary description.

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
- Cancellation is a rustX-owned signal (`ModelCancellation`) flowing through
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

### Layer 5: Skill plane

Skills are filesystem/workflow packages. A skill may include:

- `SKILL.md`
- scripts
- references
- assets
- Python dependency declarations
- Node dependency declarations

All active skills in one conversation share one Python environment and one Node environment. Skills use the same native Bash execution capability as the agent.

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

### Layer 7: Interfaces and projections

The outermost layer exposes the runtime to humans and other systems:

- Local interactive CLI
- Runtime command interface
- HTTP control interface
- Runtime event streaming
- AG-UI projection

AG-UI is an output projection, not the internal durable event model.

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

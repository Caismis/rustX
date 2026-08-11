# Agent Loop (M3 + Issue #22 inbound batching)

This document describes the runtime boundary implemented by the M3
deterministic agent loop, mirroring the M2 model-plane documentation in
`docs/architecture.md`, including the Issue #22 conversation inbound
mailbox integration.

## 1. What the Agent Loop owns

The loop (`src/agent`) executes one attempt to its single terminal outcome:

- attempt lifecycle (`AttemptStarted` → exactly one terminal event)
- turn lifecycle (one model response plus its tool calls and results)
- canonical `ModelEvent` stream consumption, validation, and message assembly
- tool resolution and tool execution (in deterministic block order)
- canonical continuation state retention and propagation
- safe-boundary inbound mailbox consumption (one finite drain per boundary)
- cancellation observation and terminal cancellation outcome
- the recorded `RuntimeEvent` trace
- the committed in-memory conversation state of the attempt
- the pending fresh inbound trigger lifecycle (`FreshInboundTurn`) and its
  composition into exactly one Agent Status snapshot per request preparation

Execution semantics are explicit: an `ExecutionStateMachine`
(`Idle → RunningModel → WaitingForTool → RunningModel → Completed`, with
failure and cancellation settling from any active state) enforces that
tools run only after the model requested them and that the model continues
only after the requested tool calls completed. The machine is the
settlement authority: it settles (`complete()` for success, `fail()` for
failure and cancellation) immediately before the single attempt terminal
`RuntimeEvent` is emitted, so the terminal event and the terminal
execution state always represent the same settlement boundary.

## 2. What provider adapters own

Adapters own provider protocol translation only: one canonical
`ModelRequest` in, one canonical `ModelEvent` stream out. They never
execute tools, never decide attempt outcomes, and never emit
`RuntimeEvent` values. Provider SDK and wire types terminate inside the
adapter modules. The loop never branches on a provider protocol.

## 3. What tools own

The M5 tool plane replaces the provisional M3 `Tool` trait with the
canonical boundary in `src/tools/executor.rs`:

- The validating [`ToolRegistry`] pairs one canonical `ToolDefinition` with
  one `Arc<dyn ToolExecutor>`; an executor object does not own its
  definition, so one implementation may serve many registrations.
- A `ToolExecutor` executes an already-resolved, already-validated
  `ToolInvocation` (call id, tool id, model-facing name, resolved
  foreground/background mode, and the stripped business arguments) inside a
  `ToolExecutionContext` that carries conversation identity, the runtime
  `CancellationSignal`, the workspace boundary, the progress reporter, the
  artifact store, and the explicit authorized environment.
- The loop preflights every model-issued call **before** the agent
  tool-call message is committed: registry identity resolution,
  execution-policy resolution, reserved-metadata extraction, and business
  argument validation against the canonical JSON Schema. An impossible
  identity mismatch or an unregistered tool is a runtime/model-stream
  contract failure and the agent message is never committed; a business
  schema violation is a normal failed result slot and the executor never
  runs.
- The loop records the returned result verbatim and feeds it back to the
  model inside a `ToolMessageBlock`; it never fabricates, modifies, or
  reinterprets a result.

A failing tool is a normal outcome: the failed `ToolExecutionResult` is
passed back to the model, which decides the next action. Cancellable
native foreground work observes the attempt's `CancellationSignal` in its
context and physically settles (for example Bash terminates its owned
process group); the loop never drops a pending tool future and leaves
external work running.

## 4. Continuation

Continuation is canonical conversation state: the loop retains the full
committed history and appends each completed agent message and tool
result. The next model request carries the opaque
`ProviderContinuationState` boundary state reported by the previous turn
(the state of the greatest-block-index reasoning block, propagated
verbatim). Protocols without reconstructable state simply carry `None` —
nothing is fabricated, and a model that cannot continue without state
fails explicitly.

The M4 context path is mandatory: every model request carries the *context
projection* of the committed history (pinned system prefix, checkpoint
summary, retained suffix) plus the ephemeral Agent Status attachment of the
pending fresh inbound turn (when one exists), instead of the raw committed
history, and the projection is what continuation state refers to. A
successful compaction establishes a new context boundary and therefore
invalidates the pending continuation; the M4 context engine enforces that
the continuation-owning turn is retired completely, so an old opaque
provider continuation is never paired with a new projection. See
`docs/context-engine.md` sections 14 and 18-19. An ordinary inbound drain
and an Agent Status attachment never clear the pending continuation.

## 4.1 Fresh inbound lifecycle

Fresh inbound identity is explicit execution state, never inferred from
message role or history shape, and the first-turn execution mode is an
explicit trigger, never an `Option` used as a status switch:

```rust
pub enum InitialTurnTrigger {
    FreshInbound(FreshInboundTurn),
    Continuation,
}
```

`AgentExecutionRequest` carries exactly one `initial_turn_trigger`:

- `FreshInbound(fresh)`: the model has not yet observed the referenced
  inbound turn. Validation against canonical history is mandatory (including
  strictly increasing canonical order — the runtime never sorts or
  reinterprets caller-supplied order), Agent Status is mandatory, and
  fresh-inbound compaction protection applies. The trigger stays pending
  until one successful model invocation observes it: a provider
  `ContextWindowExceeded` overflow does not consume it, while a successful
  `ToolCalls` response does.
- `Continuation`: there is intentionally no new inbound user turn for the
  first model invocation, so no Agent Status is attached. This is the
  explicit expression of a pure continuation, never a configuration switch
  for disabling status on inbound messages.

There is no `disable_status`, no optional status mode, and no legacy
no-context execution path: Agent Status can never be silently suppressed by
omitting an optional field.

The first successfully completed model invocation consumes the fresh
trigger (including a successful `ToolCalls` response: the model has already
observed the turn). A safe-boundary mailbox drain appends the whole batch to
canonical history and establishes one new `FreshInboundTurn` from the
drained ids in sequence order. The next model request composes exactly one
Agent Status snapshot targeting the final fresh inbound message; a
`ContextWindowExceeded` overflow does not consume the trigger, and the retry
composes a freshly sampled snapshot. A foreground-tool-only continuation
with no new drain carries no Agent Status. A failure while composing or
preparing that status is a context preparation failure
(`AttemptFailed(Runtime(ContextPreparationFailed))`), never a compaction
failure.

## 5. Usage

`ModelRequestCompleted.usage` reports the canonical final usage of the
turn: the terminal `Completed.usage` when present, otherwise the latest
`UsageUpdate`, otherwise `None`. Cumulative snapshots are never summed and
missing counters are never fabricated.

## 6. Terminal state guarantee

Exactly one terminal `RuntimeEvent` settles an attempt:
`AttemptCompleted`, `AttemptCancelled`, or `AttemptFailed`. The terminal
event is always the last recorded event, the platform outcome maps
one-to-one from it (`AttemptOutcome::from_terminal_event`), and the loop
structure makes later events impossible. The execution state machine is
settled immediately before the terminal event is emitted, so the machine's
terminal state (`Completed` or `Failed`) and the terminal event describe
the same settlement.

## 7. Cancellation

Cancellation is observed at deterministic check points (before each model
event, between tool calls, and before every model turn begins — the first
turn and every continuation) and races every tool execution: the loop
`select`s between the tool future and the attempt cancellation signal
(biased toward cancellation, so cancellation wins deterministically once
observable). Every model invocation observes a child signal of the
attempt signal through the shared runtime `CancellationSignal`, so an
in-flight generation terminates with `Failed(Cancelled)` and is converted
into `AttemptCancelled`. Cancellation is always terminal failure — never
completion.

M5 strengthens the cancellation settlement of a committed tool-call batch
(see section 7.1): the loop never drops a pending tool future while
external work keeps running. In-flight cancellable foreground executions
observe the attempt signal in their execution context and physically
settle; unstarted calls receive cancelled result slots; and the complete
result batch commits in original model call order before the attempt
settles cancelled exactly once.

### 7.1 Tool-call batch scheduling and structural settlement

Every valid committed agent tool-call message is preflighted before commit
(see section 3). Once committed, its entire tool-result batch is settled
structurally exactly once:

- Result slots are preallocated in model call order, so completion timing
  can never influence message identities or canonical ordering.
- Scheduling interprets `ToolConcurrencyPolicy` per registered tool: a
  `Sequential` invocation is an exclusive barrier; adjacent `Parallel`
  invocations execute concurrently as one group (`P P S` becomes a
  parallel group followed by a sequential barrier).
- A background call is settled for the originating attempt when its
  background dispatch is accepted (`exec_N` + `state: starting`), never
  when the detached work terminates; a sequential background call blocks
  later scheduling only through its dispatch-acceptance point.
- Foreground executions receive the attempt's `CancellationSignal` in
  their context. When attempt cancellation wins during a batch:
  in-flight cancellable foreground work physically settles, unstarted
  calls receive cancelled result slots, committed background executions
  stay conversation-owned, prepared-but-uncommitted dispatches roll back,
  the complete batch commits in call order, and no next model turn starts.
- Physical completion order may differ from canonical order; canonical
  `ToolMessageBlock` values and the next model request always observe
  model call order.
- After the structurally complete batch commits, the attempt settles
  cancelled exactly once with one terminal event last.

### Generic Agent Loop cancellation

Observable cancellation is checked **before every model turn begins**, for
every execution, regardless of mailbox attachment, mailbox contents, or
provider protocol. This is an intentional pre-1.0 Agent Loop contract
refinement, not a mailbox feature: the mailbox only adds its own
safe-boundary selection rule (below); it does not control generic
cancellation timing.

When cancellation wins at the generic checkpoint:

```text
no TurnStarted
no ModelRequestStarted
no adapter invocation
AttemptCancelled
```

The checkpoint applies before the first model turn, before a continuation
after a foreground tool turn, and before a continuation caused by a drained
inbound batch. It never replaces a terminal outcome already selected at a
mailbox safe boundary: a successful no-tool turn whose empty mailbox
snapshot settled the attempt as `Completed` settles normally, and a later
cancellation or enqueue never reopens or reclassifies that completed
attempt. Likewise, with no mailbox attached, a successful no-tool turn
settles directly because no next model turn is being started.

## 8. Deterministic execution

Given identical model events, identical tools, and identical input, the
loop produces an identical ordered `RuntimeEvent` stream and an identical
terminal outcome: the trace is a pure function of the attempt request, the
model stream, the tool results, the cancellation signal, the pinned
capability snapshot, and the mailbox state observed at each safe boundary.
Tool calls of one turn execute in block order; there is no hidden
concurrency, no hidden retry, and no hidden state.

### 8.1 Attempt capability lease (M6)

One `AgentExecution` structurally holds one RAII attempt capability lease
for its complete lifetime (`AgentExecution::new(..., capability, ...)`);
there is no capability-free constructor. Every model/tool cycle inside the
attempt uses exactly the pinned immutable `CapabilitySnapshot`:

- the ToolRegistry handle (preflight, executor resolution, model
  definitions);
- the Skill catalog attachment on every `ModelRequest` (identical on
  every turn);
- the effective `ToolEnvironment` for foreground executions;
- the effective environment captured into every background dispatch at
  `prepare_dispatch`, before the background ownership commit.

No model turn re-discovers Skills or re-queries the conversation capability
pointer. A capability commit while the attempt lease is active is rejected
as busy; the lease is moved into `AgentExecution` and releases when the
consumed execution is dropped after settlement (or when construction fails).
The lease owner is structurally bound to the `ConversationId` and canonical
Workspace root of the corresponding `ConversationToolRuntime`; a mismatch is
rejected before any model request or tool execution begins.

## 9. Conversation inbound mailbox (Issue #22)

The conversation inbound mailbox (`src/runtime/inbound.rs`) is a narrow
runtime-owned coordination contract: a per-conversation in-memory queue for
asynchronous user-role messages arriving while an attempt is running. One
conversation owns one canonical mailbox, held by the conversation tool
runtime (`ConversationToolRuntime`); the loop drains exactly
`tool_runtime.mailbox()` at every safe boundary, so background terminal
notifications always enter the same mailbox the Agent Loop drains, and no
second mailbox injection API can split the ordering domain. An
`AgentExecution` whose request conversation differs from the tool runtime's
conversation is rejected structurally at construction. The mailbox adds the
safe-boundary rules below; generic cancellation timing is unchanged.

Ownership model:

```text
mailbox          = coordination
canonical history = durable truth
Event Journal    = execution facts
```

- The mailbox owns one shared inbound sequence domain
  (`InboundSequence`). The first successful enqueue receives `1`; sequences
  advance strictly monotonically with checked arithmetic and never come
  from the Event Journal sequence. Sequence allocation and publication into
  the pending queue happen under one mailbox lock, so no
  allocated-but-unpublished sequence is ever visible to a drain. Enqueue
  accepts only ordinary messages (`InboundKind::Message`) carrying their
  persisted UTC timestamp; a runtime compaction summary is derived history,
  not new asynchronous work, and is rejected, as is an ordinary message
  without a timestamp. Human, Runtime, Agent, Fleet, and ExternalSystem
  producers share the one sequence domain.
- At a safe boundary the loop performs exactly one finite drain
  (`InboundBatch`): under the mailbox lock the watermark is established as
  the highest sequence present, exactly the pending items through that
  watermark are detached, and one non-empty batch is returned (an empty
  mailbox returns `None`). A post-watermark arrival waits for the next
  drain; the boundary never re-inspects the queue for newly arriving items.
- Every item of the selected batch is appended as its own canonical
  `UserMessageBlock` in inbound sequence order before the next model
  request. Messages remain separate blocks — never concatenated and never
  an intermediate single-message request. Once the complete batch is
  appended to canonical history it is consumed from the mailbox and is
  never requeued; canonical history carries it forward even if the attempt
  later fails before the model observes it.
- The whole drained batch becomes one `FreshInboundTurn`, so the next model
  request receives exactly one Agent Status snapshot. `inbound_message_time`
  is the persisted timestamp of the final batch item in inbound sequence
  order (the highest-sequence item), never `min`/`max` of producer wall
  clocks, the drain time, or current time; producer timestamps may be
  non-monotonic due to clock skew.

### Safe boundaries

A safe boundary occurs only after the current turn is structurally complete
(`TurnCompleted`), never between tool results of one turn:

```text
assistant tool call(s)
  ↓
execute every foreground call in existing deterministic order
  ↓
commit every ToolMessage
  ↓
state.tools_finished()
  ↓
TurnCompleted
  ↓
SAFE BOUNDARY / exactly one mailbox snapshot
```

The frozen event/history boundary is: complete the turn, emit the existing
`TurnCompleted`, snapshot/drain the mailbox, append the drained inbound
messages, then the next normal `TurnStarted` from the next model turn. No
synthetic `TurnStarted` is invented for the drain, and drained messages
never retroactively join the preceding model/tool turn.

### Stop with pending inbound

A successfully assembled model turn with no tool calls does **not**
settle the attempt immediately. After `ModelRequestCompleted`,
`AgentMessage` commit, and `TurnCompleted`, the safe boundary snapshots the
mailbox:

```text
mailbox empty → existing successful Terminal::Completed → attempt settles
mailbox batch → append the complete batch → do NOT settle → next model turn
```

The final successful model turn's finish reason remains the finish reason
carried by the eventual `AttemptCompleted`. After a tool turn the attempt
already needs another model turn: an empty mailbox takes the existing
tool-result continuation directly (no synthetic user message), and a
present batch is appended before the continuation so the next request sees
tool results plus the inbound messages.

### Cancellation and failure ownership

Attempt cancellation is separate from mailbox ownership, and mailbox
attachment is separate from generic cancellation timing. The mailbox adds
exactly one cancellation rule of its own — **cancellation before selection**:
at a safe boundary, if cancellation is already observable before batch
selection, no drain happens, all pending items stay in the mailbox, and the
attempt settles cancelled. Once a batch has been atomically drained it is
appended synchronously in full — never partially consumed and never requeued
merely because cancellation becomes observable afterwards; the batch stays
canonical exactly once, the mailbox no longer contains it, and the generic
pre-next-turn checkpoint prevents any further model turn. A successful
no-tool turn whose empty snapshot already settled the attempt as
`Completed` is never reopened or reclassified by a later enqueue or
cancellation. Terminal failures (model request failure, unknown tool,
malformed stream, cancellation before the boundary) settle directly without
draining: pending items remain in the conversation mailbox for later
conversation processing. Idle attempt creation for such later messages is
not implemented in Issue #22.

### Continuation and compaction

An ordinary inbound drain does not clear the pending provider continuation.
The existing M4 compaction rule remains the only continuation invalidation
boundary: a successful compaction caused by a drained batch still retires
the continuation-owning turn and clears the continuation. A drained batch
enters canonical history before the next M4 projection/compaction, so the
request corresponding to a selected batch always contains that batch.

## 10. Unsupported behavior (non-goals)

The M3 loop does not implement: multi-agent execution, agent delegation,
long-term memory, RAG, provider fallback routing or load balancing,
workflow/DAG engines, scheduling, distributed execution, a persistent
event store, plugin marketplaces, conversation summarization, parallel tool
scheduling, idle-conversation attempt scheduling, mailbox persistence, or
background execution. Anthropic server-side fallback blocks remain
unsupported at the adapter boundary and surface as a terminal `Unsupported`
model failure.

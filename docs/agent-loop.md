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

A tool (`src/tools/executor.rs`) owns its immutable definition (name and
input schema contract) and the execution of one call into a normalized
`ToolExecutionResult`. The loop records the returned result verbatim and
feeds it back to the model inside a `ToolMessageBlock`; it never
fabricates, modifies, or reinterprets a result. An unknown tool has no
result, so the attempt fails explicitly with `RuntimeError::UnknownTool`
and without emitting any tool-execution event.

A failing tool is a normal outcome: the failed `ToolExecutionResult` is
passed back to the model, which decides the next action.

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
observable). When cancellation wins while a tool is pending, the loop
stops awaiting the tool, drops the pending tool future, records no
completion and no tool message, executes no later call, and settles
cancelled — `AgentExecution::run()` terminates without waiting for the
tool to return. Every model invocation observes a child signal of the
attempt signal through the existing adapter cancellation mechanism, so an
in-flight generation terminates with `Failed(Cancelled)` and is converted
into `AttemptCancelled`. Cancellation is always terminal failure — never
completion. Dropping a pending tool future does not guarantee that
external work is physically killed; the tool interface exposes no
cancellation handle in M3, and executor-specific cancellation is a later
milestone.

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
model stream, the tool results, the cancellation signal, and the mailbox
state observed at each safe boundary. Tool calls of one turn execute in
block order; there is no hidden concurrency, no hidden retry, and no hidden
state.

## 9. Conversation inbound mailbox (Issue #22)

The conversation inbound mailbox (`src/runtime/inbound.rs`) is a narrow
runtime-owned coordination contract: a per-conversation in-memory queue for
asynchronous user-role messages arriving while an attempt is running. It is
attached to an execution through `AgentExecution::with_inbound_mailbox`
(which rejects a mailbox of a different conversation) and adds the mailbox
safe-boundary rules below; generic cancellation timing is unchanged by
attachment.

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

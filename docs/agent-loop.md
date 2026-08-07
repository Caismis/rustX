# Agent Loop (M3)

This document describes the runtime boundary implemented by the M3
deterministic agent loop, mirroring the M2 model-plane documentation in
`docs/architecture.md`.

## 1. What the Agent Loop owns

The loop (`src/agent`) executes one attempt to its single terminal outcome:

- attempt lifecycle (`AttemptStarted` → exactly one terminal event)
- turn lifecycle (one model response plus its tool calls and results)
- canonical `ModelEvent` stream consumption, validation, and message assembly
- tool resolution and tool execution (in deterministic block order)
- canonical continuation state retention and propagation
- cancellation observation and terminal cancellation outcome
- the recorded `RuntimeEvent` trace
- the committed in-memory conversation state of the attempt

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
result. The next model request carries the full history plus the opaque
`ProviderContinuationState` boundary state reported by the previous turn
(the state of the greatest-block-index reasoning block, propagated
verbatim). Protocols without reconstructable state simply carry `None` —
nothing is fabricated, and a model that cannot continue without state
fails explicitly.

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
event and between tool calls) and races every tool execution: the loop
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

## 8. Deterministic execution

Given identical model events, identical tools, and identical input, the
loop produces an identical ordered `RuntimeEvent` stream and an identical
terminal outcome: the trace is a pure function of the attempt request, the
model stream, the tool results, and the cancellation signal. Tool calls
of one turn execute in block order; there is no hidden concurrency, no
hidden retry, and no hidden state.

## 9. Unsupported behavior (non-goals)

The M3 loop does not implement: multi-agent execution, agent delegation,
long-term memory, RAG, provider fallback routing or load balancing,
workflow/DAG engines, scheduling, distributed execution, a persistent
event store, plugin marketplaces, conversation summarization, or parallel
tool scheduling. Inbound asynchronous messages are not drained into the
conversation mid-attempt. Anthropic server-side fallback blocks remain
unsupported at the adapter boundary and surface as a terminal
`Unsupported` model failure.

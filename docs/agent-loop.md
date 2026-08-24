# Agent Loop (M3 + Issue #22 + Issue #55 + Issue #56)

This document describes the runtime boundary implemented by the M3
deterministic agent loop, mirroring the M2 model-plane documentation in
`docs/architecture.md`, including the Issue #22 conversation inbound
mailbox integration.

## 1. What the Agent Loop owns

The loop (`src/agent`) executes one attempt to its single terminal outcome:

- attempt lifecycle (`AttemptStarted` → one terminal settlement candidate,
  normally one committed terminal event)
- turn lifecycle (one model response plus its tool calls and results)
- canonical `ModelEvent` stream consumption, validation, and message assembly
- the durable publication stream of every model request (Issue #108): opening
  it, feeding the bounded coalescer, committing frames before releasing them,
  committing the publication terminal (U) after the provider outcome (P), and
  settling the stream exactly once as canonical, unaccepted, or incomplete
- tool resolution and tool execution (in deterministic block order)
- canonical continuation state retention and propagation
- safe-boundary inbound mailbox consumption (one finite drain per boundary)
- cancellation observation and terminal cancellation outcome
- the recorded `RuntimeEvent` trace
- the moved `ConversationState` owned by the attempt while it runs
- the pending fresh inbound trigger lifecycle (`FreshInboundTurn`) and its
  composition into one native Context Assembly generation
- the finite `ContributorInputSnapshot` boundary and certified-extension
  proposal admission
- the one cancellation-vs-start arbitration point of every model turn
  (Issue #12, M9b): staging without durable effect, then the fused durable
  start commit under the attempt's start gate
- frozen `RequestSnapshot` creation and structural reconstruction checking
- the two typed lifecycle interception seams of Issue #56 (`PreStepPolicy`
  and `ToolResultObserver`), the deferred context buffer they feed, and the
  split between lifecycle *timing* and semantic *ownership*

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
  definition, so one implementation may serve many registrations. Native,
  MCP, and custom Python executors all enter through this same boundary.
- A `ToolExecutor` executes an already-resolved, already-validated
  `ToolInvocation` (call id, tool id, model-facing name, resolved
  foreground/background mode, and the stripped business arguments) inside a
  `ToolExecutionContext` that carries conversation identity, an
  `ExecutionCancellation` view, the workspace boundary, the progress
  reporter, the artifact store (genuine semantic artifacts), the managed
  tool-output store (lazy foreground textual spill files plus the
  dispatch-allocated background live-output channel), and the explicit
  authorized environment.
- `ExecutionCancellation` observes the runtime `CancellationSignal` and
  provides a **live read of the owning authority's absorbing first-winner
  cause** — not a start-time copy of it. A foreground execution views its
  attempt's `AgentCancellation`; a background execution views its
  conversation background registry record. `child_signal()` derives a
  subordinate signal: owner cancellation reaches the child, but a child
  cancellation cannot reach the owner or enter its start gate. Each owned
  execution has exactly one cause store, and the first owner cancellation
  request that wins owns it: a later request delivers the signal but can
  never relabel the winner. An executor that started before any cancellation
  existed therefore reports the cause that actually won the race —
  `RuntimeShutdown` when runtime drain won, `UserRequested` when the user won
  first — when it normalizes its own cancelled result.
- The loop preflights every model-issued call **before** the Assistant
  tool-call message is committed: registry identity resolution,
  execution-policy resolution, reserved-metadata extraction, and business
  argument validation against the canonical JSON Schema. An impossible
  identity mismatch or an unregistered tool is a runtime/model-stream
  contract failure and the Assistant message is never committed; a business
  schema violation is a normal failed result slot and the executor never
  runs.
- The loop records the returned result verbatim and feeds it back to the
  model inside a `ToolMessageBlock`; it never fabricates, modifies, or
  reinterprets a result.

A failing tool is a normal outcome: the failed `ToolExecutionResult` is
passed back to the model, which decides the next action. Cancellable native
foreground work observes `ExecutionCancellation`, derives a child signal for
its subordinate operation, and physically settles (for example Bash
terminates its owned process group); the loop never drops a pending tool
future and leaves external work running.

The loop does not branch on `ToolOrigin`: MCP transport ownership,
Python-version publication, and native process details terminate behind the
executor/subsystem boundary. Background dispatch clones the exact executor
before ownership transfer, so later capability revisions cannot redirect an
old execution to a current registry.

## 4. Continuation

Continuation is canonical conversation state: the loop retains the immutable
Message Ledger plus the current Conversation Surface and appends each
completed Assistant message and tool result. The next model request carries the opaque
`ProviderContinuationState` boundary state reported by the previous turn
(the state of the greatest-block-index reasoning block, propagated
verbatim). Protocols without reconstructable state simply carry `None` —
nothing is fabricated, and a model that cannot continue without state
fails explicitly.

The context path is mandatory: every model request carries the finite
projection of the current Surface (complete canonical messages in exact
order), the rustX-rendered Effective System Prompt, and a frozen
RequestSnapshot. Agent Status is a canonical User context fact admitted before
the snapshot. Skill guidance is request-time native system capability guidance
rendered from the attempt's immutable capability snapshot; it is not a
canonical User fact or a hidden adapter attachment.
A successful compaction appends one runtime User summary and applies one
Surface Replace, establishing a new revision and invalidating the pending
continuation; the continuation-owning turn is retired completely, so an old
opaque provider continuation is never paired with a new projection.
Issue #61 extracted the enclosing `ConversationRuntime` coordinator from the
Runtime Client boundary (see `docs/architecture.md`); the loop itself is
unchanged by the extraction.

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
drained ids in sequence order. The next model request samples Agent Status
and admits it through Context Assembly as a canonical Runtime context fact.
A `ContextWindowExceeded` overflow does not consume the trigger and the
retry reuses the already accepted context generation; it does not resample,
reinvoke contributors, or append duplicate context. A foreground-tool-only
continuation with no new drain carries no Agent Status. A failure while
composing or preparing that status is a context preparation failure
(`AttemptFailed(Runtime(ContextPreparationFailed))`), never a compaction
failure.

## 4.2 Context Assembly and model-turn start

The Agent Loop is the coordination owner. It creates one finite immutable
`ContributorInputSnapshot`, samples native observations, invokes the one
`ContextAssembly` contract, awaits every bounded contributor future, and
receives transient typed proposals. The assembly layer assigns trusted
provenance, finite semantic lanes, stable extension ordering, and
`ContextGeneration`; contributors cannot allocate canonical IDs or mutate
the conversation.

Assembly output is then **staged**, not committed:
`AgentExecution::stage_context` validates the proposals against a scratch
conversation state (no durable effect), and `prepare_model_turn` derives
the frozen `RequestSnapshot` and provider-neutral `ModelRequest` from the
staged surface. Cancellation observed anywhere up to this point settles
the attempt with nothing committed.

The one cancellation-vs-start linearization point of every model turn is
`AgentCancellation::arbitrate_model_turn_start`: the attempt's start gate
is held across the cancellation check and the fused durable start commit,
so exactly one of cancellation and the start commit can linearize first
(Issue #12, M9b):

```text
transient proposals
    ↓
stage_context: validate in scratch state, prepare canonical commits
    ↓ (no durable effect)
prepare_model_turn: freeze RequestSnapshot + ModelRequest
    ↓
┌─ start gate held ─────────────────────────────────────────────┐
│ cancellation check                                            │
│     ↓ not cancelled                                           │
│ ConversationStore::commit_model_turn_start — ONE transaction: │
│   canonical request-scoped User context + RequestSnapshot    │
│   (including the frozen Effective System Prompt) +           │
│   ModelRequestStarted + sequence binding                      │
└───────────────────────────────────────────────────────────────┘
    ↓
independent durable reconstruction/equality verification
    ↓ prepared-request equality is guaranteed by the construction contract;
      any durable read/reconstruction failure aborts before provider dispatch
invoke adapter
```

Cancellation before the arbitration commits nothing: no request-scoped
context, no Surface advancement, no snapshot, no start fact, and no
provider request. A start-commit durability failure rolls the whole
transaction back — no half-committed context — and settles the attempt as
the honest durable-store failure. Once the start commit linearizes first,
the request is durably started: a later cancellation is necessarily
post-start, settles the started request, and can never be reclassified as
never-started. The provider is invoked only after the start commit
succeeded, but a committed start fact does **not** prove the provider
received and executed the request: after the commit the loop still performs
independent durable reconstruction/equality verification before adapter
invocation, and a process can crash anywhere between the durable commit and
provider dispatch. The durable contract is therefore:

- no `ModelRequestStarted` ⇒ rustX proves the provider invocation could
  not have crossed the request-start boundary;
- `ModelRequestStarted` ⇒ rustX crossed the durable no-resend /
  external-start boundary; provider execution may or may not have actually
  occurred, so recovery must never silently resend it.

If cancellation becomes observable while a contributor future is pending,
the future is allowed to settle its bounded transient result; the
arbitration then decides the race. The same rule covers a pending
`PreStepPolicy` evaluation: the evaluation settles, and the arbitration
still decides.

`RequestSnapshot` stores the effective invocation/configuration, reasoning
values, tool definitions, capability revision, rendered Effective System
Prompt, continuation state, context generation, request identity, and the
exact `SurfaceRevision`. `RequestSnapshot::reconstruct` hydrates only that
historical revision plus the frozen values. The loop compares the actual
provider-neutral `ModelRequest` with the reconstructed value before calling
an adapter. It never reads current configuration, Skill discovery, live
contributors, package contents, filesystem state, or current runtime status.

Every actual primary request is durably started before provider dispatch:
`ConversationStore::commit_model_turn_start` commits the canonical
request-scoped User context, the immutable snapshot containing the exact
frozen Effective System Prompt, and the exact `ModelRequestStarted` fact in
one transaction. Transient accepted system sections are not independently
persisted. `RequestHistory` (`src/runtime/request_history.rs`) is
a durable read handle, never a retained snapshot vector or client
projection state. Historical reconstruction loads one snapshot, its exact
Surface revision, and keyed Ledger bodies on demand.
Historical listing is a bounded, fallible, cursor-paged read; no current model
settings, contributors, Skills, clock, or status fill historical gaps, and
runtime bootstrap never enumerates the full snapshot history.

`AgentExecution` applies the same ownership rule to the Event Journal. It
retains only active state needed to continue the current turn, not every
committed `RuntimeEvent`. A successful event append is followed by live
observer publication, if attached, and the event body is then dropped. The
settlement handoff contains current `ConversationState`, outcome, terminal
state, and durability status; historical events are read from the durable
store with `read_events` pages.

The loop's `observe_event`/`observe_committed`/`observe_status` facts are
published as runtime-owned `ConversationObservation`s
(`src/runtime/observation.rs`) into one leaf queue. That stream is folded
**exactly once**, by the Runtime Client projection; the runtime keeps no
mirrored attempt/status/compaction read model. It does not need one: a
Runtime Client host binds before the conversation runtime is activated
(Issue #61), the inactive phase admits no semantic mutation at all
(mailbox, `model_set`, `shutdown`, background dispatch commit, and
runtime-owned capability commit are all refused typed), and an inactive
runtime therefore publishes nothing, so an installed
consumer observes every observation the runtime ever emits. When no host
is bound, the queue simply has no consumer and the loop runs identically.

## 4.3 Typed lifecycle interception (Issue #56)

Issue #56 adds exactly two phase-specific typed seams. They live on one
**required** immutable per-attempt value, `AttemptLifecycle`
(`src/agent/lifecycle.rs`), passed to `AgentExecution::new`.
`AttemptLifecycle::inert()` is the identity configuration: the policy always
enters and no observers are registered, so no deferred context is produced.
Because the configuration is required and total, no execution path branches on
"is a hook attached?", so attaching a seam cannot change ordering,
cancellation, or settlement semantics. There is no hook registry, no chain, no
middleware, and no around-dispatch wrapper.

The pre-step phase has exactly **one** owner per attempt rather than a chain.
A chain would require a second deterministic ordering model purely to sequence
hook implementations, on top of the Issue #55 contributor lane/identity order,
and no consumer needs several independent admission decisions. Composition,
when a consumer eventually needs it, belongs inside that consumer's own
implementation.

The tool-result phase differs, because its output is *context*, and context
already has a rustX-owned identity ordering. Observers are **bound** to a
`DeferredContextProducer` — one per semantic owner, a duplicate binding is
rejected — so a native runtime owner and one or more certified extensions can
each own deferred context about the same settled call without speaking for
one another, short-circuiting one another, or replacing one another's result.
The deferred ordering key is
`(canonical ToolCall batch position, producer identity, proposal FIFO)`: no
priority number, no registration-order term, and no new ordering model.

### Lifecycle timing is not semantic ownership

This split is the load-bearing rule of the phase:

| Question | Answer | Owner |
|---|---|---|
| *When* does a proposal become eligible? | after the owning tool batch settles, at the next primary step | Agent Loop |
| *Who* owns the fact it states? | the identity the producing observer was registered under | Context Assembly |

The Agent Loop stamps each staged proposal with its observer's **bound**
producer — never with anything the observer returned — and Context Assembly
resolves that reference to an authoritative registration before deriving the
lane, the `UserSource`, and the `ContextKind`, through the same table it
applies to that owner's request-time proposals. There is no rule turning
post-tool proposals into native runtime context. A certified extension (Issue
#58) producing deferred post-tool context keeps its extension identity, its
extension provenance, and its own lane, and remains ordered deterministically
against every other producer.

### Binding is not admission

The lifecycle seam can *bind behavior* to a semantic owner. It can never
*establish* one:

```text
ContextAssembly::register_extension   = semantic identity, provenance,
                                        and attestation authority

AttemptLifecycle::with_extension_     = behavior binding for an
  tool_result_observer                  already-authorized owner
```

`AttemptLifecycle` exposes exactly two binders —
`with_native_tool_result_observer` and `with_extension_tool_result_observer`.
There is deliberately no public generic binder over an arbitrary contributor
identity, because that would read like a second registry. The extension binder
takes only a logical key, which any caller can construct and which proves
nothing on its own.

At assembly time each deferred producer is resolved:

- `NativeRuntimeObservation` → the rustX-owned native runtime observation
  owner; no registration is required because rustX owns it, and it carries no
  attestation;
- `CertifiedExtension { identity }` → the matching extension registered with
  the attempt's `ContextAssembly`, using **that registration's own**
  `ContributorGeneration` and attestation.

An unregistered key fails the whole assembly with
`ContextAssemblyError::UnregisteredContributor` before admission: no lane, no
`UserSource::Extension`, no synthesized generation, and no partially admitted
batch. A certified extension that only ever produces deferred context still
resolves to its authoritative generation without contributing anything at
request time.

### PreStepPolicy

```text
Context Assembly
    ↓ final immutable AcceptedContext (deferred + native + extension)
PreStepPolicy::evaluate      →  Enter | Reject { reason }
    ↓
staging (scratch validation — no durable effect)
    ↓
cancellation-vs-start arbitration     ← the one linearization point
    ↓ (start gate held across check + commit)
commit_model_turn_start → canonical User context + Surface + RequestSnapshot
                          (frozen Effective System Prompt)
                          + ModelRequestStarted, in one transaction
    ↓
invoke adapter
```

The policy observes `PreStepBatch`: attempt/conversation identity, the turn
number, the pre-staging `SurfaceRevision`, and an immutable borrow of the
validated `AcceptedContext`. It returns `Enter` or `Reject { reason }` and
nothing else — it cannot rewrite, extend, or replace the batch, because a
policy that could synthesize a replacement batch would be a second context
authority.

There is exactly **one** model-turn start path. Every request-time contribution
— native Agent Status, native Skill system guidance, certified-extension
context, and deferred context from any producer — converges on the same
assembly, staging, and arbitration before the same fused start commit. No
contributor and no observer has a private commit path; Skill guidance does not
create a separate durable commit.

A `Reject` settles the attempt as
`AttemptFailed(Runtime(PreStepRejected { reason }))`, and a policy failure as
`AttemptFailed(Runtime(PreStepPolicyFailed { message }))`. Both happen
strictly before the start arbitration, and staging has no durable effect,
so a rejection proves: no proposed dynamic context committed, no Surface
revision caused by those proposals, no `RequestSnapshot`, and no provider
request.

The policy owns no cancellation. If cancellation becomes observable while a
bounded evaluation is pending, the evaluation settles and the loop's own
start arbitration still decides — exactly the settlement rule Issue #55
defines for a pending `ContextContributor` future. The policy cannot
convert cancellation into success, restart the turn, trigger a provider
retry, or force continuation.

An overflow retry reuses the already staged `ContextGeneration` and never
re-enters assembly, so the policy is evaluated exactly once per primary
step. The retry itself passes through the same start arbitration with an
empty staged context; the compaction between the overflow and the retry is
an independent durable commit whose candidates are evaluated through the
same `TokenEstimator` over the exact hypothetical post-compaction request —
the retained Surface plus the staged (not yet committed) request-scoped
context overlay (`CompactionConstraints::staged_request_context`) plus the
Effective System Prompt plus tools — never as a scalar token reservation.

### ToolResultObserver

```text
Assistant(ToolCall A, ToolCall B) committed
    ↓ execute (scheduling per ToolConcurrencyPolicy)
every CallSlot settled (including cancellation fill)
    ↓
commit ToolResult A, then ToolResult B          ← structural settlement point
    ↓
cancellation checkpoint  ← before each observer, and again once it settles
    ↓
ToolResultObserver pass, in (canonical ToolCall order, producer order)
    ↓ bounded UserMessageProposal values
    ↓ validate count + content — the transaction boundary
    ↓ stamp the observer's bound producer reference
Agent-Loop-owned deferred buffer
    ↓ next primary step
Context Assembly → resolve producer → PreStepPolicy → admission
    ↓
canonical User context
```

Foreground progress reported while a batch executes is transient
current-execution state: each active call retains a bounded number of
normalized progress observations
(`MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL`, earliest prefix plus latest),
and only the retained observations become durable `ToolExecutionProgress`
Event Journal facts at batch commit, in canonical model-call order before
their completion event. Coalesced observations never cross the durable
commit point.

The observer receives `ToolResultObservation`, a borrow of already-canonical
facts: attempt/conversation identity, the turn, the canonical
`batch_position`, the canonical `ToolCallId`, the registry-resolved `ToolId`,
the typed `ToolOrigin`, the finalized `ToolExecutionResult` exactly as
committed, and the immutable `ObservedToolInvocation`.

#### Immutable invocation facts

`ObservedToolInvocation` carries the registry-resolved `ToolId`, the typed
`ToolOrigin`, the resolved `ToolInvocationMode`, and the **validated business
arguments** of the call — the stripped value preflight checked against the
canonical schema, with the reserved `__rustx_*` invocation metadata already
removed and no raw provider payload exposed.

The arguments are required because a result alone under-determines the fact it
describes: the native Read capability returns file *content*, and the *path*
exists only in the invocation. Without it a consumer would have to re-read
canonical history, parse Assistant messages, or maintain a duplicate
invocation index — three ways of creating a second, drifting authority for a
fact the loop already owns.

The whole value is `None` when preflight rejected the call before invocation
resolution: no invocation was ever validated, so none is exposed. `ToolId` and
`ToolOrigin` remain on the observation, so a consumer can still name the
capability that was refused.

Read-only is structural: the observation is a borrow held after the result is
already canonical, with no interior mutability and no execution handle. An
observer cannot re-run, rewrite, or replay the invocation.

The model-facing tool **name** is deliberately not part of the observation.
Capability recognition uses `ToolId` + `ToolOrigin` only, so an MCP or Python
tool whose public name happens to be `read` can never be mistaken for the
native rustX Read capability (`tool-read`, `ToolOrigin::Builtin`). Making the
name unavailable turns that discipline into a structural property rather than
advice. Both `PreflightOutcome` variants carry the registry-resolved `ToolId`
and `ToolOrigin`, taken from the same resolved `ToolDefinition`, so no second
stored identity can disagree with the registry.

A `ToolInvocationMode::Background` invocation reports an **accepted background
dispatch**, not the completion of the detached work. An observer must not
infer an external side effect from it.

#### What an observer returns

Zero or more bounded `UserMessageProposal` values and nothing else —
*content*, never *semantics*. It cannot choose a `UserSource`, a
`ContextKind`, a lane, or a contributor identity; those follow from resolving
its bound producer. It cannot mutate the finalized result, reject or undo it,
create a second `ToolMessage`, dispatch another tool, start a model request,
own cancellation, change the terminal outcome, or touch the Ledger/Surface.

The return type is deliberately **not** the full `ContextProposal` vocabulary.
A settled tool batch is a conversational fact, and the only concrete
requirement — including Issue #58's `PostToolUse additionalContext` — is
deferred conversational context. So this seam cannot contribute to the
Effective System Prompt of the following turn; system sections stay owned by
the request-time `ContextContributor` path. The restriction is a type, not a
runtime check: a deferred system section is unrepresentable.

#### The transaction boundary

Every observer return value is validated **before** anything is staged:

1. the proposal count against the established `MAX_PROPOSALS_PER_CONTRIBUTOR`
   bound — the deferred path reuses the context proposal limit rather than
   inventing a second constant;
2. the running total against `MAX_DEFERRED_CONTEXT_PROPOSALS`, counting what
   the attempt already staged;
3. every proposal body against the same bounded content contract Context
   Assembly applies.

So an observer can never append an unbounded proposal set to the attempt
buffer and have it rejected one step later in assembly. A violation settles
the attempt as `AttemptFailed(Runtime(DeferredContextRejected { message }))`.

The pass is one transaction. Any failure — a failing observer, a bound
violation, invalid content, or observable cancellation — discards every
proposal of the pass *and* clears the attempt's deferred buffer, so no partial
deferred state survives.

#### Cancellation precedence

Cancellation ownership stays entirely with the Agent Loop; an observer gets no
cancellation handle. The rule around every observation is:

```text
cancellation check          ← an observer never starts after this
await observer
cancellation check          ← wins over the observer's Ok *and* its Err
consume result, validate, stage
```

An observation already in flight is allowed to settle — it is never dropped
mid-flight just to implement this rule — but once cancellation is observable
it decides the terminal outcome:

- a later observer never starts;
- an earlier observer's proposals, still only pass-local, are discarded;
- an observer's failure can no longer become
  `ToolResultObservationFailed`; the attempt settles `AttemptCancelled`.

### Exact settlement and eligibility points

The stages are distinct and named:

| # | Point | Owner |
|---|-------|-------|
| 1 | physical tool execution completion | executor |
| 2 | normalized `ToolExecutionResult` finalization | Agent Loop (`run_single_call`) |
| 3 | every sibling `CallSlot` settled, including cancellation fill | Agent Loop (`execute_tools`) |
| 4 | canonical `ToolMessage` commit, in original model call order | Agent Loop (`commit_canonical`) |
| 5 | **batch structural settlement** — the last `commit_canonical` of the batch | Agent Loop |
| 6 | `ToolResultObserver` pass → validate at the transaction boundary → stamp producer identity → stage | Agent Loop |
| 7 | deferred proposals become **eligible** — the next `prepare_model_turn` drains the buffer into `ContextAssembly::assemble` | Agent Loop |
| 8 | next model-turn start (policy + staging + cancellation-vs-start arbitration + fused `commit_model_turn_start`) | Agent Loop |

`TurnCompleted` is emitted at the end of stage 4/5; the observation pass runs
after it and before `tools_finished()` and the safe-boundary mailbox drain.

### Canonical ordering

For `Assistant(A, B)` the canonical history is always

```text
Assistant(A, B)
ToolResult A
ToolResult B
User(deferred context …)      ← only at the next admitted step
```

even when B physically completes first. Two structures guarantee it:

- result slots are preallocated in model call order and the commit loop walks
  that vector, so completion order only decides *when* a slot's `result` field
  is filled, never *where* it is committed;
- the observation pass walks the same settled vector in the same order, and
  for each settled call invokes the registered observers in logical identity
  order, appending each observation's proposals in the order the observer
  returned them.

The deferred order key is therefore `(canonical ToolCall batch position,
producer identity, proposal FIFO)`. Physical timing and registration order
never appear in the key.

Where those facts land in the next primary step is a **semantic ownership**
question, answered by the producer identity and nothing else:

- `NativeRuntimeObservation` → the native-reserved
  `UserContextLane::RuntimeToolObservation`, immediately after `ClaimedInbound`
  and before every request-time lane, committed with `UserSource::Runtime` and
  `InboundKind::Context(ContextKind::RuntimeToolObservation)`;
- `CertifiedExtension { identity }`, **once resolved against the attempt's
  Context Assembly registration** → `UserContextLane::ExtensionEnvironment`,
  committed with `UserSource::Extension { contributor: key }` and
  `InboundKind::Context(ContextKind::ExtensionEnvironment)` — exactly as if the
  same extension had contributed at request time. An unresolved key rejects
  the batch instead.

Inside one `(lane, contributor)` bucket a deferred fact precedes the same
owner's request-time fact, because it describes the batch that precedes the
step. The accepted `ContextGeneration` records each producer's own identity
once; an extension's deferred fact is never attributed to the native
observation owner. The ordering is a documented total order in the lane
contract, never incidental `Vec` concatenation or map iteration.

The staging buffer is transient by construction: it is **not** canonical
history, not a second transcript, and not a second context ledger, and it is
drained by the very next primary step's assembly.

### Failure and cancellation semantics

| Situation | Outcome |
|-----------|---------|
| policy returns `Reject` | `AttemptFailed(Runtime(PreStepRejected))`; nothing admitted, no snapshot, no request |
| policy returns `Err` | `AttemptFailed(Runtime(PreStepPolicyFailed))`; nothing admitted |
| cancellation while policy pending | evaluation settles; the generic checkpoint wins; `AttemptCancelled`; nothing admitted |
| observer returns `Err` | `AttemptFailed(Runtime(ToolResultObservationFailed))`; the committed Assistant batch keeps its complete canonical result batch; every proposal of the failed pass is discarded and the deferred buffer is cleared; no further provider request |
| observation violates a deferred bound or proposes invalid content | `AttemptFailed(Runtime(DeferredContextRejected))`; rejected at the transaction boundary, so nothing of the pass was ever staged; same batch guarantees |
| deferred proposal names an unregistered extension | the next assembly fails before admission; no lane, no extension provenance, no synthesized generation, no snapshot, no request |
| cancellation observable before a later observer would start | that observer never runs; the whole pass is discarded; `AttemptCancelled` |
| cancellation observable when an observer settles with `Err` | cancellation wins; `AttemptCancelled`, never `ToolResultObservationFailed` |
| cancellation already observable when the batch settles | the observation phase is skipped entirely; `AttemptCancelled` |
| cancellation while an observation is pending | the bounded observation settles; the staged proposals are never admitted; `AttemptCancelled` |

In every case the attempt selects one terminal settlement candidate. When the
required Event Journal append succeeds, that candidate is the one terminal
event and it is last; if the append fails, no terminal event is emitted or
fabricated and the typed durable failure is reported to the owner.
Because the observation pass runs strictly after structural settlement,
observer failure can never prevent a committed Assistant `ToolCall` batch from
receiving its complete canonical `ToolMessage` result batch, and it can never
strand a later sibling's result.

### PreToolPolicy and native HITL (M9.2 / Issue #100)

Pre-tool approval is a typed seam in `AttemptLifecycle`, not a wrapper around
`ToolExecutor`:

```text
model ToolCall
  -> ToolRegistry::preflight
  -> PreparedInvocation
  -> canonical Assistant ToolCall commit
  -> PreToolPolicy(immutable PreToolView)
       Allow
       Deny { reason }
       Ask { reason }
          -> InteractionCoordinator
          -> Runtime Client typed response
  -> existing cancellation/start frontier
  -> exact PreparedInvocation, or one result slot without an executor
```

The attempt always carries one required policy. A runtime-created attempt is
bound to its one concrete, conversation-owned `InteractionCoordinator`; that
native owner is not a replaceable production rendezvous strategy. Standalone
inert execution has no interaction provider and fails `Ask` closed. The
runtime-created policy is derived from the admitted effective `ApprovalMode`:
`Policy` consults the resolved Tool's `ToolApprovalPolicy`, while `FullAccess`
maps effective approval to `Never` only. The policy sees only the
conversation/attempt/turn, call and resolved tool identities, safe name,
origin, mode, and validated business arguments. It cannot resolve registry
facts, mutate canonical state, dispatch a tool, receive cancellation
authority, or return replacement arguments.

`Ask` passes those facts to the conversation-owned `InteractionCoordinator`;
the coordinator injects conversation identity and the owning execution
supplies attempt identity at that narrow boundary.
The coordinator owns the pending map, non-reused identity, answer/cancel
terminal transition, and waiter; the Agent Loop remains the only owner that
can resume the original invocation. `Allow` is not a start capability: the
loop checks cancellation again at the existing start frontier. A `Deny` is a
normal structural Tool Plane result with `ToolExecutionStatus::Denied`, one
canonical result slot, no `ToolExecutionStarted`, and no executor future.

For a parallel batch rustX resolves every policy/interaction decision in
canonical call order before any executor starts. This is intentionally a
strong batch boundary: response timing can neither reorder result slots nor
let one approved sibling start while another sibling is still awaiting its
policy decision. Preflight rejection remains the earlier registry-owned
validation path and never reaches `PreToolPolicy`.

There is one post-evaluation cancellation checkpoint: when
`PreToolPolicy::evaluate()` settles, the Agent Loop checks cancellation before
consuming any `Allow`, `Deny`, `Ask`, or error value. Observable cancellation
therefore produces a normal cancelled slot and publishes no interaction or
denial from that decision. The `Ask` wait has a second post-wait checkpoint;
even `Answered(Allow)` is rechecked before the existing start frontier and
cannot grant execution authority.

The coordinator's response and owner-cancellation paths use one synchronized
pending-state transition. The first terminal winner wakes the waiter exactly
once; later responses are `interaction_not_pending`. Removing the pending
entry is not waiter settlement: a counted lifecycle admission remains with
the waiter until it consumes or drops the outcome, and the Runtime Client
settlement observation is published after that authority is released under a
second counted settlement admission. This composes with M9c drain without a
second lifecycle or shutdown participant framework.

Interaction publication and settlement are also durable-before-release
(Issue #109). The requested audit fact commits before the prompt reaches a
client, and `InteractionSettled(Approved)` commits before the Agent Loop can
emit `ToolExecutionStarted`, so the durable order the Journal shows is always

```text
InteractionRequested -> InteractionSettled(Approved) -> ToolExecutionStarted -> side effect
```

That ordering is a consequence of the wait itself: the semantic waiter is not
released until the settled fact is committed, so the post-wait cancellation
checkpoint and the start frontier are both strictly downstream of the durable
decision. The audit is still only evidence — the `Answered(Allow)` recheck
above is what grants execution authority in this process, and a historical
approval read back after a restart grants none.

The approval subject the Loop hands the coordinator is derived from the exact
canonical `ToolCall` of the committed Assistant message, not from the pending
invocation alone: it carries the call/tool identity the `ToolCall` froze and
the digest of the `ToolCall`'s own model-issued arguments. The client is still
shown the normalized invocation that will actually run, but the durable
subject pins the value the Message Ledger holds, and the durable authority
refuses a subject that does not match it. This is why the canonical Assistant
message is committed before `resolve_pre_tool_decisions` runs: at the pre-tool
policy boundary the call the approval names is already durable.

The coordinator does not arbitrate cancellation causes. Each runtime-owned
pending interaction retains an `ExecutionCancellation` observation view, not
the owning attempt's `AgentCancellation` handle. Generic ToolExecutors receive
that owner-observing `ExecutionCancellation` capability only; the native
`ask_user` path receives a
crate-private `QuestionRequester` bound to the same attempt and coordinator.
If that authority has already selected a cause before a waiter or client
response reaches the terminal transition, the interaction records the same
`Cancelled { reason }` outcome and the response is `interaction_not_pending`;
it cannot publish `Answered`. Runtime drain is also only a cancellation
contender: after requesting `RuntimeShutdown`, `ConversationRuntime` reads the
active attempt's first-winner reason and propagates it to pending interaction
settlement. `UserRequested` therefore survives a later drain, while
`RuntimeShutdown` is recorded consistently when it wins first.

The interaction domain is provider-independent and bounded to Approval and
Question in 0.1. Question is not a pre-tool Agent Loop branch: the native
`ask_user` capability is an ordinary foreground/sequential/approval-never
Tool whose executor requests a Question through that bounded requester and
the same coordinator, then returns the typed answer as an ordinary ToolResult.
Questions carry only a bounded
prompt, optional finite choices, and an optional free-text flag. In the native
tool contract, a bare prompt means open-ended free text; a supplied non-empty
choice list is choice-only unless `allow_free_text: true` is explicit. The
Registry normalizer and canonical schema reject empty lists, duplicates,
Unicode-bound violations, and invalid answer modes before an interaction
provider is consulted. The executor never revalidates model arguments after
preflight. Generic forms/workflows, provider SDK
payloads, argument rewriting, and a generalized permission language are not
part of this seam.

`FullAccess` is runtime control state, not Tool authorization. It cannot
activate disabled or excluded Tools, bypass execution ownership or
concurrency policy, grant authority, or auto-answer a pending Approval or
Question. While an attempt is busy, `desired_approval_mode` changes and
`effective_approval_mode` remains frozen; after terminal settlement the
runtime reconciles the latest desired value before admitting the next attempt.
The current runtime/project configuration may set `approvalMode` (default
`policy`), but Session history never persists `FullAccess`.

Availability, activation, approval, approval mode, execution ownership, and
concurrency are separate facts. No one of them is inferred from another.

### Request Snapshot implications

Accepted deferred context is an ordinary canonical Ledger/Surface fact before
the corresponding `RequestSnapshot` is frozen, so the Issue #55 reconstruction
invariant is unchanged:

```text
ConversationSurface @ revision X + RequestSnapshot X = exact ModelRequest X
```

No historical request needs to rerun a policy, rerun an observer, or consult
the current lifecycle configuration. The `ContextGeneration` frozen in the
snapshot names the deferred-context owner, which is an explanation of the
assembly, never a re-execution handle.

### Authority matrix

| Phase | Can observe | Can propose/decide | Cannot mutate | Owner |
|-------|-------------|--------------------|---------------|-------|
| `ContextContributor` (#55) | finite immutable `ContributorInputSnapshot` | bounded typed proposals | canonical state, identity, provenance, lanes, ordering | Context Assembly |
| `PreStepPolicy` (#56) | final immutable `AcceptedContext` + attempt/turn/revision identity | `Enter` / `Reject { reason }` | history, Surface, `MessageId`s, tool identity/arguments, cancellation, provider dispatch, terminal state | Agent Loop |
| `PreToolPolicy` (#64) | immutable preflight-resolved `PreToolView` | `Allow` / `Deny { reason }` / `Ask { reason }` | registry resolution, canonical state, tool identity/arguments, cancellation, executor start | Agent Loop / attempt lifecycle |
| `InteractionCoordinator` (#100) | immutable Approval or Question facts | one typed response/cancellation rendezvous | Agent Loop scheduling, canonical history, ToolCall arguments, executor state | ConversationRuntime |
| `ToolResultObserver` (#56) | immutable finalized `ToolExecutionResult` + canonical `ToolCallId`, batch position, stable `ToolId`/`ToolOrigin`, and the read-only `ObservedToolInvocation` (mode + validated arguments) | bounded deferred `UserMessageProposal`s | the result, the `ToolCall`, `ToolMessage` count, history, cancellation, terminal state, the Effective System Prompt, **and its own provenance/lane/identity** | Agent Loop / tool batch |
| Deferred context staging | ordered transient proposals + their bound producer reference | candidate input of the next Context Assembly | Ledger and Surface directly; the semantics of what it stages; whether a named extension is trusted | Agent Loop |
| Context admission | final accepted context | canonical `User` facts + Surface advancement | arbitrary history | Agent Loop + `ConversationState` |
| `AgentExecutionObserver` (#37) | emitted `RuntimeEvent`s, committed `MessageBlock`s, composed Agent Status | nothing | everything | Agent Loop (projection seam only) |

### Seams intentionally absent

These are absent by decision, not as TODO compatibility hooks:

- **`ToolExecutionWrapper` / `around_tool` / middleware chain** — no concrete
  native requirement that the typed pre-tool seam above cannot express.
- **Post-tool result replacement or retroactive blocking** — a finalized
  result is canonical by the time an observer sees it;
  `ToolResultObservation` is immutable by construction.
- **Pre-tool argument or identity rewriting** — a committed Assistant
  `ToolCall` (`id`, `tool_id`, `name`, `arguments`) is a conversation fact.
- **Generic forms/workflows, generalized permission/risk policy, and
  provider-specific interaction payloads** — Issue #100 deliberately keeps
  Question bounded and makes `ask_user` an ordinary Tool Plane capability.
- **Subagent lifecycle observation** — Issue #60 owns the native subagent
  runtime; the observation seam follows the owner.
- **`TurnStoppingPolicy` / forced continuation** — no native owner exists.

## 4.4 The publication boundary of a model turn (FND-03 / Issue #108)

Streaming output is released through the durable publication plane, never
through the Event Journal. One model turn traverses three distinct
linearization points in a fixed order:

```text
provider stream begins
    ↓ open_publication_stream          (frozen attempt/turn/request/message identity)
provider deltas
    ↓ assembler.push  +  coalescer     (bytes / oldest-deadline latency / structure)
    ↓ stage_publication_frames         durable staging commit
    ↓ release to the observation seam  ← never before its own commit
provider terminal
    ↓ assembler.finish()               structural acceptance; a rejection here is Incomplete
    ↓ ModelRequestCompleted            P
    ↓ commit_publication_terminal      U — final frame + marker, one transaction
    ↓ release the final buffered payload
    ↓ ToolRegistry preflight           a rejection here leaves the stream Unaccepted
    ↓ commit_canonical_publication     C — Ledger + Surface + event + staging clear
```

The latency policy is an oldest-payload bound, not a quiet-period debounce.
When the first payload enters an empty coalescer, it creates one absolute
monotonic deadline. Later provider events use only the remaining budget and
cannot restart or extend it; a quiet provider is woken by that deadline. The
coalescer owns both the deadline and the `PublicationClock` wake future, so
the Agent Loop does not maintain a second time domain. Byte, structural,
terminal, failure, and cancellation boundaries still follow their normal
precedence, and a successful drain is the only point that permits a new
deadline.

The store also owns the proposal staging state machine. `Started` freezes
`(stream_id, call_id, block_index, tool_id, name)`, argument suffixes require
that exact owner in `Started`, and `Completed` requires the same frozen
identity and advances the owner exactly once. Duplicate, orphan, foreign, or
post-completion frames are rejected atomically by both ordinary staging and U
terminal staging. Audit consolidation proves the owner exists; C validates
proposal ownership in both directions: every canonical Assistant ToolCall
must match one completed owner on call ID, block index, tool ID, and name, and
every completed owner must appear exactly once, with no Started-only owner
left behind. Only then is that owner retained for Tool Plane execution or
recovery. The same Store dependency guard also rejects a dependent event whose
tool ID differs from the frozen proposal or canonical owner, even when its
call ID matches.

`run_turn` is the one mutual-exclusion point of settlement. A turn that
reached canonical acceptance already cleared its stream inside the compound C
transition; every other exit — cancellation, model failure, structural
assembly rejection, preflight rejection, a durable failure — leaves the stream
open and it terminalizes as an audit whose kind the durable store derives from
the P/U evidence. Canonical acceptance and audit terminalization can therefore
never both happen for one stream.

An overflow retry starts a second provider request inside one turn. The
abandoned request's stream never reached canonical acceptance, so the Agent
Loop commits its audit before any retry preparation or second provider start:

```text
first request ends with recoverable ContextWindowExceeded
    ↓
terminalize old publication as Incomplete (must COMMIT)
    ↓ only after success
compact / prepare retry
    ↓
durably start retry Request Snapshot + ModelRequestStarted
    ↓
invoke retry adapter
    ↓ physical Started
open retry publication stream
```

Exactly one publication stream is open at any instant. If the old audit
transaction fails, the attempt records `DurableFailureKind::Publication` and
fails at the original request: no compaction, retry schedule, retry snapshot,
`ModelRequestStarted`, adapter invocation, or second stream is allowed. The
original stream remains unsettled staging for startup recovery to classify from
its durable evidence.

A publication-plane failure — a stream that cannot open, frames that cannot
stage before release, a terminal that cannot commit, an audit that cannot
terminalize — is a durable-authority failure like any other. The attempt
reports `DurableFailureKind::Publication` and never returns to a healthy
durability state.

The durable store independently enforces the publication contract. Opening
proves the exact Request Snapshot/start-event generation; U and C re-prove
that identity and the exact successful provider outcome; C also proves the
frozen provisional Assistant message and its exact event envelope. The store's
single proposal-dependency transition rejects every dependent Tool Plane
fact for Incomplete or Unaccepted proposals, including execution outcomes,
single/batch/recovery ToolResults, background authorization, and subagent
ownership, and compares every supplied tool ID with the accepted owner. Agent
Loop order is therefore a necessary sequencing rule, not the only protection.

## 5. Usage

`ModelRequestCompleted.usage` reports the canonical final usage of the
turn: the terminal `Completed.usage` when present, otherwise the latest
`UsageUpdate`, otherwise `None`. Cumulative snapshots are never summed and
missing counters are never fabricated.

## 6. Terminal state guarantee

Normally exactly one terminal `RuntimeEvent` settles an attempt:
`AttemptCompleted`, `AttemptCancelled`, or `AttemptFailed`. The terminal
event is the last recorded event when its durable append succeeds, the
platform outcome maps one-to-one from it
(`AttemptOutcome::from_terminal_event`), and the loop structure makes later
events impossible. The execution state machine is settled immediately before
the terminal event append is attempted. A failed append leaves the machine
terminal and the execution result with its settlement candidate, but no
terminal Journal fact or observer event.

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

Every valid committed Assistant tool-call message is preflighted before commit
(see section 3). Once committed, its entire tool-result batch is settled
structurally exactly once:

- The loop resolves the one `PreToolPolicy` decision for every preflight-ready
  call before entering the scheduling groups. `Allow` preserves the original
  `PreparedInvocation`; `Deny` or an unavailable/cancelled `Ask` settles that
  call's result slot without creating an executor future. This decision phase
  is in canonical model-call order, including for parallel groups.
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
- Foreground executions receive an `ExecutionCancellation` view in their
  context and derive child signals for subordinate work. When attempt
  cancellation wins during a batch:
  in-flight cancellable foreground work physically settles, unstarted
  calls receive cancelled result slots, committed background executions
  stay conversation-owned, prepared-but-uncommitted dispatches roll back,
  the complete batch commits in call order, and no next model turn starts.
- Physical completion order may differ from canonical order; canonical
  `ToolMessageBlock` values and the next model request always observe
  model call order.
- After the structurally complete batch commits, the attempt settles
  cancelled exactly once with one terminal event last.
- The batch's **structural settlement point** is the last canonical
  `ToolMessage` commit of the batch. The Issue #56 `ToolResultObserver` pass
  runs strictly after that point (see section 4.3), so an observer failure
  can never split the batch or prevent a committed Assistant tool-call
  message from receiving its complete canonical result batch.

### 7.2 Runtime drain composition (M9c)

The Agent Loop remains the owner of foreground tool-batch structure and the
current attempt's terminal settlement. `ConversationRuntime::shutdown()`
owns the lifetime boundary around it:

```text
Running -> Draining
    request RuntimeShutdown on current AgentCancellation
    -> Agent Loop cancels/settles model and foreground tools
    -> finish_attempt clears the current-attempt slot
    -> runtime drain may continue toward Quiescent
```

The M9b `arbitrate_model_turn_start` gate is unchanged. If runtime drain
wins before the durable start commit, there is no provider request,
`ModelRequestStarted` fact, or request snapshot. If the start commit already
won, the provider request is an owned started operation and the Agent Loop
waits for its native settlement before `finish_attempt` returns. The first
cancellation authority wins the absorbing cause, so runtime drain reports
`RuntimeShutdown` rather than relabeling a user cancellation.

For a parallel foreground batch, cancellation closes the start frontier: no
not-yet-started sibling receives a start fact or execution future. Started
siblings receive the shared signal, settle through their native executor
contract, and retain one result slot each. Canonical tool messages still
commit in model-call order; the runtime does not add a second tool state
machine.

Background dispatch ownership is separate from the attempt. Once the
registry's prepared-to-committed boundary wins, attempt cancellation does not
reclaim the execution. Runtime drain does: it cancels every active
conversation-owned record and waits for the registry's explicit terminal
state. Terminal visibility follows the durable terminal Pending Inbound
acceptance, so an inbound notification already accepted before drain remains
durable even though no new attempt adopts it during shutdown.

Cancellation requested, operation settled, and runtime quiescent are distinct
observations. `ConversationRuntime::shutdown()` completes only after the
current attempt, foreground tools, background terminal publication, counted
capability/environment preparation, retained MCP process closure, owned
process terminality, and the admission worker have settled. A stale callback
after `Quiescent` cannot mutate the conversation; an unproven owned process
settlement is returned as a shutdown failure instead of being called
quiescent.

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
- the immutable Skill snapshot used to produce request-time Skill system
  guidance once per admitted primary step;
- the effective `ToolEnvironment` for foreground executions;
- the effective environment and immutable Skill resource map captured into
  every background dispatch at `prepare_dispatch`, before the background
  ownership commit. The detached runner owns those values and does not query
  a later capability revision.

No model turn re-discovers Skills or re-queries the conversation capability
pointer. The Skill catalog is re-rendered from the same pinned snapshot for
each primary request and is never deduplicated against canonical history. A
capability commit while the attempt lease is active is rejected
as busy; the lease is moved into `AgentExecution` and releases when the
consumed execution is dropped after settlement (or when construction fails).
The lease owner is structurally bound to the `ConversationId` and canonical
Workspace root of the corresponding `ConversationToolRuntime`; a mismatch is
rejected before any model request or tool execution begins.

Normal rustX agent composition always contains canonical native Read; optional
tool activation cannot remove it. Skills are trusted instruction packages in
the current rustX threat model, so the pinned Skill snapshot's model-visible
projection is filtered by Skill metadata such as
`disable-model-invocation`, not by a downstream optional-Read predicate.

For M7 the pinned snapshot also owns the exact composed registry. Its MCP
executors retain their `McpServerRuntime`; its Python executors retain their
published ToolVersion source and PythonToolEnvironment. `tools/list_changed`
only invalidates future preparation and never changes the tools visible to an
active attempt.

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
  Message Ledger + Surface = conversation truth
Request Snapshot = exact non-history inputs for one model request
Event Journal    = execution facts
```

- `ConversationStore` owns one shared inbound sequence domain
  (`InboundSequence`). The first successful acceptance receives `1`; sequences
  advance strictly monotonically with checked arithmetic and never come
  from the Event Journal sequence. Sequence allocation and the pending row
  commit in one SQLite transaction before the mailbox publishes its
  process-local wake. Enqueue accepts only ordinary messages
  (`InboundKind::Message`) carrying their persisted UTC timestamp; a runtime
  compaction summary is derived history, not new asynchronous work, and is
  rejected, as is an ordinary message without a timestamp. Human, Runtime,
  Agent, Fleet, and ExternalSystem producers share the one sequence domain.
- At a safe boundary the loop performs exactly one finite selection
  (`PendingBatch`): the store establishes a watermark from the pending rows,
  returns the selected items without consuming them, and adoption later
  removes exactly that committed prefix. A post-watermark arrival waits for
  the next batch; the boundary never re-inspects the queue for newly arriving
  items.
- Every item of the selected batch is appended as its own canonical
  `UserMessageBlock` in inbound sequence order before the next model
  request. Messages remain separate blocks — never concatenated and never
  an intermediate single-message request. Adoption removes the pending rows
  only in the same transaction as Ledger and Surface advancement, so a crash
  exposes either the complete pending batch or the complete canonical
  adoption; the process-local batch is never the authority.
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
`AssistantMessage` commit, and `TurnCompleted`, the safe boundary snapshots the
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
Successful incompatible Surface replacement is the only continuation
invalidation boundary: it retires the continuation-owning turn and clears the
continuation exactly once after the semantic commit. A drained batch enters
the Ledger and current Surface before the next projection/compaction, so the
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

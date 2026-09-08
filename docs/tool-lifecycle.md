# Canonical Tool lifecycle

WF-02 separates canonical framing from native invocation. `ToolInvocation.id`
is a caller-neutral `ToolInvocationId`: Agent correlation contains its accepted
`ToolCallId`; Workflow correlation contains its concrete `WorkflowNodeInstance`.
Capability `ToolOrigin` remains Builtin/MCP and is not caller identity.

One shared argument preparation helper performs normalization and native schema
validation. The shared permission gate consumes those immutable prepared facts.
`tools::invocation::ForegroundInvocation` owns the single foreground driver:
start protection, genuine progress, hard/idle arbitration, cancellation and
physical settlement-plane consumption. Callers publish their own execution facts;
only the Agent Loop owns canonical slots and atomic ordered ToolResult batches.
Workflow Tool nodes neither synthesize model calls nor append internal history.

Native Prepared, Started, Progress, settlement and Completed Journal appends are
best-effort observations, not permission gates. When policy selects Ask, the
InteractionCoordinator commits the exact preparation subject through durable
interaction authority before exposing a prompt. This transaction pins the concrete
invocation, tool identity and normalized-argument digest; it refuses replacement
subjects for the same invocation. Agent approval still verifies its canonical
proposal. A failed approval audit commit fails closed before publication/start;
an ordinary native observation failure cannot veto or relabel execution.

Each native lifecycle owns an absorbing cancellation-cause view. Its winner is
selected in cancellation, hard, idle, completion priority order. The winning
cause is installed before the child signal is cancelled. Descendants read that
typed cause, falling through to their ancestor until their own lifecycle wins:
an ancestor hard deadline stays `Deadline(Hard)`, never `Attempt(UserRequested)`.
Only a locally expired deadline emits a local Deadline fact. Physical adapters'
attempt `reason()` is not native provenance. Settled results remain immutable.

Trusted registrations, not `ToolExecutor`, freeze `ForegroundPolicy`: ordinary Leaf uses the current
ordinary finite execution policy; Composite supplies a validated finite hard-only
total policy. Workflow defaults to ten minutes (at most 24 hours), ordinary Tools
still default to two minutes. Both start at their native execution-start frontier.
Progress and nested nodes cannot extend a total deadline. Descendants retain their
own leaf deadlines and also observe ancestor cancellation. Composite cleanup drains
the descendant settlement owners; their control guards remain authoritative rather
than competing with an equal outer guard. Guard expiry is control-plane failure,
never proof of physical stop. See [Workflow programs](workflow-programs.md) for
projection, typed certainty, exact frontiers and fixed leaf admission.

For every accepted canonical ToolCall, its owning generic Tool lifecycle must
reach exactly one canonical ToolResult. Invocation-local failures settle as
results. A failure before trustworthy call identity exists, or loss of runtime
or durable authority itself, may terminate outside that result lifecycle.

## Acceptance, execution, and commit

```text
model proposal
  -> structural validation of identity, correlation, JSON, and finish reason
  -> prepare preflight (retain business/schema rejection as a result slot)
  -> canonical Assistant message commit: ToolCall acceptance
  -> policy / approval decisions and scheduling
  -> executor-start fact (CallSlot.executor_started)
  -> handle construction (no physical dispatch)
  -> first operation poll / executor-specific effect frontier
  -> normal completion OR cancellation/deadline intent
  -> executor physical settlement and outcome evidence
  -> generic terminal selection
  -> atomic canonical ToolResult batch commit
  -> recovery reads committed canonical truth
```

Preflight is computed before Assistant commit to reject structural identity
errors without admitting an untrustworthy call. Business/schema rejection is
retained until that commit, then occupies exactly one accepted CallSlot.
Policy denial is `Denied`. Ordinary business failure is `Failed`. Neither is an
Attempt failure. Broken interaction routes and durable-store failures retain
their runtime-authority failure semantics.

The executor-start fact is a conservative recovery frontier, recorded before
physical work can begin. It is distinct from the physical effect frontier.
Handle construction must only capture resources; dispatch belongs inside the
handle. The generic entry and the cooperative handle check cancellation before
construction and before the first operation poll. Thus cancellation can close
admission without accidentally starting a synchronous mutation when settlement
first polls an otherwise-unstarted future. The owning lifecycle selects the
canonical cancellation phase from its logical start frontier.

The Agent Loop owns canonical call slots, sibling ordering and
`commit_tool_result_batch`; the shared native driver owns the resolved frozen
foreground policy and invocation cancellation arbitration. The batch commit appends
all sibling ToolMessages and their canonical facts atomically. The conversation
structural authority rejects duplicate ToolCall and ToolResult identities.
Physical completion order never changes canonical model-call order. Cancellation
fills every unstarted slot and drains every started sibling before the batch
commits. No executor or observer can replace a committed result.

## Physical evidence and terminal meaning

Executors own physical execution, effect-frontier knowledge, progress, physical
cancellation, and settlement evidence. Canonical history owns committed truth.
The Event Journal records facts; it is not another outcome authority. Providers
consume `ToolExecutionResult::model_facing_projection`, which derives bounded
feedback from typed status, never error substrings.

| Status | Evidence required |
| --- | --- |
| Success | Known successful completion |
| Failed | Known invocation/execution failure |
| Denied | Policy or approval refusal |
| Cancelled | Never started, or confirmed physical cancellation |
| TimedOut | Deadline intent plus confirmed physical cancellation/terminality |
| OutcomeUnknown | Started effect whose final outcome cannot be established |

A request to cancel is not evidence that the operation stopped. The foreground
arbitration order remains cancellation, hard deadline, idle deadline, completion.
Once intent wins, the owner drives only `ToolExecutionHandle::settlement`.
`Confirmed(result)` retains known success/failure/denial even when cancellation
was requested. Confirmed cancellation becomes `Cancelled` with the owning cause,
or `TimedOut` when the generic deadline won. `Unconfirmed` becomes
`OutcomeUnknown` only after local ownership is reclaimed; uncertainty concerns
an external effect, not a still-running rustX worker.

The foreground settlement guard remains a typed control-plane failure, separate
from executor-returned unconfirmed evidence. It never proves physical
termination. Executors must retain all local cleanup within their ownership
contract. No automatic retry, ambiguous replay, or foreground-to-background
switch is introduced.

The cooperative handle consumes its operation on completion, so a repeated poll
cannot execute it or produce another terminal winner. Invocation panics are
contained inside that ownership slot, avoiding poisoning the slot mutex and
unwinding accepted siblings. Construction panics are pre-dispatch `Failed`;
operation panics are conservatively `OutcomeUnknown`, never proof of rollback.
The operation future is consumed before evidence is returned. This boundary
does not make unmanaged child tasks or processes permissible: executors still
own their cleanup, and runtime/durable authority failures are not business
failures to hide. Panic payload text is not exposed to the model.

Bash retains its process-group supervision, termination ladder, and wait/reap
proof. Output readers and the combined writer are all joined even if one reader
fails, panics, or is aborted. A capture timeout aborts then joins the remaining
workers before publishing output or terminality. Read's blocking document
worker is joined even after cancellation; its JoinError is a typed failure.
MCP terminates local outbound/HTTP ownership and consumes its release proof
before reporting remote uncertainty. A remote `isError` is known failure;
post-dispatch transport loss without a correlated result is unknown outcome.
Python packages are managed MCP servers and use that same boundary.

## Detached ownership

`commit_dispatch` is the registry's ownership-transfer point: a prepared runner
is parked behind its gate until the record is accepted. The originating
ToolCall receives one canonical successful dispatch receipt. That result is
final; it is never replaced by the detached operation's later result.

The conversation-owned registry then owns the detached execution and its single
durable terminal inbound publication. The registry retains its logical start
frontier: cancellation while still Starting prevents executor admission and
settles with BeforeStart; cancellation after the start transition uses
DuringExecution. It drives completion until cancellation,
then consumes the executor's independent settlement plane. Foreground deadline
policy is intentionally absent. The same status meanings apply, including
`Denied`, `TimedOut`, and `OutcomeUnknown`. A durable terminal candidate is
retained during publication failure; repeated finish/cancel cannot replace it.
Terminal publication precedes the observable terminal state. Its deterministic
correlation prevents duplicate inbound messages. The model-facing `execution`
tool only routes observations/control to this authority.

Subagent creation similarly returns a creation result; the child registry owns
the child's later lifecycle. Workflow foreground execution retains its existing
Workflow and child authorities. `execution(steer)` uses the shared cooperative
handle while keeping the subagent registry's exact guidance admission frontier
and ticket cleanup. Cancelling a steer never invents child cancellation.

## Recovery

Recovery repairs only calls without a committed result. An existing canonical
result is authoritative and absorbing. Durable known-outcome evidence can fill
a missing result; start evidence without a trustworthy terminal outcome yields
`OutcomeUnknown`; no start evidence yields before-start cancellation. Repair
commits atomically and is idempotent. Detached ownership recovery publishes the
missing terminal notification without changing the original dispatch receipt.
Neither Tool recovery nor MCP connection restoration replays ambiguous effects.
A new MCP generation serves future calls only.

## Issue #206 conformance audit

The audit baseline is `bb91d64de8a8f51f0e8afcff31bfee68da47feb1`, including
#201, #202, #204, #205, and the merged #193 steering integration. All paths below
use the acceptance and commit authorities described above; no separate Python,
Workflow, or Subagent canonical lifecycle was introduced.

| Actual executor/origin/path | Baseline finding and disposition | Evidence |
| --- | --- | --- |
| Native Read, Write, Edit, Glob, Grep | Business errors already typed; shared first-poll cancellation and panic containment strengthened | Native module tests; executor settlement conformance tests |
| Native Read document task | Blocking decoder cancellation waits for join; JoinError already typed; unchanged | Read document cancellation/decoder tests |
| Native todo and ask_user | Business errors already typed; broken interaction authority deliberately fails outside business results | Native tests; scripted interaction and Agent policy suites |
| Bash foreground/background | Process semantics conform; capture drain abandoned siblings on early error and did not join aborted tasks; fixed | Capture panic/abort regression; Bash boundary and #204 deadline suites |
| MCP stdio/HTTP | Remote error, pre-dispatch refusal, post-dispatch uncertainty, local ownership release, progress, and reconnect already conform | `tests/boundary/mcp_recovery.rs`; MCP output/remote-error tests |
| Managed Python packages | No separate executor: package preparation precedes capability admission, invocation uses MCP; unchanged | Python package and MCP provider boundary suites |
| Subagent Tool | Parse/resolve/prepare/commit failures already typed; accepted creation transfers child authority; unchanged | Native subagent and staged-child boundary suites |
| Workflow Tool | Command rejection already Failed; typed cancellation and Workflow-owned child settlement retained | Workflow scripted/boundary suites |
| execution status/list/cancel | Domain routing already uses typed results and owns no lifecycle; unchanged | Scripted execution-intrinsic suite |
| execution steer | Effect frontier and ticket cleanup conform; duplicated shared operation ownership removed | Existing #193 steer cancellation/decision tests |
| Foreground sequential/parallel | Atomic batch commit, cancellation drain, deadline arbitration conform; executor panic could unwind batch; contained | #204/#136 and #206 parallel panic/failure/success and settlement-panic tests |
| Detached runner | Ignored independent settlement; pre-start cancellation still invoked work; panic orphaned runner; Denied projected as Failed; fixed | #206 background split-settlement/status matrix and panic test |
| Preflight / policy / approval | Structural errors precede acceptance; business/schema rejection and Denied occupy slots; unchanged | Malformed-proposal, scheduling, and pre-tool policy suites |
| Recovery/restoration | Missing-only repairs, committed-result absorption, no ambiguous replay conform; unchanged | Durable recovery and #202/#205 regressions |
| Provider-independent / Runtime Client / TUI | No production cancellation substring inference found; background Denied projection added | Typed status tests; protocol 17 denied regression |
| Optional subsystems | Generic foreground does not branch on origin or optional background support; registry exists in every ToolRuntime; unchanged | Shared foreground entry; background tests with/without event observer |

Runtime Client protocol 17 and SQLite development schema 23 carry the new
background denial vocabulary. Old versions are rejected, with no migrations or
compatibility decoding. The Event Journal envelope shape is unchanged.
Composite settlement retains a finite control guard: native child-scope leases
cover admitted child futures through physical settlement. The composite's guard
starts only after these owners drain (immediately if there are none), so child
cleanup does not compete with an equal outer timer. A composite that then fails
to settle still produces control-plane failure and OutcomeUnknown, not a false
confirmed timeout. The scope counter is ownership accounting, not another outcome
state machine or an executor registry.
## Candidate-bound native invocation

WF-03 supplies the exact authorized candidate through `ToolExecutionContext.workspace` to the same native invocation lifecycle. `ToolExecutor::honors_workspace` defaults false; unsupported executors fail admission, never fall back to parent cwd. Native filesystem executors and Bash honor the context. Candidate access precedes scheduling/approval and stays owned through physical settlement. Every candidate Tool is an exclusive validator: an actual source change invalidates its input certification even when native execution succeeded. `WorkflowCandidateInvocation` records the original native outcome, input candidate and unchanged flag. No Workflow-specific executor or model-text evidence path exists. See [the candidate contract](workflow-programs.md#run-scoped-candidate-workspace-wf-03).

A successful candidate projection receives CandidateReference directly from invoke_tool after physical settlement and source/mutation verification. The interpreter commits it beside JSON and preserves it through value construction and Parallel export. Derived arguments are checked against live currentness and rechecked under exclusive admission. A later writer can make the check stale; rejection belongs to Workflow consumption and leaves the native historical status intact. The Event Journal is not consulted for applicability.

Mutation observation covers existing empty directories under fixed entry/depth/watch limits. New directories on Linux and directory-entry events on macOS conservatively invalidate incomplete coverage; ignored file activity may remain excluded, but unknown coverage cannot certify a check. Watch/inspection uncertainty after a settled operation produces PhysicalSettlement resource retention and retirement of ended active ownership. OutcomeUnknown physical work retains NestedContainment authority. See the candidate contract for exact platform semantics and disposal continuation after failed durable settlement append.


### Fixed Workflow human waits

Fixed Tool(ask_user) uses ordinary preparation, normalization, execution and typed
result projection. ToolExecutionContext reborrow rebinds its existing Questionnaire
requester to the driver's subordinate cancellation/deadline view. Explicit decline
is the ordinary successful JSON result, not cancellation. Review is a Workflow
business node using the same InteractionCoordinator, outside Tool Approval.
FullAccess affects only the configured permission gate. The existing durable
Allow-before-unchanged-prepared-invocation-start ordering remains unchanged.

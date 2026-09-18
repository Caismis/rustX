# Goal Plugin admission design

## Lifecycle authority (Issue #351)

Durable `GoalPhase` is the one product-visible Goal lifecycle authority. There
is no process-local activation state and no `armed`/`disarmed` mode:

```text
Active   = continuation is authorized when runtime admission is eligible
Paused   = continuation is not authorized
Blocked  = continuation is not authorized
Complete = terminal
```

> **`GoalPhase::Active` is durable authorization to continue when runtime
> admission becomes eligible.**

Concretely: if a composed Goal is durably `Active`, rustX is authorized to
continue pursuing it whenever the owning `ConversationRuntime` reaches an
eligible safe idle admission boundary. Nothing else has to happen — there is no
arm, play or start operation after Create or Resume, and no state in which an
`Active` Goal is silently inert.

```text
None / Complete
      |
      | create
      v
    Active
      |
      +-- eligible safe idle boundary --> Goal continuation --> Active
      |
      +-- explicit pause / Goal-turn interrupt --> Paused
      |
      +-- genuine model blocker -----------> Blocked
      |
      +-- objective achieved --------------> Complete

Paused / Blocked --explicit resume--> Active
```

Two consequences are stated explicitly because they were previously conflated:

> **Runtime stopped is not Goal paused.** Drain and shutdown close runtime
> admission and supervise runtime-owned work. They never rewrite `Active` to
> `Paused`, and they never refund a committed round. Reopening that Session
> later resumes the Active Goal through ordinary admission.

> **Explicit interruption of an autonomous active Goal attempt durably pauses
> the Goal and requests cancellation of that attempt.** Both actions win their
> commit boundaries before the interrupt is reported successful.

Ownership stays split:

```text
Goal phase            -> GoalDomain (durable)
runtime availability  -> ConversationRuntime lifecycle
admission provenance  -> ConversationRuntime, per attempt, never persisted
```

The round linearization point is the SQLite commit that both inserts
ordinary Pending Inbound, updates Goal revision and consumed rounds, and records
the bounded Goal round-admission journal fact.
This extends the existing `accept_inbound_tx` transaction seam; neither local
reservation nor later canonical adoption consumes an additional round.

The coordinator checks eligibility at its existing idle admission boundary.
Eligibility is: the Goal extension is composed in the current root profile, the
durable phase is `Active`, autonomous budget remains, and the runtime
coordination conditions below hold. There is no activation precondition.
The driver runs synchronously there, under the coordinator lock, and enters
the mailbox lifecycle commit guard. The Goal publication mutex covers the
revision observation and durable commit; it is commit/publication ordering
only and holds no lifecycle value. The transaction checks the current
GoalRef and that Pending Inbound is empty before accepting anything. Human
acceptance and this transaction are serialized by the existing durable
connection/SQLite write lock: previously accepted Human input always wins.
All pending native result inbound also takes precedence.
An atomic acceptance storage error seals runtime admission through the existing
non-transient `goal_round_admission` durability failure owner and leaves the
durable phase truthfully `Active`. Runtime health and Goal intent are separate
concerns: the fence is absorbing, so there is no unbounded automatic retry
cycle and no charge for a failed transaction, and reopening through existing
recovery semantics decides future work.

Outstanding background/child ownership is held stable across that frontier using
the existing registry locks, in the order coordinator -> background -> Subagent ->
lifecycle commit -> Goal publication -> SQLite. The bounded `with_goal_idle` seams
inspect native active records while retaining their ownership locks. A concurrent
new work commit therefore either precedes the Goal check or follows the Goal's
durable frontier; it cannot slip between a detached snapshot and admission.

GoalDomain owns durable state through the conversation store and nothing else:
no runtime lifecycle state, no wake handle, no activation. GoalRoundDriver owns
no task or queue; the existing supervised coordinator worker owns its
synchronous execution. Drain closes the lifecycle commit guard and settles the
existing worker without touching durable phase. An explicit interrupt of an
autonomous Goal attempt commits `Active -> Paused` and then cancels that exact
attempt; committed rounds are never refunded.

A successful typed Create or Resume wakes the ordinary admission owner. That
wake is issued by `ConversationRuntime::control_goal` after the durable commit,
not by Goal code: it is a liveness notification, and every eligibility question
is re-answered under the coordinator lock against durable state. A model
`create_goal` needs no wake at all — it runs inside a Human attempt and reaches
the ordinary settlement/admission handoff when that attempt settles.

Create consumes zero rounds. Only driver-accepted continuation consumes a
round. Ordinary Human attempts, including the attempt creating a Goal,
consume zero. Accepted work recovers through ordinary Pending Inbound,
Request Snapshot and attempt recovery; there is no Goal replay.

## State and command contract

`agent.plugins.goal.enabled` defaults to false and is frozen per published
configuration generation. `/reload` publishes Plugin composition at the safe
boundary. When disabled, Goal Tools, context and driver are unavailable and
`/goal` reports feature-disabled. Conversation-owned stored state remains
untouched and can be projected when the Plugin is enabled again. Named child
definitions and invocation overrides share the closed syntax, but effective
enabled Goal fails `unsupported_child_scope` before child spawn. Workflow applies
that same check; root Goal is never implicitly inherited.

`goal_state` is one native singleton record inside the existing conversation
SQLite database (schema 40; Goal storage introduced in schema 34). Its bounded JSON snapshot stores GoalRef, objective,
phase, blocked reason, origin, autonomous budget/consumed count and the last
round's ordinary MessageId. Identity is conversation-scoped. Complete is terminal;
a subsequent create starts a new identity. Revision increases exactly once for
each successful mutation, including autonomous admission. Reads and refusals
increase nothing. SQLite's immediate write transaction checks the observed
GoalRef before mutation. A stale result includes current bounded state and is
never automatically retried against a newer revision.

The stable model Tool definitions are `get_goal`, `create_goal`, `update_goal`.
ExtensionToolPlane is derived from materialized owners, and ConversationRuntime
checks both configured and published Tool shapes against the frozen composition.
Phase changes publish no capability generation. Ordinary `agent.tools.builtin`
selection neither grants nor strips Goal Tools.

`create_goal` is permitted only when the Human clearly authorizes persistent
cross-turn pursuit, including ordinary natural-language requests such as “keep
working until …”. Complexity, length, many Tools, Workflow or Subagent use alone
do not justify it. This semantic boundary is explicit in the Tool description;
there is no keyword classifier or evaluator model. Runtime supplies the actual
Human MessageId and AttemptId through
`AgentExecution::goal_creation_authorization`: the current Human request owns one
consumable model Goal-creation authorization. Immediately after validating
`pending_fresh_inbound`, `prepare_model_turn` selects the most recent Human ordinary
`Message` in that batch's native order and replaces any previous authorization.
A batch containing only Runtime/background input leaves the current Human
authorization intact. The execution owns this state and lends it to foreground
Tools through `ToolExecutionContext`; it is not GoalDomain state.

Three lifetimes differ. Transport and compaction retries retain the same frozen
logical model request. A logical step ends after its model response and Tool
batch, consuming fresh-inbound context. The Human request's Goal authorization
survives those ordinary steps until replaced by newer Human input or consumed by
successful model `create_goal`. Thus read -> test -> create remains authorized.
The native adapter consumes it immediately after the authoritative create commits,
before result serialization or outer cancellation settlement. Invalid input,
unfinished-Goal rejection, storage failure, and cancellation/drain before commit
do not consume it. Completing a Goal never restores consumed authorization, even
when create -> complete -> create appears in one Tool batch.

A Human message adopted at a later safe boundary can therefore authorize creation
in subsequent steps, including in an attempt that started from non-Human continuation.
History outside that exact fresh batch is never searched for authorization, and
Tool execution never rediscovers an origin from history. Model arguments cannot
supply or rebind that correlation. Explicit client creation is
identified as RuntimeControl. Both paths enforce one unfinished Goal and native
bounds (objective/reason at most 8192 UTF-8 bytes; budget 1–100, default 10).

Authorization is process-local execution context. Recovery of ordinary Pending
Human Inbound adopts a fresh batch and installs its trusted identity as usual.
An already-adopted recovered continuation starts without Goal-create authority:
the existing `ContinueAdoptedTurn` report carries only an answer obligation, not
the current Human identity or whether its authorization is unused. Request
Snapshots freeze provider inputs but carry no such authorization. Consequently,
a crash after adoption can require a later Human message or explicit `/goal create`
before the model can create a Goal, even when the interrupted request was Human.
Preserving that right would require extending recovery's trusted request-boundary
contract; this repair does not infer rights from historical messages or Goal
journal facts, nor add a Goal replay log. Existing durable Goals still recover
from GoalDomain alone, with their durable phase intact.

The model may only declare Active -> Complete or Active -> Blocked with a reason.
Completion is a domain declaration, not evidence verification. User controls own
pause, resume, objective edits and bounded budget edits. Budget cannot drop
below already consumed rounds. Resume is `Paused|Blocked -> Active` and clears
the blocker; `Active -> Active` does not exist, so a resume against an Active
Goal is a rejected transition rather than a silent revision bump. Waiting for
owned work does not mark a Goal Blocked, and an Active Goal never opens a
polling attempt merely to discover whether owned work finished.

## Context, controls and recovery

Every relevant new root model step samples GoalDomain and contributes a boxed
typed `ContextKind::GoalStatus` snapshot through native Context Assembly. Its
User role preserves objective instruction priority. Request Snapshot freezes
this observation along with normal request inputs. History may show older
observations; those never supply current Goal authority. Provider adapters only
project the existing provider-neutral messages and Tool definitions.

Runtime Client protocol 40 includes the typed `goal` operation (Show/Create/Mutate)
and bounded `goal_changed { view: GoalView }` event. `GoalView` carries durable
state only — `{ current: GoalSnapshot | null }` — so no snapshot and no event can
represent `Active + disarmed`, and there is no activation-only `goal_changed`.
Version 39 removed the obsolete `armed` member; strict negotiation rejects
version 38 clients and no compatibility field is retained. `GoalDomain` revision
is not `RuntimeClientCursor`: the former versions durable state; the latter
governs all externally visible snapshot state. Two successful snapshots at the
same cursor cannot contain different Goal views.

The inactive `ConversationRuntime::install_observation_bridge` bootstrap cut reads
Goal with the other native state. Lifecycle gating prevents model/control writes
before activation; the coordinator lock excludes activation and installs the
Goal observer before releasing the cut. `RuntimeBootstrapSnapshot.goal` seeds
`RuntimeClientProjection` with the recovered durable view. Attach/snapshot
return only the projection's copy and never overwrite it with a later domain
read. Attach, reconnect and snapshot reads remain observations: none is an
independent start authority.

Under its publication mutex, GoalDomain commits state and publishes a bounded
authoritative view through `GoalObserver` -> ConversationRuntime's
`RuntimeObserver` -> the existing reliable observation queue ->
`RuntimeClientProjection::fold`. Create, pause, resume, block, complete, edit,
budget, and round admission all carry `GoalChanged`. Every `GoalChanged` follows
a committed durable transition; there is no activation-only observation, so
cancellation and drain publish a Goal change only when they actually changed
durable phase. The fold updates the read-model copy and publishes `goal_changed`
at a new cursor. Duplicate/no-change observations publish nothing; stale CAS and
reads produce no observation. Goal journal facts are INTERNAL in the existing
event mapping, never an alternative projection input. TUI folds the same typed
event and snapshot. `/goal` remains a client adapter, with no state or
scheduling authority.

### Recovery contract

```text
durable Active + durable state read only
  = no work starts

durable Active + Runtime Client attach/reconnect only
  = no independent start authority

durable Active + Session/ConversationRuntime explicitly opened/activated
              + first eligible safe idle boundary
  = Goal continuation may resume automatically
```

Opening/activating the runtime is the product act that makes the conversation
live again. Recovery restores the durable phase and nothing else — there is no
second arm operation to perform, and the obsolete "recovers Active but
disarmed" contract from #84 is gone. rustX adds no background daemon scanning
unopened Sessions, no Scheduler and no timers; a dormant Session is never loaded
merely because a durable Goal is Active. A Goal-disabled runtime composition
likewise never executes stored Goal state: existing extension-composition
semantics remain authoritative.

A previously accepted ordinary continuation follows the existing inbound/attempt
recovery evidence exactly once; its round remains consumed. Disabling and
re-enabling the extension never deletes or rewrites durable state. Event Journal
is not required to reconstruct Goal state.

## Execution facts and drain ownership

`RuntimeEvent::Goal { fact }` has two bounded fact forms:

- `Written { previous, current, phase, change }`, where `change` is Create, Pause,
  Resume, Block, Complete, Edit, or Budget. It omits objective and reason text.
- `RoundAdmitted { previous, current, round, message_id }`.

Both are durable state facts. The obsolete `ActivationChanged` fact is gone
along with the state it observed.

`write_goal` commits the new `goal_state` and Written fact in one Immediate SQLite
transaction. Failed CAS writes no fact. A journal failure rolls back the state
write. `accept_goal_round` commits accounting, ordinary Pending Inbound acceptance,
and RoundAdmitted in its original single SQLite transaction; rollback or process
death cannot split any of the three. Generic `append_event` refuses the two
compound fact forms, which require their specialized durable transition.
Recovery never reads Goal facts, whether present or missing: `goal_state` alone
supplies durable state, and the Event Journal remains observational/audit
evidence rather than recovery authority.

Model Goal mutations use the existing mailbox `with_running_commit` seam.
The foreground Tool retains ordinary AgentExecution ownership while its synchronous
operation holds lifecycle commit guard -> Goal publication mutex -> SQLite. It
checks cancellation under that mutex before writing. Runtime drain requests
attempt cancellation and commits Running -> Draining through the same lifecycle
guard, without any Goal mutation. A Tool that already passed
the cancellation check under those locks commits before drain; otherwise it is
refused by cancellation or the lifecycle guard. Drain waits for foreground Tool
settlement even when refused. No Tool takes the coordinator lock, and no Goal
lifecycle or worker was added. Deterministic gates before guard acquisition and
after the cancellation check prove both orders for create and update.

## Interrupt, drain and durability-failure ownership (Issue #351)

### Attempt provenance

`ConversationRuntime` records, at its one admission publication point, what it
admitted the attempt *for*:

```text
AttemptProvenance::Inbound               ordinary adopted inbound (Human and
                                         every other runtime-generated inbound)
AttemptProvenance::GoalContinuation      the adopted batch carries the durable
                                         Goal continuation admitted at the
                                         Goal-round frontier
AttemptProvenance::RecoveredContinuation recovery over already-canonical history
```

This is derived from the durable inbound the coordinator actually adopted — the
only place that knows it. It is runtime-owned admission provenance, never Goal
lifecycle state: it is not persisted, never appears in `GoalView`, never appears
in a Runtime Client snapshot or event, is not user-visible, and disappears with
the attempt it describes. It exists so the runtime can distinguish an autonomous
Goal attempt from an ordinary Human attempt, from a Human attempt during which
`create_goal` happened, and from recovery continuation.

### Interrupt ordering

Under the one coordinator lock, which also owns Goal-round admission and the
Goal control commit boundary, `cancel_current_attempt` runs:

```text
1. prove the named attempt is the current attempt and read its provenance
2. if and only if it is a Goal continuation: durably commit Active -> Paused
3. request cancellation of that same attempt
4. report acceptance only after both boundaries are won
```

The durable pause commits *before* cancellation is requested, so there is no
window in which the Goal is `Active` with a cancelled attempt. Because step 2 is
gated on provenance, cancelling an ordinary Human attempt never pauses an Active
Goal merely because one exists. Explicit `/goal pause` remains an independent
durable user control.

Both sides of the Goal-round frontier are deterministic:

```text
pause/interrupt wins before the frontier
  -> Active -> Paused commits, no round is accepted, the round count is
     unchanged, and no later autonomous Goal admission occurs

the frontier wins first
  -> the round is durably accepted and stays consumed, the interrupt pauses the
     Goal, the current attempt receives cancellation, nothing is refunded, and
     no subsequent Goal round is admitted while Paused
```

If the durable pause does not commit, nothing fabricates one. The attempt is
still cancelled (containment), a genuine storage fault is recorded through the
existing absorbing `goal_pause` durability owner — which fences all further
admission, so no later Goal round can be admitted even though the phase still
reads `Active` — and the caller receives `CancelAttemptError::GoalPauseFailed`
rather than a success. A lifecycle refusal (the runtime no longer admits
semantic commits, and therefore no longer admits Goal rounds either) is reported
the same way but earns no durability fact.

### Drain

Runtime drain means `runtime availability -> Draining/Quiescent`. It closes new
runtime admission, supervises/cancels current runtime-owned work under the
existing rules, and leaves `GoalPhase::Active` exactly as it is. No post-drain
Goal round can be admitted, and reopening/activating that Session later resumes
the Active Goal through ordinary admission. No durable Goal mutation is ever
introduced merely to express runtime unavailability.

### Idle residency

Residency ownership follows real semantics rather than a replacement activation
bit. A composed Goal that is durably `Active` with autonomous budget remaining
still owns future autonomous work, so idle eviction must not race its eligible
continuation. `Paused`, `Blocked`, `Complete`, an absent Goal, an uncomposed
extension, and an `Active` Goal whose autonomous budget is exhausted all own
nothing — which is why an exhausted Goal cannot spin to pin residency forever.
A successful Create or Resume commits through the existing lifecycle boundary
and advances the runtime activity token, so a stale idle probe cannot cross a
later continuation authorization.

Goal = WHAT, Workflow = HOW, Scheduler = WHEN, Subagent = WHO, Tool = DO, Todo =
bounded working state. This feature adds no plugin registry, separate Agent Loop,
private queue, replay engine, provider prompt mode, evaluator, resumable child,
multiple-goal scheduler, distributed worker or Workflow-owned Goal state.

## Deterministic conformance map

Numbers below refer to the 44 acceptance contracts in issue #84. Test names are
searchable suffixes; all new Rust tests use the `goal84_` prefix. Tests use the
real SQLite store, channel/Barrier order, coordinator gates, watched provider
state, native owned-work registries, or the SIGKILL rendezvous harness. Timeouts
are liveness guards, never correctness ordering.

| Contracts | Owning tests |
| --- | --- |
| 1, 17–19, 41–43 | `goal351_recovery_starts_only_when_a_composed_runtime_is_explicitly_opened`; `goal351_file_reopen_uses_durable_state_even_without_the_journal`; existing `ext256_an_empty_extension_composition_changes_nothing_but_agent_status` |
| 2–4 | `natural_intent_creates_from_human_with_stable_tools_and_current_context`; `model_schema_is_stable_narrow_and_source_cannot_be_spoofed`; `commands_are_rejected_on_every_ordinary_tool_selection_surface`; `partial_or_altered_goal_tools_cannot_masquerade_as_disabled_composition` |
| 5–7, 11–13 | `goal351_phase_is_the_only_lifecycle_authority_under_cas`; `revision_races_have_one_durable_winner`; `model_schema_is_stable_narrow_and_source_cannot_be_spoofed` |
| 8–10 | `natural_intent_creates_from_human_with_stable_tools_and_current_context`; `model_schema_is_stable_narrow_and_source_cannot_be_spoofed` (semantic intent instructions, strict argument vocabulary, actual Human MessageId/AttemptId binding) |
| 14–16 | `natural_intent_creates_from_human_with_stable_tools_and_current_context` (unchanged and changed revisions, typed User-role observations) plus existing Context Assembly and provider projection contracts |
| 20–22 | `human_before_frontier_and_single_concurrent_reservation`; `human_and_goal_acceptance_share_a_deterministic_durable_order` |
| 23–26 | `revision_races_have_one_durable_winner` (both commit orders for pause, blocked, complete, objective and budget mutations) |
| 27–29, 36 | `goal351_pause_or_drain_before_the_round_frontier_consumes_no_round`; `goal351_create_while_idle_admits_one_continuation_and_drain_preserves_active`; `goal351_atomic_round_frontier_survives_a_fresh_domain_owner` |
| 30 | `process_death_cannot_split_round_accounting_from_ordinary_pending_work` (SIGKILL immediately before/after commit, repeated ordinary recovery) |
| 31–33 | `goal351_create_while_idle_admits_one_continuation_and_drain_preserves_active`; `natural_intent_creates_from_human_with_stable_tools_and_current_context`; `goal351_paused_goal_admits_zero_rounds_and_a_human_turn_charges_no_budget`; TUI typed create command test |
| 34–35 | `goal351_owned_background_is_awaited_without_polling_or_blocking_goal`; `owned_child_and_terminal_publication_prevent_goal_polling` |
| 37 | `workflow_completion_and_goal_completion_are_independent` |
| 38–40 | `root_only_scope_is_enforced_for_definition_model_and_workflow_overrides`; `workflow_program_cannot_enable_goal_for_an_agent_node`; `closed_vocabulary_is_opt_in_and_children_are_refused` |
| 44 | Native typed context/Tool tests above, unchanged provider implementations, and the complete `provider` target |

TUI command tests additionally prove typed creation and revision-pinned pause,
including a stale response that is surfaced rather than automatically retried.
The Runtime Client backpressure test waits for actual runtime settlement before
asserting that its reconnect cursor is serviceable; provider emission alone is
not a settlement barrier.

## Architecture review

The only durable Goal writes use the conversation store's Goal-specific CAS and
atomic acceptance transactions. There is exactly one Goal lifecycle source of
truth, `GoalSnapshot.phase`, and no process-local field whose value changes
whether `Active` actually means active. GoalRoundDriver has no task, model adapter, execution loop,
history, capability-generation mutation or private queue. Its sole caller is the
ordinary coordinator at idle. The Goal reservation and activation lock, existing
owned-work locks, lifecycle guard and SQLite transaction establish exact winners.
Ordinary acceptance rejects typed Goal continuation outside the accounting seam.
Current context samples domain state at each request boundary and stays User data.
Child scope fails in the shared resolver before ownership. Event Journal is never
read by GoalDomain. Drain settles the existing worker; no Goal-owned task can
outlive it. No generic plugin, state, scheduling or transaction framework was added.


## Issue #351 regression map

| #351 requirement | Owning test |
| --- | --- |
| 1 explicit create while idle admits exactly one continuation | `goal351_create_while_idle_admits_one_continuation_and_drain_preserves_active` |
| 2 model `create_goal` starts no nested attempt | `goal351_model_create_goal_starts_no_nested_attempt_and_continues_after_settlement` |
| 3 recovered Active resumes after explicit reopen | `goal351_recovery_starts_only_when_a_composed_runtime_is_explicitly_opened` |
| 4 storage read alone starts nothing | `goal351_recovery_starts_only_when_a_composed_runtime_is_explicitly_opened` |
| 5 client attach/reconnect is no start authority | `goal351_recovery_starts_only_when_a_composed_runtime_is_explicitly_opened` |
| 6 Paused admits zero rounds | `goal351_paused_goal_admits_zero_rounds_and_a_human_turn_charges_no_budget`; `goal351_stopped_phases_admit_nothing_and_resume_alone_restores_eligibility` |
| 7 Blocked admits zero rounds | `goal351_stopped_phases_admit_nothing_and_resume_alone_restores_eligibility`; `goal351_paused_blocked_complete_and_exhausted_admit_zero_rounds` |
| 8 Complete admits zero rounds | `goal351_stopped_phases_admit_nothing_and_resume_alone_restores_eligibility`; `goal351_paused_blocked_complete_and_exhausted_admit_zero_rounds` |
| 9 Resume needs no second operation | `goal351_stopped_phases_admit_nothing_and_resume_alone_restores_eligibility`; `goal_resume_and_idle_claim_have_both_winner_orders` |
| 10 interrupt pauses and cancels that exact attempt | `goal351_interrupting_a_goal_continuation_pauses_it_and_cancels_that_attempt` |
| 11 unrelated Human cancel never pauses a Goal | `goal351_cancelling_an_unrelated_human_attempt_never_pauses_an_active_goal` |
| 12 winner before the frontier consumes no round | `goal351_pause_or_drain_before_the_round_frontier_consumes_no_round` |
| 13 interrupt after the frontier does not refund | `goal351_interrupting_a_goal_continuation_pauses_it_and_cancels_that_attempt` |
| 14 no later Goal round once Paused | `goal351_interrupting_a_goal_continuation_pauses_it_and_cancels_that_attempt` |
| 15 shutdown preserves durable Active | `goal351_create_while_idle_admits_one_continuation_and_drain_preserves_active`; `goal351_pause_or_drain_before_the_round_frontier_consumes_no_round` |
| 16 no post-drain Goal work | `goal351_create_while_idle_admits_one_continuation_and_drain_preserves_active` |
| 17 reopening the preserved Active Goal resumes | `goal351_recovery_starts_only_when_a_composed_runtime_is_explicitly_opened`; `a_reopened_active_goal_owns_residency_until_it_is_paused` |
| 18 accepted Human inbound wins | `goal351_model_mutation_and_drain_have_one_owned_commit_order`; `goal84_human_before_frontier_and_single_concurrent_reservation` |
| 19 outstanding background work prevents polling | `goal351_owned_background_is_awaited_without_polling_or_blocking_goal` |
| 20 outstanding Subagent work prevents polling | `goal84_owned_child_and_terminal_publication_prevent_goal_polling` |
| 21 round durability failure fences without pausing | `goal351_a_round_durability_failure_fences_admission_without_changing_active` |
| 22 stale CAS cannot overwrite a newer transition | `goal351_phase_is_the_only_lifecycle_authority_under_cas`; `goal84_revision_races_have_one_durable_winner` |
| 23 at most one durable round at the frontier | `goal84_human_before_frontier_and_single_concurrent_reservation` |
| 24 budget exhaustion does not busy-loop | `goal351_paused_blocked_complete_and_exhausted_admit_zero_rounds`; `goal351_recovery_starts_only_when_a_composed_runtime_is_explicitly_opened`; `goal351_a_round_durability_failure_fences_admission_without_changing_active` |
| 25 no activation in snapshots/events | `goal351_runtime_client_projection_same_cursor_controls_and_replay` |
| 26 WebUI renders no "Inactive Goal" | web-console `status and the single lifecycle control derive from durable phase alone`; browser acceptance `composer.spec.ts` |
| 27 TUI carries no product arm/re-arm state | TUI `creates through the typed control without an inbound turn`; `goal351 folds every durable Goal phase change at its stream cursor` |
| 28 trace still identifies the native Goal tools | `goal351_trace_still_identifies_the_exact_native_goal_tools` |
| 29 disabled composition does not execute stored state | `goal351_recovery_starts_only_when_a_composed_runtime_is_explicitly_opened` |
| 30 re-enabling follows the new recovery contract | `goal351_recovery_starts_only_when_a_composed_runtime_is_explicitly_opened` |

## Review repair regressions

- `goal351_runtime_client_projection_same_cursor_controls_and_replay`:
  parks the projection after real control commits; snapshot and attach retain the
  old Goal/cursor, then exact folds advance cursors, deliver events, and replay.
  Stale CAS emits nothing, and the serialized snapshot and event are asserted to
  contain no activation vocabulary at all.
- `goal84_runtime_client_subscriber_sees_model_create_block_and_complete`:
  subscribers present before invocation observe model create and block/complete.
- `goal351_create_while_idle_admits_one_continuation_and_drain_preserves_active`:
  live create and atomic round accounting reach subscribers; shutdown publishes no
  Goal observation because it commits no durable Goal transition.
- `goal84_safe_boundary_non_human_start_authorizes_exact_later_human` and
  `goal84_safe_boundary_human_b_replaces_a_survives_steps_and_is_consumed`:
  a gated first request accepts two Humans and a Runtime tail; the next request
  sees that batch, performs read and test steps, then creates from the newer Human.
  A subsequent create after completion cannot fall back to either older Human.
- `goal84_human_authorization_survives_read_and_test_steps_before_create` and
  `goal84_safe_boundary_runtime_input_preserves_human_authorization`: delayed
  creation keeps the exact Human/Attempt identity, including across Runtime input,
  and consumes no autonomous round.
- `goal84_successful_create_consumes_authorization_even_within_one_tool_batch` and
  `goal84_failed_create_retains_authorization_for_later_valid_create`: successful
  creation consumes the right; a bounded-value rejection does not.
- `goal84_recovery_authorizes_pending_human_but_does_not_infer_continuation_authority`:
  durable crash prefixes distinguish fresh pending Human adoption from an
  already-adopted continuation that has no recovered authorization identity.
- `goal351_model_mutation_and_drain_have_one_owned_commit_order`: four gated
  interleavings (create/update, Tool/drain wins), awaited to full quiescence. Its
  admission gate also proves accepted Human inbound wins over automatic Goal
  continuation: the Goal-round frontier is never reached while inbound is pending.
- `goal351_journal_failure_rolls_back_the_durable_write` and
  `goal351_atomic_round_frontier_survives_a_fresh_domain_owner`: failed journal or
  admission commit cannot expose half a durable transition, and a fresh
  process-local domain owner inherits the durable phase and nothing else.
- `goal351_file_reopen_uses_durable_state_even_without_the_journal`: reopening
  after deleting the journal produces the same durable Goal, still Active and
  still authorized. The process-death regression checks the admission fact and
  Pending Inbound/accounting on both sides of SIGKILL.
- TUI `goal351 folds every durable Goal phase change at its stream cursor`:
  event and snapshot read models agree, and the folded view has no activation
  member to disagree about.

# Goal extension admission design

The round linearization point is the SQLite commit that both inserts
ordinary Pending Inbound, updates Goal revision and consumed rounds, and records
the bounded Goal round-admission journal fact.
This extends the existing `accept_inbound_tx` transaction seam; neither local
reservation nor later canonical adoption consumes an additional round.

The coordinator checks eligibility at its existing idle admission boundary.
The driver runs synchronously there, under the coordinator lock, and enters
the mailbox lifecycle commit guard. The Goal activation lock covers the
revision observation and durable commit. The transaction checks the current
GoalRef and that Pending Inbound is empty before accepting anything. Human
acceptance and this transaction are serialized by the existing durable
connection/SQLite write lock: previously accepted Human input always wins.
All pending native result inbound also takes precedence.
An atomic acceptance storage error disarms Goal and seals runtime admission through
the existing non-transient `goal_round_admission` durability failure owner. It does
not enter an unbounded automatic retry cycle or charge a failed transaction.

Outstanding background/child ownership is held stable across that frontier using
the existing registry locks, in the order coordinator -> background -> Subagent ->
lifecycle commit -> Goal activation -> SQLite. The bounded `with_goal_idle` seams
inspect native active records while retaining their ownership locks. A concurrent
new work commit therefore either precedes the Goal check or follows the Goal's
durable frontier; it cannot slip between a detached snapshot and admission.

GoalDomain owns durable state through the conversation store. Its shared
process-local activation starts disarmed and is never reconstructed from
phase or events. GoalRoundDriver owns no task or queue: the existing
supervised coordinator worker owns its synchronous execution. Drain closes
the lifecycle commit guard and settles the existing worker. Cancellation
disarms without changing durable phase or refunding committed rounds.

Create consumes zero rounds. Only driver-accepted continuation consumes a
round. Ordinary Human attempts, including the attempt creating a Goal,
consume zero. Accepted work recovers through ordinary Pending Inbound,
Request Snapshot and attempt recovery; there is no Goal replay.

## State and command contract

`extensions.goal.enabled` defaults to false and is frozen for the launch. With it
disabled there is no GoalDomain composition, model Tool, context or driver, and
`/goal` reports feature-disabled. Stored state remains untouched. Named child
definitions and invocation overrides share the closed syntax, but effective
enabled Goal fails `unsupported_child_scope` before child spawn. Workflow applies
that same check; root Goal is never implicitly inherited.

`goal_state` is one native singleton record inside the existing conversation
SQLite database (schema 34). Its bounded JSON snapshot stores GoalRef, objective,
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
Phase changes publish no capability generation. Ordinary `--tools`/`defaultTools`
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
from GoalDomain alone and start disarmed.

The model may only declare Active -> Complete or Active -> Blocked with a reason.
Completion is a domain declaration, not evidence verification. User controls own
pause, resume/re-arm, objective edits and bounded budget edits. Budget cannot drop
below already consumed rounds. An explicit Active re-arm also uses CAS and
increments revision. Pause/block/complete disarm. Waiting for owned work does not
mark a Goal Blocked.

## Context, controls and recovery

Every relevant new root model step samples GoalDomain and contributes a boxed
typed `ContextKind::GoalStatus` snapshot through native Context Assembly. Its
User role preserves objective instruction priority. Request Snapshot freezes
this observation along with normal request inputs. History may show older
observations; those never supply current Goal authority. Provider adapters only
project the existing provider-neutral messages and Tool definitions.

Runtime Client protocol 31 includes the typed `goal` operation (Show/Create/Mutate)
and bounded `goal_changed { view: GoalView }` event. `GoalDomain` revision is not
`RuntimeClientCursor`: the former versions durable state; the latter governs all
externally visible snapshot state, including Goal activation. Two successful
snapshots at the same cursor cannot contain different Goal views.

The inactive `ConversationRuntime::install_observation_bridge` bootstrap cut reads
Goal with the other native state. Lifecycle gating prevents model/control writes
before activation; the coordinator lock excludes activation and installs the
Goal observer before releasing the cut. `RuntimeBootstrapSnapshot.goal` seeds
`RuntimeClientProjection`: absent when disabled, `{ current, armed: false }` on
recovery when composed. Attach/snapshot return only the projection's copy and
never overwrite it with a later domain read.

Under its mutex, GoalDomain commits state and publishes a bounded authoritative
view through `GoalObserver` -> ConversationRuntime's `RuntimeObserver` -> the
existing reliable observation queue -> `RuntimeClientProjection::fold`. Create,
pause, resume/re-arm, block, complete, edit, budget, and round admission all carry
`GoalChanged`; an activation-only cancellation/drain carries `GoalDisarmed`.
The fold updates the read-model copy and publishes `goal_changed` at a new cursor.
Disarm preserves the durable revision and phase. Duplicate/no-change observations
publish nothing; stale CAS and reads produce no observation. Goal journal facts
are INTERNAL in the existing event mapping, never an alternative projection input.
TUI folds the same typed event and snapshot. Attach/reconnect/read never arms.
`/goal` remains a client adapter, with no state or scheduling authority.

Recovery creates a new disarmed activation owner even for durable Active. It
enqueues nothing. A previously accepted ordinary continuation follows the existing
inbound/attempt recovery evidence exactly once; its round remains consumed.
Disabling and re-enabling the extension never deletes durable state or implicitly
re-arms it. Event Journal is not required to reconstruct Goal state or activation.

## Execution facts and drain ownership

`RuntimeEvent::Goal { fact }` has three bounded fact forms:

- `Written { previous, current, phase, change }`, where `change` is Create, Pause,
  Resume, Block, Complete, Edit, or Budget. It omits objective and reason text.
- `RoundAdmitted { previous, current, round, message_id }`.
- `ActivationChanged { armed }`, a conversation-scoped process observation.

`write_goal` commits the new `goal_state` and Written fact in one Immediate SQLite
transaction. Failed CAS writes no fact. A journal failure rolls back the state
write. `accept_goal_round` commits accounting, ordinary Pending Inbound acceptance,
and RoundAdmitted in its original single SQLite transaction; rollback or process
death cannot split any of the three. Generic `append_event` refuses the two
compound fact forms, which require their specialized durable transition.
ActivationChanged is best-effort audit after the activation mutex transition wins,
still under that mutex for ordering. Audit failure cannot undo disarm or suppress
its live observation. Recovery never reads Goal facts, whether present, missing,
or recording historical armed=true: `goal_state` supplies durable state and a new
process-local owner always starts disarmed.

Model Goal mutations use the existing mailbox `with_running_commit` seam.
The foreground Tool retains ordinary AgentExecution ownership while its synchronous
operation holds lifecycle commit guard -> Goal activation mutex -> SQLite. It
checks cancellation under the Goal mutex before writing. Runtime drain requests
attempt cancellation, disarms under that same Goal mutex, and commits
Running -> Draining through the same lifecycle guard. A Tool that already passed
the cancellation check under those locks commits before drain; otherwise it is
refused by cancellation or the lifecycle guard. Drain waits for foreground Tool
settlement even when refused. No Tool takes the coordinator lock, and no Goal
lifecycle or worker was added. Deterministic gates before guard acquisition and
after the cancellation check prove both orders for create and update.

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
| 1, 17–19, 41–43 | `disabled_reenabled_and_reconnect_never_rearm_or_start_a_request`; `file_reopen_uses_domain_even_when_goal_journal_is_removed`; existing `ext256_an_empty_extension_composition_changes_nothing_but_agent_status` |
| 2–4 | `natural_intent_creates_from_human_with_stable_tools_and_current_context`; `model_schema_is_stable_narrow_and_source_cannot_be_spoofed`; `commands_are_rejected_on_every_ordinary_tool_selection_surface`; `partial_or_altered_goal_tools_cannot_masquerade_as_disabled_composition` |
| 5–7, 11–13 | `cas_state_machine_revision_and_terminal_authority`; `revision_races_have_one_durable_winner`; `model_schema_is_stable_narrow_and_source_cannot_be_spoofed` |
| 8–10 | `natural_intent_creates_from_human_with_stable_tools_and_current_context`; `model_schema_is_stable_narrow_and_source_cannot_be_spoofed` (semantic intent instructions, strict argument vocabulary, actual Human MessageId/AttemptId binding) |
| 14–16 | `natural_intent_creates_from_human_with_stable_tools_and_current_context` (unchanged and changed revisions, typed User-role observations) plus existing Context Assembly and provider projection contracts |
| 20–22 | `human_before_frontier_and_single_concurrent_reservation`; `human_and_goal_acceptance_share_a_deterministic_durable_order` |
| 23–26 | `revision_races_have_one_durable_winner` (both commit orders for pause, blocked, complete, objective and budget mutations) |
| 27–29, 36 | `cancel_and_drain_win_before_the_gated_admission_frontier`; `driver_uses_ordinary_admission_consumes_one_and_drain_disarms`; `atomic_round_frontier_failure_recovery_and_no_refund` |
| 30 | `process_death_cannot_split_round_accounting_from_ordinary_pending_work` (SIGKILL immediately before/after commit, repeated ordinary recovery) |
| 31–33 | `driver_uses_ordinary_admission_consumes_one_and_drain_disarms`; `natural_intent_creates_from_human_with_stable_tools_and_current_context`; `cancellation_before_admission_consumes_zero_and_human_turn_does_not_charge_budget`; TUI typed create command test |
| 34–35 | `owned_background_is_awaited_without_polling_or_blocking_goal`; `owned_child_and_terminal_publication_prevent_goal_polling` |
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
atomic acceptance transactions. Activation is exclusively the shared process-local
GoalDomain mutex. GoalRoundDriver has no task, model adapter, execution loop,
history, capability-generation mutation or private queue. Its sole caller is the
ordinary coordinator at idle. The Goal reservation and activation lock, existing
owned-work locks, lifecycle guard and SQLite transaction establish exact winners.
Ordinary acceptance rejects typed Goal continuation outside the accounting seam.
Current context samples domain state at each request boundary and stays User data.
Child scope fails in the shared resolver before ownership. Event Journal is never
read by GoalDomain. Drain settles the existing worker; no Goal-owned task can
outlive it. No generic plugin, state, scheduling or transaction framework was added.


## Review repair regressions

- `goal84_runtime_client_projection_same_cursor_controls_activation_and_replay`:
  parks the projection after real control commits; snapshot and attach retain the
  old Goal/cursor, then exact folds advance cursors, deliver events, and replay.
  Stale CAS emits nothing; disarm changes activation without a durable revision.
- `goal84_runtime_client_subscriber_sees_model_create_block_and_complete`:
  subscribers present before invocation observe model create and block/complete.
- `goal84_driver_uses_ordinary_admission_consumes_one_and_drain_disarms`:
  live create, atomic round accounting, and shutdown disarm each reach subscribers.
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
- `goal84_model_mutation_and_drain_have_one_owned_commit_order`: four gated
  interleavings (create/update, Tool/drain wins), awaited to full quiescence.
- `goal84_journal_failure_rolls_back_durable_write_but_not_activation` and
  `goal84_atomic_round_frontier_failure_recovery_and_no_refund`: failed journal or
  admission commit cannot expose half a durable transition.
- `goal84_file_reopen_uses_domain_even_when_goal_journal_is_removed`: recovery with
  armed audit facts and after deleting the journal produces the same durable Goal,
  always disarmed. The process-death regression checks the admission fact and
  Pending Inbound/accounting on both sides of SIGKILL.
- TUI `goal84 folds live Goal and activation changes at their stream cursors`:
  event and snapshot read models agree without changing a Goal revision on disarm.

# Goal extension admission design

The round linearization point is the SQLite commit that both inserts
ordinary Pending Inbound and updates Goal revision and consumed rounds.
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
initiating Human MessageId and AttemptId from admitted canonical input. Model
arguments cannot supply or rebind that correlation. Explicit client creation is
identified as RuntimeControl. Both paths enforce one unfinished Goal and native
bounds (objective/reason at most 8192 UTF-8 bytes; budget 1–100, default 10).

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

Runtime Client protocol 30 adds the typed `goal` operation (Show/Create/Mutate),
current Goal snapshot/ref and process-local `armed` projection. Goal has its own
revision domain; snapshot reads sample current domain state without reconstructing
it from the observation cursor. Attach/reconnect/read never arms. `/goal` is a
client adapter over those operations and has no scheduler or state authority.

Recovery creates a new disarmed activation owner even for durable Active. It
enqueues nothing. A previously accepted ordinary continuation follows the existing
inbound/attempt recovery evidence exactly once; its round remains consumed.
Disabling and re-enabling the extension never deletes durable state or implicitly
re-arms it. Event Journal is not required to reconstruct Goal state or activation.

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
| 1, 17–19, 41–43 | `disabled_reenabled_and_reconnect_never_rearm_or_start_a_request`; `file_reopen_and_disabled_launch_preserve_domain_without_journal`; existing `ext256_an_empty_extension_composition_changes_nothing_but_agent_status` |
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

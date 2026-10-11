# Fixed Workflow product conformance

See [canonical named Agent resources](subagent-resources.md) for schema 8
Agent files, discovery/admission, bounded roots, source provenance, and frozen
reload/child contracts.


The shipped programs are `parallel_review` and `implement_and_review` in
[`examples/local-runtime`](../examples/local-runtime/README.md). Both enter through
an explicitly registered foreground Tool. The reference set covers Agent, Tool,
Branch, Parallel, Review, Loop and Return without new grammar or execution APIs.

## Composed scenarios and authoritative owner tests

The four shipped-program scenarios live in
[`tests/conformance/workflow.rs`](../tests/conformance/workflow.rs). They copy the
actual workspace and selection, substitute only local provider configuration,
and use real native child processes, file writes, Bash supervision and root HITL.
The emulator emits model responses; it never executes tools or supplies check results.

| Contract | Product composition and owning regression |
| --- | --- |
| Outer Tool boundary | `shipped_repair_uses_real_writes_checks_and_root_human_decisions`: 8 requests = parent admission + plan + child question + 2 writer requests + 2 child output requests + parent continuation; one canonical ToolResult whose JSON is exactly the declared output. At the plan Review exactly 2 requests exist and no writer has started. |
| Real verification | Same scenario: first native write produces an incorrect greeting despite model prose claiming success; real Bash/Python check fails, second write fixes it, second check passes. Exactly 2 writer Agents and 2 checker executions. `shipped_repair_checker_tampering_cannot_redefine_frozen_verification` additionally approves a fake checker printing `passed` written into the shared workspace: wrong source still commits failed feedback, then corrected source passes, and the fake file stays in place. The same frozen command is approved in both iterations: 8 requests, 3 AgentRuns, 2 bodies, 3 writes, 2 native checks, 5 approvals, 2 questions and 1 Review. |
| Structured dataflow | Shipped Parallel children reject parent-only history in provider expectations; downstream quality pass receives only declared assessment. Repair excludes private model claims and child questions from parent continuation. Native lexical-scope tests reject sibling/optional-path reads. |
| Fixed Parallel | `shipped_parallel_reverses_completion_without_git_or_parent_orchestration`: first response remains gated until the second child's native settled publication. Exactly 3 AgentRuns, 5 provider requests, one result, no Git initialization. `reversing_completions_preserves_keyed_outputs_failures_and_join_commit_order` and `nested_capacity_one_blocks_never_reserve_descendant_capacity` own keyed all-settle and capacity-one detail. |
| Shared workspace | The shipped reference runs over a plain non-Git workspace: Implementer writes land in the Session workspace, the frozen checker observes them in place, and the final bytes remain after every outcome. Native `implement_check_repair_runs_in_the_shared_session_workspace` proves the exact binding for each child and that no copy exists; `rejection_after_a_write_never_rolls_it_back` and `pending_review_does_not_block_an_independent_parallel_branch` own post-write Review and lease-free Review. |
| Human participation | Shipped repair answers fixed and child Questionnaires, accepts the plan Review, and allows 2 child writes plus 2 fixed check invocations through root Runtime Client. The parent never relays them. `fixed_question_and_review_use_root_client_while_parent_model_remains_in_outer_call` also covers rejection and wrong-route responses; `explicit_review_branch_gates_implementation` proves rejection takes the declared Return without admitting the gated Agent. |
| Bounded repair | Shipped success: 2 bodies, 3 total AgentRuns, 8 requests, 2 checks. `shipped_repair_exhaustion_leaves_completed_writes_without_another_body`: exactly 3 bodies, 4 AgentRuns, 10 requests, 3 checks; explicit needs_revision and the last write left in place. Native `cancellation_at_second_iteration_frontier_consumes_and_starts_nothing_new` proves zero next admissions when cancellation wins. |
| Version binding | `loop_review_and_questionnaire_reject_old_responses_and_allocate_fresh_instances`, `review_wrong_instance_subject_and_kind_leave_original_pending_detach_resync_is_live_only` and root routing regressions reject stale iteration, subject, owner and interaction identities. |
| Failure certainty | `every_native_non_success_survives_the_actual_outer_adapter_once` and `every_native_non_success_remains_outer_status_without_a_second_iteration` preserve Failed, Denied, Cancelled, TimedOut and OutcomeUnknown. No failure takes a normal Loop exit port or gets replayed. |
| Physical settlement | Native Bash supervision and the subagent registry own physical settlement; the repair Loop admits the next writer only after the previous body, including its checker, settled (`implement_check_repair_runs_in_the_shared_session_workspace` observes the iteration frontier). |
| Observation/resync | Native read-model snapshot/publication races and `native_observation_lag_does_not_change_loop_requests_or_outcome`; real TUI stdio `native Workflow retirement preserves visible Tool identity through stdio, reconnect and reopen`. Viewing is not execution. |
| Process death | Native interaction recovery suites and the real stdio reopen test retain only historical evidence, never old executable nodes or actionable human authority. |
| Bounds | Native 100-iteration observation-loss test, aggregate value/step/Agent budget tests, capacity/drain suites, and real nine-run retirement scenario (10 provider requests, 8 retained detailed runs). |

The owner tests are in `src/runtime/workflow/tests/{scoped,tools,tool_composition,
shared_workspace,human,loops}.rs`, `src/runtime/workflow/read_model.rs`, the existing
interaction suites and `tui/test/integration.test.ts`. They remain the
source of truth for detailed races; this slice does not duplicate their matrices.

Every shipped YAML is discovered from its canonical root and compiled in the pure
`contracts` target. Representative invalid references, unavailable profiles,
unadmitted selectors, cross-scope reads, zero limits, unavailable producers and
missing entries are rejected without model execution. The process example suite
also composes all discovered resources. `cargo test --all-targets --all-features`
includes both targets; CI's explicit target split retains them.

## Ownership and frontiers

Workflow owns the finite program, values, iteration budgets and run settlement.
Agent Loop owns model turns and the single canonical outer result. Native Tool
invocation/process owners own preparation, permissions, execution and physical
settlement. InteractionCoordinator owns rendezvous. Workflow owns no workspace
resource: Agents and Tools act directly in the shared Session workspace. Event
Journal records facts; Runtime Client projects the authoritative Workflow owner;
TUI reads that projection through App Server. There is no second engine.

* Verification trust is fixed before any writer runs: the Workflow program is
  compiled/frozen, including its Tool selector and literal verification command.
  The Implementer supplies the source under test, not the verifier implementation.
  Edits to Workflow YAML in the workspace cannot mutate the admitted program
  generation. A fake `checks/verify_greeting.py` is merely workspace data, even
  when its write was explicitly approved. Tool Approval authorizes an exact
  invocation, not verifier semantics. This does not sandbox arbitrary external
  host processes.
* Checking executes the frozen verifier against the shared workspace through
  native Bash. Native Tool status stays distinct from the machine-derived
  `passed`/`failed` finding; only successful execution enters typed business
  Branch/Return. A result is an observation, not certification of current files.
* Review freezes its exact plan subject before durable publication. The
  coordinator response must match the still-live instance and digest. Rejection
  is typed data; only an explicit Branch on `accepted` gates implementation.
* Loop validates committed body output only after all native children and
  Parallel siblings settle. Cancellation observation and budget consumption share
  the next-body admission frontier. Carried state and outer deadlines never reset
  run-wide accounting.
* Cancellation intent stops new admissions; it does not prove a process stopped.
  Existing invocation/child owners must reach physical settlement before the run
  settles. Unknown ownership remains unresolved.
* Run termination validates/settles owned work before its final synchronous
  cancellation-versus-terminal commit. The outer Agent Loop then commits its one
  canonical result. Completed effects stay in place on every terminal path.

The planner has no tools, and the first implementation writer is gated by an
explicit Branch on the plan Review's `accepted`. A rejected plan returns the
declared `rejected` result without starting any writer. Exhaustion,
cancellation and Tool/child failure leave completed modifications in place.
Nothing automatically commits, merges, resets, stashes, deploys or starts again.

## Goal and Scheduler

Workflow = HOW one finite fixed program executes. Goal #84 = WHAT objective persists
across ordinary rounds. Scheduler #85 = WHEN a target becomes eligible.

`Workflow completed != business checks passed != Goal complete`.

Workflow requires neither Goal nor Scheduler. A Goal-driven normal Agent round may call a
discovered Workflow Tool. Loop exhaustion never automatically admits another round;
Goal-level accounting/continuation belongs to #84 as revised by #351. Pausing
future rounds is a durable Goal transition and differs from cancelling current
work; runtime shutdown is neither. A pending Review/question is not
implicitly Goal Blocked. Recovering Goal state cannot recover old Workflow continuation
or human authority. Scheduler owns time eligibility, never graph execution. This change
keeps Workflow independent of Goal authority and adds no Scheduler placeholders.

## Interactive TUI smoke (Linux, local emulator)

An interactive `pnpm start` smoke against the scripted `workflow_reference_repair`
and `workflow_reference_cancel` providers predates Issue #462; its Candidate,
retained-handoff and source-repository observations describe the retired design
and are superseded by the composed conformance scenarios above. Its TUI finding
still holds: human Review was labeled as Tool Approval in the footer, and the
renderer now distinguishes Review, Questionnaire, Approval and mixed human
waits, with a focused regression.

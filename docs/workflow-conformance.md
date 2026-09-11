# Fixed Workflow product conformance

See [canonical named Subagent resources](subagent-resources.md) for schema 8
role files, registration/admission, bounded roots, source provenance, and frozen
reload/child contracts.


The shipped programs are `parallel_review` and `implement_and_review` in
[`examples/local-runtime`](../examples/local-runtime/README.md). Both enter through
an explicitly registered foreground Tool. The reference set covers Agent, Tool,
Branch, Parallel, Review, Loop and Return without new grammar or execution APIs.

## Composed scenarios and authoritative owner tests

The four shipped-program scenarios live in
[`tests/conformance/workflow.rs`](../tests/conformance/workflow.rs). They copy the
actual workspace and registration, substitute only local provider configuration,
and use real native child processes, file writes, Bash supervision and root HITL.
The emulator emits model responses; it never executes tools or supplies check results.

| Contract | Product composition and owning regression |
| --- | --- |
| Outer Tool boundary | `shipped_repair_uses_real_writes_checks_and_root_human_decisions`: 8 requests = parent admission + plan + child question + 2 writer requests + 2 child output requests + parent continuation; one canonical ToolResult. At plan/candidate Review, exactly 2/7 requests exist. |
| Real verification | Same scenario: first native write produces an incorrect greeting despite model prose claiming success; real Bash/Python check fails, second write fixes it, second check passes. Exactly 2 writer Agents and 2 checker executions. `shipped_repair_candidate_checker_tampering_cannot_redefine_frozen_verification` additionally approves a candidate-side fake checker printing `passed`: wrong source still commits failed feedback, then corrected source passes. The same frozen command is approved in both iterations: 8 requests, 3 AgentRuns, 2 bodies, 3 writes, 2 native checks, 5 approvals, 2 questions and 2 Reviews. |
| Structured dataflow | Shipped Parallel children reject parent-only history in provider expectations; downstream quality pass receives only declared assessment. Repair excludes private model claims and child questions from parent continuation. Native lexical-scope tests reject sibling/optional-path reads. |
| Fixed Parallel | `shipped_parallel_reverses_completion_without_git_or_parent_orchestration`: first response remains gated until the second child's native settled publication. Exactly 3 AgentRuns, 5 provider requests, one result, no candidate or Git initialization. `reversing_completions_preserves_keyed_outputs_failures_and_join_commit_order` and `nested_capacity_one_blocks_never_reserve_descendant_capacity` own keyed all-settle and capacity-one detail. |
| Candidate handoff | Shipped repair checks and Review operate in the same retained candidate; exact final dirty bytes are inspected and parent bytes are unchanged. Native `agent_dirty_bytes_reach_exact_tool_context_after_child_settlement_with_frozen_resources` owns cross-node context transfer. |
| Human participation | Shipped repair answers fixed and child Questionnaires, accepts plan/candidate Reviews, and allows 2 child writes plus 2 fixed check invocations through root Runtime Client. The parent never relays them. `fixed_question_and_review_use_root_client_while_parent_model_remains_in_outer_call` also covers rejection and wrong-route responses. |
| Bounded repair | Shipped success: 2 bodies, 3 total AgentRuns, 8 requests, 2 checks. `shipped_repair_exhaustion_retains_dirty_work_without_another_body`: exactly 3 bodies, 4 AgentRuns, 10 requests, 3 checks; explicit needs_revision and retained dirty source. Native `cancellation_at_second_iteration_frontier_consumes_and_starts_nothing_new` proves zero next admissions when cancellation wins. |
| Version binding | `loop_candidate_mutation_clears_old_acceptance_and_stale_review_cannot_certify_new_version`, `loop_review_and_questionnaire_reject_old_responses_and_allocate_fresh_instances`, and root routing regressions reject stale candidate, iteration, subject, owner and interaction identities. |
| Failure certainty | `every_native_non_success_survives_the_actual_outer_adapter_once` and `every_native_non_success_remains_outer_status_without_a_second_iteration` preserve Failed, Denied, Cancelled, TimedOut and OutcomeUnknown. No failure takes a normal Loop exit port or gets replayed. |
| Physical settlement | `loop_checker_supervised_process_gate_blocks_next_writer_and_exhaustion_retains_candidate` holds a real supervised process behind a TCP gate for each body. The next writer and disposal cannot precede release/settlement, even after provisional stdout. |
| Observation/resync | Native read-model snapshot/publication races and `native_observation_lag_does_not_change_loop_requests_or_outcome`; real TUI stdio `native Workflow retirement preserves visible Tool identity through stdio, reconnect and reopen`. Viewing is not execution. |
| Process death | `reopened_journal_retains_resource_facts_without_recreating_borrowers`, native interaction recovery suites, and the real stdio reopen test retain only historical evidence, never old executable nodes or actionable human authority. |
| Bounds | Native 100-iteration observation-loss test, aggregate value/step/Agent budget tests, capacity/drain suites, and real nine-run retirement scenario (10 provider requests, 8 retained detailed runs). The shipped terminal snapshots also assert zero candidate users. |

The owner tests are in `src/runtime/workflow/tests/{scoped,tools,tool_composition,
candidate,human,loops}.rs`, `src/runtime/workflow/read_model.rs`, the existing
workspace/interaction suites and `tui/test/integration.test.ts`. They remain the
source of truth for detailed races; this slice does not duplicate their matrices.

Every shipped YAML is enumerated against registration and compiled in the pure
`contracts` target. Representative invalid references, unavailable profiles,
unadmitted selectors, cross-scope reads, zero limits, unavailable producers and
missing entries are rejected without model execution. The process example suite
also composes all registered resources. `cargo test --all-targets --all-features`
includes both targets; CI's explicit target split retains them.

## Ownership and frontiers

Workflow owns the finite program, values, iteration budgets and run settlement.
Agent Loop owns model turns and the single canonical outer result. Native Tool
invocation/process owners own preparation, permissions, execution and physical
settlement. InteractionCoordinator owns rendezvous. Native workspace infrastructure
owns the lease, candidate content/version and disposal. Event Journal records facts;
Runtime Client/TUI read the authoritative Workflow owner. There is no second engine.

* Verification trust is fixed before candidate writer admission: the Workflow
  program is compiled/frozen, including its Tool selector and literal verification
  command. The verification program is frozen as trusted Workflow Tool arguments.
  The candidate supplies the source under test, not the verifier implementation.
  Candidate edits to Workflow YAML cannot mutate the admitted program generation.
  A fake `checks/verify_greeting.py` is merely candidate data, even when its write
  was explicitly approved. Tool Approval authorizes an exact invocation, not
  verifier semantics. This does not sandbox arbitrary external host processes.
* Candidate publication follows native writer settlement and content inspection,
  including uncommitted source bytes. Workflow commits only the resulting native
  candidate reference with validated structured output.
* Checking executes the frozen verifier against the exact candidate through native
  Bash and holds exclusive candidate access through physical Tool settlement.
  Native Tool status stays distinct from the machine-derived `passed`/`failed`
  finding; only successful execution enters typed business Branch/Return.
  `WorkspaceAccess::finish(true)` validates unchanged content before applicability
  can commit. A validator that mutates source cannot certify its input.
* Review freezes its exact plan/candidate subject before durable publication. The
  coordinator response must match the still-live instance and digest; candidate
  validation/access remains held through acceptance and downstream consumption.
* Loop validates committed body output only after all native borrowers/children and
  Parallel siblings settle. Cancellation observation and budget consumption share
  the next-body admission frontier. Carried state and outer deadlines never reset
  run-wide accounting.
* Cancellation intent stops new admissions; it does not prove a process stopped.
  Existing invocation/child owners must reach physical settlement before another
  candidate writer or lease cleanup. Unknown ownership remains unresolved.
* Run termination validates/settles owned work before its final synchronous
  cancellation-versus-terminal commit. The outer Agent Loop then commits its one
  canonical result. Disposal runs only through the native lease owner after users
  settle; dirty work is retained on every terminal path.

The existing grammar acquires a configured run workspace at admission, before plan
Review. It does not support a lazy workspace node. The planner has no tools, and the
first implementation writer is gated by plan acceptance. Clean rejected plans settle
without dirty handoff. Accepted/rejected candidates, exhaustion, cancellation and
Tool/child failure retain useful modifications; unknown containment retains unresolved
work. Nothing automatically commits, merges, resets, stashes, deploys or starts again.

## Goal and Scheduler

Workflow = HOW one finite fixed program executes. Goal #84 = WHAT objective persists
across ordinary rounds. Scheduler #85 = WHEN a target becomes eligible.

`Workflow completed != business checks passed != Goal complete`.

Workflow requires neither Goal nor Scheduler. A Goal-driven normal Agent round may call a
registered Workflow Tool. Loop exhaustion never automatically admits another round;
Goal-level accounting/manual continuation belongs to #84. Pausing/disarming future
rounds differs from cancelling current work. A pending Review/question is not
implicitly Goal Blocked. Recovering Goal state cannot recover old Workflow continuation
or human authority. Scheduler owns time eligibility, never graph execution. This change
keeps Workflow independent of Goal authority and adds no Scheduler placeholders.

## Interactive TUI smoke (Linux, local emulator)

The actual `pnpm start` TUI and built `rustx` binary ran against copied reference
configuration and isolated Git source fixtures, with explicit Workflow Tool selection.
Provider setup was local Chat Completions (`workflow_reference_repair`), no paid
credentials. The smoke answered fixed/child questions, accepted plan/candidate Reviews,
allowed both child writes and both fixed Bash checks, inspected candidate Review's
actual `stdout: passed` report, and expanded foreground details. It displayed all
seven node kinds, two repair iterations, execution completed, separate business
`accepted`, candidate v2 and a retained dirty handoff. The provider's final control
report recorded exactly eight requests, all settled, no failures.

A second isolated source fixture used `workflow_reference_cancel`: after both
questions, plan acceptance and the first allowed dirty write, Ctrl+C at the first
checker's approval gate cancelled the foreground attempt. No checker or next writer
started. The TUI removed actionable interaction authority, displayed execution cancelled
and retained candidate v1/handoff. Exactly five provider requests settled, with no
continuation. This smoke covers cancellation before checker start; the gated native
process regression is the physical-draining proof for a running checker.

The smoke exposed and fixed a TUI-only status error: human Review was labeled as Tool
Approval in the footer. The renderer now distinguishes Review, Questionnaire, Approval
and mixed human waits, with a focused regression. No protocol or execution change was
needed. Compiler diagnostics for unadmitted Agent profiles and invalid Loop limits
also now name the owning node, verified by the invalid-reference fixture set. The corrected human-review label was verified in the second live session.

A preliminary second-runtime-root attempt against the first source repository refused
its already-retained deterministic branch. The successful candidate was left intact;
the cancellation smoke used a fresh copied source repository. These smokes are
scripted-provider product exercises, not measurements of live model effectiveness.
macOS containment/filesystem coverage remains in CI; this local run is Linux only.

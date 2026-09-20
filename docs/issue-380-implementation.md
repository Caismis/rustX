# Configuration application and Session adoption (#380)

The native configuration coordinator owns desired immutable inputs, application
attempts, finite preparation and final publication. Session identity owns the
adopted binding independently of runtime residency. Execution admission captures
model, context, resources, approval, model deadlines and Tool deadlines once;
derived execution never reads current registry policy.

## Ownership and finite units

| Unit | Owner and closure |
| --- | --- |
| ExecutionPolicy | Configuration coordinator publishes approval and execution deadlines for future independent Attempts. |
| SharedCapacity | Existing native capacity owner; changing the shared limit does not rebuild execution resources. |
| Capabilities | One complete Tool definition/implementation, named Agent/Workflow, Skill guidance, MCP/Python environment and resource lease closure. |
| Instructions | Captured Root and project instructions plus context policy, composed with each Session's adopted capability binding. |
| Provider | Actual selected primary/summary/child provider and model definitions, adapter parameters and request namespace. Global defaults do not replace existing selections. |
| ProcessBindings | Actual App Server owner updates live connection/attachment/residency limits; its captured shutdown deadline requires restart. Startup path/listener bindings remain process-owned. |

Client appearance remains local. Independent units can be applied, preparing,
failed, ready for adoption and restart-required simultaneously. A failed context
closure does not undo an independently applied execution policy.

`ConfigurationApplications` in `src/local_runtime/configuration/application.rs`
and its `worker.rs` own one preparation worker and latest pending input per
Session. A two-minute preparation deadline requests cancellation and retains the
slot until physical preparation settles. Existing resource owners provide
cancellation and lease retirement; publication does not destroy old snapshots.
Failed or superseded candidates release their exclusive ownership.

`SessionController.configuration_bindings` retains the adopted descriptor and
revision across unload/load. Reloading a runtime allocation re-prepares an
available candidate against the new allocation without adopting it. The dedicated
`settings/setModel` operation uses the same preparation and atomic binding commit,
with durable selection CAS. It does not import unrelated pending context.

`src/model/request_shape.rs` compares configuration evidence built through the
actual Chat Completions, Responses and Anthropic request constructors. Canonical
history is excluded. System contributions, Tool definitions/order, selected
provider/model bindings and emitted parameters participate. Unknown equivalence
requires adoption. Preservation makes no cache-hit promise.

## Linearization and deterministic evidence

Source CAS, stable input capture, application identity replacement and final
publication share the configuration state mutex. A source capture verifies its
finite file/directory manifest; external edits cannot be overwritten by a stale
CAS or assembled into an accepted mixed capture. A same-revision retry after failure gets a new
attempt identity. Unchanged healthy inputs retain their candidate and state without
preparation. Failed immutable capture reports an unavailable input revision, never
a fabricated manifest identity. The final identity check, runtime pointer commit and native
unit-state update remain under this fence.

The runtime coordinator gate orders complete binding publication and Attempt
admission. Explicit adoption additionally checks idle state, candidate identity,
Session binding revision and physical allocation identity. It returns Busy without
cancellation, NotReady before readiness, and Conflict for stale intent. Model
selection uses the same gate and a durable settings revision fence. Adoption does
not mutate canonical history, invoke a model, compact or change a default model.

The following tests use watch/oneshot channels, barriers or owned fake-provider
request gates, not sleeps, to establish race order. Test deadlines are liveness
guards only.

| ID | Exact deterministic evidence |
| --- | --- |
| T01 | `tests/scripted/app_server/configuration.rs::t01_new_attempt_captures_automatic_policy_old_attempt_retains_capture`: provider gate holds A across approval/timeout publication; B captures the new generation after A settles. |
| T02 | Same file `t02_later_tool_batch_and_model_step_keep_admitted_registry_policy`; `tests/scripted/agent/retry.rs::transient_retry_uses_shared_identity_and_frozen_request`, `transient_and_overflow_recovery_share_ordinal_and_budgets`; `src/tools/native/subagent/mod.rs::t02_subagent_created_after_publication_inherits_admitted_policy`; `tests/boundary/subagent/conformance.rs::the_child_spec_carries_the_frozen_timeout_policy`; `src/runtime/workflow.rs::workflow_run_and_future_invocation_keep_separate_program_snapshots`. Provider/child gates and the manual retry clock prove later derived work retains captured inputs. |
| T03 | Headless `t03_mixed_application_and_true_process_binding_restart`; coordinator `t03_units_have_simultaneous_independent_outcomes`; Web/TUI mixed-state tests below. |
| T04 | Headless `t04_session_relative_prefix_and_t09_policy_noop`: two Sessions retain different prefixes through one global policy edit. |
| T05 | Headless `t05_complete_policy_registry_publishes_while_instructions_remain_pending`: policy-only registry closure publishes while the unrelated context candidate remains explicit. |
| T06 | Headless `t06_offside_preparation_allows_admission_and_t07_new_failure_supersedes_old_candidate`: preparation gate parks off-side while admission proceeds; `src/capabilities/coordinator.rs::mcp_race_tests::an_old_lease_keeps_serving_its_generation_while_future_leases_resolve_to_the_new_one`. |
| T07 | Same headless gate test and `application.rs::t07_newer_failure_does_not_authorize_old_success_or_failure`: newer input/failure wins the source fence before old work resumes. |
| T08 | Headless `t08_model_baseline_change_rejects_prepared_context`, `t08_model_capture_ignores_unrelated_resource_directories_t09_same_selection_noop`; `src/local_runtime/settings_e2e.rs::t08_capture_rejects_external_change_between_layers_and_resource_manifest`; coordinator `t08_failed_capture_has_no_manufactured_input_revision`. Publication gate pauses after preparation; model commit advances baseline before release. Capture hook edits the source after layer capture and before final manifest validation. |
| T09 | Headless `t11_concrete_candidate_conflict_and_t09_healthy_rescan_preserves_candidate`, `t04_session_relative_prefix_and_t09_policy_noop`, `t09_unselected_model_and_default_edits_are_noop_for_existing_session_t15_new_default`, model-selection no-op above; MCP `an_unchanged_binding_and_definitions_is_a_true_noop`. |
| T10 | `src/model/request_shape.rs::t10_configuration_evidence_matches_all_actual_adapter_prefixes`: actual wire constructors cover history exclusion, instructions, Tool order, namespace, model context-window changes and unproven parameters for all three adapters. |
| T11 | Headless `t11_admission_gate_orders_busy_adoption_without_cancelling_attempt`, `t11_concrete_candidate_conflict_and_t09_healthy_rescan_preserves_candidate`, `t12_lost_source_write_response_does_not_cancel_native_application`. Admission gate owns the runtime mutex before adoption arrives; Busy leaves A running. Explicit idle adoption then commits the inspected binding. |
| T12 | Headless `t12_lost_source_write_response_does_not_cancel_native_application`: persistence acknowledgement hook parks the RPC, caller is aborted, native worker finishes; Web `test/client.test.ts` test `T12 native configuration notifications reject stale versions independently per Session`; TUI `test/convergence.test.ts` test `T12/T16 native application notifications reject reorder and adoption sends the inspected identity`; real Web `test/e2e/recovery.spec.ts`. |
| T13 | Headless `t13_failed_preparation_retries_same_input_and_t14_latest_pending_is_bounded`, `t11_concrete_candidate_conflict_and_t09_healthy_rescan_preserves_candidate`; coordinator `t13_same_revision_retry_has_a_new_identity`. |
| T14 | Same headless test parks the sole preparation worker, commits twelve sources and observes only the active and latest pending preparations; coordinator `t14_pending_work_is_latest_wins_with_one_worker`; MCP `dropped_mcp_preparation_still_owes_physical_settlement`, `one_failed_mcp_close_never_abandons_a_sibling_runtime`, and old-lease test above cover physical settlement/retirement. |
| T15 | Headless `t15_unload_load_does_not_adopt_pending_context`, `t09_unselected_model_and_default_edits_are_noop_for_existing_session_t15_new_default`; `tests/process/app_server.rs::app_server_current_sources_and_persisted_selection_survive_process_reconstruction`. |
| T16 | Headless tests above; Web `test/settings.test.tsx` test `T03/T16 renders simultaneous native application outcomes without inferring field impact`; TUI `test/convergence.test.ts` test `T03/T16 permissions render native mixed application state`; TUI `test/commands.test.ts` test `T16 rescans only through native reconciliation`; real Web settings, integrations, console and recovery acceptance. |

## Protocol and clients

App Server v14 replaces v13; RuntimeClient v44 replaces v43. Generated artifacts
come from `pnpm generate`. The native API exposes `configuration/reconcile` and
`session/adoptConfiguration`, composable application state, actual process policy,
Session adopted binding and scope/version identified `configuration/changed`
notifications. Reconnect reads authority; lost side-effect responses are never
blindly replayed. Ordinary Save has no second publication step.

The former generic configuration publication endpoint, pending-publication boolean,
client permission publication action and duplicate model-authoring endpoints are
deleted, including their compatibility semantics and obsolete tests. Web and TUI
render native results and submit concrete candidate/revision intent; they do not
classify fields or infer request-cache impact.

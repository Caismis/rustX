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
and its `worker.rs` owns distinct source and Session state. Canonical configuration
Workspace paths key immutable desired inputs and the latest successfully available
binding. Bootstrap validates the captured configuration, resolved model registry,
default selection, context budgets and physical source authority. Subsequent source
capture does not replace availability: the native preparation owner advances it
under the source revision fence only after successful preparation. Resource preparation
uses the captured source default, then separately rebinds a complete candidate to
the existing Session selection. A removed retained model can prevent that Session
from adopting without vetoing a successfully prepared new default for new Sessions. Independent
execution policy updates also update that available binding's policy component.
New Sessions obtain their initial binding from this authority; they resolve their
default against the available catalog at creation. Preparing or failed desired
units cannot replace their corresponding available components; independently
validated units can advance the complete composition. An existing Session's resolved
model and adopted context remain unchanged when availability advances.

The coordinator's maps have distinct authorities:

| State | Exact meaning |
| --- | --- |
| `available` | Latest complete validated component composition usable by a new Session, keyed by canonical source scope; it need not equal any whole authored revision. |
| `desired_sources` | Newest coherently captured authored intent, including capture failure; never initial-binding authority. |
| `scope_sources` | Session-to-source membership, retained independently of residency. |
| Session retained binding | Exact configuration and model selection adopted by that Session. |
| `scopes` | One Session's relationship and unit outcomes against captured desired intent. |
| `inputs` / `pending` | Immutable input for that fenced application / latest queued work per Session. |
| `ready` | Allocation-specific prepared candidate, fenced by application identity and expected adopted binding. |
| `deferred` | Desired work waiting for natural residency or replacement allocation; never adopted configuration. |
| `desired_process` / `process` | Latest process intent / one process-owned applied outcome projected into Session views. |

Creation takes its initial descriptor from available authority. Once durable Session
creation completes, `register_session_scope` publishes source membership and the
retained binding under the same coordinator mutex. Source membership does not itself
imply pending Session application. Registration compares the finite Session-owned
units against the adopted binding: Capabilities (including admitted Workflow Agent
closure), Instructions, selected Provider, ExecutionPolicy and SharedCapacity.
Only unresolved Session-relative desired units create application/deferred work.
Capture failure remains unresolved. ProcessBindings alone never manufactures a
Session candidate or preparation cycle. Fully current new Sessions have no synthetic
Preparing application, deferred marker or configuration preparation on natural load.
Registration still joins desired changes that raced durable creation without
rereading source files. Unresolved work is deferred until natural load; load first constructs
the retained descriptor, then schedules allocation-specific preparation. A Session
created while N+1 prepares therefore keeps N and obtains a concrete N+1 candidate
without another Save or reconcile. Context-changing/unproven candidates require
exact-identity explicit adoption. Cache-preserving publication retains the existing
independent Attempt-boundary contract.

Available composition moves complete unit authority: runtime values, authored
effective projection, field provenance and component identity move together.
ExecutionPolicy owns approval, model timeouts and Tool deadlines; SharedCapacity
independently owns `subagents.max_concurrent`. Their captured authority survives
resource-resolution failure, and the same composition operations serve source
availability, retained Session bindings and runtime snapshots. An unchanged policy
unit does not gain an identity merely because another unit changed.

The finite Instructions unit owns project instruction values (`instructions`,
`agents_md`, context policy and their effective fields), discovered project and
Root instruction content, physical project path authority (`project_resources`),
Instructions provenance and Instructions component identity. Mixed composition
moves these together from the helper argument, whether retaining old Instructions
with new Capabilities or applying new Instructions with old Capabilities. Runtime context composition also moves Provider
provenance with its effective catalog and identity. Mixed C1+I2 retains the C1 source
manifest as a diagnostic baseline and records I2 through its component identity
and actual field origins. It does not replace that manifest with the newer authored
revision containing failed C2 or claim whole-source success. Source diagnostics may
therefore continue to report authored differences. Immutable adopted resources and
Attempt snapshots are never rewritten by later source publication.

Session applications have one bounded preparation worker and latest pending input
per Session. A two-minute preparation deadline requests cancellation and retains
the slot until physical preparation settles. Existing resource owners provide
cancellation and lease retirement; publication does not destroy old snapshots.
Failed or superseded candidates release their exclusive ownership.

Save never calls `SessionRuntimeManager::load`. Cold Sessions retain their adopted
binding and desired application without a registry reservation. Policy, capacity
intent, immutable capture, semantic comparisons, model validation and process policy
work run without a ConversationRuntime. Context/provider-only source preparation
can advance availability using the already available capability definitions.
Allocation-specific resource preparation and provider request-shape comparison stay
Preparing until natural load. `SessionController.configuration_bindings` retains
the exact adopted descriptor and binding revision across unload/load. Natural load
first composes that descriptor; `rebind_runtime` then prepares the newest captured
desired input against the new allocation and retained Session model. It does not
adopt context. Rebinding never rewinds source desired authority.

ProcessBindings has one desired process input and effective outcome under the
native source fence. The process owner applies changed limits once; Session jobs
project that result instead of repeatedly applying the same process policy.

Capability preparation includes the resource registry and its dependent provider
bindings. If that closure fails, it publishes no new capability resources.
Instructions can still be prepared from the retained adopted capabilities and
provider plus the new instruction/context input and independently applied execution
policy. Its candidate is a complete immutable snapshot. Explicit adoption changes
only Ready units, preserving failed capability/provider outcomes for retry.
After independent Instructions preparation succeeds, source availability composes
its Instructions component with the source's available Capabilities and Provider,
plus already-applied execution policy. This uses source availability rather than
an arbitrary Session's older selection. Default model/context-budget and physical
authority validation fence publication. Component revisions preserve C1/P1 and
advance I2; no failed C2 or fictitious whole-source success is published. A later
successful C2 preparation makes C2+I2 available without rewriting context-changing
retained S1/S2 bindings. This is one explicit fixed-unit composition, not subset search.
Conversely, cache-preserving capability changes can publish while independent
instructions remain Ready. Failure reporting names the finite units whose
preparation failed; it does not enumerate and fail all Preparing units. Component
manifest identities follow Capabilities, Instructions and Provider through mixed
composition in both the retained descriptor and execution resource snapshot.

`ProspectiveSessionConfig::admitted_agent_dependencies` owns the Agent dependency
set: directly selected Root Agents plus profiles referenced by every selected
Workflow's Agent nodes, including nested nodes and invocation overrides. Capability
equality compares those definitions; provider equality compares their explicit
primary/summary model selections and catalog bindings. Workflow program identity
already covers static invocation overrides (which cannot override instructions or
models). Physical resource validation uses the same set at immutable admission and
preparation. Unselected unrelated catalog definitions remain inert.

The dedicated `settings/setModel` operation uses the same preparation and atomic
binding commit, with durable selection CAS. It does not import unrelated pending
context.

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

Resubscription retirement checks consult the native host's current registration,
which changes atomically with closing the previous registration. They do not consult
the attachment's later-published delivery-handle cache. This closes the interval
where resync could be mistaken for attachment retirement by a waiting transport.

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
| T03 | `t03_t06_t09_t15_t16_healthy_registration_preserves_complete_policy_authority` proves Workspace policy value/effective/provenance agreement, separate policy/capacity identities, no deferred/application/candidate, zero load preparation and stable resource pointers. The mixed-failure regression verifies User I1 → Workspace I2 provenance in source and runtime, C1/I2 component identities, the retained diagnostic source manifest and S2’s unresolved C2 relationship. The gated publication regression replaces new.md after C2 fails and I2 prepares; final authority validation rejects publication and new Sessions retain C1+I1. The positive old.md → new.md regression verifies S2 receives C1+I2 and captured new.md content. Strengthened `t03_t05_failed_capabilities_preserve_leases_while_instructions_are_adopted` proves C1+I2 availability for a brand-new Session and C2+I2 after retry. Headless `t03_t05_failed_capabilities_preserve_leases_while_instructions_are_adopted`: failure injected after construction retires the uncommitted capability candidate; instruction adoption retains the exact registry and Skill lease identities and failed capability status. Also `t03_mixed_application_and_true_process_binding_restart`; coordinator `t03_units_have_simultaneous_independent_outcomes`; Web/TUI mixed-state tests below. |
| T04 | Headless `t04_session_relative_prefix_and_t09_policy_noop`: two Sessions retain different prefixes through one global policy edit. |
| T05 | The mixed-failure regression verifies User I1 → Workspace I2 provenance in source and runtime, C1/I2 component identities, the retained diagnostic source manifest and S2’s unresolved C2 relationship. The gated publication regression replaces new.md after C2 fails and I2 prepares; final authority validation rejects publication and new Sessions retain C1+I1. The positive old.md → new.md regression verifies S2 receives C1+I2 and captured new.md content. Real-child `t05_t09_workflow_only_agent_content_rebuilds_frozen_execution` and `t05_t09_workflow_only_agent_model_rebuilds_frozen_execution` verify old admitted execution and later Workflow execution. Both directions: `t03_t05_failed_capabilities_preserve_leases_while_instructions_are_adopted` and `t05_complete_policy_registry_publishes_while_instructions_remain_pending`: policy-only registry closure publishes while the unrelated context candidate remains explicit. |
| T06 | `t03_t06_t09_t15_t16_healthy_registration_preserves_complete_policy_authority` proves Workspace policy value/effective/provenance agreement, separate policy/capacity identities, no deferred/application/candidate, zero load preparation and stable resource pointers. `t06_t09_t16_process_restart_alone_does_not_defer_new_session` proves restart remains process-owned with zero Session preparation. Creation registration queues no residency and natural load first uses the retained binding. `t06_t15_configuration_save_keeps_cold_sessions_outside_residency_budget`: five cold Sessions, resident limit one, native worker acknowledgements, no added resident/loading entries, and successful natural load/rebind. Also `t06_offside_preparation_allows_admission_and_t07_new_failure_supersedes_old_candidate`: preparation gate parks off-side while admission proceeds; `src/capabilities/coordinator.rs::mcp_race_tests::an_old_lease_keeps_serving_its_generation_while_future_leases_resolve_to_the_new_one`. |
| T07 | Same headless gate test and `application.rs::t07_newer_failure_does_not_authorize_old_success_or_failure`: newer input/failure wins the source fence before old work resumes. |
| T08 | Headless `t08_model_baseline_change_rejects_prepared_context`, `t08_model_capture_ignores_unrelated_resource_directories_t09_same_selection_noop`; `src/local_runtime/settings_e2e.rs::t08_capture_rejects_external_change_between_layers_and_resource_manifest`; coordinator `t08_failed_capture_has_no_manufactured_input_revision`. Publication gate pauses after preparation; model commit advances baseline before release. Capture hook edits the source after layer capture and before final manifest validation. |
| T09 | `t03_t06_t09_t15_t16_healthy_registration_preserves_complete_policy_authority` proves Workspace policy value/effective/provenance agreement, separate policy/capacity identities, no deferred/application/candidate, zero load preparation and stable resource pointers. `t06_t09_t16_process_restart_alone_does_not_defer_new_session` proves restart remains process-owned with zero Session preparation. The preparation-window test now waits for S2’s concrete candidate and explicitly adopts it with no second Save/reconcile, no model request and unchanged history. Workflow-only dependencies invalidate false no-ops. `t09_available_default_preparation_is_independent_of_retained_session_selection` proves a new default and capability definition become available even when the old Session retains a removed model and its exact registry. `t09_t15_new_session_during_preparation_keeps_available_binding_after_success` and `t09_t13_t15_new_sessions_use_available_during_preparation_failure_and_retry`: deterministic preparation gates prove actual retained model/context/component identity, unchanged S2, and new availability only after success. Also `t11_concrete_candidate_conflict_and_t09_healthy_rescan_preserves_candidate`, `t04_session_relative_prefix_and_t09_policy_noop`, `t09_unselected_model_and_default_edits_are_noop_for_existing_session_t15_new_default`, model-selection no-op above; MCP `an_unchanged_binding_and_definitions_is_a_true_noop`. |
| T10 | `src/model/request_shape.rs::t10_configuration_evidence_matches_all_actual_adapter_prefixes`: actual wire constructors cover history exclusion, instructions, Tool order, namespace, model context-window changes and unproven parameters for all three adapters. |
| T11 | Headless `t11_admission_gate_orders_busy_adoption_without_cancelling_attempt`, `t11_concrete_candidate_conflict_and_t09_healthy_rescan_preserves_candidate`, `t12_lost_source_write_response_does_not_cancel_native_application`. Admission gate owns the runtime mutex before adoption arrives; Busy leaves A running. Explicit idle adoption then commits the inspected binding. |
| T12 | Web `test/settings.test.tsx` parameterized `T12/T16 source projection %s preserves subsequent drafts` (before acknowledgement, after acknowledgement, after the next edit). `src/runtime_client/host.rs::resubscription_is_superseded_before_local_handle_publication` deterministically holds the host-registration/local-handle publication interval and proves resync cannot retire a live attachment. Also `t12_lost_source_write_response_does_not_cancel_native_application`: persistence acknowledgement hook parks the RPC, caller is aborted, native worker finishes; Web `test/client.test.ts` test `T12 native configuration notifications reject stale versions independently per Session`; TUI `test/convergence.test.ts` test `T12/T16 native application notifications reject reorder and adoption sends the inspected identity`; real Web `test/e2e/recovery.spec.ts`. |
| T13 | The partial-capability-failure test also checks independently available Instructions, then successful C2 retry. `t09_t13_t15_new_sessions_use_available_during_preparation_failure_and_retry`: the same desired revision fails then succeeds; only subsequently created Sessions obtain its new default and instructions. Also `t13_failed_preparation_retries_same_input_and_t14_latest_pending_is_bounded`, `t11_concrete_candidate_conflict_and_t09_healthy_rescan_preserves_candidate`; coordinator `t13_same_revision_retry_has_a_new_identity`. |
| T14 | Same headless test parks the sole preparation worker, commits twelve sources and observes only the active and latest pending preparations; coordinator `t14_pending_work_is_latest_wins_with_one_worker`; MCP `dropped_mcp_preparation_still_owes_physical_settlement`, `one_failed_mcp_close_never_abandons_a_sibling_runtime`, and old-lease test above cover physical settlement/retirement. |
| T15 | `t03_t06_t09_t15_t16_healthy_registration_preserves_complete_policy_authority` proves Workspace policy value/effective/provenance agreement, separate policy/capacity identities, no deferred/application/candidate, zero load preparation and stable resource pointers. The strengthened preparation-window test proves eventual explicit adoption; the partial-failure test checks new S2=C1+I2 and S3=C2+I2. `t09_available_default_preparation_is_independent_of_retained_session_selection`, `t06_t15_configuration_save_keeps_cold_sessions_outside_residency_budget`, `t09_t15_new_session_during_preparation_keeps_available_binding_after_success`, `t09_t13_t15_new_sessions_use_available_during_preparation_failure_and_retry`, `t15_unload_load_does_not_adopt_pending_context`, `t09_unselected_model_and_default_edits_are_noop_for_existing_session_t15_new_default`; `tests/process/app_server.rs::app_server_current_sources_and_persisted_selection_survive_process_reconstruction`. |
| T16 | `t03_t06_t09_t15_t16_healthy_registration_preserves_complete_policy_authority` proves Workspace policy value/effective/provenance agreement, separate policy/capacity identities, no deferred/application/candidate, zero load preparation and stable resource pointers. `t06_t09_t16_process_restart_alone_does_not_defer_new_session` proves restart remains process-owned with zero Session preparation. The same three source-projection/draft ordering regressions in T12. Headless tests above; protocol `web08_catalog_commit_preserves_admitted_attempt_and_updates_cold_resolution` waits for native availability before new Session creation; Web `test/settings.test.tsx` test `T03/T16 renders simultaneous native application outcomes without inferring field impact`; TUI `test/convergence.test.ts` test `T03/T16 permissions render native mixed application state`; TUI `test/commands.test.ts` test `T16 rescans only through native reconciliation`; real Web settings, integrations, console and recovery acceptance. |

Settings editors consume their own save acknowledgement whether source publication
arrives before or after it. A subsequent edit clears that acknowledgement; later
configuration notifications cannot overwrite the new draft or adopt a newer CAS
revision. Three promise-controlled tests exercise both response orders and a
projection delayed until after the next edit.

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

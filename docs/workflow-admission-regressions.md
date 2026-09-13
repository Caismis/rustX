# CFG2-05 deterministic admission regressions

The tests below use fake immutable capability generations, real merged Skill
catalogs, and existing native test owners. Ordering uses watches, publication
gates and notifications. No sleep establishes a correctness assertion.

| Contract | Evidence |
| --- | --- |
| Enabled programs execute exact internal Tools | `cfg274_enabled_run_freezes_generation_across_dependency_loss`; existing fixed Tool and real provider-emulator Workflow conformance |
| One missing exact Tool disables the complete graph before its entry child | `cfg274_disabled_direct_start_runs_zero_nodes_tools_and_children` |
| Disabled direct and native starts have identical retained reasons and zero nodes, Tools or children | `cfg274_disabled_direct_start_runs_zero_nodes_tools_and_children`, including empty executable registrations, read-model runs and start/settlement events |
| Missing named Agent disables the program | `cfg274_missing_role_and_required_child_exact_tool_disable_instead_of_suppression` |
| Exact child Tool and unavailable ToolSource diagnostics disable Workflows, while ordinary Agents warn and suppress | `cfg274_missing_role_and_required_child_exact_tool_disable_instead_of_suppression` |
| Child source `All` freezes exact definitions and source bindings | `cfg274_all_freezes_exact_source_set_and_recovery_readmits_only_new_catalog` |
| Main can invoke a selected enabled Workflow without its internal ordinary Tool | `cfg274_model_exposure_requires_agent_selection_and_enabled_admission`; existing `tool_only_inactive_capability_has_no_provider_or_canonical_history_and_business_false_survives` |
| Main gains no direct internal Tool authority | `cfg274_model_exposure_requires_agent_selection_and_enabled_admission` and the existing ordinary registry preflight coverage |
| Disabled sources remain inspectable but absent from executable/model exposure | `cfg274_disabled_direct_start_runs_zero_nodes_tools_and_children`; `cfg274_skill_recovery_and_loss_publish_workflow_and_root_exposure_atomically` |
| Dependency recovery publishes Enabled only for later generations | `cfg274_skill_recovery_and_loss_publish_workflow_and_root_exposure_atomically` uses the real loader's before-publication gate |
| Dependency loss cannot mutate an admitted/running R1 program | `cfg274_enabled_run_freezes_generation_across_dependency_loss` gates an in-flight Tool; the real reload test retains old resource snapshots |
| No failing node or child selection is silently removed | Disabled source retains its entire graph; `cfg274_disabled_direct_start_runs_zero_nodes_tools_and_children` puts a valid child before a failing Tool; required child selection tests require Disabled instead of a narrowed executable |
| Global/workspace Skills merge with workspace precedence; absent, invalid and excluded required Skills disable | `cfg274_child_skills_use_merged_catalog_explicitly_and_remain_lazy`; real Skill loss/recovery publication test |
| Root automatic Skill visibility never enters children | `cfg274_child_skills_use_merged_catalog_explicitly_and_remain_lazy` |
| Known root-only Goal composition disables a static child | `cfg274_replacements_remove_entire_defaults_and_known_goal_disables_child`; external `goal84_workflow_program_cannot_enable_goal_for_an_agent_node` |
| Present dimensions replace whole defaults | `cfg274_replacements_remove_entire_defaults_and_known_goal_disables_child`, including empty tools/skills/extensions and omitted Goal dimension |
| Enabled identities alone do not grant model exposure | `cfg274_model_exposure_requires_agent_selection_and_enabled_admission`; real root exposure publication test |
| Obsolete manifest and registration fields are rejected | `cfg274_obsolete_registration_and_manifest_fields_are_authoring_errors`; strict schema Tool-selector tests |

Existing Workflow compiler, cancellation, journal, candidate/workspace, parallel,
loop, native outcome and provider-emulator conformance tests remain in place.
Their fake execution contexts explicitly admit programs before calling the
production runtime. New disabled-start tests call the production identity-based
start gate directly, and also exercise the native outer adapter.

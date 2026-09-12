# Agent Profile regression map

The numbered rows track the CFG2-04 acceptance contract. Tests use owned fake
catalogs, admitted generation snapshots, or explicit channel/watch gates. No
sleep establishes publication or freeze correctness.

Abbreviations below identify concrete files:

- **profile**: `src/runtime/agent_profile.rs`
- **authoring**: `src/local_runtime/config.rs`
- **extensions**: `src/extensions.rs`
- **definitions**: `tests/subagent/definitions.rs`
- **overrides**: `tests/subagent/overrides.rs`
- **coordinator**: `src/capabilities/coordinator.rs`

| # | Invariant | Concrete regression |
| --- | --- | --- |
| 1 | Shared Tool semantics | profile: `cfg273_root_and_named_share_tools_skills_extensions_and_keep_independent_authority` |
| 2 | Shared Skill semantics | profile: `cfg273_admitted_skill_selection_is_identical_in_root_and_named_scope` |
| 3 | Shared Extension semantics | profile: `cfg273_root_and_named_share_tools_skills_extensions_and_keep_independent_authority`; authoring: `cfg273_root_and_named_share_the_complete_profile_document` |
| 4 | Child can select admitted root-hidden Tools | profile: `cfg273_root_and_named_share_tools_skills_extensions_and_keep_independent_authority` |
| 5 | Delegation grants no direct child Tool invocation | profile: `cfg273_delegation_composes_dispatcher_without_granting_child_tools` (includes typed ToolRegistry preflight refusal) |
| 6 | Unavailable builtin warns and suppresses | profile: `cfg273_known_builtin_unavailable_in_generation_is_suppressed` |
| 7 | Unavailable source warns and suppresses | profile: `cfg273_source_failures_are_distinct_and_selection_cannot_activate_sources`; definitions: `an_unavailable_source_warns_and_suppresses_without_blocking_the_agent` |
| 8 | Ready source missing Exact Tool is distinct | profile: `cfg273_source_failures_are_distinct_and_selection_cannot_activate_sources` |
| 9 | Missing Skill warns and suppresses | profile: `cfg273_missing_builtin_and_catalog_selections_warn_in_canonical_order` |
| 10 | Missing named Agent warns and suppresses | profile: `cfg273_missing_builtin_and_catalog_selections_warn_in_canonical_order` |
| 11 | Missing/unadmitted Workflow warns and suppresses | profile: `cfg273_missing_builtin_and_catalog_selections_warn_in_canonical_order` |
| 12 | Unknown Extension is hard authoring error | authoring: `cfg273_closed_profile_authoring_rejects_unknown_and_duplicate_tools` |
| 13 | Known scope-ineligible Extension follows profile policy | profile: `cfg273_scope_ineligible_goal_suppresses_only_the_extension`; overrides: `goal84_root_only_scope_is_enforced_for_definition_model_and_workflow_overrides` |
| 14 | Selection cannot enable disabled source | profile: `cfg273_source_failures_are_distinct_and_selection_cannot_activate_sources` |
| 15 | Absent override uses named defaults | overrides: `sub258_no_override_and_an_empty_override_both_reproduce_the_definition` |
| 16 | Present dimension replaces completely | overrides: `sub258_each_present_dimension_replaces_and_missing_dimensions_inherit` |
| 17 | Explicit empty is meaningful | overrides: `sub258_explicit_empty_dimensions_have_their_documented_meaning` |
| 18 | Dynamic override cannot widen its ceiling | overrides: `sub258_the_delegation_ceiling_is_role_union_parent_and_nothing_more`; `sub258_skill_delegation_follows_frozen_model_visible_authority` |
| 19 | Named defaults independent of caller toolbar | profile: `cfg273_root_and_named_share_tools_skills_extensions_and_keep_independent_authority`; overrides: `sub258_the_delegation_ceiling_is_role_union_parent_and_nothing_more` |
| 20 | Later publication cannot mutate admitted profiles/children | coordinator: `source_projection_freezes_at_the_real_candidate_commit_boundary`; definitions: `cfg236_gated_frozen_child_retains_r1_after_canonical_role_r2_publication`, `an_attempt_frozen_on_r1_resolves_r1_after_r2_becomes_current`, `ext256_a_child_frozen_on_r1_keeps_r1_extensions_after_r2_publishes`; overrides: `sub258_r1_resolution_and_authority_survive_r2_publication` |
| 21 | Selection preserves host invocation policy | profile: `cfg273_resolved_generation_is_owned_and_preserves_host_tool_policy`; definitions: `a_non_default_builtin_policy_survives_child_materialization_exactly` |
| 22 | Canonical diagnostic order | profile: `cfg273_missing_builtin_and_catalog_selections_warn_in_canonical_order` |

Extension omission is additionally covered by extensions:
`cfg273_omitted_extension_document_selects_no_extensions` and
`cfg273_root_product_defaults_are_an_explicit_profile_layer`.

## Publication and freeze owners

Root profile resolution occurs while `CapabilityCoordinator` prepares the
candidate, against its candidate Tool catalog, source availability and frozen
Skill snapshot. `CapabilitySnapshot` retains that resolved profile. The resource
snapshot resolves named profiles against the same admitted generation and owns
the resulting immutable map.

`ConversationRuntime::reload_resources` commits through
`CapabilityCoordinator::commit_runtime` and replaces `state.resources` under the
existing runtime coordinator lock. The coherent resource observation follows
that replacement. Failed or cancelled preparation preserves the prior snapshot.

Attempt capability leases pin the published snapshot. `SubagentResolver::resolve`
consumes the generation's resolved named default, or applies an authorized
replacement through `resolve_agent_profile`, then freezes exact Tool definitions,
source materialization bindings, Skill identities/versions, model invocation,
project instructions, extension composition and workspace policy into
`ResolvedSubagentSpec`. Child preparation verifies the frozen physical bindings.
A later publication owns different values; existing Arcs and owned child values
remain unchanged.

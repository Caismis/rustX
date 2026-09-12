# Skill discovery and root visibility regression map

The numbered rows track the CFG2-04B acceptance contract (Issue #280). Every
test uses isolated temporary HOME/workspace roots, owned generation snapshots,
or exact ownership gates. **No sleep establishes discovery, publication, or
freeze correctness**, and no test depends on the developer's real home
directory.

Abbreviations below identify concrete files:

- **package**: `src/skills/package.rs`
- **source**: `src/skills/source.rs`
- **authoring**: `src/local_runtime/config.rs`
- **profile**: `src/runtime/agent_profile.rs`
- **launch**: `src/local_runtime/launch_tests.rs`
- **snapshots**: `tests/scripted/capability/snapshots.rs`
- **skills**: `tests/tools/skills.rs`

| # | Invariant | Concrete regression |
| --- | --- | --- |
| 1 | Default policy scans exactly `~/.agents/skills` and `<workspace>/.agents/skills` | package: `cfg280_default_source_policy_scans_exactly_global_and_workspace`; source: `cfg280_default_policy_resolves_exactly_the_two_canonical_roots`; launch: `cfg280_launch_resolves_the_canonical_skill_sources_and_explicit_authority` |
| 2 | `sources = ["global"]` excludes workspace discovery | package: `cfg280_single_source_policies_exclude_the_other_root`; snapshots: `cfg280_the_source_policy_selects_the_scanned_roots`; launch: `cfg280_launch_resolves_the_canonical_skill_sources_and_explicit_authority` |
| 3 | `sources = ["workspace"]` excludes global discovery | same three tests as row 2 |
| 4 | Unknown source identities are hard configuration errors | authoring: `cfg280_skill_source_policy_is_closed_and_duplicate_free` (also covers duplicates, unknown fields, `all`, `explicit`, wildcards); launch: `cfg280_launch_resolves_the_canonical_skill_sources_and_explicit_authority` |
| 5 | Missing automatic roots are benign empty sets | package: `cfg280_missing_automatic_roots_are_benign_empty_sets`; snapshots: `cfg280_the_source_policy_selects_the_scanned_roots` |
| 6 | Root sees every eligible catalog Skill without a positive list | profile: `cfg280_root_sees_the_eligible_catalog_minus_its_deny_list`; snapshots: `cfg280_the_committed_generation_owns_sources_merge_and_root_visibility`; launch: `cfg280_launch_resolves_the_canonical_skill_sources_and_explicit_authority` |
| 7 | `disabled_skills` removes a Skill from root visibility only | profile: `cfg280_root_sees_the_eligible_catalog_minus_its_deny_list` (catalog membership, bindings, and a named child's selection all survive); snapshots: `cfg280_the_committed_generation_owns_sources_merge_and_root_visibility` |
| 8 | An absent `disabled_skills` identity is one diagnostic, not a failure | profile: `cfg280_root_sees_the_eligible_catalog_minus_its_deny_list`; snapshots: `cfg280_the_committed_generation_owns_sources_merge_and_root_visibility` |
| 9 | Named selection resolves against the merged global/workspace catalog | profile: `cfg280_root_sees_the_eligible_catalog_minus_its_deny_list`; profile: `cfg273_admitted_skill_selection_is_identical_in_root_and_named_scope` |
| 10 | Named Skill selection stays lazy | profile: `cfg280_root_sees_the_eligible_catalog_minus_its_deny_list` (body sentinel absent from every projection) |
| 11 | One malformed package is excluded; valid packages publish | package: `cfg280_one_malformed_package_never_suppresses_valid_ones`; snapshots: `cfg280_a_malformed_declaration_excludes_only_its_own_package`; launch: `cfg271_all_catalogs_publish_together_and_failed_candidates_publish_nothing` |
| 12 | Representative malformed cases diagnose at the owning boundary | skills: `name_must_match_the_parent_directory`, `invalid_standard_names_are_rejected`, `empty_and_oversized_descriptions_are_rejected`, `malformed_yaml_is_rejected`, `malformed_metadata_is_rejected`, `candidate_without_skill_markdown_is_rejected`, `package_symlinks_are_rejected`, `malformed_dependency_declaration_fails_the_transaction`, `a_non_utf8_package_root_is_rejected_rather_than_published_lossily`; package: `cfg280_a_symlinked_package_root_is_excluded_not_fatal`, `cfg280_an_unusable_source_root_excludes_only_that_source` |
| 13 | Workspace shadows global regardless of enumeration or array order | package: `cfg280_workspace_shadows_global_independently_of_configured_order`; package: `cfg280_an_invalid_workspace_candidate_does_not_shadow_a_valid_global_one`; snapshots: `cfg280_the_committed_generation_owns_sources_merge_and_root_visibility` |
| 14 | Shadow provenance is retained | package: `cfg280_workspace_shadows_global_independently_of_configured_order`; snapshots: `cfg280_the_committed_generation_owns_sources_merge_and_root_visibility`; launch: `cfg280_launch_resolves_the_canonical_skill_sources_and_explicit_authority` |
| 15 | Same-scope conflicts never choose an arbitrary winner | package: `cfg280_same_scope_duplicates_exclude_every_definition` (permuted input, every definition excluded, origins canonically ordered) |
| 16 | Catalog and diagnostic ordering are deterministic under permutation | package: `cfg280_workspace_shadows_global_independently_of_configured_order`, `cfg280_same_scope_duplicates_exclude_every_definition`; profile: `cfg280_root_selection_is_order_independent`; snapshots: `cfg280_the_committed_generation_owns_sources_merge_and_root_visibility` (asserts the stored order is already canonical) |
| 17 | Package-level invocation eligibility is not widened by root auto-selection | profile: `cfg280_root_sees_the_eligible_catalog_minus_its_deny_list`; snapshots: `hidden_skills_keep_attempt_provenance_but_not_model_visibility` |
| 18 | A later generation never changes an admitted R1 attempt | snapshots: `cfg280_a_later_generation_never_mutates_an_admitted_attempt` (attempt lease held across candidate preparation; no sleep) |
| 19 | Invalid candidate publication preserves the atomic generation contract | snapshots: `failed_preparation_leaves_revision_authoritative`, `cfg280_a_later_generation_never_mutates_an_admitted_attempt`; launch: `cfg271_all_catalogs_publish_together_and_failed_candidates_publish_nothing` |
| 20 | No race/order test uses a wall-clock sleep | all of the above: ordering is proven by permuted input and derived `Ord`; freeze is proven by a held attempt lease and an explicit publication gate |
| 21 | Root/child selection never eagerly injects a `SKILL.md` body | profile: `cfg280_root_sees_the_eligible_catalog_minus_its_deny_list`; snapshots: `cfg280_a_later_generation_never_mutates_an_admitted_attempt`; snapshots: `lazy_skills_follow_frozen_read_authority_without_changing_discovery` |
| 22 | The rustX-config Skill root is gone from behavior, tests, examples and docs | package: `cfg280_the_legacy_config_relative_skill_root_is_never_read`; launch: `cfg280_launch_resolves_the_canonical_skill_sources_and_explicit_authority` (a package under `<config>/skills` is neither read nor modified) |
| 23 | Explicit `--skill` uses the same validation and freeze | package: `cfg280_explicit_paths_use_the_same_validation_and_win_the_merge`, `cfg280_a_missing_explicit_path_is_a_launch_error`, `cfg280_same_scope_duplicates_exclude_every_definition`; launch: `cfg280_launch_resolves_the_canonical_skill_sources_and_explicit_authority` |

## Ownership summary

```text
[skills].sources            launch/configuration    which roots are scanned
skills::source              runtime                 source identity + precedence
skills::package             candidate construction  enumerate, validate, merge
skills::diagnostics         candidate construction  typed facts + provenance
SkillSnapshot               capability candidate    frozen catalog/bindings/facts
AgentProfile::from_document authoring               root vs named Skill polarity
resolve_agent_profile       generation resolution   selection over one catalog
CapabilityCoordinator       commit                  atomic publication and freeze
```

Precedence is `explicit --skill > workspace > global`, carried by the
`SkillSource` ordering itself. It is never derived from `[skills].sources`
array order, from the order resolved roots reach discovery, or from filesystem
enumeration order.

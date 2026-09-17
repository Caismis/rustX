# CFG3 Agent Profile regression map

The complete profile contract is in [configuration](configuration.md) and
[Agent profiles](agent-profiles.md). These tests exercise native ownership:

| Contract | Tests |
| --- | --- |
| Complete Provider/Model and per-dimension semantic replacement | `src/local_runtime/authoring_cfg3_tests.rs`, `tests/cfg3_catalog.rs` |
| Whole Agent resource shadow, including invalid Workspace duplicates | `tests/cfg3_catalog.rs` |
| Independent child Tool/Skill/Plugin profile, no Root ceiling | `src/runtime/subagent/resolver.rs` CFG332 tests; `tests/subagent/definitions.rs`, `tests/subagent/overrides.rs` |
| Explicit delegation does not grant direct use of child Tools | `src/runtime/agent_profile.rs` delegation and registry tests |
| Default-off closed Plugins and child scope validation | `src/extensions.rs`, `tests/scripted/extensions/mod.rs`, subagent overrides |
| Child model inherits the invoking frozen Attempt | subagent definitions `a_frozen_child_model_never_observes_a_later_rustx_toml_edit`; `tests/subagent/process_conformance.rs` |
| Named-Agent-only MCP remains inert | `tests/cfg3_catalog.rs` `an_uninvoked_named_agent_does_not_connect_its_mcp_source` |
| Admitted child retains its generation | subagent definitions and overrides R1/R2 publication tests; `tests/boundary/subagent/conformance.rs` gated live-child tests |
| Invocation policy survives materialization | subagent definitions `a_non_default_builtin_policy_survives_child_materialization_exactly` |
| Skills disclose metadata and roots before ordinary reads | `tests/tools/skills.rs`, subagent definitions Skill tests, `tests/subagent/process_conformance.rs` |

The publication owner is `ConversationRuntime::reload_configuration` and the
capability coordinator's complete snapshot commit. Candidate preparation happens
off-side. Busy/failed publication retains the old snapshot. Named profiles resolve
from admitted generation data; child processes receive a frozen specification and
captured resources. Tests use channels, watches, owned fake catalogs and real
provider-emulator processes; sleeps are not ordering evidence.

# Agent Profiles

rustX has one semantic definition of an Agent. Root `rustx.toml` selects its
profile under `[agent]`; `.agents/agents/<name>.toml` contains that same profile
directly. The filename establishes the named resource identity. There is no
root registry entry. Root and named profiles differ by selected values and
execution scope.

```toml
# rustx.toml
[agent]
instructions = "Coordinate the review."
# No positive Skill list: the root automatically sees every eligible Skill in
# the effective catalog. Subtract one from root visibility only when needed:
# disabled_skills = ["legacy-java"]
agents = ["reviewer"]
workflows = ["check"]

[agent.model]
model = "local/reasoner"

[agent.tools]
builtin = ["read"]

[agent.extensions]
# Explicit empty composition: no extensions.

[agent.agents_md]
inherit = true
```

```toml
# .agents/agents/reviewer.toml
description = "Review changes"
instructions = "Inspect the diff and report actionable findings."
skills = ["review-guide"]
timeout_ms = 120000

[tools]
builtin = ["read", "grep"]

[tools.sources]
github = ["get_diff"]
"python:analysis" = "all"

[extensions.todo]
enabled = true

[agents_md]
inherit = true

[worktree]
enabled = true
require_clean_parent = true
```

## Authoring and capability decisions

`AgentProfileDocument` is the strict shared TOML contract. It lowers to native
`AgentProfile`; `resolve_agent_profile` resolves that intent into an owned,
finite `ResolvedAgentProfile` against one admitted generation.

| Dimension | Meaning |
| --- | --- |
| `tools.builtin` | Exact ordinary native Tool names. |
| `tools.sources` | `ToolSourceId` to `"all"` or an exact name array, for both MCP and Managed Python. |
| `skills` | **Named Agents only.** Exact admitted Skill names, in canonical order after resolution. |
| `disabled_skills` | **Root only.** Skill identities hidden from root visibility. It removes nothing from the generation catalog: a named Agent that selects a disabled Skill explicitly still gets it. |
| `extensions` | Closed native composition: Agent Status, Todo and Goal. Extension Tools belong to their composition, never ordinary selectors. |
| `agents` | Named Agents this caller may delegate to. Catalog existence alone grants no delegation. |
| `workflows` | Admitted named Workflows this caller may invoke. Static Workflow program admission remains a separate owner. |
| `model` | Native model selection, including reasoning, request parameters and summary policy. A named profile omitting it uses the invoking attempt's frozen model. |
| `instructions` | Primary Agent instructions. Named reusable Agents require nonempty instructions and description. |
| `agents_md` | Whether to inherit canonical project guidance, plus bounded supplemental files within admitted workspace authority. |
| `worktree` | Isolated child workspace policy; clean parent required by default. No arbitrary workspace path may be authored. |
| `timeout_ms` | Optional whole-child lifecycle deadline. Root scope rejects child lifecycle/worktree requests. |

## Skill selection

Skill selection is not Skill loading. Both polarities resolve against the same
effective merged Skill catalog, produce the same frozen
`SkillId`/`SkillVersionId` bindings, and feed the same lazy Read-based loading
path. Only the *selection* differs:

```text
root selection policy
    -> every eligible catalog identity minus agent.disabled_skills

named selection policy
    -> exactly the authored skills identities
```

"Eligible" is the catalog's own Skill-level model-invocation filtering: a
package declaring `disable-model-invocation: true` stays owned by the
generation and is never widened into any Agent's selection, root included.

The polarity is owned by the authoring boundary. A root document naming
`skills` and a named document naming `disabled_skills` are both hard
authoring errors, not silently ignored fields. A malformed Skill identity is
a hard error in either. A syntactically valid `disabled_skills` identity that
the effective catalog does not contain is one generation-scoped
`disabled_skill_absent` diagnostic and never a startup failure.

A Workflow child or dynamic invocation override replaces the whole Skill
dimension with an exact identity list; there is no deny-list override.

Where Skills are *discovered* is a separate owner — the session
`[skills].sources` policy and the discovery/merge pipeline documented in
[runtime resources](runtime-resources.md#skill-sources-and-discovery).

Resource existence is not Agent selection. Source enabled is not Agent exposure.
Agent Tool ownership is not invocation approval. A profile cannot discover a
missing package, activate a disabled or untrusted source, change credentials,
widen host authority, or change native/MCP invocation policy.

Malformed authoring is a hard error: unknown fields, wrong types, malformed
identities, duplicate exact names, unknown Extensions and invalid extension
configuration reject the document. For a valid profile, unavailable selections
produce typed generation-scoped diagnostics and are suppressed. The remaining
profile stays usable. Tool diagnostics distinguish unavailable builtins,
undefined/inactive/unprepared/failed sources, and missing exact Tools in a ready
source. Skills, Agents and Workflows have their own typed unavailable facts, and the
root deny-list has its own typed absent fact.
Diagnostics are canonically ordered and stored with the resolved generation,
not emitted anew on every model turn.

An omitted extension dimension selects no extensions, identically for root and
named documents. Root product defaults explicitly compose Agent Status and Todo
in a lower-priority profile layer. Authoring `[agent.extensions]` replaces that
whole dimension, so an empty table composes none. Known Goal in a complete child
profile is suppressed with a scope diagnostic. A dynamic request to compose an
unsupported child Extension remains a typed refusal. One-shot child scope also
suppresses delegation and Workflow invocation selections; it does not introduce
nested child lifecycle support.

## Defaults, overrides and freezing

Named defaults resolve against generation-wide admitted resources, independently
of the root toolbar. In the examples, root can delegate to reviewer but cannot
call reviewer-only Grep or `github/get_diff` directly.

Dynamic invocation overrides preserve per-dimension replacement for Tools,
Skills and Extensions: absent means named default; present replaces the whole
dimension. Explicit empty means empty. There is no union, recursive merge or
inherit wildcard. Unauthorized additions are refused against the accepted union
of named default authority and the invoking Agent's frozen delegation authority.
The broader generation is not itself dynamic override authority. Warning and
suppression never substitute for override authorization.

The existing capability/resource candidate and commit owner publishes resolved
profiles with the generation. Root attempts pin a capability lease. Child
admission consumes the shared semantic profile and freezes exact Tool definitions,
source bindings, Skill versions, complete model decision and execution policies
into `ResolvedSubagentSpec`. That child contract additionally owns physical
materialization and worktree acquisition. It does not reinterpret profile intent.
Later resource publication or file edits cannot mutate these owned frozen values.

Root model selection seeds a new Session. Persisted Session model state and
attempt `FrozenModelSpec` retain their existing durable owners on resume and
recovery; launch configuration does not overwrite history. Native extension
owners remain launch-scoped, so changing root extension composition requires a
new launch. Runtime child concurrency capacity and host execution/approval policy
remain outside Agent Profiles.

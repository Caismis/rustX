# Workflow admission and Agent selection

TOML authors rustX settings, models, projects and Agent Profiles. YAML authors
Workflow programs. Markdown authors model content such as AGENTS.md and SKILL.md;
JSON is the runtime, wire and schema representation.

A canonical `.agents/workflows/<id>.yaml` defines the entire fixed program and
its static capability references. Discovery creates an inspectable source; it
requires no settings registration. Each Tool node declares exactly one ordinary
Tool using its `selector`. There is no top-level Tool manifest or Tool-node `all`.
Extension-provided Tools belong to Agent extension composition, never ordinary
Workflow Tool selection. There are no nested Workflow calls.

The resource loader compiles syntax, graph structure, schemas and references
before preparing capabilities. Malformed authoring rejects the candidate resource
load. After source preparation and merged Skill discovery, static admission checks
every Tool node and every effective child Agent composition against that exact
candidate generation. A Workflow entry contains its source and either:

- `Enabled`: the frozen executable program, including exact Tool definitions and
  policies and each child's frozen Tool identities, Skill versions and metadata,
  native extensions and source materialization bindings.
- `Disabled`: bounded, ordered typed dependency diagnostics associated with the
  Workflow and node path. No partial executable program is retained.

A missing named Agent, unavailable source, absent exact Tool, missing or excluded
Skill, or unsupported child extension scope disables the entire Workflow. Direct
and native starts consult this retained decision before starting any node, Tool
or child. A disabled source remains inspectable and is omitted from executable
model exposure. Runtime does not rediscover its failure.

Agent nodes reference a discovered named profile with `profile`. Their optional
`override` uses the shared AgentProfile vocabulary. An absent tools, skills or
extensions dimension keeps the named profile's dimension. A present dimension
replaces it completely, including when empty. There is no recursive merge,
addition, subtraction or Workflow-specific selector mode.

Child Skills are explicitly selected against the generation's effective merged
catalog: `~/.agents/skills` and `<workspace>/.agents/skills`, with workspace taking
precedence by identity. Invalid, excluded and missing required Skills disable the
Workflow. Selection freezes metadata and version bindings, not SKILL.md bodies;
loading remains lazy. Root automatic eligible Skill visibility minus
`disabled_skills` does not apply to Workflow children.

AgentProfile and Workflow dependency failures deliberately differ:

| Owner | Unavailable selected capability |
| --- | --- |
| Agent Profile | Warn once for the generation, suppress that capability, keep the Agent usable. |
| Workflow program | Disable the entire program; no execution and no model exposure. |

An Agent's `workflows` dimension selects which enabled identities it may invoke.
Existence, enabled admission, Agent selection, model exposure and running are
separate facts. Selecting a missing or disabled Workflow produces an Agent
warning and suppresses that selection. It does not disable the Agent.

Workflow internal authority is the admitted host/resource generation plus the
static YAML program. It is independent of the invoking Agent's ordinary toolbar:
main may invoke `review` while only that Workflow can execute `github:get_diff`.
Selecting the Workflow grants main no direct access to its internal Tools, Skills
or child capabilities. Dynamic model-generated Subagent overrides retain their
separate delegation ceiling.

The loader finalizes Agent Workflow exposure after static admission, then uses
the existing resource coordinator's single commit/publication boundary to publish
the complete generation. A later generation may disable or recover a dependency.
Already admitted attempts and Workflow runs keep their original catalog, exact
capability bindings and resource snapshot throughout execution.

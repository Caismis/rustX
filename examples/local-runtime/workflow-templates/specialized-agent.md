# One role, two differently specialized children

`.agents/workflows/specialized_agent.yaml` runs the same canonical
`.agents/agents/reviewer.toml` role twice. The first node takes the role's
defaults; the second carries an invocation `override` that specializes exactly
that one child.

Minimum project selection:

```toml
[subagents]
workflow = ["reviewer"]

[workflows]
main = ["specialized_agent"]
```

## What the override means

An override has exactly three dimensions — `tools`, `skills`, `extensions` —
and one rule:

> A **missing** dimension uses the role's value. A **present** dimension
> **replaces** that dimension completely.

In `specialized_agent.yaml`:

| Node | `tools` | `skills` | `extensions` |
| --- | --- | --- | --- |
| `describe` | the role's (empty) | the role's (empty) | the role's authored default |
| `inspect` | replaced with exactly `read` and `grep` | missing, so the role's | replaced with none composed |

`inspect` does **not** get "the role's tools plus `read` and `grep`": a present
`tools` object is the child's whole selection. `extensions: {}` composes no
native extension at all — the authoring *defaults* that make an unconfigured
role compose Agent Status belong to the role document, not to an override,
where presence rather than emptiness is what selects an extension.

There is no additive or subtractive syntax (`addTools`, `removeTools`), no
wildcard, no recursive merge, and no interpolation. `null` is not an accepted
spelling for a dimension or for `override` itself.

## Where the authority comes from

A Workflow Agent node's `override` is **trusted static program data**. It is
compiled into the program, validated at compilation and again when the runtime
prepares its resource generation, and it is not reachable from model output,
node input values, or task text. It may therefore replace the role's defaults
with capabilities the invoking main model does not itself hold — but never with
anything the admitted generation does not authorize. `rustx workflow check`
reports an unknown capability, an unknown or hidden Skill, and a structurally
invalid selection with the offending node's precise authored path.

The model-facing `subagent` Tool accepts the same `override` vocabulary, under
a deliberately narrower rule: a main model may delegate only what the named
role already has or what its own frozen admitted profile holds. See
[Canonical named Subagent resources](../../../docs/subagent-resources.md#invocation-scoped-overrides).

Nothing else is overridable. Model, instructions, timeout, workspace policy,
`AGENTS.md` policy, approval policy, credentials, and source enablement belong
to the role definition alone.

## Running the checks

```sh
rustx --workspace examples/local-runtime/workflow-templates --trust grant
rustx workflow check specialized_agent \
  --workspace examples/local-runtime/workflow-templates \
  --models examples/local-runtime/minimal/models.toml --model example/demo-model --json
rustx workflow explain specialized_agent \
  --workspace examples/local-runtime/workflow-templates \
  --models examples/local-runtime/minimal/models.toml --model example/demo-model --json
```

`explain` projects each node's override as identities only — capability
selectors, Skill names, and composed extension names — never literals, task
text, Skill bodies, or credentials. Exit 3 is expected even for a valid
template: runtime readiness stays unresolved offline.

Tool invocation input: `{"topic":"How this repository organizes tests"}`.
Expected output shape: `{"summary":"..."}`. Content is a future model result.
This example requires no Git repository, workspace candidate, Python, or MCP.

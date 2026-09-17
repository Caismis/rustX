# CFG2 capability inspection example

Use this directory as a workspace with an existing host `settings.toml` and
`models.toml`. Replace the placeholder MCP endpoint before online use. Host
settings own model/provider credentials and approval; this project contains
no credentials. Set `agent.model.model` to a declared model identity (the example
uses `example/demo-model`). The whole model selection replaces the lower one,
including its request settings. Grant workspace trust explicitly before runtime use.

```sh
rustx config show --agent main --workspace examples/cfg2 --json
rustx config show --agent reviewer --workspace examples/cfg2 --json
rustx workflow explain review --workspace examples/cfg2 --json
rustx workflow check unavailable --workspace examples/cfg2 --json
```

Inspection is offline: these commands do not connect, install, run `uv`,
capture credentials, call a model, or execute a Workflow. External sources
remain `unprepared`; dependent Workflows are disabled in this prospective
snapshot. An online generation can admit `review` after demand prepares the
`python:echo` source. Inspection never invents its Tool schema.

Main selects only direct native Read, the reviewer Agent, and the review
Workflow. Reviewer independently selects Read/Grep, all Tools from the exact
`github` source, and exact `echo` from the Managed Python package. `all` trusts
the source's frozen eligible Tool set; exact selection trusts only the named
Tools. Neither selection enables or creates a source.

The optional `annotate` selection produces a typed suppression and reviewer
remains usable. The `unavailable` Workflow instead requires that missing Tool:
its entire program is disabled, with a `source_unavailable` / `undefined`
reason. It remains discoverable and starts zero nodes. `review` uses a complete
Tool-dimension replacement for its reviewer child, so the optional selection
is not a dependency of that program.

Main may invoke `review` after online admission without acquiring direct
`python:echo/echo` authority. Main, named children, and Workflow internal
capabilities are independent. Invocation approval still applies separately.

Skills come from `~/.agents/skills` and this workspace's `.agents/skills`.
If both contain `rust-review`, workspace wins and inspection retains both
origins. Main automatically sees admitted Skills minus `disabled_skills`;
reviewer explicitly selects `rust-review`. Selection does not load Skill
instructions into model context; content remains lazy.

The final authoring boundary is TOML for settings, models, projects and named
Agents; YAML for fixed Workflows; Markdown for Skills and AGENTS.md; JSON for
schemas and wire/runtime values. Nested provider parameters in `rustx.toml`
normalize directly to the runtime JSON request-parameter representation.

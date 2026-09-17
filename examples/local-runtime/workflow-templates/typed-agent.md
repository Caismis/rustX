# Typed Agent → Return

Copy `.agents/workflows/typed_agent.yaml` and the canonical
`.agents/agents/reviewer.toml` into your Workspace. Select a model in
user settings (`model.model = "your-provider/your-model"`), or supply
`--config <absolute-rustx.toml> --model <model-name>` at launch and inspection.

Minimum project selection:

```toml
[subagents]

[agent]
workflows = ["typed_agent"]
```

The role need not appear in `agent.agents`. The canonical Agent TOML file defines the resource;
Workflow YAML references the discovered Agent directly. `agent.workflows`
permits this concrete Workflow Tool to be exposed to the main model. Inspection
changes none of those lists.

Run `rustx workflow check typed_agent` and `rustx workflow explain typed_agent`.
The explicit `args.topic` binding enters `summarize`; `return_summary` returns
the committed typed result. The 60000 ms configured timeout covers the whole
foreground invocation. The conservative maximum is one Agent run, not a promise
of one provider request. The Agent Loop may make multiple requests.

Tool invocation input: `{"topic":"Why explicit typed boundaries help"}`.
Expected output shape: `{"summary":"..."}`. Content is a future model result.
This example requires no Git repository, workspace candidate, Python or MCP.

# Fixed keyed parallel checks

Copy `.agents/workflows/parallel_checks.yaml` and
`.agents/agents/reviewer.toml` into your Workspace. Select your model as
described in [the setup guide](README.md).

Minimum project configuration:

```toml
[subagents]
max_concurrent = 2

[agent]
workflows = ["parallel_checks"]
```

`rustx workflow explain parallel_checks` shows the real Start, two sibling
Agent nodes, their input bindings and output contracts, and the connections to
`return_checks`. Start connects to both Agents. The Return join waits for both
and constructs the named `brevity` and `clarity` fields from their direct node
references. Completion order never determines those output keys.

Tool input: `{"text":"Use explicit inputs and return a typed result."}`.
Output shape: `{"brevity":{"passed":true},"clarity":{"passed":true}}`.
The booleans shown are illustrative; static explanation does not predict them.
These are model assessments, not executable test evidence. The configured
60000 ms Workflow timeout includes waits; two Agent runs are a conservative
bound. `maxConcurrent` controls native child capacity; it does not change the
Workflow Tool's Sequential outer sibling policy. No Git repository is required.

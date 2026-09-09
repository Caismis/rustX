# Fixed keyed parallel checks

Copy `.agents/workflows/parallel_checks.yaml` and
`.agents/subagents/reviewer.md` into your trusted workspace. Select your model as
described in [the setup guide](README.md).

Minimum project configuration:

```json
{
  "subagents": {"definitions": ["reviewer"], "workflow": ["reviewer"], "maxConcurrent": 2},
  "workflows": {"definitions": ["parallel_checks"], "main": ["parallel_checks"]}
}
```

`rustx workflow explain parallel_checks` shows the fixed `brevity` and `clarity`
branch identities, their explicit input projections, Agent output contracts,
Return bindings, and the edge from `check_text` to `return_checks`. Both branches
must settle before the join commits. Their keys are independent of completion
order. Each private block only sees its own `args` and local results.

Tool input: `{"text":"Use explicit inputs and return a typed result."}`.
Output shape: `{"brevity":{"passed":true},"clarity":{"passed":true}}`.
The booleans shown are illustrative; static explanation does not predict them.
These are model assessments, not executable test evidence. The configured
60000 ms Workflow timeout includes waits; two Agent runs are a conservative
bound. `maxConcurrent` controls native child capacity; it does not change the
Workflow Tool's Sequential outer sibling policy. No Git candidate is requested.

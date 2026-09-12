# Human Review of a bounded plan

Copy `.agents/workflows/human_plan.yaml` into a trusted workspace and select a
model for the ordinary invoking Agent turn. This template needs no Subagent role:

```toml
[workflows]
main = ["human_plan"]
```

Inspect with `rustx workflow check human_plan` or `rustx workflow explain human_plan`.
Invoke the concrete `human_plan` Tool with `{"plan":{"summary":"Write a short summary, then check clarity."}}`.
The committed input object is the structured plan subject. Existing native Review ownership
publishes that exact business decision to the interaction provider; the result
contains `accepted` and `feedback`. The template returns both without treating
acceptance as Tool execution approval or a Question answer.

The entire invocation, including human wait, has a configured 120000 ms timeout.
Use an attached client with native Review support for execution. Offline inspection
neither creates a waiter nor verifies future interaction availability. Native
Workflow and Review value/subject bounds apply to the plan. No candidate is
allocated and no Git repository is required. Review of an actual candidate,
candidate identity and handoff remain demonstrated by the complete reference stack.

# Small Workflow authoring templates

These three independent starting points use the same native loader, compiler,
and foreground Tool boundary as the complete reference workflows. All three
work in a **non-Git workspace**: none requests a run candidate, and the canonical
`reviewer.md` role explicitly disables worktrees. Inputs and retained values are
bounded by the native Workflow value limits (64 KiB per value, 4 MiB aggregate);
the closed Workflow schema vocabulary intentionally does not support `maxLength`.

Review the files before explicitly trusting this directory. From the repository
root, with a built `rustx` on PATH:

```sh
rustx --workspace examples/local-runtime/workflow-templates --trust grant
rustx workflow check typed_agent --workspace examples/local-runtime/workflow-templates \
  --models examples/local-runtime/minimal/models.jsonc --model example/demo-model --json
rustx workflow explain parallel_checks --workspace examples/local-runtime/workflow-templates \
  --models examples/local-runtime/minimal/models.jsonc --model example/demo-model --json
```

The example model is a declaration, not a working provider credential. Select
your configured model for execution. The role inherits that selected model.
Offline commands never read credential values or verify a provider. Exit 3 is
expected even for valid templates: runtime readiness remains unresolved.

The included `rustx.jsonc` registers all three for convenience. To start with
only one, use the minimum configuration in its guide:

- [Typed Agent → Return](typed-agent.md): explicit input, one named role, typed output.
- [Fixed keyed parallel checks](parallel-checks.md): two lexical branches and deterministic join.
- [Human plan Review](human-plan.md): existing Review rendezvous, no Agent or candidate required.

Invoke each by its concrete Tool name from an ordinary foreground Agent turn,
using the JSON input in its guide. There is no `workflow run` command. Main Tool
selection can further exclude an exposed Workflow; registration alone grants
neither model exposure nor a running Workflow's frozen execution authority.

Keep the full `../workspace/.agents/workflows/implement_and_review.yaml` and
`parallel_review.yaml` reference stack. Those examples and their provider-emulator
conformance scenarios exercise real candidate implementation, verification,
repair, human interaction and settlement; these small templates supplement them.

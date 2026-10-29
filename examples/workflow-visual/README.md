# Visual nested Workflow example

[`collect_and_repeat.yaml`](.agents/workflows/collect_and_repeat.yaml) is a native
file with a real Start, two concurrent one-iteration Loops, a join, and a bounded
Loop. Multiple outgoing edges express parallelism.
All three child Blocks deliberately reuse `done`. Root input is `{message:
"hello"}`; both branches return `{echo: "hello"}` and the Loop returns a
`satisfied` aggregate after one iteration. The final result contains keyed
`parallel.left`/`parallel.right` plus `loop.status`, `loop.iterations` and
`loop.result`. No model, Agent or Tool leaf is needed.

The file includes an author display title, a position and a private Markdown note.
Those change source revision only; they never become execution or Tool output.
Open this directory as an authorized Workspace in the Web console, choose
**Workflows**, and open `collect_and_repeat`. Expand the Loop frames;
select a Start/Return to edit its own Block contract. Check and Save use the same
native authoring service as other Workflow files.

From the repository root, inspect without execution:

```sh
rustx workflow check collect_and_repeat --workspace examples/workflow-visual \
  --config "$(pwd)/examples/local-runtime/rustx.toml" --model example/demo-model --json
```

As for the other offline examples, exit 3 means provider readiness was not
established, even for a structurally valid graph. For foreground invocation,
select `collect_and_repeat` in your Agent's Workflows and provide the declared
input; there is no `workflow run` command. See the [editing guide and deterministic
native/browser tests](../../docs/workflow-visual-editor.md).

# Visual nested Workflow example

[`collect_and_repeat.yaml`](.agents/workflows/collect_and_repeat.yaml) is a native
file with a real Start, two concurrent one-round Loops, a join, and a Loop
that counts. Multiple outgoing edges express parallelism. As in Dify, a Loop's
value is its loop variables, and a body reads the variables around it. Root
input is `{message: "hello"}`; both one-round Loops return `{echo: "hello"}`,
and the `repeat` Loop's Variable Assigner adds one to `count` in each of its
three rounds. The final result is
`{parallel: {left: {echo}, right: {echo}}, loop: {count: 3, echo: "hello"}}`.
No model, Agent or Tool leaf is needed.

The file includes an author display title, a position and a private Markdown note.
Those change source revision only; they never become execution or Tool output.
Open this directory as an authorized Workspace in the Web console, choose
**Workflows**, and open `collect_and_repeat`. Expand the Loop frames;
select the root Start or Return to edit the workflow's contract. Check and Save use the same
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

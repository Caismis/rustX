# Visual Workflow authoring

The editor authors the same native Workflow file used by the CLI and runtime.
It uses App Server's file revision, validation, analysis and CAS boundaries.
See [the file contract](workflow-authoring.md) and [execution semantics](workflow-programs.md).

## Reference and graph model

The read-only Dify checkout was inspected outside rustX at
[`80265fcfe110f0aea056fcb3566d322986014b78`](https://github.com/langgenius/dify/tree/80265fcfe110f0aea056fcb3566d322986014b78).
Reference paths below are relative to `web/app/components/workflow/`.

| Reference | Adopted interaction |
| --- | --- |
| `nodes/_base/node.tsx`, `nodes/_base/components/node-handle.tsx` | Readable cards, colored icons, output handles and add-after actions |
| `nodes/_base/components/workflow-panel/index.tsx`, `nodes/panel-components.ts` | Shared inspector shell and specialized configuration panels |
| `nodes/start/panel.tsx` | Start input schema and an inline next-step list |
| `nodes/_base/components/variable/var-reference-picker.tsx` | Searchable variables beside the exact consumer |
| `nodes/if-else/node.tsx` | IF/ELSE exits |
| `nodes/loop/node.tsx`, `nodes/iteration/use-interactions.ts` | Nested body frame, focus and sizing |
| `custom-edge.tsx`, `block-selector/blocks.tsx` | Searchable next-node picker and explicit edge insertion |
| `block-selector/tabs.tsx`, `blocks.tsx`, `tool/action-item.tsx`, `preview-card.tsx` | Nodes and Tools tabs, classified rows, a node per Tool and a hover preview card |
| `nodes/tool/panel.tsx`, `components/tool-form/`, `mixed-variable-text-input/` | Tool parameters as a form: text with inline variables, a variable/constant switch, folded output variables |
| `note-node/index.tsx`, `utils/elk-layout.ts` | Notes and hierarchical organize |
| `nodes/llm/components/json-schema-config-modal/` (studied at `ea5e4e9e21`) | Transactional structured-output dialog, visual field cards, JSON Schema tab and JSON-example import |

The implementation uses rustX's existing XState, XYFlow, shared icons, tokens,
localization and Markdown components. Dify source, CSS, artwork, branding,
backend and DSL are not copied or vendored.

Start is a real native node in every Block. Its input contract belongs to the
Block, while its title and geometry use ordinary node metadata. There is no
Parallel node or branch-lane container. Parallelism comes from multiple
connections on any output port, including Start and either Branch/Loop exit.
Only Loop introduces a private Block.

## Next step and canvas gestures

The inspector's **Next step** section shows the source icon, every existing
successor and a dashed add row. An empty port says **Select the next node**;
a connected port says **Add parallel node**. Branch and Loop list each output
port separately. All node icons have a solid semantic background and a white
glyph, consistently across cards, inspector headers, successor rows and pickers.

The canvas **+** and the next-step popover share one chooser, as Dify's block
selector: a **Nodes** tab classified into a default group and **Logic**, a
**Tools** tab, and, for a next step, the **Existing** nodes. The Tools tab
lists the builtin Tools (the scope's vocabulary carries each one's
description, argument schema and result; `ask_user` is left out, as a
Workflow asks a person through Review), then, as Dify's tool providers, each
MCP server and Managed Python package a Workflow of this scope can use, with
a Global or Workspace badge: a Global Workflow sees the User sources, a
Workspace Workflow also the Workspace's. A provider folds its Tools; one that
cannot be listed shows why. `workflow/toolCatalog` lists them when the tab
first opens, connecting to each source once (preparing a package's
environment first) and closing it. Each tab keeps its own search; hovering a
row shows its description in a card beside it. A Tool row adds a Tool node for
that Tool, titled by its name, described by its first sentence, and with the
result its Tool declares.

A Tool node's panel is its Tool's parameters as a form: each name, required
mark and description above its control. Text parameters take typed text with
`/` or `{` inserting variables (a `template` value); other types switch
between a variable and a constant, as Dify's `{x}` / ✎ switch. Unset
parameters are absent from the arguments. Output variables fold below and
are fixed by the Tool, never edited. Below them, as on a Model, come retry on failure and
error handling; a fail branch adds the card's error row and `fail` port.

Selecting a node type from the searchable popover creates a new sibling and
adds one connection. Selecting an existing node adds only that connection.
Already connected targets are omitted. Existing successors remain connected.
Each row can focus its target or disconnect that specific edge. All changes
are ordinary document transactions with Undo/Redo.

The output handle's **+** opens the same append action whether that port has
zero, one or several edges. Dragging from the handle adds another connection.
Explicit insertion uses the connection's midpoint button: it replaces only
that edge with two edges through a new single-exit node, preserving siblings.
Select a connection to reveal its reconnectable endpoints. The selected edge
is raised above crossing lines. Reconnect changes only that connection,
including when its new source is Start. Duplicate edges, self-connections, Start as a target and connections
across Block boundaries are rejected before changing the document.

The picker uses Floating UI for viewport-aware placement and renders through a
portal. Search takes focus; Escape closes it and restores focus to the trigger.
Click-away dismissal and desktop/mobile bounds are covered by browser tests.
The icon rail and connected handle markers do not intercept handle buttons.

## Ownership

| Owner | Responsibility |
| --- | --- |
| Native file service | Complete document, source revision, compilation, serialization and CAS |
| Native candidate analysis | Declarations, activation scope, consumer compatibility and bounded diagnostics |
| `machines/editor.ts` | Working document, exact-version drafts, history, checks, conflicts and uncertain writes |
| `document/commands.ts`, `schema.ts`, `geometry.ts`, `clipboard.ts` | Atomic typed transformations |
| `WorkflowEditor`, `Inspector`, specialized panels | Selection, field navigation, inspector sizing and configuration |
| `NextStep` | Successor rows and searchable append picker |
| `NodeChooser`, `Palette` | The shared node and Tool chooser, at the pointer or beside a port |
| `ToolPanel`, `TemplateInput` | Builtin Tool parameter forms and text with inline variables |
| `SchemaEditor`, `ValueEditor`, `ReferencePicker` | Native schema/value forms and consumer-addressed analysis queries |
| `StructuredOutputDialog` | Model-output working copy, field confirmations, native declaration validation and one saved history transaction |
| `scene.ts`, `WorkflowNode`, `WorkflowEdge`, `Canvas` | Recursive scene, connection gestures and transient viewport state |
| `connections.ts` | One connection policy for previews and committed gestures |
| `layout.ts` | Lazy ELK, measured layout, stale-request cancellation and metadata-only updates |
| File `editor` | Node geometry, titles/descriptions, notes and field order |
| Personal view state | Exact-file viewport, Loop collapse and inspector width |

## Node panels and scopes

| View | Authoring behavior |
| --- | --- |
| Start | Nondeletable Block input schema, descriptions and one or more next nodes |
| Model | Session/registered model, fixed instruction, bindings and text/JSON output |
| Agent | Profile picked from the scope's profiles, task in the prompt editor with variables, structured output fields |
| Tool | Exact selector, arguments and text/JSON result projection |
| Branch | Compact IF/ELIF/ELSE card, ordered cases, searchable variable conditions, AND/OR and multiple successors per exit |
| Loop | Initial input, private body contracts, `until`, `carry`, iteration limit and both exits |
| Review | Plan subject/context and native accepted/feedback outputs |
| Return | Whole or constructed output and the owning Block's output contract |

Branch settings add or remove ELIF cases and build conditions from native variable
choices. AND/OR groups use the existing predicate model; advanced conditions
retain nested predicates. Case IDs stay stable when an earlier case is removed,
and only the removed case's edges are deleted. The grouped Next step section
uses those same exits. Empty conditions remain unfinished and cannot be saved.

Visual schema fields cover scalars, objects, arrays, requiredness, descriptions,
supported enum/const and display order. Advanced JSON edits the same value.
Opening an imported schema does not normalize it. Array items are optional;
removing them retains unfinished child text as a detached draft.

References are structured arrays of literal field names. Native analysis proves
that a producer is an ancestor and runs whenever the consumer runs. A join can
read both concurrent producers; it cannot read an optional producer from a
conditional path. Loop bodies see only their projected `args` and local values.
Loop `until` and `carry` see only body `result`. Parent consumers see the Loop's
structural result. The browser does not implement a second type checker.

Qualified identities encode the complete Block path and local node. Each path
step names a Loop owner. Start, ordinary nodes, edges and notes have separate
view identity domains; actual Start IDs still remain native node IDs. Renaming
a node updates references, edges, presentation and draft identity atomically.
Copy/paste clones private Loop subtrees and their presentation, assigns fresh
IDs, and preserves literal data. Start cannot be deleted or duplicated. Orphan
presentation is retained, and a rename/copy collision refuses the transaction.

Field drafts remain attached to their exact document version and structured
field identity. Navigation, file conflicts and native analysis never silently
replace unfinished text. Save/Check still require the authoritative native
compiler. Metadata-only organize, move and resize actions leave executable
schemas, nodes, references and edges unchanged.

## Verification

| Contract | Coverage |
| --- | --- |
| Multi-edge append, duplicate rejection, real Start reconnect and sibling-preserving insertion | `web-console/test/workflow-graph.test.ts` |
| Inspector append, canvas +, real pointer reconnect, Save YAML and desktop/mobile picker | `web-console/test/e2e/workflow-visual.spec.ts` |
| Schema/value forms, drafts, references, presentation, clipboard and history | `web-console/test/workflow-*.test.*` |
| Active-path joins, conditional scope, skipped Returns and retired syntax rejection | `src/runtime/workflow/tests/graph.rs` |
| Opposite completion orders, capacity one, failure aggregation and cancellation drain | Native Workflow runtime and template tests |
| Sibling work continues during pending human Review | `src/runtime/workflow/tests/shared_workspace.rs` |
| Nested Loop execution, output values and inert author notes | `editor_authored_nested_file_executes_keyed_results_and_both_loop_exits` |
| Native examples, canonical YAML, file size and revision boundaries | Native file and authoring tests |

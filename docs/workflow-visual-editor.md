# Visual Workflow authoring

Issue #511 builds on #510, merged by #512. The implementation base is
`60d669ee5b392b5872419fc897b1112d4330c216`, which already contains native file
metadata, scope-aware candidate analysis and bounded model discovery. This
change consumes App Server v62 without a new protocol, backend, runtime node,
execution endpoint or browser program compiler.

## Reference and design decisions

The read-only Dify checkout was inspected outside rustX at
[`80265fcfe110f0aea056fcb3566d322986014b78`](https://github.com/langgenius/dify/tree/80265fcfe110f0aea056fcb3566d322986014b78).
Paths below are relative to `web/app/components/workflow/`.

| Inspected source | Adopted interaction | rustX decision |
| --- | --- | --- |
| `nodes/_base/node.tsx`, `nodes/_base/components/node-handle.tsx` | Readable card header, selection, handles, add-after/actions | Current icons, tokens and XYFlow 12; IDs remain native local names |
| `nodes/_base/components/workflow-panel/index.tsx`, `nodes/panel-components.ts` | Shared panel shell with specialized content | One resizable Settings inspector and independent per-kind panels |
| `nodes/start/panel.tsx` | Independent input anchor | Synthetic Start edits the owning native Block input/entry |
| `nodes/llm/components/json-schema-config-modal/visual-editor/{index,schema-node}.tsx` | Structured nested properties, requiredness, description | Native closed schema vocabulary; advanced JSON retains the exact same value |
| `nodes/_base/components/variable/var-reference-picker.tsx` | Searchable grouped variables beside each consumer | Native consumer-addressed facts; no expression engine or interpolation |
| `nodes/if-else/node.tsx` | IF/ELSE regions and separate exits | Boolean/equality/Not/And/Or only; no ELIF or ordering predicates |
| `nodes/loop/node.tsx`, `nodes/iteration/use-interactions.ts` | Nested frame, child focus and sizing | Loop body and fixed named Parallel lanes; no array Iteration semantics |
| `custom-edge.tsx`, `block-selector/blocks.tsx`, `operator/control.tsx`, `shortcuts/workflow-canvas.tsx` | Atomic insert, search palette, canvas modes and keyboard ownership | Legal native ports only; no image export, single-node Run or Publish |
| `note-node/index.tsx`, `utils/elk-layout.ts` | Notes and hierarchical organize | Safe existing Markdown, durable native metadata and lazy ELK |

Dify's modified Apache license, branding/multitenant conditions and appearance
patent statement were inspected. No Dify source, CSS, artwork, branding, store,
backend or DSL is copied or vendored. The implementation is independent rustX
code. It retains XState, XYFlow, shared icons, localization and Markdown rather
than importing Dify's Zustand/Jotai, Lexical or design system. The surrounding
application sidebar, authorized Workspace selection and library-to-file flow
remain the existing rustX product.

An image-generated concept was used privately to compare hierarchy (readable
cards, a separate Start, an in-place Loop and a 392px inspector). It is design
exploration, not the owner's screenshot or rendered evidence. Actual rustX
shell, neutral Save button, current icon system and localization intentionally
supersede that concept's invented navigation and colors. No generated asset is
shipped in the product.

## State and component ownership

| Owner | Responsibility |
| --- | --- |
| Native `WorkflowFileDocument` and authoring service | Typed definition plus metadata, complete source revision, validation, serialization and CAS |
| Native candidate analysis | Declarations, scope, consumer compatibility, pending/unknown facts and diagnostics |
| `machines/editor.ts` | One working document, exact-version field drafts, history/coalescing, check generations, conflict and uncertain-write receipts |
| `document/commands.ts`, `schema.ts`, `geometry.ts`, `clipboard.ts` | Atomic typed document transformations; no independent persistence |
| `WorkflowEditor` / `Inspector` | Selection, field navigation, one panel shell, width, disclosure and specialized panels |
| `panels/{Inference,Control,Result}Panels` | Model/Agent, Branch/Parallel/Loop and Tool/Review/Return configuration |
| `SchemaEditor`, `ValueEditor`, `ReferencePicker` | Shared native schema/value forms and bounded native variable queries |
| `scene.ts`, `WorkflowNode`, `Canvas` | One qualified recursive scene; transient selection/pointer state and viewport |
| `layout.ts` | Lazy ELK, bottom-up measured layout, stale-request cancellation and metadata-only result |
| File `editor` metadata | Titles/descriptions, node/entry/lane geometry, notes and field order |
| Personal view state | One viewport for the exact file, collapse state and inspector width; never executable or file authority |

The old universal `forms/NodeForms.tsx`, Config/Input/Output tabs, per-Block
renderer/cache, node-move-only machine event and shortest-distance layout/reset
path are removed. There is one editor and one production layout implementation;
no compatibility reader, migration or legacy mode is retained.

## Nine node experiences

| View | Authoring behavior |
| --- | --- |
| Start | Nondeletable per-Block input schema, model-facing descriptions and native entry selection |
| Model | Session or explicit registered model/profile, fixed instruction, named bindings, text/JSON Schema, request parameters and output budget |
| Agent | Profile, fixed task, bindings, declared schema and absent versus explicit-empty invocation override |
| Tool | Exact builtin/source selector, argument value AST, result content part and text/JSON projection |
| Branch | IF/ELSE summaries, typed predicate builder and true/false successors |
| Parallel | Named private lanes, input mappings/contracts, keyed outputs, branch addition/rename/delete and collapse |
| Loop | Initial input, in-place body contracts, post-body `until`, next-input `carry`, limit and both exits |
| Review | Plan subject/context plus native `accepted` and `feedback` declarations |
| Return | Constructed or whole output and the owning Block contract; private Return settles only that Block |

Displaying a form does not normalize imported native values. Selecting another
value/schema type is explicit and undoable. Visual schema fields cover scalars,
objects and arrays, requiredness, descriptions, supported enum/const and display
order; advanced JSON preserves supported imported constraints. Deletion/type
changes list structurally affected consumers and leave native Check to decide
compatibility rather than substituting fabricated values.

## Variables, identities and clipboard

The picker sends the exact authored consumer, not just a producer ID. It groups
choices by native producer with display title, ID, field path, type,
availability, compatibility and diagnostic. Unknown schema and pending upstream
choices remain selectable when native facts permit them. Proven unavailable or
incompatible choices are disabled. Queries return at most 128 choices and
explicitly show truncation; model catalogs retain their existing page bounds.

| Scope | Native reference |
| --- | --- |
| Root input | `["args", "field"]` |
| Ordinary producer in its lexical Block | `["node", "field"]`, or `["node"]` for the whole value |
| Text Model | `["node"]`, the exact full string; no invented `.text` |
| Private branch/body input | Its own `["args", ...]`; parent/sibling locals do not leak |
| Loop post-body until/carry | `["result", ...]`; parent or body-local producer selectors are not silently substituted |
| Downstream Parallel | `["parallel", "branch", ...]`, keyed rather than completion-order output |
| Downstream Loop | `["loop", "status"]`, `iterations`, or `result` fields |

View IDs encode `[structured Block steps, view kind, local identity]`; Loop body
steps and a Parallel branch literally named `body` remain different. Repeated
private `done` IDs do not collide. Parent IDs and extents express geometry only;
connection/reconnect commands also enforce Block ownership and native port
legality. Moving containers never reparents children. Collapse changes no file;
selection or diagnostic navigation expands the exact ancestor chain.

Display-title edits change metadata only. Explicit native renames update owned
references, edges, entry, metadata, subtree addresses and attached draft identity,
including Undo/Redo. Schema property renames rewrite only provably corresponding
structured references/construction keys, required fields and orders. Literal and
instruction text, array element selectors and whole-value references are not
inferred or rewritten. A rename cannot adopt retained orphan order fields or
descendant targets; rejection preserves the complete executable/metadata edit.

Clipboard export includes selected definitions, internal edges and owned metadata
only. A decoder rejects unknown transport/runtime fields, unsafe JSON numbers,
malformed geometry, duplicate metadata identities, excessive depth/count/bytes
and unsupported AST variants before mutation. Nodes receive deterministic fresh
IDs, internal references and structured metadata follow, and private local IDs
stay local. Notes get fresh global IDs. External root references are retained
only within the same live source owner and Block; other destinations require a
self-contained fragment. Owned pending input refuses export. Paste/insertion is
one history transaction, with no mutation on rejection. Multi-exit Branch/Loop
and terminal Return are excluded from occupied-edge insertion.

## Stale results and input ownership

Candidate-analysis results retain the original target, authority, complete
document/generation, consumer and query owner. Model choices retain model/source
revision and page ownership. Changing a field/query/node/target retires obsolete
responses. Deferred promises prove these boundaries without sleeps.

Organize imports ELK only on demand, lays out private Blocks first and uses
measured sizes and fixed-order ports. It preserves executable definition identity
and manual notes, then applies all geometry in one transaction. Exact document
identity plus a request generation rejects old completion after a newer layout,
edit, drag, target change or unmount. Failure leaves prior geometry intact. No
layout runs on typing, selection, pan or zoom.

JSON, new keys, numeric text and bounded title/description/note fields share
editor-owned drafts. Switching selection/disclosure cannot lose them. Rename
history carries live drafts; deleting a field keeps detached recoverable text.
Save/Check first flush visible fields, renames last, and refuse while any draft
remains. Show expands the correct container/disclosure and focuses that field.
IME, text selections, CodeMirror, fields, dialogs and popovers own their keys
before canvas shortcuts. Variable selection, field order and inspector resizing
have keyboard controls.

## Acceptance and invariant traceability

| Requirement | Authoritative evidence |
| --- | --- |
| Complete create/bind/all eight native nodes/nested contracts/notes/save/fresh-profile/copy, zero model requests | `test/e2e/workflow-visual.spec.ts`, complete visual authoring case |
| Required structured output, description/enum/array/nested object and repeated downstream binding | Same spec, structured authoring case |
| Real 256-node nested rendering, native valid Check, one-step layout Undo/Redo and execution/notes unchanged | Same spec, bounded-scene case; `test/fixtures/workflow-scale.ts` |
| Qualified repeated IDs, scopes, container-relative movement and collapse identity | `test/workflow-visual-document.test.ts`, qualified scene tests |
| Atomic insertion, strict untrusted fragments, cross-scope refusal and deterministic remapping | Same unit suite, insertion/clipboard tests |
| Schema field renames, args/Return/Loop-result/aggregate reference identity | Same unit suite, schema contract tests |
| Lazy real ELK, measured containment, deterministic output, deferred stale/drag/cancel | Same unit suite, organize tests |
| Actual native 256 nodes / depth 8 / 32 branches and bounded metadata drafts | Same unit suite, capacity/draft tests |
| Exact field text, deletion recovery, versioned apply and rename history | `test/workflow-drafts.test.tsx`, `workflow-fields.test.tsx`, `workflow-editor-identity.test.tsx` |
| Out-of-order native analysis/model pages/targets | `test/workflow-analysis.test.ts`, `workflow-model.test.tsx` |
| Native scope/join violations, unknown contracts, numeric fidelity, keyboard and source conflict | `test/e2e/workflow-editor.spec.ts` |
| Complete metadata CAS, copy/delete, lost ACK and program/Tool identity | `src/local_runtime/workflow_authoring/tests.rs`: `complete_file_revisions_fence_metadata_writes_copies_deletes_and_lost_ack_repair` and analysis tests from #510 |
| Actual values, keyed Parallel, Loop satisfied/exhausted and no author notes/children in runtime snapshot | `src/runtime/workflow.rs`: `editor_authored_nested_file_executes_keyed_results_and_both_loop_exits` |

The last native test consumes the exact
[`visual-authoring.json`](../tests/fixtures/workflow/visual-authoring.json)
produced by the frontend command acceptance test. Its two private branches and
Loop body reuse `done`; the runtime returns actual keyed values and verifies
both Loop exits with a bounded three-iteration exhaustion variant. This is test
harness execution, not a new product run API. The independent
[executable example](../examples/workflow-visual/README.md) shows the same native
features without model or Tool leaves.

## Visual evidence and reproduction

The Browser plugin was unavailable in this task's tool inventory. Browser QA
therefore uses the repository's Playwright driver over the real native App
Server/Product Host. Only Chromium is in the immutable authority container:
`mcr.microsoft.com/playwright:v1.63.0-noble@sha256:bc6ab0d6d44ff4826e4cb8c1e6d801e185bfc42bb0753f8e2a30efc70db054c7`.
No host fonts or alternate screenshot authority are used.

The committed [evidence directory](workflow-visual-evidence/) contains real full
shell captures for no selection, Start, Model, variable picker, schema, all
specialized panels, expanded/collapsed containers, notes, conflict and narrow
windows. They are inspected alongside the design hierarchy; regression snapshots
remain separate exact comparisons. The fidelity ledger is:

| Surface | Verification and intentional adaptation |
| --- | --- |
| Shell/navigation | Existing rustX sidebar and two-level file flow retained; no Dify navigation copied |
| Cards | Readable 256px cards at 100% initial zoom; explicit Fit handles overview, so large graphs do not silently shrink ordinary editing |
| Inspector | 392px default, bounded 320..640px width, one Settings surface and progressive disclosure |
| Containers | Children visible in place with real private Starts/Returns; viewport pans to selection and preserves zoom |
| Schema/picker | Nested native fields and consumer facts, rather than Dify-specific types or template interpolation |
| Narrow window | Inspector becomes a bottom sheet based on actual editor width, toolbar moves horizontally, minimap yields space |
| Notes/errors | Existing safe Markdown and truthful native diagnostics/conflicts, no hidden pending-field writes |

Reproduce all interactions and the always-on trace (including fresh-profile
reopen) with:

```sh
pnpm -C web-console build
bash web-console/scripts/browser-tests.sh workflow-visual.spec.ts workflow-editor.spec.ts
pnpm -C web-console exec playwright show-trace <test-results/.../trace.zip>
```

The complete create/bind/nest/save/reopen case records `trace.zip` and ordered PNG
attachments. CI's merged browser report retains them. The bounded-scene case
writes `bounded-scene-measurements.json` with wall-clock render/layout durations,
fixture source and viewport. Each local budget is declared as 10 seconds before
measurement. These are acceptance budgets for this specific 256-node fixture,
not a claim of unbounded scalability. Native bounds remain unchanged.

## Validation policy

The issue requires the complete web unit and browser suites, typecheck, i18n,
provenance/build, formatting, all-target Clippy, native Workflow execution,
protocol/schema drift checks and affected consumers. Targeted runs are debugging
feedback only. The final PR must carry `full-verification`; its exact final head
must pass **Merge verification** with FULL selected. The PR report records final
SHA, independent review, complete results and machine-specific measurements.
No auto-merge or self-merge is part of this delivery.

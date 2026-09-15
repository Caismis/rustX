# Pinned Harness presentation provenance

**UI source reuse does not transfer semantic ownership.**

Repository: https://github.com/deepseek-ai/deepseek-harness

Exact commit: `c291e7961a515f6d7af9304e7fd1d257929aef26`.
Verified with `git rev-parse HEAD` in the detached external checkout
`/home/caismis/Documents/codes/deepseek-harness-reference` before implementation.
The reference checkout is neither a build input nor a runtime dependency.

## Reproducible inventory

[source-inventory.json](source-inventory.json) extends the #289 format. Each actual
source-derived file records its upstream path/hash, local destination, treatment,
retained direct imports, excluded semantic dependencies and MIT copyright notice.
Hashes identify **original pinned upstream bytes**, not edited local bytes.
`inspected` separately records the #304 inspection; membership does not imply reuse.

`pnpm check:provenance` verifies destinations, notices, headers, pinned SHA, and
presentation import direction. `pnpm build` additionally verifies that both public
notice files reached the production artifact. `node scripts/notices.ts --write`
reproduces dependency notices from the frozen production install closure; the check
fails on drift. No check fetches Harness. To audit original hashes, compare the
inventory against the separate checkout at the exact pinned commit.

## Source-derived groups

| Upstream, relative to `packages/client/` | Local destination under `src/` | Treatment |
| --- | --- | --- |
| `ui-primitives/src/{Button,Input,Pill,StateDot,DisclosureRow}.{tsx,module.css}` and `icons/{index.tsx,props.ts}` | `presentation/primitives/` | Retained #289 extraction; Button gains React 19 ref support; Disclosure gains named icon button and controlled-region association. Four generic icons only. |
| `ui-theme/src/styles/{base,design-platform}.css` | `presentation/theme/{base,tokens}.css` | Retained token/font/motion foundation; shared browser reset moved here from console CSS. Existing brand-token rename remains. |
| `ui-layout/src/client/AppFrame.{tsx,module.css}` | `presentation/layout/AppFrame.*` | Reworked #289 geometry: optional navigation/main/dock children, occupant-supplied dock label, responsive grid/stack. Removed developer-specific required inspector API. No layout service, stores, resize lifecycle or slots. |
| `ui-sidebar/src/client/SidebarRoot.*`, `ui-chat/src/client/chat/MessageItem.*`, `ui-conversation/src/client/skeleton/InputBar.*`, `ui-tool/src/client/tool/components/ToolRow.*`, `ui-approval/src/client/ApprovalPanel.*`, `ui-user-questions/src/client/QuestionComposer.*` | `app/components/` | Existing #289 extracted product components moved out of generic presentation. Questionnaire retains native DTO/index encoding in the product/binding layer. No compatibility reexports. |
| `ui-primitives/src/Menu.tsx`, `HoverCard.tsx`, `Modal.tsx` | `presentation/primitives/{Menu,Popover,Dialog}.tsx` and `useAnchoredSurface.ts` | Rewritten from selected anchor/dismissal, arrow navigation and modal chrome. Flat native-button menu, explicitly activated popover, native modal dialog plus Tab wrapping. Shared viewport-clamped placement only; no submenu, hover grace or iframe modes. |
| `ui-primitives/src/{Menu,Modal}.module.css` | `presentation/primitives/` | Adapted menu/card/modal token geometry; unused menu modes removed. |
| `ui-primitives/src/markdown/{incremental,parse,cjkFriendlyStrong,mathCompatibility}.ts` | `presentation/markdown/` | Copied parser algorithms with source headers; retains two-block frontier and completed-line open-fence frontier. |
| `ui-primitives/src/markdown/{MarkdownText,render,katex,CodeBlock}.tsx`, `useViewportHighlighting.ts`, `highlight.ts`, `MarkdownText.module.css`, `CodeBlock.module.css` | `presentation/markdown/` | Adapted direct renderer, caches, code chrome and safe TeX. Removed file-mention actions, local/remote image loading, path classification and image resolver vocabulary. Restricted highlighter to five grammars; adjusted nullable refs for React 19. Clipboard uses a local async browser helper without deprecated fallback. rustX intentionally discards streaming-only code-render state at settlement: all settled fences use the canonical cold renderer regardless of lazy-grammar timing. |
| `ui-theme/src/styles/shiki.css` | `presentation/theme/shiki.css` | Copied syntax palette, with source header. |
| `ui-primitives/tests/{markdown-incremental,streaming-code-block}.client.spec.tsx` | `test/{markdown-incremental,streaming-code-block}.test.tsx` | Adapted deterministic upstream contracts to local imports/labels and selected grammars. Markdown-source highlighting tests excluded with that grammar; CRLF, token equality and streaming DOM retention preserved. Upstream settlement-identity assertions are replaced by exact cold-settled equivalence. |
| repository `LICENSE` | `public/LICENSE-DeepSeek-Harness.txt` | Verbatim MIT license, Copyright (c) 2026 DeepSeek. |

New `Surface` card/feedback composition, clipboard helper, browser fixtures,
security/state tests and validation scripts are rustX-authored; they are not claimed
as imported Harness source. Existing generic icon assets retain the Harness notice.
No official branding assets are added.

## Inspected-only source and dependency decisions

The exact paths are in `inspected`. Important inspected-only inputs include:

- `packages/client/README.md` and package manifests for ui-primitives, ui-theme,
  ui-layout, ui-conversation, ui-chat and ui-commands: package/dependency map.
- `ui-chat/src/client/chat/AssistantMarkdown.tsx`: actual Web Markdown path and
  its Host `/api/file` rewrite, attachment slots and file-mention owners. None of
  that adapter was copied. Assistant text is supplied by rustX `Conversation`.
- `ui-commands/src/client/PopupSelectView.tsx`: focus/outside interaction contract
  inspected; Session-aware popup controller, command directory and Remote actions
  excluded. The generic menu is derived from Menu, not this command subsystem.
- `ui-primitives/src/clipboard.ts`: inspected; deprecated execCommand fallback
  excluded. The small rustX helper reports browser permission failure accurately.
- `ui-primitives/tests/markdown.client.spec.tsx`: semantic/security examples
  inspected; local representative tests are independently authored.

#289 source files were reviewed alongside the new closure. Inspecting an existing
upstream component does not create a new reuse claim beyond its inventory entry.

## Production dependencies and licenses

Existing: React/React DOM 19.3.0, clsx 2.1.1 (scheduler transitively).
Added exact versions matching the selected upstream manifest baseline:

- `mdast-util-from-markdown` 2.0.3, `mdast-util-gfm` 3.1.0,
  `micromark-extension-gfm` 3.0.0: semantic GFM AST without react-markdown/rehype.
- `mdast-util-math` 3.0.0, `micromark-extension-math` 3.1.0, KaTeX 0.16.47:
  settled math and safe incomplete/malformed fallback, including KaTeX font assets.
- `micromark-core-commonmark` 2.0.3, `micromark-util-character` 2.1.1,
  `micromark-util-classify-character` 2.0.1, `micromark-util-symbol` 2.0.1,
  `micromark-factory-space` 2.0.1, `micromark-util-sanitize-uri` 2.0.1:
  explicit imports used by the upstream CJK/TeX grammar extensions and URL renderer.
- Shiki and `@shikijs/langs` 4.3.1: fine-grained core/JavaScript regex engine;
  TypeScript (also approximate JS/JSX/TSX), shell and JSON eager, Rust/Python lazy.
  Other languages render plain. No Monaco, editor, WASM import, full language bundle
  or theme registry enters the application. Shiki's installed dependency graph is
  larger than its selected browser modules; notices cover that whole graph.

Development types: `@types/mdast` 4.0.4 and `micromark-util-types` 2.0.2.
KaTeX supplies its own types. Exact direct and transitive resolution is in pnpm-lock.yaml.
All retained package license texts (98 production-install packages, including
MIT/ISC/Apache/BSD notices) are reproduced in THIRD-PARTY-NOTICES.txt. KaTeX's
shipped MIT notice covers its distribution. System fonts remain unbundled references.
The build reports an approximately 1 MB minified main chunk from React + parser +
KaTeX + Shiki; this is a known payload cost, not a timing correctness claim.

## Excluded authority

No Harness Host, Session Controller, Session/event log, Agent runtime, Remote API,
Cordis lifecycle, dynamic module loader, slots, provider configuration, workspace
mutation or browser persistence of runtime facts is imported. No service shim is
provided. rustX client/bindings continue to consume the generated native protocol.
Menu/dialog state and Markdown caches are disposable browser presentation state.

## WEB-02 inspection and adaptation

The external checkout was reverified at the same exact pinned SHA before Chat
changes. The `inspected` array now includes the Chat scroll/partial helpers,
conversation submission policy tests, attachment cards/rail/lightbox, attachment
load/retry and composer tests, Tool call tree/CSS, and the five package READMEs.
These are inspection records; they do not imply copying entire packages.

New source-derived destinations:

- `presentation/layout/ChatViewport.tsx`: rewritten from `ui-chat/.../ChatView.tsx`.
  Retains stable row/viewport measurement, distinct reader/follow ownership and
  ResizeObserver cleanup. Uses React's pre-mutation snapshot lifecycle, with no
  upstream timers, turn navigation, Session store, optimistic echoes or slots.
- `presentation/attachments/AttachmentCard.tsx` and its CSS: rewritten/adapted from
  `ui-attachment/MessageImage.tsx`, its CSS and `FileCard.tsx`. The inventory's
  `additional_sources` records secondary origins under the same destination's
  license, dependency closure and exclusions. Uses the WEB-01 native Dialog for
  focus containment/restoration instead of importing the upstream lightbox owner.
- Existing `app/components/InputBar.tsx`: extends the inventoried source extraction
  with one mixed ordered draft rail. Native File drafts replace Harness upload,
  Session, Remote and submission-echo semantics.

No Harness Session Controller, event assembler, Host/Remote API, Cordis lifecycle,
workspace path owner, provider translation, retry/fork or plugin slots are imported.
The 64px thumbnails, 240px file cards and original-image dialog follow the inspected
attachment presentation. Deliberate deviation: durable bytes load on explicit
activation so a large history page cannot eagerly consume the finite image budget.
The original is rendered at fit-to-viewport size; no thumbnail bytes are durable.

## WEB-03 Trace / Trajectory

Reverified the same external checkout at
`c291e7961a515f6d7af9304e7fd1d257929aef26` before implementing #306. The existing
`inspected` list now includes WEB-03 paths; it is an inspection inventory, not a
claim that every upstream feature was ported.

| Inspected upstream input | WEB-03 treatment |
| --- | --- |
| `ui-trajectory/src/client/TrajectoryTable.tsx`, `TrajectoryTimeline.tsx`, `trajectory-search-index.ts`, `trajectory-virtual-rows.ts` | Rewritten into `src/app/Trajectory.tsx`: native Trace props, timing lanes, bounded loaded-window search, stable selection and end-anchored virtualization |
| `ui-trajectory/src/client/TrajectoryTable.module.css` | Adapted dense row/inspector geometry into `src/app/Trajectory.module.css`, using existing rustX theme tokens |
| `ui-trajectory/tests/table.client.spec.tsx` | Adapted selection/folding/scroll interaction contracts into `test/trajectory.test.tsx`, with measured deterministic layout |
| Trajectory contract, record, event projection, snapshot builder, Assistant/compaction definitions and layout/virtual-row/snapshot-builder tests | Inspected only; their Session event interpretation and state machines are excluded |
| `ui-conversation` request inspection/selection, `ui-tool` ToolRow/tree, `ui-primitives` disclosures and tests, `session-projection` types/registry/tests and package docs | Inspected native-data/presentation boundary; existing WEB-01/02 primitives and artifact components reused, no new Harness runtime imports |

Retained external presentation dependencies: existing React, CSS Modules and
rustX-owned primitives; added `@tanstack/react-virtual` 3.14.9 (locked dependency
closure includes `@tanstack/virtual-core` 3.17.7). The original Harness manifest
also selects React Virtual starting at 3.14.9. `diff`, Cordis, Session Controller,
Host/Remote APIs, dynamic view/service registries, provider configuration,
workspace mutation and Harness event/node assembly were deliberately excluded.
The existing generated dependency notice file includes the TanStack MIT notices.

No upstream server projection was copied. `runtime_client::trace`, the internal
durable query seam and Web Trace cache are rustX-authored. The implementation
uses rustX logical Step/request identities and terminal certainty; it does not
claim Harness request payload visibility, compaction IDs, first-token timing,
plugin records or other semantics rustX cannot truthfully supply. See
[Trace architecture](../docs/trace.md) for the explicit field withholding policy.

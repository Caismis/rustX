# Pinned Harness presentation provenance

## WEB-05 command and historical-action inspection

Verified external reference HEAD again before editing for #308:
`c291e7961a515f6d7af9304e7fd1d257929aef26`. The shared JSON inventory contains the
individual inspected paths, source hashes, local destinations and retained imports.

| Inspected upstream implementation/tests/styles/docs | Local treatment |
| --- | --- |
| `ui-commands`: directory, resolution, presentation, service, popup controller, PopupSelectView, README and popup/view tests | One rustX registry replaces the Host directory. `CommandPanel.tsx` rewrites selector filtering, arrows/Enter, loading/errors and dismissal. Native Dialog supplies focus containment/restoration rather than Harness slots. No popup controller or generic execution is imported. |
| `ui-input-trigger`: detect/menu core, MenuView and styles, README | `CommandMenu.tsx` rewrites textarea-focused discovery, highlight, mouse-down selection and outside dismissal. Leading bare-token grammar is deliberately narrower than upstream inline trigger/claim/reference grammar. |
| `ui-primitives/src/rank-by-name.ts` and its tests | `matching.ts` adapts prefix/alignment/source-order ranking to one stable identity and multiple aliases. |
| `ui-commands/PopupSelectView.module.css`, `ui-input-trigger/MenuView.module.css` | `Commands.module.css` adapts rounded elevated bounded menu/row chrome to existing tokens. |
| `ui-conversation`: InputBar, PermissionSelect, composer styles, apply wiring, keymap tests and README | Existing InputBar extraction gains typed command integration, retaining the #307 dock stack and #319 receipt lifecycle. No Lexical claims, submit machine, queue store or permission presets are imported. |
| `ui-workspace`: navigation, index wiring, apply tests and README | Inspected fork-after-success and navigation supersession; rustX `NavigationEpoch` revokes only continuations, with native Session operations in `CommandSession`. |
| `ui-chat`: MessageItem, MessageIconActions, branch-tail tests and README | Historical User-row actions are rustX-authored native-boundary controls. No event assembler or browser response alternatives. |
| `ui-model-selection`: catalog, ModelSelect/styles, catalog tests and README; `ui-permission-presets` README/styles and actual `ui-conversation/PermissionSelect` consumer | Inspected shared catalog generation, exact selection, popup composition and policy presentation. rustX reads generated native model DTOs and its two ApprovalMode values. Provider grouping, Host presets and upstream permission slash execution excluded. |

The four new source-derived destinations carry MIT headers. `registry.ts`,
`native.ts`, native integration tests and browser/provider fixtures are rustX-owned
rewrites of the product contract, not copied Harness runtime code. React and existing
CSS tokens/primitives are retained; no dependencies were added. The inventory records
the updated InputBar import closure. Authority deliberately excluded throughout:
Host/Remote lifecycle, Session Controller, conversation/events, Cordis/plugins,
generic command execution, queue ownership, retry policy, persistence and upload
ownership. No second provenance mechanism is introduced.

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
- `ui-commands/src/client/PopupSelectView.tsx`: initially inspected only in #304;
  WEB-05 now rewrites its selector interaction in `app/commands/CommandPanel.tsx`.
  Its Session-aware popup controller, Host directory and Remote actions remain
  excluded. The generic primitive Menu still derives from ui-primitives/Menu.
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
All retained package license texts (100 production-install packages, including
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

### WEB-03 inspected paths

The following implementation, test, style and package inputs were inspected for
this change (relevant sections of the large files). The adaptation subset is the
table above; all other paths below are inspection-only. Existing WEB-01/02 source
records continue to own the reused primitives, Markdown and artifact UI.

```text
packages/client/ui-conversation/README.md
packages/client/ui-conversation/package.json
packages/client/ui-conversation/src/client/contract/request-inspection.ts
packages/client/ui-conversation/tests/selection-survival.client.spec.tsx
packages/client/ui-primitives/README.md
packages/client/ui-primitives/package.json
packages/client/ui-primitives/src/DisclosureRow.tsx
packages/client/ui-primitives/tests/atoms.client.spec.tsx
packages/client/ui-tool/README.md
packages/client/ui-tool/package.json
packages/client/ui-tool/src/client/tool/components/ToolRow.module.css
packages/client/ui-tool/src/client/tool/components/ToolRow.tsx
packages/client/ui-tool/tests/tool-row.client.spec.tsx
packages/client/ui-trajectory/README.md
packages/client/ui-trajectory/package.json
packages/client/ui-trajectory/src/client/TrajectoryTable.module.css
packages/client/ui-trajectory/src/client/TrajectoryTable.tsx
packages/client/ui-trajectory/src/client/TrajectoryTimeline.tsx
packages/client/ui-trajectory/src/client/trajectory-assistant-definition.ts
packages/client/ui-trajectory/src/client/trajectory-compaction-definition.ts
packages/client/ui-trajectory/src/client/trajectory-contract.ts
packages/client/ui-trajectory/src/client/trajectory-event-projection.ts
packages/client/ui-trajectory/src/client/trajectory-record.ts
packages/client/ui-trajectory/src/client/trajectory-search-index.ts
packages/client/ui-trajectory/src/client/trajectory-snapshot-builder.ts
packages/client/ui-trajectory/src/client/trajectory-virtual-rows.ts
packages/client/ui-trajectory/tests/layout.client.spec.tsx
packages/client/ui-trajectory/tests/snapshot-builder.client.spec.ts
packages/client/ui-trajectory/tests/table.client.spec.tsx
packages/client/ui-trajectory/tests/virtual-rows.client.spec.ts
packages/session/session-projection/README.md
packages/session/session-projection/package.json
packages/session/session-projection/src/index.ts
packages/session/session-projection/src/types.ts
packages/session/session-projection/tests/registry.spec.ts
```

### PR #318 native review corrections

The Journal cut receipt, SQLite schema/index contract, loaded-record lifecycle
refresh and cache epoch regressions are rustX-authored. No additional Harness
implementation or dependencies were copied. The existing `trajectory.test.tsx`
source record continues to identify its adapted upstream presentation contracts;
the new historical-selection regression exercises rustX's native lifecycle DTO.

The second review's semantic-publication receipts, contiguous history rebase,
separately retained selection and canonical artifact merge are also rustX-authored.
They add no Harness source files or dependencies. The existing source inventory
and MIT notices remain the sole provenance mechanism.

## WEB-04 composer context docks

The external checkout was reverified at the exact pinned SHA before changes. The
`inspected` array adds the ui-goal and ui-conversation READMEs, `GoalBar` with its
activation source, slots, locales and tests, `TodoPanel`, `QueueDock`, the queue
contract, `ConversationRoot` composer-stack geometry, the Harness `InputBar`
submit-mode code and the Todo/Queue/skeleton/input-bar tests, plus the ui-primitives
README and icon set.

| Upstream, relative to `packages/client/` | Local destination under `src/` | Treatment |
| --- | --- | --- |
| `ui-conversation/src/client/skeleton/ConversationRoot.{tsx,module.css}` | `app/composer/ComposerContextStack.*` | Fixed Todo/Goal/Queue/Composer seats and the shared dock-inset width axis; no slot registry, overlay chain, hero or sticky seat. |
| `ui-conversation/src/client/skeleton/TodoPanel.{tsx,module.css}` | `app/composer/TodoDock.*` | Collapsed summary, bounded list and glyph family bound to native `TodoTask`; composed-empty strip, `blocked_by` and `active_form` added. |
| `ui-goal/src/client/GoalBar.{tsx,module.css}` | `app/composer/GoalDock.*` | Strip, phase labels, single-flight actions and inline Enter/Escape form bound to `GoalView`; budget editor, rounds/revision meta and uncertainty added; clear and the Harness activation source excluded. |
| `ui-conversation/src/client/queue/QueueDock.{tsx,module.css}` | `app/composer/QueueDock.*` | Single-row strip, count header and sending echo bound to native inbound rows and MessageId settlement; edit/remove/steer actions, thumbnails and the input-card tuck excluded. |
| `ui-primitives/src/icons/index.tsx` | `presentation/primitives/icons/index.tsx` | Existing record; seven dock glyphs (Checklist, Queue, Goal, Pause, Play, Edit, Close) copied verbatim. |

Excluded semantic owners: Harness Session Controller queue store and `updateQueue`,
`dsh-goal` projection/activation Remote reads, `dsh-tool-todo` projection, Cordis
slots and registration order, Harness locale runtime, Tooltip. The binding module,
client Goal control, provisional-echo settlement, fixture, deterministic tests and
the real-server scenario are rustX-authored.

### WEB-06 exact pending controls

Reverified the detached reference at
`c291e7961a515f6d7af9304e7fd1d257929aef26`. Reinspected
`packages/client/ui-conversation/src/client/queue/QueueDock.tsx`, its CSS,
`tests/queue-dock.client.spec.tsx`, `src/client/input/queue-store.ts` and
`src/client/contract/queue.ts`. Adapted row Edit/Remove, blank-save and IME guards,
edit cancellation, busy disclosure, and compact action styling. rustX keeps its
standalone Todo → Goal → Queue → Composer geometry and wraps actions on narrow
screens. The shared source inventory records these adaptations.

Harness Session Controller `updateQueue`, placement/steering, Host ownership,
queue identity/storage, and optimistic/local submission authority remain
excluded. rustX controls use exact native pending sequence/MessageId/revision,
retain stale drafts, and reread after responses. Nonrepresentable typed content
cannot be edited; upload references are preserved. No per-row Steer is exposed
because native Queue and Steer do not define separate durable delivery classes.

## WEB-07 Workspace navigation inspection

Verified external checkout `/home/caismis/Documents/codes/deepseek-harness-reference`
at `c291e7961a515f6d7af9304e7fd1d257929aef26` before implementation. Inspected
`ui-workspace` browser/rows/picker/navigation, their styles, picker/browser tests and
README; `ui-sidebar` shell, styles, tests and README; `ui-conversation` EmptyHero,
Workspace chip, HeroShell styles and skeleton tests; directory-picker seam, native
and browse flows/styles/tests; and `api/workspace-controller` commands, DTOs,
directory controller, tests and README.

`src/workspaces/WorkspaceNavigation.tsx` and its CSS adapt grouped/flat rows,
expansion, selected rows, bounded search, metadata actions and capability-gated
picking. The shared inventory records precise paths, hashes, additional sources,
retained imports and MIT attribution. No upstream source was copied wholesale.
The Host implementation and native navigation bindings/tests are rustX-authored.

Excluded owners: Harness Workspace Registry/Controller, stored Session membership,
Session Controller, automatic blank-Session allocation, Remote/Cordis/slots,
archive/subagent/schedule policies, content-search service, native OS directory
allocation and browser filesystem browsing. Product Host owns finite authorized
roots and metadata; native rustX owns Session cwd, history, trust, runtime and Fork.
There is no build/runtime dependency on the external checkout.

PR #324 review repair retains the same Harness presentation sources. The updated
Workspace rows display Host classification independently of registration;
`endpoint.ts` and the shared admission/focus bindings are rustX-authored. No new
Harness authority or source was imported. The inventory records the new local
endpoint helper dependency.

## WEB-08 Settings

Pinned reference HEAD verified as `c291e7961a515f6d7af9304e7fd1d257929aef26` before edits.
Settings adapts the general Settings panel, Provider editor cards, model-selection
controls and explicit save footer. The inventory records inspected source, tests,
CSS, docs and excluded dependencies. rustX uses its existing React shell/primitives;
no Harness settings mirror, source merge, credentials service or catalog is imported.
Native typed projections replace all upstream configuration authority. There is no
secret input because the Product Host has no write-only credential API.

## WEB-09 integration settings

Verified the pinned Harness checkout at
`c291e7961a515f6d7af9304e7fd1d257929aef26`. Inspected the Settings
scope/contract/schema, plugin card/form fields and tests/CSS, plugin inventory
component/tests/CSS/docs, shared Tag/StateDot primitives, and MCP client
configuration/transport/lifecycle tests/docs. No MCP-specific TSX component was
found under `packages/client` at this pin.

`Integrations.tsx` adapts disclosure cards, explicit save/discard, scoped inventory
identity and separate activation/runtime facts. Existing Settings primitives and
CSS provide the shell and responsive layout. All inspected paths are recorded in
`source-inventory.json`; adapted sources have hashes and dependency closure.
Session selection controls and native/protocol tests are rustX-authored.

Excluded: Harness namespace/schema engine, Cordis services, dynamic plugin loader,
plugin activation/runtime, configuration merge, credential store and MCP client.
The browser uses rustX Settings CAS and native inventory. There is no secret input,
source content editor, browser-direct MCP connection or plugin backend.


## WEB-10 final audit

The external clean checkout was verified at the exact pin above. All **74** source
records, including every `additional_sources` hash, match the reference. All 201
inspected paths exist. The mapped package closure is ui-primitives, ui-theme,
ui-layout, ui-sidebar, ui-chat, ui-conversation, ui-tool, ui-approval,
ui-user-questions, ui-attachment, ui-goal, ui-commands, ui-input-trigger,
ui-trajectory, ui-workspace, ui-settings-general, ui-settings-models and
ui-settings-plugin-inventory. Exact files/treatments remain in the same inventory.

Run the optional maintainer audit without downloading anything:

```sh
node scripts/provenance.ts --reference /absolute/path/deepseek-harness-reference
```

This checks clean reference HEAD and original-byte hashes in addition to the
ordinary inventory/import/header gate. Normal CI/build neither needs that checkout
nor fetches upstream. Existing notices cover 100 installed production packages;
production artifact checks compare both notice files byte-for-byte.

No new Harness source is imported. Existing derived `Trajectory.tsx` gains native
browser tab keyboard behavior/panel labelling; `Integrations.tsx` fixes absent
command draft encoding. Both retain their original attribution; the inventory
records the added Trajectory import and amended treatments. The icon treatment
now correctly says eleven glyphs (the earlier four-only text predated WEB-04).
The keyboard helper, fault probe, new acceptance tests and launcher repairs are
rustX-authored. There are no new dependencies or lockfile changes.

Review of actual source/imports, Host metadata and browser persistence found no
Harness Host/Remote/Cordis/Session runtime owner, source-fetch build step, hidden
config store or branding assets. `artifact/read` remains exclusively Tool managed
output; typed Session upload receipts/files have no ArtifactStore compatibility
path. No second provenance mechanism has been introduced.

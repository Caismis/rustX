# Harness presentation provenance — WEB-RESET-01

## WEB-12 Session convergence

PR #361 repair retains these exact upstream pins and imports no new Harness code.
Existing WorkspaceNavigation, WorkspaceBrowser, Rows/Rows.module.css and QueueDock
adaptations now carry Sidebar-only selection/close callbacks, visible scoped status,
and product conflict wording. Their reviewed local hashes/import inventories are
updated; upstream hashes, MIT headers and license/notice closure are unchanged.
The title helper and bounded native catalog cache invalidation are rustX-authored.
No LLM title generation or browser-owned naming authority is introduced.

Re-fetched upstream remains `ddefc45fbc7f8e46dd73185e68295696d1297887`.
Compared the Epic baseline `c291e7961a515f6d7af9304e7fd1d257929aef26`
and current `ui-conversation/src/client/skeleton/ConversationSession.tsx`,
`ConversationRoot.tsx`, `contract/slots.ts` and `apply.ts`. Current
`skeleton/DefaultConversationViews.tsx` is absent at the Epic baseline; its earlier
body lives in `ConversationSession.tsx`. Current `ui-layout/src/client/columns.ts`,
`ui-sidebar/src/client/HeaderLeadingControls.tsx`, and
`ui-sidebar-right/src/client/shell/{SidebarRight,RightbarRoot}.tsx` informed the
header corner, right-panel seat and responsive boundary.

This change uses interaction/layout knowledge and existing imported primitives;
no new current-upstream source is copied and no baseline is repinned. The native
product-state mapping, Inspector sections and App bindings are rustX code. Existing
derived GoalDock, QueueDock and WorkspaceNavigation records have reviewed local
hash/treatment updates for concise product copy; original hashes and MIT/DeepSeek
license remain unchanged. No new dependency or license closure is introduced.

**Harness owns the presentation baseline; rustX owns all product/runtime semantics.**

Upstream: https://github.com/deepseek-ai/deepseek-harness

The reset pins **`ddefc45fbc7f8e46dd73185e68295696d1297887`**, the #345 release baseline (dsh 0.1.6-alpha.2), inspected again at that exact
commit for #346. No newer branch was substituted.
The browser mount/seed, composed layout, Sidebar and workspace registrations,
Settings composition, locale provider and primitive dependency closure were read
before selecting this commit. The reference checkout is an audit input only.
No build or runtime downloads upstream code or follows a floating branch.

This replaces the previous extract-and-rewrite shell. See
[SHELL-ARCHITECTURE.md](SHELL-ARCHITECTURE.md) for the A/B/C classification and
[RESET-345-VALIDATION.md](RESET-345-VALIDATION.md) for shell acceptance;
[AGENT-ARCHITECTURE.md](AGENT-ARCHITECTURE.md) and
[RESET-346-VALIDATION.md](RESET-346-VALIDATION.md) cover the completed Agent integration.

## Actual source closure

The authoritative per-file record is [source-inventory.json](source-inventory.json).
Each derived destination has its own exact upstream commit/path, original SHA-256,
local SHA-256, treatment, direct imports, exclusions and copyright. Additional
source inputs inherit the destination's commit unless explicitly pinned otherwise.
A means substantially retained presentation with bounded adaptations; B means
Harness UI with rustX bindings. C extensions are listed separately below.

Paths in the following table are relative to `packages/client/` upstream and `src/`
locally. Full paths and hashes are in the JSON; no compatibility export tree exists.

| Upstream at selected reset commit | Local destination | Class |
| --- | --- | --- |
| `ui-primitives/src/Button.tsx` | `presentation/primitives/Button.tsx` | A |
| `ui-primitives/src/Button.module.css` | `presentation/primitives/Button.module.css` | A |
| `ui-primitives/src/Input.tsx` | `presentation/primitives/Input.tsx` | A |
| `ui-primitives/src/Input.module.css` | `presentation/primitives/Input.module.css` | A |
| `ui-primitives/src/Pill.tsx` | `presentation/primitives/Pill.tsx` | A |
| `ui-primitives/src/Pill.module.css` | `presentation/primitives/Pill.module.css` | A |
| `ui-primitives/src/StateDot.tsx` | `presentation/primitives/StateDot.tsx` | A |
| `ui-primitives/src/StateDot.module.css` | `presentation/primitives/StateDot.module.css` | A |
| `ui-primitives/src/icons/props.ts` | `presentation/primitives/icons/props.ts` | A |
| `ui-primitives/src/icons/index.tsx` | `presentation/primitives/icons/index.tsx` | A |
| `ui-theme/src/styles/base.css` | `presentation/theme/base.css` | A |
| `ui-layout/src/client/AppFrame.tsx` | `presentation/layout/AppFrame.tsx` | A |
| `ui-layout/src/client/AppFrame.module.css` | `presentation/layout/AppFrame.module.css` | A |
| `ui-primitives/src/markdown/MarkdownText.module.css` | `presentation/markdown/MarkdownText.module.css` | A |
| `ui-primitives/src/markdown/CodeBlock.module.css` | `presentation/markdown/CodeBlock.module.css` | A |
| `ui-theme/src/styles/shiki.css` | `presentation/theme/shiki.css` | A |
| `ui-primitives/src/Menu.module.css` | `presentation/primitives/Menu.module.css` | A |
| `ui-primitives/src/Modal.module.css` | `presentation/primitives/Modal.module.css` | A |
| `ui-primitives/src/Menu.tsx` | `presentation/primitives/Menu.tsx` | A |
| `ui-primitives/src/HoverCard.tsx` | `presentation/primitives/HoverCard.tsx` | A |
| `ui-primitives/src/HoverCard.module.css` | `presentation/primitives/HoverCard.module.css` | A |
| `ui-primitives/src/Tooltip.tsx` | `presentation/primitives/Tooltip.tsx` | A |
| `ui-primitives/src/Tooltip.module.css` | `presentation/primitives/Tooltip.module.css` | A |
| `ui-primitives/src/Modal.tsx` | `presentation/primitives/Modal.tsx` | A |
| `ui-primitives/src/pointer-grace.ts` | `presentation/primitives/pointer-grace.ts` | A |
| `ui-primitives/src/clipboard.ts` | `presentation/primitives/clipboard.ts` | A |
| `ui-primitives/src/relative-time.ts` | `presentation/primitives/relative-time.ts` | A |
| `ui-theme/src/styles/scrollbar.css` | `presentation/theme/scrollbar.css` | A |
| `ui-theme/src/styles/design-platform.css` | `presentation/theme/design-platform.css` | A |
| `ui-theme/src/styles/corner-shape.css` | `presentation/theme/corner-shape.css` | A |
| `ui-layout/src/client/columns.ts` | `presentation/layout/columns.ts` | A |
| `ui-sidebar/src/client/SidebarRoot.tsx` | `presentation/sidebar/SidebarRoot.tsx` | B |
| `ui-sidebar/src/client/SidebarRoot.module.css` | `presentation/sidebar/SidebarRoot.module.css` | B |
| `ui-workspace/src/client/rows/Rows.tsx` | `presentation/workspace/Rows.tsx` | B |
| `ui-workspace/src/client/rows/Rows.module.css` | `presentation/workspace/Rows.module.css` | B |
| `ui-workspace/src/client/rows/WorkspaceBrowser.module.css` | `presentation/workspace/WorkspaceBrowser.module.css` | B |
| `ui-settings-general/src/client/SettingsRoot.tsx` | `presentation/settings/SettingsRoot.tsx` | B |
| `ui-settings-general/src/client/SettingsRoot.module.css` | `presentation/settings/SettingsRoot.module.css` | B |
| `ui-sidebar-right/src/client/shell/SidebarRight.module.css` | `presentation/right-panel/SidebarRight.module.css` | B |
| `ui-workspace/src/client/locales.ts` | `presentation/locale/workspace.ts` | A |
| `ui-workspace/src/client/rows/WorkspaceBrowser.tsx` | `presentation/workspace/WorkspaceBrowser.tsx` | B |
| `ui-sidebar-right/src/client/shell/SidebarRight.tsx` | `presentation/right-panel/RightPanel.tsx` | B |

## Bounded changes to the imported shell

- Theme, typography, elevations, scrollbars, corner shapes and light/dark palettes
  come from the current upstream sheets. `reset.css` supplies rustX's browser root
  sizing and accessible focus outline. The old token file and accent overrides
  were deleted. `data-ds-dark-theme` is a styling marker, not product branding.
- AppFrame retains the measured three-column grid and pointer-captured drag
  handle. The pure upstream column solver retains 280px default / 264–420px
  Sidebar, 56px rail, 1024px auto-collapse, 400px protected center and 300px minimum
  right panel. Explicit React render seats replace Cordis slots/layout stores.
  Local widths and expansion are disposable, not persisted preferences.
- SidebarRoot retains brand/new/global/region/footer DOM, 150ms fade/unmount,
  frozen expanded width during collapse, rail glyphs and scrollbar linger.
  rustX `rX`/wordmark replaces upstream product artwork. Global Connection and
  Settings callbacks are supplied by the product composition root.
- WorkspaceBrowser and Rows retain tree/header/search/hover/menu geometry.
  Native metadata search uses upstream contextual search rows; it does not claim
  conversation-content search. Project hover cards omit unavailable creation time.
  Schedule, provisional Session, archive and browser reorder paths are removed.
  Session Delete opens native revision-checked preview, never a local archive.
  Additional labelled buttons expose row gestures to keyboard users.
- Settings retains the mask/panel/navigation/content frame. Sections and bodies
  are adapter seats for existing CFG3 editors. Narrow screens use a scrollable
  horizontal section strip because CFG3 has more sections. No Settings Controller,
  mirror, source cache or publication authority was imported.
- RightPanel keeps upstream normal/fullscreen panel geometry and transition;
  rustX Inspector supplies its header and scrolling body. No docking runtime,
  terminal/file authority or floating-window manager was included.
- Menu retains portaled positioning, submenus, pointer grace and keyboard behavior;
  autofocus waits for measured placement. Modal retains upstream DOM/chrome with
  focus restoration, nested-layer Escape/background isolation and bounded report
  scrolling. Clipboard uses the async browser API only.
- Localization retains the required English workspace dictionary and parameter
  interpolation. Labels for native extensions are rustX copy. No language service,
  Host preference authority or generic plugin/localization registry is necessary.

The current Markdown/code CSS accompanies the previously audited incremental
renderer. Parser/frontier/highlight algorithms and safe rustX media/link boundaries
remain in place: raw HTML is text, links allow HTTP(S)/mailto, remote images are
inert alt text, and settled output is rendered canonically. No Harness file action,
image resolver, Session event assembler or content search service was imported.

## Agent closure (#346)

All new Agent sources use the same exact `ddefc45fbc7f8e46dd73185e68295696d1297887`
pin. The pre-implementation A/B/C decision is in AGENT-ARCHITECTURE.md. The following
table is generated from the reviewed per-file inventory; hashes and direct imports
remain recorded there.

| Upstream source | Local destination | Class |
| --- | --- | --- |
| `packages/client/ui-conversation/src/client/skeleton/InputBar.module.css` | `src/presentation/agent/Composer.module.css` | A |
| `packages/client/ui-conversation/src/client/skeleton/ConversationRoot.module.css` | `src/presentation/agent/Conversation.module.css` | A |
| `packages/client/ui-chat/src/client/chat/MessageItem.module.css` | `src/presentation/agent/Message.module.css` | A |
| `packages/client/ui-chat/src/client/chat/ChatView.module.css` | `src/presentation/agent/Chat.module.css` | A |
| `packages/client/ui-chat/src/client/chat/ReasoningRow.module.css` | `src/presentation/agent/Reasoning.module.css` | A |
| `packages/client/ui-tool/src/client/tool/components/ToolRow.module.css` | `src/presentation/agent/Tool.module.css` | A |
| `packages/client/ui-tool/src/client/tool/ToolCallTree.module.css` | `src/presentation/agent/ToolTree.module.css` | A |
| `packages/client/ui-approval/src/client/ApprovalPanel.module.css` | `src/presentation/agent/Approval.module.css` | A |
| `packages/client/ui-user-questions/src/client/QuestionComposer.module.css` | `src/presentation/agent/Question.module.css` | A |
| `packages/client/ui-model-selection/src/client/ModelSelect.module.css` | `src/presentation/agent/ModelSelect.module.css` | A |
| `packages/client/ui-permission-presets/src/client/PermissionSelect.module.css` | `src/presentation/agent/PermissionSelect.module.css` | A |
| `packages/client/ui-chat/src/client/chat/MessageItem.tsx` | `src/presentation/agent/Message.tsx` | B |
| `packages/client/ui-chat/src/client/chat/ReasoningRow.tsx` | `src/presentation/agent/Reasoning.tsx` | B |
| `packages/client/ui-tool/src/client/tool/components/ToolRow.tsx` | `src/presentation/agent/ToolCard.tsx` | B |
| `packages/client/ui-approval/src/client/ApprovalPanel.tsx` | `src/presentation/agent/ApprovalTakeover.tsx` | B |
| `packages/client/ui-model-selection/src/client/ModelSelect.tsx` | `src/presentation/agent/ModelSelect.tsx` | B |
| `packages/client/ui-permission-presets/src/client/PermissionSelect.tsx` | `src/presentation/agent/PermissionSelect.tsx` | B |
| `packages/client/ui-conversation/src/client/skeleton/InputBar.tsx` | `src/app/agent/AgentComposer.tsx` | B |
| `packages/client/ui-user-questions/src/client/QuestionComposer.tsx` | `src/app/agent/Questionnaire.tsx` | B |
| `packages/client/ui-theme/src/styles/gradient-shadow-text.css` | `src/presentation/theme/gradient-shadow-text.css` | A |

ConversationRoot/ChatView composition and ToolCallTree's single keyed dispatch were
inspected directly. The adapter consumes native ordered transcript entries, not
Harness Chat nodes or Session events. Native Tool types select the shared row body;
structured upstream file/search/terminal data that rustX does not expose is not
fabricated. Read/search show native output; Edit/Write label requested changes;
images use existing native ArtifactResources. There is no inferred subcall tree.

The original editor's Cordis slots become explicit React seats and a textarea;
model/profile and permission menus use the shared Harness Menu focus/placement
primitive. Questionnaire binds the native schema instead of upstream questions.
The pinned gradient-shadow-text sheet completes the Agent's elevation/Markdown
token dependency, including the light Composer outline. No theme was redesigned.

Deleted: the former Conversation, InputBar, ChatMessage, MessageItem, ToolRow,
ApprovalPanel and QuestionComposer path and their replaced CSS. No compatibility
exports or old/new Agent mode remain.

## Retained audited utility provenance

The following utilities/extensions retain their honestly recorded earlier exact
`c291e7961a515f6d7af9304e7fd1d257929aef26` origin. Markdown parsing, stable scroll
anchoring, attachments, command discovery, native docks and CFG3 interiors are
bounded reused capabilities, not a second Agent presentation architecture.

| Retained local destination | Original upstream source |
| --- | --- |
| `src/app/commands/matching.ts` | `packages/client/ui-primitives/src/rank-by-name.ts` |
| `src/app/commands/CommandMenu.tsx` | `packages/client/ui-input-trigger/src/client/MenuView.tsx` |
| `src/app/commands/CommandPanel.tsx` | `packages/client/ui-commands/src/client/PopupSelectView.tsx` |
| `src/app/commands/Commands.module.css` | `packages/client/ui-commands/src/client/PopupSelectView.module.css` |
| `src/presentation/primitives/DisclosureRow.tsx` | `packages/client/ui-primitives/src/DisclosureRow.tsx` |
| `src/presentation/primitives/DisclosureRow.module.css` | `packages/client/ui-primitives/src/DisclosureRow.module.css` |
| `public/LICENSE-DeepSeek-Harness.txt` | `LICENSE` |
| `src/presentation/markdown/MarkdownText.tsx` | `packages/client/ui-primitives/src/markdown/MarkdownText.tsx` |
| `src/presentation/markdown/parse.ts` | `packages/client/ui-primitives/src/markdown/parse.ts` |
| `src/presentation/markdown/incremental.ts` | `packages/client/ui-primitives/src/markdown/incremental.ts` |
| `src/presentation/markdown/render.tsx` | `packages/client/ui-primitives/src/markdown/render.tsx` |
| `src/presentation/markdown/mathCompatibility.ts` | `packages/client/ui-primitives/src/markdown/mathCompatibility.ts` |
| `src/presentation/markdown/cjkFriendlyStrong.ts` | `packages/client/ui-primitives/src/markdown/cjkFriendlyStrong.ts` |
| `src/presentation/markdown/katex.tsx` | `packages/client/ui-primitives/src/markdown/katex.tsx` |
| `src/presentation/markdown/CodeBlock.tsx` | `packages/client/ui-primitives/src/markdown/CodeBlock.tsx` |
| `src/presentation/markdown/highlight.ts` | `packages/client/ui-primitives/src/markdown/highlight.ts` |
| `src/presentation/markdown/useViewportHighlighting.ts` | `packages/client/ui-primitives/src/markdown/useViewportHighlighting.ts` |
| `test/markdown-incremental.test.tsx` | `packages/client/ui-primitives/tests/markdown-incremental.client.spec.tsx` |
| `test/streaming-code-block.test.tsx` | `packages/client/ui-primitives/tests/streaming-code-block.client.spec.tsx` |
| `src/presentation/layout/ChatViewport.tsx` | `packages/client/ui-chat/src/client/chat/ChatView.tsx` |
| `src/presentation/attachments/AttachmentCard.tsx` | `packages/client/ui-attachment/src/MessageImage.tsx` |
| `src/presentation/attachments/AttachmentCard.module.css` | `packages/client/ui-attachment/src/MessageImage.module.css` |
| `src/app/trajectory/Trajectory.tsx` | `packages/client/ui-trajectory/src/client/TrajectoryTable.tsx` |
| `src/app/trajectory/Trajectory.module.css` | `packages/client/ui-trajectory/src/client/TrajectoryTable.module.css` |
| `test/trajectory.test.tsx` | `packages/client/ui-trajectory/tests/table.client.spec.tsx` |
| `src/app/composer/ComposerContextStack.tsx` | `packages/client/ui-conversation/src/client/skeleton/ConversationRoot.tsx` |
| `src/app/composer/ComposerContextStack.module.css` | `packages/client/ui-conversation/src/client/skeleton/ConversationRoot.module.css` |
| `src/app/composer/TodoDock.tsx` | `packages/client/ui-conversation/src/client/skeleton/TodoPanel.tsx` |
| `src/app/composer/TodoDock.module.css` | `packages/client/ui-conversation/src/client/skeleton/TodoPanel.module.css` |
| `src/app/composer/GoalDock.tsx` | `packages/client/ui-goal/src/client/GoalBar.tsx` |
| `src/app/composer/GoalDock.module.css` | `packages/client/ui-goal/src/client/GoalBar.module.css` |
| `src/app/composer/QueueDock.tsx` | `packages/client/ui-conversation/src/client/queue/QueueDock.tsx` |
| `src/app/composer/QueueDock.module.css` | `packages/client/ui-conversation/src/client/queue/QueueDock.module.css` |
| `src/workspaces/WorkspaceNavigation.tsx` | `packages/client/ui-workspace/src/client/rows/WorkspaceBrowser.tsx` |
| `src/app/settings/Settings.tsx` | `packages/client/ui-settings-general/src/client/SettingsRoot.tsx` |
| `src/app/settings/CatalogEditor.tsx` | `packages/client/ui-settings-models/src/client/ProviderEditor.tsx` |
| `src/app/settings/Integrations.tsx` | `packages/client/ui-settings-plugin-inventory/src/client/PluginInventorySettingsTab.tsx` |

## Inspected-only current dependencies

These paths informed the composition and exclusion boundary, but are not imported:

- `apps/web/src/main.ts`
- `packages/client/web/src/mount.ts`
- `packages/client/web/src/seed.ts`
- `packages/client/ui-layout/src/client/stores.ts`
- `packages/client/ui-layout/src/client/index.ts`
- `packages/client/ui-sidebar/src/client/index.ts`
- `packages/client/ui-workspace/src/client/tree.ts`
- `packages/client/ui-workspace/src/client/stores.ts`
- `packages/client/ui-workspace/src/client/WorkspacePicker.tsx`
- `packages/client/ui-settings-general/src/client/index.ts`
- `packages/client/ui-settings/src/client/settings-mirror.ts`
- `packages/client/locale/src/client/index.ts`
- `packages/client/ui-theme/src/client/index.ts`
- `packages/client/ui-theme/src/client/styles.ts`
- `packages/client/ui-primitives/package.json`

`inspected_only` records their current pinned hashes. `historical_inspection`
separately preserves the old inspection list and its exact commit; membership is
not a claim that a file was copied. Imported source records are the source of truth.

## rustX extensions and semantic owners

`app/App.tsx` registers native occupants/gestures. `workspaces/WorkspaceNavigation`
projects native Session summaries and current snapshots to presentation props;
`WorkspaceSessionNavigation` and `NavigationEpoch` own admission/supersession.
`client/AppServerClient` owns the typed transport, authoritative reconnect and
resynchronization. Product Host owns navigation registration/classification and
authorization; canonical Session cwd and history remain native server state.
A classification response is valid only for its exact observed summary array.
No browser-owned durable Session-to-Workspace map exists.

Connection, Inspector, Appearance, the existing CFG3 editors, conversation feedback
and browser root/focus support are rustX-specific integration code. These use the
imported tokens/primitives/seats. Presentation `types.ts` objects are disposable
view models, never alternate canonical DTOs or lifecycle stores.

Excluded throughout: Harness Host, Remote, Session Controller, Workspace Controller,
Agent Loop, Cordis/module loader/plugin runtime, durable event/history stores,
provisional Session identity, archive membership, workspace ownership, desktop
caption/update systems, file/terminal/dock runtime, configuration mirrors and
interaction settlement. Native additions are canonical transcript Tool read projections backed by an
occurrence/result association index (SQLite schema 40), and resolved prospective
approval policy. Assistant MessageId plus block index preserves exact Tool
ownership across reused provider call IDs. These are rustX-owned adaptations. No upstream runtime semantics are imported.

## License, dependencies and branding

`public/LICENSE-DeepSeek-Harness.txt` retains the complete upstream MIT license,
**Copyright (c) 2026 DeepSeek**, byte for byte. Derived TS/TSX/CSS carry source
headers. The license is shipped in `dist` and checked against its original hash.

No dependency or lockfile changes were needed. The bounded browser closure uses
existing React/ReactDOM, clsx, Markdown/HAST/MDAST utilities, micromark, KaTeX and
Shiki packages. `public/THIRD-PARTY-NOTICES.txt` inventories the complete 100-package
production dependency closure (versions, license texts and package attribution);
`node scripts/notices.ts` reproduces/checks that installed closure. Build tooling
and test-only dependencies are not falsely described as bundled runtime code.

FishLogo, BrandWordmark, official DeepSeek product/logo assets, endorsements and
upstream update/download links are excluded. Text branding is rustX; upstream
copyright and internal CSS variable prefixes remain for attribution and fidelity.

## Reproducible checks

Normal CI performs no upstream fetch:

```sh
pnpm check:provenance
pnpm build
```

The checker validates pinned per-file baselines, local hashes, all derived-source
headers/destinations, exact retained imports, the presentation-only import boundary,
MIT bytes, installed production notices, and shipped notice files. Local source
hashes must be deliberately updated in the inventory with reviewed edits; they
are not silently refreshed by build or CI.

An optional maintainer source audit uses a clean external checkout with HEAD at the
selected reset commit and both exact recorded commits available in its object DB:

```sh
node scripts/provenance.ts --reference /path/to/deepseek-harness
```

It compares original bytes using `git show <recorded-commit>:<recorded-path>`.
Neither this command nor normal validation fetches, syncs or executes Harness.
There is no automatic upstream-sync framework.

## WEB-RESET-03 Settings and native auxiliary surfaces

The same immutable `ddefc45fbc7f8e46dd73185e68295696d1297887` baseline supplies:

- `presentation/settings/SettingsContent.module.css`: adapted ModelsSection
  outlined Provider rows/filled detail editor, Plugin field styles, and inventory
  badges/diagnostics. It replaces the deleted app Settings sheet.
- `presentation/primitives/Switch.tsx` and `.module.css`: controlled accessible
  toggle and token-based appearance, with the retained MIT notice.
- `presentation/right-panel/ArtifactPreview.module.css`: bounded adaptation of
  TextPreview header, scroll body and wrap treatment. Host filesystem operations,
  registry, resource store, slots, automatic reload and binary renderer framework
  are excluded. The TS preview component is rustX-authored.

The existing SettingsRoot gains optional section grouping, and the existing
RightPanel gains occupant-appropriate close labels. The current ToolCard preserves
exact native Tool names separately from display labels when reconciling #351.
Native Settings adapters retain their historical attribution and record their
new modifications; they are not relabelled as freshly copied upstream code.
`SettingsContent.tsx`, ResourceInventory, NativeFacts, draft ownership and native
artifact adapters are rustX-authored composition/props, not copied Harness sources.

Every changed derived destination has an explicit treatment update and local hash.
Upstream hashes remain immutable. Normal checks/builds read checked-in source only;
`node scripts/provenance.ts --reference /tmp/rustx-345-harness` additionally audits
against the clean pinned external checkout. No upstream synchronization was added.

The PR #357 review repair keeps the same pins. `Settings.tsx` records symmetric
read-result/error fencing, and `Integrations.tsx` records the new pure rustX MCP
transport display adapter. Their reviewed local hashes/imports were updated
explicitly. Shared Summary controls in `AgentEditor.tsx` and `bindings/mcp.ts`
are rustX-authored; no additional Harness source or business authority was imported.

## WEB-11 composer convergence (#355)

Both `c291e7961a515f6d7af9304e7fd1d257929aef26` (Epic baseline) and
`ddefc45fbc7f8e46dd73185e68295696d1297887` (current issue reference) were
inspected for `InputBar.tsx`, `InputBar.module.css`,
`ConversationRoot.module.css`, `contract/slots.ts`, `input/submission-policy.ts`
and `submission-settings.ts` under `packages/client/ui-conversation/src/`
(the first three are under `client/skeleton/`; the next two under `client/`).

The repository already used `ddefc45…` for AgentComposer and its stylesheet.
Their existing source records now describe removal of the local tall-input and
delivery-selector overrides, the one primary seat, and native textarea scroll
adaptation. **No WebUI repin or unrelated source resync occurs.**

`src/app/composer/submission-policy.ts` materially adapts the gesture mapping from
current `client/input/submission-policy.ts` and root-seat decision from
`client/skeleton/InputBar.tsx`; its new record includes both exact upstream
SHA-256 hashes, the local hash, treatment, dependencies and exclusions. The
settings file was inspected for the Queue default, not imported. The existing
`PermissionSelect.tsx` record also tracks its bounded effective-policy tooltip
prop; its upstream pin and hash are unchanged. The settings
store, Lexical, slot/plugin system, continuable-child transport and queued-item
steering remain excluded. rustX's typed client/native owners replace those
runtime boundaries. The textarea measurement hook is rustX code.

All twelve reviewed source versions are recorded in the inspection inventory
unless already present. MIT/DeepSeek copyright is retained in the adapted policy;
the existing complete Harness license and production third-party notices remain
unchanged. `check:provenance` checks local hashes/import closure; the optional
`--reference` audit verifies original hashes against the exact external checkout.

## Native Trace inspection and Trajectory (#364)

The reference checkout `/home/caismis/Documents/codes/deepseek-harness-364` is
pinned at `ddefc45fbc7f8e46dd73185e68295696d1297887`. The ui-trajectory table,
layout, search index, virtual rows, timeline, code inspector and their tests,
ui-conversation composition, ui-tool presentation, ui-primitives JSON/Markdown/code
and attachment rendering informed the implementation. The per-file inventory
records the precise original paths, hashes, additional sources and exclusions.

`src/app/trajectory/` replaces the old single-file view. It adapts dense native
Attempt/Step sections, folding, search, stable selection, anchored virtualization,
tail following, timeline zoom/pan/focus and entity-specific inspection. JsonTree
and its stylesheet are imported with bounded primitive bindings; existing audited
Markdown/code and artifact components are reused. No dependency was added.

Deliberate semantic deviations: rustX resolves all native ordering, hierarchy,
retries, acceptance and outcomes on the server; it never imports Harness Session
or Cordis event assembly. Details are bounded on-demand reads, with explicit
truncation. Unknown timing is a marker, never an invented span. Code source is
recognized only by exact native Bash/Write contracts; no Tool-name heuristic or
extension-derived language is used. Generation evidence is four bounded scalar offsets, including the native
start/dispatch clock bridge, not a per-token event stream. Duration phases use
request-relative native positions; missing bridge evidence leaves the span
unsplit. Equal-width sequence mode has no duration splits. Inspector distinguishes
Journal wall duration from measured request and dispatch intervals.

TTFT deliberately differs from the pinned Harness `TrajectoryTable.tsx`'s
`firstTokenTime - stepStartTime`: Harness measures step start → first token;
rustX measures adapter dispatch → first provider-independent output. Native
durable request preparation precedes dispatch and is rendered separately when
measured. See [the native timing contract](../docs/trace.md#generation-clock-contract).
Inspection never acquires execution authority.

The rebase preserves #366 Goal chat presentation while retaining exact native
Goal Tool identity, arguments and result in Trajectory. Its regression uses the
new summary/detail vocabulary. Lifecycle repairs invalidate cached and pending
details so a selected running Tool cannot retain a pre-settlement payload.

## WEB-15 Harness-first Trajectory (#371)

The pinned reference remains `ddefc45fbc7f8e46dd73185e68295696d1297887`.
The inventory records the inspected Trajectory View/Table/Cell/Turn/TurnHeader,
Toolbar/Timeline, layout/record/preview/virtual-row modules, request-header and
Tool definitions, code inspector and five requested test files. ToolRow,
GenericToolCard and conversation request/record contracts were also inspected.

`TrajectoryCell.tsx` adapts the current Table's role icons, compact role tags,
Tool name / argument / result composition and content-led previews. Existing
MarkdownText, CodeBlock, JsonTree, Artifact, Tooltip, tabs and clipboard primitives
remain the renderers. The legacy standalone Harness Cell is inspected, not copied.
The generic runtime kind/state/duration table is replaced by an event/content
rail: native Attempt/Step evidence is structural, Request evidence subdued,
Assistant previews use Markdown, and successful lifecycle state is unobtrusive.
Native identity moves behind an inspector disclosure. Tool Summary includes native
source/input and result; Prompt, Context, Thinking, Code and Artifacts tabs exist
only when supported by the native detail. No dependency was added.

Local adaptations preserve the native finite cache, order, location, selected
identity, historical request snapshot and dispatch-relative timing. A fixed-height
history boundary fixes the browser-tested final-page prepend shift. Timeline
selection reveals folded groups and scrolls to the exact native identity; keyboard
selection, zoom/reset and pan supplement pointer gestures. Desktop and 390px
screenshots are pinned with the repository's immutable Playwright container.

Issue #372 remains a native dependency: there is no browser prompt comparison,
standalone System/Context fabrication, Tool-domain ownership guess, or Subtool
model. Background/Subagent/Workflow/Interaction remain independent records.
Unscoped records never acquire Attempt ownership, including during folding.
Harness event definitions, interrupted lifecycle synthesis, Session/Cordis
assembly, step-relative TTFT and Tool-name-based code detection stay excluded.

Changed derived files have reviewed local SHA-256/import records; original hashes
and MIT/DeepSeek attribution remain intact. `check:provenance` and the external
`--reference /tmp/rustx-371-harness` audit verify local and upstream bytes.

## WEB-15 review follow-up (#371)

The pinned reference remains `ddefc45fbc7f8e46dd73185e68295696d1297887`. No new
upstream file was introduced: the earlier-history marker is adapted from the
already-inventoried `ui-trajectory/src/client/TrajectoryTimeline.tsx`, whose
`EarlierHistoryBoundary` and `.earlierHistory` treatment were re-read for this
change and mapped onto existing rustX theme tokens.

Five review findings were closed. `retry_number` is no longer presented as
`Request #N` anywhere; requests are named by model plus exact native request
identity. The overview gained the
upstream earlier-history affordance while paging authority stays single: the
Trace cache owns the cursor, the pending load and `TRACE_LIMIT`, and the marker
calls the same load the ledger and toolbar call. Ordinary rows now carry compact
artifact identity read from the native summary alone, with no `ArtifactResources`
read from a virtualized row. An explicit overview selection clears a search that
would otherwise hide the selected record, so overview and ledger cannot disagree.
The rustX-specific `background`, `subagent`, `workflow` and `interaction` kinds
keep a persistent short label, because they share a fallback glyph and a
hover-only Tooltip is unavailable to touch and keyboard readers.

Local SHA-256 and import records were refreshed for the nine changed derived
files. Upstream hashes, the pinned commit, licensing, classifications and
exclusions are unchanged. Harness Session/Cordis assembly, client-side turn and
step inference, and step-relative TTFT remain excluded.

A second review pass closed two further findings.

The native ordinal is no longer named in primary presentation at all. Native
defines `retry_number` as the actual-request ordinal within a logical Step, so a
nonzero value proves only that the request was not the first one; it does not say
whether the repeat was a retry or a recovery. The inspector title is therefore
`Request · <model>` and the overview span label is `Request · <model> ·
<request_id>`, which disambiguates by native identity rather than by an
interpretation the browser is not entitled to make. The ordinal keeps its honest
home in the inspector's native disclosure, under `Retry / recovery ordinal`.
Deciding retry against recovery would need an explicit native request cause,
which the pinned contract does not carry.

The timing overview also became the containing block for its own earlier-history
marker. Upstream renders that marker inside the positioned canvas, which does not
exist when no loaded record carries a usable start and the duration projection has
nothing to place. The marker is absolutely positioned, so in that branch it
resolved against whatever outer ancestor happened to be positioned. `.root` is now
`position: relative`, which leaves the normal geometry untouched — `.canvas` is
still the nearer positioned ancestor for every span, lane and boundary — and gives
the empty state the same single paging props, the same cursor and the same load
call as the normal one.

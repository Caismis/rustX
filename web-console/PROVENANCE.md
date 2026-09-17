# Harness presentation provenance — WEB-RESET-01

**Harness owns the presentation baseline; rustX owns all product/runtime semantics.**

Upstream: https://github.com/deepseek-ai/deepseek-harness

The reset pins **`ddefc45fbc7f8e46dd73185e68295696d1297887`**, the current `master`
release HEAD (dsh 0.1.6-alpha.2) fetched and inspected at implementation start.
The browser mount/seed, composed layout, Sidebar and workspace registrations,
Settings composition, locale provider and primitive dependency closure were read
before selecting this commit. The reference checkout is an audit input only.
No build or runtime downloads upstream code or follows a floating branch.

This replaces the previous extract-and-rewrite shell. See
[SHELL-ARCHITECTURE.md](SHELL-ARCHITECTURE.md) for the A/B/C classification and
[RESET-345-VALIDATION.md](RESET-345-VALIDATION.md) for acceptance evidence.

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

## Retained interior provenance

The following files remain derived from the earlier exact commit
`c291e7961a515f6d7af9304e7fd1d257929aef26`; they are **not falsely relabelled as
new upstream source**. They serve Agent/conversation/composer/Tool/interaction,
attachment, command or CFG3 business interiors which #345 expressly defers.
Their shared primitive imports now reach the one new foundation. The old shell,
Sidebar, primitive rewrites and Settings chrome do not remain alongside it.

| Retained local destination | Original upstream source |
| --- | --- |
| `src/app/commands/matching.ts` | `packages/client/ui-primitives/src/rank-by-name.ts` |
| `src/app/commands/CommandMenu.tsx` | `packages/client/ui-input-trigger/src/client/MenuView.tsx` |
| `src/app/commands/CommandPanel.tsx` | `packages/client/ui-commands/src/client/PopupSelectView.tsx` |
| `src/app/commands/Commands.module.css` | `packages/client/ui-commands/src/client/PopupSelectView.module.css` |
| `src/presentation/primitives/DisclosureRow.tsx` | `packages/client/ui-primitives/src/DisclosureRow.tsx` |
| `src/presentation/primitives/DisclosureRow.module.css` | `packages/client/ui-primitives/src/DisclosureRow.module.css` |
| `src/app/components/MessageItem.tsx` | `packages/client/ui-chat/src/client/chat/MessageItem.tsx` |
| `src/app/components/MessageItem.module.css` | `packages/client/ui-chat/src/client/chat/MessageItem.module.css` |
| `src/app/components/InputBar.tsx` | `packages/client/ui-conversation/src/client/skeleton/InputBar.tsx` |
| `src/app/components/InputBar.module.css` | `packages/client/ui-conversation/src/client/skeleton/InputBar.module.css` |
| `src/app/components/ToolRow.tsx` | `packages/client/ui-tool/src/client/tool/components/ToolRow.tsx` |
| `src/app/components/ToolRow.module.css` | `packages/client/ui-tool/src/client/tool/components/ToolRow.module.css` |
| `src/app/components/ApprovalPanel.tsx` | `packages/client/ui-approval/src/client/ApprovalPanel.tsx` |
| `src/app/components/ApprovalPanel.module.css` | `packages/client/ui-approval/src/client/ApprovalPanel.module.css` |
| `src/app/components/QuestionComposer.tsx` | `packages/client/ui-user-questions/src/client/QuestionComposer.tsx` |
| `src/app/components/QuestionComposer.module.css` | `packages/client/ui-user-questions/src/client/QuestionComposer.module.css` |
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
| `src/app/Trajectory.tsx` | `packages/client/ui-trajectory/src/client/TrajectoryTable.tsx` |
| `src/app/Trajectory.module.css` | `packages/client/ui-trajectory/src/client/TrajectoryTable.module.css` |
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
| `src/app/settings/Settings.module.css` | `packages/client/ui-settings-models/src/client/ModelsSection.module.css` |
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
interaction settlement. No backend semantics were added for upstream widgets.

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

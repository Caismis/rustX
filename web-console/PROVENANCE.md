# Harness presentation provenance — WEB-RESET-01

## #406 current Harness conversation contract

Reference: `/home/caismis/Documents/codes/deepseek-harness`, read-only; fetched
HEAD/master/origin/master all `477b4f420553e8a52c2fbccc464d7561b239c443`.
This is a bounded current-source adaptation, not a blanket repin of the earlier
inventory. Existing primary records retain their original commits/hashes;
changed descendants name current `additional_sources`, updated local hashes,
imports and treatments. No Harness runtime, Cordis, assets or packages were added.

Material source/CSS adaptations at this commit:

| Harness file | rustX descendant / material |
| --- | --- |
| `packages/client/ui-conversation/src/client/skeleton/InputBar.tsx` | AgentComposer resident editor/launcher interaction adapted to native textarea and receipts |
| `packages/client/ui-conversation/src/client/skeleton/ConversationContent.tsx` | ConversationComposer unconditional seat and explicit binding |
| `packages/client/ui-conversation/src/client/skeleton/ConversationRoot.module.css` | Conversation.module.css resident hero geometry; HeroShell workspace column alignment |
| `packages/client/ui-chat/src/client/chat/TurnProcessNodeView.tsx` | TurnProcess local clock and terminal vocabulary |
| `packages/client/ui-chat/src/client/chat/TurnProcessNodeView.module.css` | TurnProcess.module.css current row typography, chevron, hover/disabled and reduced-motion rules |
| `packages/client/ui-chat/src/client/chat/TurnTailNodeView.tsx` | TurnTail direct action/usage/clock ownership; native branch/fork identities |
| `packages/client/ui-chat/src/client/chat/StatsPills.tsx` | ConversationStats hierarchy and details over native totals |
| `packages/client/ui-chat/src/client/chat/StatsPills.module.css` | TurnTail.module.css centered footer pills, typography, spacing and tabular figures |
| `packages/client/ui-model-selection/src/client/ModelSelect.tsx` | ModelSelect binding/default/unavailable-selection presentation |
| `packages/client/ui-workspace/src/client/rows/WorkspaceBrowser.tsx` | WorkspaceNavigation dialog-local mutation pending/error treatment |

The existing ResponseTail CSS/module source lineage is retained under TurnTail;
MessageIconActions-derived copy feedback now resets locally. No entire upstream
package was copied. New selector, input-trigger reducer, preference, native
projection, Turn-node binding and deletion code are rustX-authored conceptual
adaptations, not copied Harness implementation or runtime authority.

Additional files studied as behavioral/type references (no new copied source):

- `packages/client/ui-conversation/src/client/skeleton/InputBar.module.css`
  and `ConversationSession.tsx`;
- `packages/client/ui-conversation/src/client/input/{hub,facade,machine,submission-policy}.ts`,
  `input/editor/view-binding.ts`, and
  `contract/{input,conversation,draft-editor,composer-submission}.ts`;
- `packages/client/ui-input-trigger/src/client/{controller.ts,MenuView.tsx}`,
  `src/core/{detect,menu}.ts`;
- `packages/client/ui-commands/src/client/{directory,resolution,popup,service,contract,presentation}.ts`;
- `packages/client/ui-model-selection/src/client/{directory,service}.ts`;
- `packages/core/agent-default-model/src/index.ts`,
  `packages/core/agent/src/model-selection.ts`, `apps/web/tests/default-model.e2e.ts`;
- `packages/client/ui-chat/src/client/chat/{MessageIconActions,ChatView,ReasoningRow}.tsx`;
- `packages/client/ui-primitives/src/Toast.tsx`;
- `packages/client/ui-workspace/src/client/session-actions/{RenameSession,ArchiveSession}.tsx`.

See [the ownership report](../docs/issue-406/conversation-surface.md) for semantic
differences. Native deletion is not upstream archiving; native uncertainty is
never discarded as a transient notice. Earlier provenance sections describe
their historical changes, not a competing current conversation implementation.

## #393 Settings visual convergence, responsive UX and browser acceptance

This change imports no new upstream source and repins nothing; the Harness pin
remains `ddefc45fbc7f8e46dd73185e68295696d1297887`. Reviewed inventory updates
only, each with a new local hash, import closure and a `#393` treatment note:

- `presentation/primitives/Menu.tsx` and `Menu.module.css` move from class A to
  B. The upstream Menu API, DOM, classes, keyboard walk, selection marker,
  disabled rows, submenus, pointer grace and focus restoration are retained. Its
  hand-written portal geometry — anchor and list measurement, viewport
  clamping, capture-phase scroll and resize listeners and the hidden
  measurement pass — is deleted and replaced by Floating UI, the one geometry
  owner, and so are the CSS-positioned in-place list (with its `portal` prop)
  and the CSS-positioned submenu card: the list and each open submenu are
  body-portaled Floating UI surfaces, the submenu anchored to its own row.
  Floating UI publishes the room the viewport leaves; the CSS keeps the design
  bounds within it. A submenu is its own keyboard layer. Every surface is
  marked as a React Aria top layer, Escape stops before a containing React Aria
  modal, and the selected row is announced. Its callers drop the `portal` prop.
  Every surface lives only while its reference is a visible anchor (Floating
  UI `hide`): a list whose anchor leaves layout closes through `onClose`, a
  submenu whose row scrolls out closes. The render carrying that placement
  stops presenting the surface, and the layout phase of its commit settles the
  keyboard and closes it once, before paint. A keyboard the list held moves,
  without scrolling, to the `focusOwner` its host names (never inferred from
  the DOM), and a keyboard in a hidden submenu to the parent list, never to a
  hidden control or a removed row. Passive hover never replaces or closes a
  submenu the keyboard holds; a pointer press on a parent row takes it over
  with the keyboard on that row.
- `presentation/settings/SettingsRoot.tsx` and `SettingsRoot.module.css` keep
  the Harness mask/panel/rail/content frame and classes. The viewport
  media-query store, the horizontal tab orientation and the strip-scrolling tab
  label are deleted; the narrow layout is a container query on the panel. The
  section menu closes through the shared Menu when that query takes its
  trigger out of layout.
- `presentation/settings/SettingsContent.module.css`, `app/settings/Settings.tsx`
  and `app/settings/extensions/ExtensionDetail.tsx` carry the converged page
  vocabulary.

Rows, the section menu, the Session configuration banner
(`presentation/agent/SessionConfiguration.module.css`) and every other new
presentation in this change are rustX-authored and carry no DeepSeek header.
Page glyphs come from the existing `presentation/primitives/icons` family; no
icon package is added.

Pinned dependencies adopted for demonstrated #393 requirements:

- `@floating-ui/react-dom` 2.1.9 (MIT, Floating UI contributors), production,
  with its install closure `@floating-ui/dom` 1.8.0, `@floating-ui/core` 1.8.0
  and `@floating-ui/utils` 0.2.12 (all MIT). Their license texts are reproduced
  in the regenerated `public/THIRD-PARTY-NOTICES.txt`.
- `@axe-core/playwright` 4.13.0 (MPL-2.0) with `axe-core` 4.13.0 (MPL-2.0),
  development only. They run inside the Playwright test process against
  rendered test pages, are never bundled, never modified and never
  redistributed with the Web Console, so they add no production notice.

- `@base-ui/react` 1.8.0 (MIT), production, with `@base-ui/utils` 0.4.0
  and `reselect` 5.3.0 added transitively. Its closure also makes the existing
  `@babel/runtime` 7.29.7 a production dependency. The normal notice generator
  reproduces the complete closure in `public/THIRD-PARTY-NOTICES.txt`; no
  package-specific license exception is needed.
  `DialogSurface` imports only Dialog and AlertDialog, for Settings and its
  confirmations. Base UI owns generic modal mechanics. rustX keeps Harness
  styling and workflow semantics; UnitShell supplies the stable outcome form.
  Cancel uses `initialFocus`. The public `finalFocus` callback applies the exact
  workflow focus destination and returns `false` to suppress automatic
  restoration to a different descendant. No styles, tokens or state ownership
  are imported. See <https://base-ui.com/react/components/dialog>.
  Other application modal surfaces remain unchanged.

React Aria retains non-dialog interaction semantics and TanStack Form retains
field mechanics. Menus and selects portal into their containing dialog scope;
Menu positioning, reference-hidden settlement and container-query ownership
remain unchanged.

Reference products were inspected only, with no source, asset or text copied.
Harness remains the visual family (the pinned commit above). ZCode documentation
(<https://zcode.z.ai/cn/docs/configuration>, <https://zcode.z.ai/cn/docs/mcp-services>)
informs the Provider → Model and resource list → detail structure with status
and actions beside their object. Kimi Web documentation (`MoonshotAI/kimi-cli`
`docs/en/reference/kimi-web.md` at `934b704a5eff1726623dd80db62907fbc1f7dd72`)
informs restrained local feedback and progressive disclosure. None of their
configuration precedence, account, billing, plugin runtime, Session or approval
semantics is adopted.

## #392 Settings product pages and resource workflows

This change imports no new upstream source and repins nothing; re-fetched
upstream remains `ddefc45fbc7f8e46dd73185e68295696d1297887`. Reviewed inventory
updates only:

- `app/settings/CatalogEditor.tsx` moved to `app/settings/models/ModelsPage.tsx`
  (the same `ProviderEditor.tsx`-derived Provider/Model cards, now the Models page
  with Provider list → Provider detail → Model detail).
- `app/settings/Integrations.tsx` moved to
  `app/settings/extensions/ExtensionDetail.tsx` (the same
  `PluginInventorySettingsTab.tsx`-derived resource cards, now one resource detail).
- `app/settings/Settings.tsx`, `presentation/settings/SettingsRoot.tsx` and
  `SettingsRoot.module.css` carry new local hashes and import closures. The
  Harness overlay/nav/content DOM and classes are retained; its focus trap,
  portal and section buttons are replaced by React Aria `ModalOverlay`/`Modal`/
  `Dialog`/`Tabs`.

Every other new Settings module (`general/`, `agent/`, `tools/`,
`extensions/ExtensionsPage.tsx`, `extensions/inventory.ts`,
`extensions/NativeExtensions.tsx`, `advanced/`, `capability.ts`, `primitives/`,
`forms/` and `presentation/settings/SettingsWorkflow.module.css`) is
rustX-authored and carries no DeepSeek header.

New pinned production dependencies, the only two this issue allows:
`react-aria-components` 1.21.1 (Adobe, Apache-2.0) and `@tanstack/react-form`
1.33.5 (MIT), with their install closure (`react-aria`, `react-stately`,
`@react-types/shared`, `@internationalized/*`, `@swc/helpers`, `aria-hidden`,
`client-only`, `@tanstack/form-core`, `@tanstack/store`, `@tanstack/react-store`,
`@tanstack/pacer-lite`, `@tanstack/devtools-event-client`, `tslib`). Their license
texts are reproduced in the regenerated `public/THIRD-PARTY-NOTICES.txt`.
`client-only` 0.0.1 declares MIT but publishes no license file; `scripts/notices.ts`
records it through an exact-version, exact-license reviewed exception that fails
closed on any change. No Spectrum stylesheet, second design system, router,
global state library, query cache or schema library is added.

`@tanstack/devtools-event-client` is installed but never bundled: `vite.config.ts`
resolves it to the rustX-authored `forms/inert-devtools-event-client.ts`, because
the stock client broadcasts and queues complete form state (typed secrets
included) on `window`. `scripts/provenance.ts --artifact` fails the build if the
devtools handshake appears in the bundle. TanStack Form Devtools are not used.

Reference products were inspected only, with no source, asset or text copied:
Z.ai ZCode configuration docs (<https://zcode.z.ai/cn/docs/configuration>) for
the Provider → Provider detail → Model detail hierarchy, and Kimi Web reference
docs (`MoonshotAI/kimi-cli` `docs/en/reference/kimi-web.md`, commit
`934b704a5eff1726623dd80db62907fbc1f7dd72`, 2026-03-23) for task-grouped pages
with diagnostics kept apart.

## #391 Settings orchestration actors

This change imports no new upstream source and repins nothing. The derived
`Settings.tsx`, `CatalogEditor.tsx` and `Integrations.tsx` records carry reviewed
local-hash and import-closure updates: their configuration orchestration moved to
rustX-authored XState v5 machines under `app/settings/machines/`, and their
`SaveSource` callback prop was removed. Those machines, the configuration actor
system and the per-unit CAS transaction machine are rustX-authored; Harness
supplies no equivalent, and no upstream hash, MIT header or license closure
changes. The inventory gains no entry.

`xstate` 5.33.2 and `@xstate/react` 6.1.0 are new pinned production
dependencies; their license text is reproduced in the regenerated
`public/THIRD-PARTY-NOTICES.txt`. Both run entirely locally: no Stately cloud or
runtime service is contacted, and the XState v6 alpha is deliberately not used.

Re-fetched upstream remains `ddefc45fbc7f8e46dd73185e68295696d1297887`.

## #372 server-resolved Trace relationships

This change imports no new upstream source and repins nothing. The pinned
`ui-trajectory/trajectory-request-header-definition.ts`,
`ui-trajectory/trajectory-message-definitions.ts`, `ui-trajectory/layout.ts` and
the `ui-conversation` `contract/system-prompt.ts` /
`contract/request-inspection.ts` contracts were read again at
`ddefc45fbc7f8e46dd73185e68295696d1297887` to understand the presentation
semantics of a System-prompt change, a context record and Harness's `uncertain`
answer. Harness derives all three in the browser from raw session events; rustX
deliberately does not, so the closed `TraceSystemPromptState` /
`TraceContextKind` vocabularies and their resolution are rustX-authored native
Rust, not adapted upstream code.

Upstream `ui-trajectory` Subtool support (`rootCallId` / `parentCallId` /
`subCallId`, its `subtool` cell kind and `expandSubCalls`) is deliberately not
adapted: it projects Harness's real nested `run_code` sub-dispatch pairs, and
rustX has no native nested Tool-call execution path with parent/child call
identities. Subagent and Workflow are not Subtool.

Existing derived `TrajectoryCell.tsx`, `TrajectoryInspector.tsx`, `search.ts`
and `Trajectory.module.css` records carry reviewed local-hash updates for the
new native DTO bindings and the generated-protocol path rename; upstream hashes,
import closures, MIT headers and license closure are unchanged. The provenance
inventory gains no entry.

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
| `ui-primitives/src/Menu.module.css` | `presentation/primitives/Menu.module.css` | B |
| `ui-primitives/src/Modal.module.css` | `presentation/primitives/Modal.module.css` | A |
| `ui-primitives/src/Menu.tsx` | `presentation/primitives/Menu.tsx` | B |
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
  are adapter seats for existing CFG3 editors. A narrow Settings panel replaces
  the rail with a section menu (#393; the earlier horizontal section strip is
  deleted). No Settings Controller, mirror, source cache or publication
  authority was imported.
- RightPanel keeps upstream normal/fullscreen panel geometry and transition;
  rustX Inspector supplies its header and scrolling body. No docking runtime,
  terminal/file authority or floating-window manager was included.
- Menu retains portaled rendering, submenus, pointer grace and keyboard behavior;
  all of its geometry, submenus included, is Floating UI's since #393, and
  autofocus waits for that placement. Modal retains upstream DOM/chrome with
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

Earlier-history presentation stays on the model-backed timeline path. Native
`TraceTiming.started_at` is mandatory and `TraceProjection::page` only sets
`next_cursor` when it retained an anchor, so a loaded window that projects no
span is also a window with no earlier cursor. The marker therefore lives where
upstream puts it, inside the positioned `.canvas`, and the empty overview is a
plain message. Malformed or incomplete DTO states are not modelled as product
modes.

## Issue #377 — Agent Status placement and Todo composer-dock semantics

Reference checkout: `deepseek-ai/deepseek-harness` at
**`c291e7961a515f6d7af9304e7fd1d257929aef26`**, the commit the Todo dock records
already pin. It was read at that exact revision in a separate checkout outside
every rustX worktree; upstream HEAD was not substituted and no build or runtime
step downloads Harness code.

Files read for this change:

| Upstream at `c291e79` | Use |
| --- | --- |
| `ui-conversation/src/client/skeleton/TodoPanel.tsx` | already the derived source of `TodoDock.tsx`; its `if (todos.length === 0) return null` is the behaviour rustX had diverged from |
| `ui-conversation/tests/todo-panel.client.spec.tsx` | acceptance behaviour: empty renders nothing, collapsed count summary with zero segments omitted, every parallel `in_progress` row counted, all-completed is not empty |
| `.agents/notes/archived/feature/2026-07-23-web-todo-display.md` | the current standing-plan strip versus the historical tool row |
| `.agents/notes/archived/feature/2026-07-28-todo-plan-clears-on-next-turn.md` | Harness's turn-scoped plan lifetime — read to exclude it deliberately |
| `ui-tool/src/client/tool/toolviews/todo-row.tsx` | historical todo evidence stays at its own transcript location |

Adopted: the empty-renders-nothing result, the collapsed count-summary header with
no activity ticker, the disclosure interaction, the status glyph family already
present locally, and the current-panel / historical-row separation.

**Deliberately excluded: Harness's Todo ownership and lifetime.** Its standing plan
is turn-scoped and clears on the next `turn/start`; that rule belongs to its
`todo/write` domain. rustX Todo is the conversation-owned `ConversationTodoList`
authority projected as `snapshot.todos`, and the browser clears nothing.

No upstream source was copied for this change. `TodoDock.tsx` is an existing derived
file whose local hash and treatment are refreshed in
[source-inventory.json](source-inventory.json); the four newly read files are
recorded under `inspected_only`. The Agent Status binding
(`src/bindings/agent-status.ts`) and annotation (`src/app/agent/AgentStatus.tsx`)
are rustX-authored against the native `AgentStatusView` contract and reuse the
existing derived `DisclosureRow` primitive and icon set; they carry no Harness
copyright header because no Harness source is present in them. Upstream hashes, the
pinned reset commit, licensing, classifications and dependency closure are otherwise
unchanged, and no Harness or Cordis build-time or runtime dependency is introduced.

## Issue #394 — one native-owned Trajectory display projection

This section supersedes the earlier WEB-15 renderer/selection/folding descriptions.
The reference remains **ddefc45fbc7f8e46dd73185e68295696d1297887**. No repin,
Harness runtime dependency, event assembler or compatibility renderer is added.
`source-inventory.json` records immutable upstream SHA-256, commit, local destination,
local SHA-256, imports, exclusions and treatment for every derived file. Its
`inspected_only` entries record the excluded/reference-only source below.

| Pinned source under `packages/client/ui-trajectory/src/client/` | Local treatment / semantic replacement |
| --- | --- |
| `TrajectoryView.tsx` | Inspect composition, exclude view registry, controller, assembler and global state. `Trajectory.tsx` receives the existing bounded native Trace cache. |
| `TrajectoryTable.tsx`, `TrajectoryTable.module.css` | Adapt dense Event/Content ledger, request boundaries, selection, Inspector and virtual-row grammar in `Trajectory.tsx`, `Trajectory.module.css`, `TrajectoryInspector.tsx`. Exclude index selection, global Request numbers, adjacent-call matching, generic custom pane resizing and scroll-height prepend arithmetic. Native IDs + local display keys/facets replace them. |
| `TrajectoryToolbar.tsx`, `TrajectoryToolbar.module.css` | Adapt thin Duration/Attempts/Calls/search controls into local Trajectory files. Actual time is an explicit rustX extension exposing the pinned time/actual projections. No permanent load/latest/count chrome. |
| `TrajectoryTimeline.tsx`, `TrajectoryTimeline.module.css`, `timeline.ts` | Adapt Input/Model/Tools overview, linked selection, zoom/pan, phase drawing and shared idle compression. Native Request clock bridge/TTFT replaces Harness Assistant step-start timing. Accepted Assistant adds no second generation span. |
| `TrajectoryCell.tsx`, `TrajectoryCell.module.css` | Inspect role/tag/content grammar; retain the existing adapted `TrajectoryCell.tsx`/ledger CSS. Exclude the legacy standalone metrics-card layout and Subtool. |
| `TrajectoryTurn.tsx`, `TrajectoryTurn.module.css`, `TrajectoryTurnHeader.tsx`, `TrajectoryTurnHeader.module.css`, `TrajectoryGroupHeader.tsx`, `TrajectoryGroupHeader.module.css` | Inspect section/header hierarchy; adapt lightweight Attempt/Step segments in `layout.ts` and local CSS. Exclude Turn lifecycle interpretation and permanent metric columns. Native AttemptId/TurnId own identity. |
| `layout.ts` | Replace assembler-derived layout with closed `TrajectoryDisplayItem` projection; no client prompt comparison, context-role inference or synthetic runtime events. |
| `locales.ts` | Inspect and adapt English SYSTEM/update, Context, Calls and timing labels. Exclude plugin locale registration; native relationship enum determines the label. |
| `trajectory-contract.ts`, `trajectory-virtual-rows.ts` | Inspect envelope/row grammar; exclude Harness snapshot authority and index keys. Generated Trace DTOs and semantic display keys replace them. |

Also inspected `packages/core/session/src/types.ts` (EpochHeader, RequestContext,
request/system/context events) and
`packages/client/ui-conversation/src/client/contract/request-inspection.ts`
(ConversationPromptSnapshot, SystemPromptNode, RequestPromptChange, RequestView).
Their runtime semantics are **excluded**: rustX has no canonical SYSTEM message;
immutable RequestSnapshot + one native exact predecessor own prompt/Tool
relationships. Context comes only from frozen `request_context_ids` with native
producer/family. Background/Subagent/Workflow remain native domains, not Subtools.

Parity and deliberate differences:

| Upstream behavior | rustX mapping / difference | Verification |
| --- | --- | --- |
| Initial/system/tools/combined labels | Native enum → SYSTEM immediately before its actual Request, after Step; no invented Session-start event | T1-01/03 |
| Prompt rows and Context | Request-owned facets, exact frozen Message IDs/order; unchanged-only page has direct System Prompt access | T1-04/05 |
| Request boundaries | Actual RequestIdentity and retry ordinal within native Turn, including no-Assistant failures; no global number | T1-06 |
| Calls fold | Exact scoped proposal/execution match, loaded only; warnings and native domains remain visible | T1-07/08/09 |
| Virtual rows / prepend | Semantic keys + pixel offset; bounded TanStack mount window; inspectable items retain owners, structural focus follows the same native Attempt/Step segment without borrowing a child owner | T1-10/11 |
| Duration and Actual time | sequence = equal operations; duration = recorded spans with shared idle compression; time = absolute-start markers; actual = absolute spans, gaps and overlap retained | T1-12 |
| Inspector geometry | `react-resizable-panels` owns drag, keys, constraints and reset; local measured-container responsive policy | T1-13 |

New production dependencies are exactly `diff@9.0.0` (jsdiff line algorithm only,
never relationship classification) and `react-resizable-panels@4.12.4`. Lockfile,
production dependency notices and artifact checks include both. Existing TanStack
Virtual, React Aria Tabs, Markdown, Shiki, JSON/artifact and authorization primitives
remain the only corresponding engines. No diff UI framework or new state machine.

See [acceptance record](../docs/trajectory-convergence-validation.md) for concrete
T1/X tests, commands and browser evidence. Browser control uses the repository's
pinned Playwright container: agent-browser is unavailable in this environment and
no Browser plugin is installed; the container is also the CI screenshot authority.

## Issue #402 — conversation and Models convergence

The pin remains `ddefc45fbc7f8e46dd73185e68295696d1297887`. All source is
vendored and checked into this repository; builds and runtime never fetch Harness.
Each entry below was inspected at that pin before adaptation. Hashes, exact
imports and additional-source provenance are recorded in `source-inventory.json`.

| Pinned source | Adapted destination |
| --- | --- |
| `packages/client/ui-conversation/src/client/skeleton/EmptyHero.tsx` | `src/app/new-conversation/NewConversation.tsx` |
| `packages/client/ui-conversation/src/client/skeleton/HeroShell.module.css` | `src/presentation/agent/HeroShell.module.css` |
| `packages/client/ui-permission-presets/src/client/PermissionSelect.tsx` | `src/presentation/agent/PermissionSelect.tsx` |
| `packages/client/ui-permission-presets/src/client/PermissionSelect.module.css` | `src/presentation/agent/PermissionSelect.module.css` |
| `packages/client/ui-primitives/src/RiskConfirmation.module.css` | `src/presentation/primitives/RiskConfirmation.module.css` |
| `packages/client/ui-chat/src/client/chat/TurnProcessNodeView.tsx` | `src/presentation/agent/TurnProcess.tsx` |
| `packages/client/ui-chat/src/client/chat/TurnProcessNodeView.module.css` | `src/presentation/agent/TurnProcess.module.css` |
| `packages/client/ui-settings-models/src/client/ModelsSection.module.css` | `src/presentation/settings/ModelsCards.module.css` |
| `packages/client/ui-primitives/src/TerminalBlock.tsx` | `src/presentation/primitives/TerminalBlock.tsx` |
| `packages/client/ui-primitives/src/TerminalBlock.module.css` | `src/presentation/primitives/TerminalBlock.module.css` |
| `packages/client/ui-primitives/src/DiffBlock.tsx` | `src/presentation/primitives/DiffBlock.tsx` |
| `packages/client/ui-primitives/src/DiffBlock.module.css` | `src/presentation/primitives/DiffBlock.module.css` |
| `packages/client/ui-primitives/src/ReadBlock.tsx` | `src/presentation/primitives/ReadBlock.tsx` |
| `packages/client/ui-primitives/src/ReadBlock.module.css` | `src/presentation/primitives/ReadBlock.module.css` |
| `packages/client/ui-primitives/src/SearchBlock.tsx` | `src/presentation/primitives/SearchBlock.tsx` |
| `packages/client/ui-primitives/src/SearchBlock.module.css` | `src/presentation/primitives/SearchBlock.module.css` |
| `packages/client/ui-primitives/src/FoldToggle.tsx` | `src/presentation/primitives/FoldToggle.tsx` |
| `packages/client/ui-primitives/src/head-tail-cap.ts` | `src/presentation/primitives/head-tail-cap.ts` |
| `packages/client/ui-primitives/src/use-copy-feedback.ts` | `src/presentation/primitives/use-copy-feedback.ts` |
| `packages/client/ui-chat/src/client/chat/ContextInjectionRow.tsx` | `src/app/agent/AgentStatus.tsx` |
| `packages/client/ui-chat/src/client/chat/ContextInjectionRow.module.css` | `src/app/agent/AgentStatus.module.css` |

Existing pinned source adaptations reused/extended:

- `ui-conversation/.../skeleton/InputBar.tsx` and `InputBar.module.css`: the one
  shared composer, now with pre-Session File drafts and permission/model seats.
- `ui-workspace/.../WorkspacePicker.tsx`: Workspace chip/menu and adoption seat;
  rustX Product Host handles replace upstream path/root/runtime authority.
- `ui-permission-presets/.../PermissionSelect.tsx` and
  `ui-primitives/src/RiskConfirmation.tsx`: visual controls only. The upstream
  read-only/workspace-write presets and automatic permission controller are
  excluded; rustX exposes only policy/full_access and reuses its source actor.
- `ui-chat/.../ReasoningRow.tsx` and the existing Markdown stack: compact
  Markdown typography, one parser/security/streaming implementation.
- `ui-tool/.../components/ToolRow.tsx`: native-ID dispatch into the body primitives
  above. The existing `CodeBlock.tsx`, Shiki, Pill, StateDot, clipboard, and
  DisclosureRow remain the common rendering infrastructure.
- `ui-settings-models/.../ModelsSection.tsx`, `ModelsSection.module.css` and
  `ProviderEditor.tsx`: dense provider identity/action cards and progressive
  details. Harness credential writes, adapter-family runtime, built-in/custom
  taxonomy, provider connectivity claims and stores are excluded. Existing
  rustX TanStack typed forms and exact semantic-unit actors remain authoritative.

Inspected but not imported: `ui-tool/.../models/terminal-card-model.ts`,
`diff-card-model.ts`, `read-card-model.ts`, and `search-card-model.ts`. Their
Harness Tool metadata/identity models are incompatible with rustX. Native exact
Tool IDs and typed result blocks replace those models. Bash's native JSON
`combined` field is displayed without parsing human output. Read/Search use the
opaque bounded body when native output provides no structured file/match facts.
Diffs show requested Write/Edit changes, explicitly labelled, without pretending
the request contains a filesystem before-image. ANSI parsing is excluded from
TerminalBlock: no extra interpreter dependency was added; bounded native text is
rendered verbatim. SearchBlock retains its upstream structured primitive but its
opaque text seat is used for rustX's current projection.

Harness conversation controllers, event assembly, runtime stores, permission
catalogs and browser Session ownership are excluded. TurnProcess uses only
native `turn_process` membership, never Harness event reduction or browser
adjacency. Independent subagent lifecycles are not assigned a parent process by
provider call-ID coincidence; only native-owned process rows are folded. The
upstream fish logo/wordmark is deliberately excluded in favor of rustX branding.

Intentional visual differences: native permission vocabulary, native model IDs,
source provenance labels, unclassified-Session disclosure, and opaque Read/Search
content without invented line numbers. Models retains rustX's complete typed
configuration details and distinguishes unobserved application from availability.
The Agent Status status-section definition list replaces Harness ContextBody,
while its disclosure axes, separator and indentation reuse ContextInjectionRow.

PR #409 review corrections retain the existing Harness source pins and notices.
`WorkspaceBrowser` gains a presentation render slot so native per-row activity
subscriptions do not invalidate the browser; `WorkspaceNavigation` supplies that
binding. Their local hashes and import closure are updated in the inventory.
`ConversationSeat`, `ConversationHeader`, and `SettingsNavigationFeedback` split
existing rustX App ownership; they add no copied upstream source. Journal-owned
terminal transcript facts and failure routing are native rustX adaptations.


PR #409 terminal-process follow-up re-inspected Harness commit
`477b4f420553e8a52c2fbccc464d7561b239c443`, specifically
`packages/client/ui-chat/src/client/chat/TurnProcessNodeView.tsx`,
`packages/client/ui-chat/src/client/contract/turn-process.ts`,
`packages/client/ui-chat/src/client/conversation-nodes/turn-process.ts`, and
`packages/client/ui-chat/src/client/conversation-nodes/turn-process-presentation.ts`.
These are behavioral references for whole-Turn ownership, control placement,
counts and always-open stopped/failed disclosure. No additional upstream source
was copied. The existing adapted TurnProcess presentation is unchanged. Native
Journal projection and cursor-based Web composition are rustX-authored. Generated
App Server v23 imports and their local hashes/import closures move together.

## Issue #407 — Turn/Step projection and System Prompt cells

The read-only local Harness checkout was clean at
`477b4f420553e8a52c2fbccc464d7561b239c443` (`master`), matching fetched
`origin/master`. This is the reference actually studied for this change, not an
assumption based on the issue's historical SHA. The repository-wide provenance
baseline remains unchanged; per-source `additional_sources` records pin the new
review/adaptation precisely.

Adapted presentation patterns from `packages/client/ui-trajectory/src/client/`:
`layout.ts` (Turn owns Message/Step groups), `TrajectoryTurn.tsx`,
`TrajectoryTurnHeader.tsx`, `TrajectoryGroupHeader.tsx` (hierarchy), the matching
TurnHeader/GroupHeader CSS (sticky full-width bar and indented group labels), and
`TrajectoryTable.tsx` (System Prompt versus request detail tab organization).
These extend existing derived rustX files; no upstream runtime, module or whole
file was vendored. Inventory hashes/import closures and MIT attribution cover the
modified existing adaptations.

Studied only: `TrajectoryView.tsx`, `TrajectoryCell.tsx`, `trajectory-record.ts`,
`timeline.ts`, the `trajectory-*.ts` projection/search/virtualization/definition
family and module CSS; `ui-chat/.../chat/SystemPromptRow.tsx`, `ContextBody.tsx`,
`conversation-nodes/request-prompt.ts` and the conversation-node ownership,
retry, Tool and process definitions; `ui-conversation/.../conversation/` assembly,
registries, group store and especially `location-index.ts`. Newly pinned key
reference-only files are listed under `inspected_only` in the inventory.

Native `AttemptId` maps to visible Turn; native `TurnId`/`step_id` maps to visible
Step. Unlike Harness's assembler, the Web never fills missing ownership from a
following Assistant or location cursor. Request-owned initial System Prompt and
Context remain in their native Step; only native attempt-only inputs form Message.
Outside records stay outside. Exact scoped native Tool proposal/execution matching
is retained; Background/Subagent/Workflow/Interaction keep their own domain cells.
Request retries remain compact metadata, and the current frozen Tool catalog is
inspectable without inventing a historical Tool-catalog diff absent from Trace.

One `TrajectoryProjection` feeds both ledger and overview. All interaction state
uses native or record keys, including drag focus; Turn ordinals can change after
prepend. Drag focus lives only within the Trace cache epoch that created it,
while in-flight coordinate interactions belong to a separate semantic Timeline
projection revision. Same-epoch geometry changes retire gestures atomically;
status-only updates preserve the interaction generation. Epoch rebases retire
both layers. The overview and focus resolver consume the same Timeline model.
Turn/Step headers never own detail reads; their exact loaded native
Attempt/Step record is shown in a separate bounded structure inspector. The old contiguous Attempt/Step segments and SystemRow facet abstraction
are removed. Request-relative timing, finite cache/frontiers and native prompt
classification remain unchanged. See `docs/trace.md` for the current contract and
`docs/trajectory-407-validation.md` for deterministic regressions and validation.

## Issue #410 — browser-owned English/Chinese presentation

The read-only Harness checkout was clean at
`477b4f420553e8a52c2fbccc464d7561b239c443`. Its locale owner, browser bootstrap,
settings store and static UI-copy check were studied as design references only.
The rustX controller, React subscription, typed translation path and bounded
TypeScript AST gate are independently implemented; no Harness Host, Cordis,
plugin registration or dynamic language-pack runtime is imported.

Matching English/Chinese presentation wording was materially adapted from the
current Harness workspace, sidebar, General Settings and conversation locale
files into the feature-owned `src/locale/dictionaries` modules. Each adapted
module records its actual sources and this exact upstream revision in
`source-inventory.json`, including the moved legacy workspace dictionary.
Commands and Inspector dictionaries are independently authored. Existing
derived presentation files retain their original attribution; their local
hashes and import closures are refreshed. The repository-wide historical
baseline is unchanged.

The former English-only static translator, separate workspace translation API
and Sidebar inline translator are removed. All authored chrome uses one typed
translation path; native/user/model/Tool text remains opaque. The locale layer
imports only itself and React, and the presentation dependency boundary permits
only its React and translation entry points.

PR #418 review corrections (invariant command search, deferred clipboard notices,
option-label checking and typed status presentation) are independent rustX work.
No additional Harness material was copied or adapted. Existing source records
retain their upstream revisions; local hashes and import closures are refreshed.

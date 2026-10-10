## Shared model selection surfaces

Behavioral reference: Harness `5badb15009ae1756c3afe0ae0cef1faafc290ccc`,
`packages/client/ui-model-selection/src/client/index.ts`: the composer selector
and `/model` popup share one loaded model directory. The rustX attachment owns
its catalog and invalidates it on attachment, transport or resource revision
changes. `/model` now uses the existing composer popup styling, with provider
sections, search and keyboard selection; the obsolete model dialog is removed.
Native model mutation and authoritative rereads remain unchanged.

## Running Turn indicator

The running status, whale-tail APNG and static SVG come from
`packages/client/ui-chat/src/client/chat/{RunningStatus.tsx,RunningWhaleTail.tsx,running-whale@2x.png,ChatView.module.css}`
at Harness `5badb15009ae1756c3afe0ae0cef1faafc290ccc`. The matching light/dark
Deep-diving color tokens come from that revision's `ui-theme/src/styles/design-platform.css`.
The existing TextShimmer renders the text sweep. The native active Attempt mounts
one indicator at the transcript tail; its own start timestamp drives an isolated
one-second clock. Historical windows and settled Attempts show no live indicator.
Reduced-motion and forced-color modes use the original static whale artwork.

## Information-flow spacing

Reference: local `../deepseek-harness` at
`5badb15009ae1756c3afe0ae0cef1faafc290ccc`,
`packages/client/ui-chat/src/client/chat/{ChatView,ChatGroupSeat,TurnProcessNodeView,MessageItem}.module.css`
and `packages/client/ui-chat/src/client/locale.ts`.
The native transcript seats use the same 6px process, 12px response and 16px
Turn spacing, 14px Turn heading, 20px user bubble radius, and Chinese Think /
completed-duration wording. Hidden native seats no longer leave sibling gaps.
Native ownership, grouping, tool status and disclosure state remain rustX-owned.

## Interrupted Assistant presentation

Behavioral reference: local `../deepseek-harness` at
`5badb15009ae1756c3afe0ae0cef1faafc290ccc`, specifically
`packages/client/ui-chat/src/client/chat/{AssistantNodeView,AssistantMarkdown,TurnProcessNodeView,TurnTailNodeView}.tsx`.
Harness freezes released prose/reasoning on interruption, uses the stopped
process label, and does not replace it with a diagnostic JSON disclosure.
The rustX adapter renders typed publication-audit text, reasoning and refusal
through the existing Assistant/Markdown components. Empty audits and unexecuted
Tool proposals have no Chat body. Audits retain their native cursor; their
message identity suppresses stale streaming duplicates. Native terminal facts
continue to own stopped/failed labels. Copy and the native settlement time are
available for partial prose; completed-response actions are not fabricated.
Native audit data remains available through existing Inspector/Trajectory reads.
The adapter and grouping changes are rustX-authored; no Harness runtime or
interruption state machine is copied.

## Compaction continuation and checkpoint UI

Read-only reference: local `../deepseek-harness` at
`5badb15009ae1756c3afe0ae0cef1faafc290ccc`.
`packages/client/ui-chat/src/client/chat/CompactionItem.tsx` and
`src/client/locale.ts` supply the checkpoint disclosure and bilingual wording;
`CompactionCommandCard.tsx` was consulted for one completed marker per command.
The existing Message stylesheet already carries the Harness geometry, sticky
expanded header, muted title, context icon and hover disclosure. The Message
adapter now uses those styles for native `compaction_summary` Ledger messages.
No history count, token estimate, command row or runtime outcome is invented.
The temporary completion notice yields only when its exact native summary is
present in the displayed transcript page. Source hashes are in the inventory.

`context/compact` awaits model generation and native maintenance release. It
uses the existing bounded domain-wait lane rather than the ordinary 30-second
RPC deadline that disconnected every Session on the shared socket. Native
execution, lost-response correlation and no-replay behavior remain authoritative.
Controlled-clock tests hold the reply beyond the RPC deadline and verify read
and control capacity. The real App Server with a gated scripted provider covers manual compaction,
continued input, questionnaire settlement, automatic overflow compaction and
final response in both locales at desktop and narrow viewport widths.

The trajectory was also checked against this reference's
`packages/client/ui-trajectory/src/client/trajectory-compaction-definition.ts`,
`layout.ts`, and `TrajectoryTable.tsx`: completed compaction has a summary result
with rendered Markdown and raw output. Manual maintenance now persists its
start/failure boundaries, making that result reachable in the native trajectory
both live and after reconnect. The inspector shows detail loading/errors, and
an incomplete operation is no longer mislabeled as a failure. Native summary
content remains the authority; unrecorded provider usage is not synthesized.
Docker browser tests inspect the summary during completion and after reconnect
in English/Chinese on desktop/mobile.

## #439 Composer keyboard policy

Approved read-only reference: `deepseek-ai/deepseek-harness` at
`639ed015397290b3745d163aafe02ffee4aa3f84`, inspected using `git show` without
changing its working checkout. Existing source pins remain unchanged.

- Adapted: `packages/client/ui-conversation/src/client/input/submission-policy.ts`
  (preferred/complementary delivery mapping in rustX's existing pure policy), and
  `packages/client/ui-conversation/src/client/stop-sequence.ts` (bounded two-state
  sequence, exact identity comparison and reset-before-dispatch in `stop-sequence.ts`).
- Behaviorally consulted only: `packages/client/ui-conversation/src/submission-settings.ts`,
  `src/client/stop-shortcut.ts`, `packages/client/shortcuts/README.md` and
  `packages/client/ui-conversation/README.md`. Closed union/default and focus/menu
  arbitration inform the local contract; no source from these files is copied.
- Excluded: Cordis, Host user-settings/storage authority, generic shortcut registry,
  editable keybindings, configurable timing, optimistic submission consumption,
  continuable-child scope and upstream runtime execution/admission ownership.

The dedicated origin/device preference and textarea event adapter are rustX-authored.
The source inventory records the added sequence's MIT lineage, additional policy
source, consulted-only files, exact upstream/local hashes and import closure.
Native cancellation, Queue/Steer admission, upload gates and retained-first-submission
semantics keep their existing rustX owners. See [COMPOSER.md](COMPOSER.md).

PR #445's accepted Composer behavior is integrated with #447's mandatory App Server v35/file-delivery
contract. Inventory hashes and retained dependency closures describe this final
source, including v35 imports; upstream reference pins are unchanged.

## #432 Host desktop opening

Read-only behavioral comparison: `deepseek-ai/deepseek-harness@639ed015397290b3745d163aafe02ffee4aa3f84`,
`packages/client/ui-open-in-app/src/client/OpenInAppAction.tsx` and
`packages/host/open-in-app/src/{index,resolver,catalog,shared}.ts`. The inventory records their
immutable hashes under inspected-only sources. The new code is rustX-authored;
no upstream source, application registry, icons, Cordis or runtime was copied.
The shared primary/menu pattern, lazy closed catalog and detached safe-argv
handoff inform the implementation. rustX instead resolves exact Session/node
identities via the native catalog, reuses Host root authority, requires an
explicit filesystem mapping, and reports spawn acknowledgement without a timed
GUI-success heuristic or automatic replay. See `docs/open-workspace.md`.

## #420 incremental projection and Chat synchronization

Read-only reference inspected: `477b4f420553e8a52c2fbccc464d7561b239c443`,
`packages/client/ui-chat/src/client/chat/{ChatView.tsx,use-chat-scroll.ts,
use-scroll-follow.ts,use-chat-viewport.ts}`. Existing pinned lineage is unchanged.
ChatViewport retains its attributed source lineage and adopts the semantic anchor,
follow-intent and browser-clamp attribution patterns. rustX owns the single RAF
commit rule and native message identity adapter. PR #443 repairs active reading
with native start intervals rather than marker heights; the viewport remains the
sole geometry reader and automatic scroll writer. Smooth native scrolling and
Harness runtime/store authority were not imported: they do not satisfy the required
single frame writer or rustX native authority boundary. Generated imports advance
with the mandatory protocol; local hashes/dependency closure are refreshed.

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
`workspaces/associations.ts` owns the bounded, identity-keyed display projection
shared by sidebar and selected Session. Classification publication is fenced by
captured Session IDs/cwds, authority, request and catalog revision; it is never
operation authorization. No browser-owned durable Session-to-Workspace map exists.

The independently authored `workspaces/host.ts` is the environment-neutral
Workspace/configuration contract, pure result validator and single shared
`WorkspaceHostError` definition. Node `host/workspaces.ts` and `host/http.ts` depend
on it directly. The independently authored browser adapter moved intact to
`workspaces/http-host.ts`, which imports that contract and `carrier/http.ts`;
`app/App.tsx` composes the adapter directly. There is no browser re-export from
the neutral module. Existing derived consumers' `workspaces/host` dependency
records remain contract-only; this split introduces no additional upstream reuse.

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
  TextPreview 38px path/action header, scroll body and wrap treatment, plus
  MarkdownBody document padding and normal whitespace. Native preview strips reuse
  the already attributed dockkit chips, file icons and sidebar chrome. Host filesystem operations,
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
runtime boundaries. The native textarea uses CSS content sizing; no temporary-collapse measurement hook remains.

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
App Server v24 imports and their local hashes/import closures move together.

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

## #411 Jobs and continuable Agents

No new upstream source is copied and the presentation baseline is unchanged.
Current source review used DeepSeek Harness commit
`477b4f420553e8a52c2fbccc464d7561b239c443`:
`packages/client/ui-subagent/src/client/sidebar-chat/index.tsx` and
`packages/client/ui-jobs/src/client/JobListAction.tsx`. Stable child detail and
separate finite Job output/control patterns inform rustX-authored ActivityCards.
The old one-shot/continuable mode branch is deliberately not adopted.

Reviewed local inventory changes update App Server imports to v27 and ToolCard's
identity discriminator to finite `job`, including its `data-job-id` marker.
Existing Tool dispatch, primitives, styling, licenses and pinned upstream hashes
are retained. Inventory local hashes and import closure reflect those edits.

Review corrections retain native Admitting state and typed creation/message/client/
Workflow activation provenance. Current local hashes and import closures track
App Server v27; the preceding #409 v27 record describes its historical audit.
## Issue #419

Client-lifetime first-submit ownership and localized pending presentation are
independent rustX work using the existing Harness-derived composer and primitives.
Local hashes/import closures are refreshed; every upstream revision is unchanged.
Codex startup source was inspected for ownership patterns at
`985cf47a4eb6084b2ff6b30ebdb1216acda85bb4`; no source was copied or vendored.

## #421 semantic execution ledger convergence

Inspected read-only at approved Harness commit
`477b4f420553e8a52c2fbccc464d7561b239c443`. The complete source → pattern → native
owner → adaptation/rejection mapping is in
[`docs/trajectory-harness-convergence.md`](../docs/trajectory-harness-convergence.md#pinned-harness-mapping).

`TrajectoryTable.tsx` and its CSS supply semantic-row chrome, fold retention,
input/result tracks and compact Inspector geometry. `trajectory-virtual-rows.ts`
supplies measurable seat separation; rustX deliberately rejects its attachment
of request-only separators to the next row by position. Exact Request ownership
selects System, Context or a 10px native marker seat. `TrajectoryTurn` and
`TrajectoryGroupHeader` hierarchy is retained as native metadata, not standalone
content rows. `TrajectoryTimeline.module.css` supplies three compact lanes;
native Request timing, epoch/revision and gesture ownership remain rustX's.
Search borrows structural vocabulary, never Harness heavyweight detail indexing.

New `TrajectoryLedger.tsx` and `TrajectoryRow.tsx` are bounded descendants with
separate viewport and semantic/chrome responsibilities. No Harness runtime,
request inference, manual resize system, subtool authority or canonical history
is imported. Earlier per-file baselines remain; additional-source hashes identify
this exact approved source. Its implementation-time import closure used App Server v29.

The PR #424 contract repair keeps the pinned Harness revision above. Exact
record membership repairs JSON-null Step ownership in the shared projection;
local source hashes and import closures reflected the mandatory v28 vocabulary.
The #409/#411 v27 descriptions above are historical audit records. The full
pinned-container browser acceptance passed without screenshot baseline changes.

PR #425 integrates image/Bash presentation into those same native abstractions.
Its implementation-time import closure used App Server v29; upstream revisions,
licenses, and semantic ledger ownership remain unchanged.

The #433 review repair updates the local WorkspaceNavigation hash for consuming
current-demand status from the existing display owner, and the Settings dictionary
hash for English/Chinese evidence acknowledgement and deletion outcome copy. The upstream source, license
and import closure are unchanged; no additional upstream code or dependency is used.

The #436 contract hardening updates the local WorkspaceNavigation hash so Sidebar
metadata intent capture requires the same display catalog that rendered the action,
matching the Composer. The upstream source, license, and import closure are unchanged;
no additional upstream code or dependency is used.

## Issue #430 conversation reading surface

The approved reference is `deepseek-ai/deepseek-harness@639ed015397290b3745d163aafe02ffee4aa3f84`.
The suggested `/home/caismis/Documents/codes/deepseek-harness` checkout was absent;
a separate clean, detached reference at `/workspace/deepseek-harness-reference`
was inspected without modification. Its newer per-file pins do not change the
inventory's existing baseline or license artifact.

Materially adapted interaction patterns:

- `packages/client/ui-conversation/src/client/skeleton/ConversationWidthControls.tsx`:
  measured column, pointer capture, symmetric handles, frame-coalesced drag,
  explicit commit/cancel. The implementation publishes local CSS and persists
  only deliberate browser preferences.
- `packages/client/ui-chat/src/client/chat/TurnNavigator.tsx`:
  the fixed-pitch virtual rail of every known turn, active/busy marks, focus
  range retention, first-size placement, follow-outside-the-fade-band and the
  hover/focus prompt/response preview. The implementation consumes rustX's
  native Attempt outline: its count names every mark, its one bounded page
  supplies loaded marks, and an unloaded mark reads its native page before
  navigating (Harness pages its event window the same way).
- `packages/client/ui-chat/src/client/chat/turn-rail-items.ts` was inspected for
  outline/loaded-anchor reconciliation; no source was copied. Loaded anchors
  are reused only with the same native cut and exact origin/cursor.
- `TurnNavigator.module.css` supplies the rail geometry, ticks, fades, preview
  card and narrow-container hiding. The slot is the rustX reading surface,
  which already excludes the composer, so the frame centers in it.
- `tests/turn-navigator-fixture.ts` supplies the jsdom rail size observer.

The preview text is native: `ConversationTurn.prompt` (the turn's first human
prompt) and `ConversationTurn.response` (its final text once settled), bounded to
Harness's one-line and three-line budgets. No preview is derived in the browser.

No Harness runtime, Session store, semantic turn derivation, scroll coordinator
or shell was imported. Native Attempt origins own turn identity; display ordinal
only selects a native page. Existing derived ChatViewport and conversation CSS
retain their original provenance with updated local hashes and dependency closure.
The current generated imports and local dependency closure use mandatory App Server v35.

## #431 explicit delivery reference audit

The approved read-only pin `639ed015397290b3745d163aafe02ffee4aa3f84` was inspected;
[file inventory and exclusions](../docs/issue-431-reference-audit.md) records the
actual files. Explicit declaration, canonical action coordinates, and one shared
preview seat informed the design. No new Harness source was copied/adapted at that
pin, so no conceptual-only source records are added. The existing attributed
Markdown and RightPanel descendants are reused with their original upstream
baselines; current local hashes/import closure are refreshed for v35 and Markdown
preview composition. image-size is an independently pinned MIT dependency for
encoded raster dimensions, included in generated production dependency notices.

## Issue #435 observable native compaction

Read-only behavioral reference at approved pin
`639ed015397290b3745d163aafe02ffee4aa3f84`:

- `packages/client/ui-chat/src/client/chat/CompactionCommandCard.tsx`
- `packages/client/ui-chat/src/client/chat/CompactionItem.tsx`
- `packages/client/ui-chat/src/client/chat/GenericCommandCard.tsx`
- `packages/client/ui-chat/src/client/conversation-nodes/compaction.ts`
- `packages/client/ui-chat/src/client/conversation-nodes/command.ts`
- `packages/client/ui-conversation/src/client/context-occupancy.ts`
- `packages/client/ui-chat/src/client/contract/snapshot.ts`
- `packages/client/ui-conversation/src/client/skeleton/ContextMeter.tsx`

These informed the non-modal lifecycle, inspectable diagnostic and checkpoint identity.
The Composer occupancy disclosure was later removed for Harness parity; ContextSeat
is rustX-authored over native read state and renders only compaction lifecycle. No Harness command execution/storage, heuristic token breakdown, or
projected token estimate was imported. Existing canonical transcript rendering owns
historical checkpoints; the context seat adds no history rows. The failed lookup
of `ui-chat/src/client/chat/ContextMeter.tsx` was corrected to the actual
`ui-conversation` path above. Source hashes are recorded under `inspected_only`.

## Ask-question takeover and transcript row (UI/UX alignment)

Reference pin `5badb15009ae1756c3afe0ae0cef1faafc290ccc`:

- `packages/client/ui-user-questions/src/client/QuestionComposer.tsx` (QuestionFlow):
  the takeover keeps the header-only eyebrow, the minimize and dismiss icon
  actions, the always-visible custom answer row, single-choice advance, the
  skip/next/submit footer and the recommended first-choice draft. Dismissing
  the set is the native decline; a skipped question is omitted from the native
  index submission, exactly like any unanswered one. Countdown, Remote,
  plan-review and slot-store machinery is excluded.
- `packages/client/ui-tool/src/client/tool/toolviews/ask-question-row.tsx`,
  `components/QuestionToolRow.tsx` and `components/AskQuestionCard.tsx` →
  `src/presentation/agent/QuestionRow.tsx`: the row summary is the interaction
  verdict (waiting, answered count, cancelled, interrupted) and its expansion is
  the read-only question/answer record. `src/bindings/ask-user.ts` reads it from
  the persisted native `ask_user` arguments and result, pairing answers by their
  echoed question index; anything it cannot pair keeps the generic tool card.
- `components/AskQuestionCard.module.css` → `src/presentation/agent/AskQuestionCard.module.css`.

The timed-question panel actions and the trajectory Inspect pill have no rustX
counterpart and were not imported.

## Step-process groups (UI/UX alignment)

Reference pin `5badb15009ae1756c3afe0ae0cef1faafc290ccc`:

- `packages/client/ui-chat/src/client/conversation-nodes/process-groups.ts` and
  `process-activity.ts` were followed for segmentation and the category ranking;
  `src/bindings/step-groups.ts` re-expresses them over native transcript entries
  and rustX Tool names. No source was copied.
- `packages/client/ui-chat/src/client/chat/ChatGroupSeat.tsx` and `step-process.ts`
  → `src/presentation/agent/StepGroup.tsx`: the group header (activity icon
  swapped for the chevron on hover or expansion), the closed-group title
  composition and the capped body with scroll-edge fades.
- `ChatGroupSeat.module.css` → `src/presentation/agent/StepGroup.module.css`.
- `packages/client/ui-chat/src/client/presentation-policy.ts`: rustX has no work-details
  mode and uses Harness's default `detailed` policy (history grouped, live in place).

Live group titles and details, follow scrolling, presentation modes and the
searchable-hidden reveal were not imported.

## Usage pills and context meter (UI/UX alignment)

Reference: `/home/caismis/Documents/codes/deepseek-harness@5badb15009ae1756c3afe0ae0cef1faafc290ccc`,
read-only. `ui-chat` `stat-dialog.ts` / `.module.css` became the
`StatDialog` primitive (Base UI Popover owns placement and dismissal);
`StatsPills.tsx`, `TurnUsagePanel.tsx` and `ui-conversation` `ContextMeter.tsx`
became `app/agent/UsageStats.tsx` with their styles combined in
`UsageStats.module.css`; `token-format.ts` (plus `formatDuration` and
`formatTokensPerSecond`) became `app/agent/token-format.ts`. The retired
`ConversationStats.tsx` record is removed. Harness's Compact mode, plugin dock
slots, cache-write bucket and browser-side token meter are not imported: every
figure is a native rustX reading (`ConversationStatistics.timing`,
`CompletedResponseView.usage`/`models`, `ContextOccupancy.breakdown`).

## Completed-response lineage actions (UI/UX alignment)

Reference: `/home/caismis/Documents/codes/deepseek-harness@5badb15009ae1756c3afe0ae0cef1faafc290ccc`,
read-only. `ui-chat` `TurnTailNodeView.tsx`, `MessageIconActions.tsx` and the
`forkAt` seat in `apply.ts` were consulted; no new upstream source is imported
and no pin changes. The tail keeps one branch action, Branch into a new Session
(`session/fork`), and rustX's Regenerate sits where Harness seats extra actions.
Both run on the click from `App.tsx`, as `forkAt` does, instead of opening the
`CommandPanel` chooser; the duplicate in-Session Branch tail action, the
chooser's response-anchored rows and their copy are removed. `TurnTail.tsx`,
`CommandPanel.tsx` and `dictionaries/agent.ts` are rehashed in the inventory.

## Turn-rail reading window (UI/UX alignment)

Reference: `/home/caismis/Documents/codes/deepseek-harness@5badb15009ae1756c3afe0ae0cef1faafc290ccc`,
read-only. `ui-chat` `ChatView.tsx` and `use-chat-navigation.ts`, and
`session-controller` `Session.loadOlder`/`loadThrough`, were consulted; no new
upstream source is imported and no pin changes. The earlier contiguous
load-through adaptation is superseded by PR #453's native bounded reading
contract: direct anchor-relative windows, finite browser history, independent
live-tail authority and explicit Return-to-latest reset. A viewport gesture now
starts before outline I/O and fences both native window publication and the final
layout frame. The fixed-pitch Turn rail computes ordinal ranges arithmetically;
no total-sized mark array, index Map or virtualizer measurement cache remains.
`ChatViewport.tsx` and `TurnNavigator.tsx` retain the same source attribution and
are rehashed with the current dependency closure in the inventory.

## Conversation header and Trajectory chrome (UI/UX alignment)

Reference: `/home/caismis/Documents/codes/deepseek-harness@5badb15009ae1756c3afe0ae0cef1faafc290ccc`,
read-only. Consulted: `ui-conversation` `ConversationSession.tsx` and
`ConversationRoot.module.css`; `ui-open-in-app` `OpenTargetButton.tsx`/`.module.css`;
`ui-sidebar-right` `ExpandButton.tsx`/`.module.css`; `session-log-export`
`HeaderAction.tsx`/`.module.css`; `ui-trajectory` `TrajectoryToolbar.tsx`/`.module.css`
and `TrajectoryTable.tsx`/`.module.css`. Each adapted file records its exact
additional source in the inventory; the repository baseline is unchanged.

The header title is Harness's current crumb; the workspace opener is the compact
hairline split button; More, Inspector and the Previews toggle (the mirrored
panel glyph in the corner seat) are 28px icon controls; view tabs are plain
buttons, and the Chinese Chat tab reads 对话 as in Harness. The Trajectory
toolbar uses Harness's 32px strip, icon toggles and trailing search field;
Actual time stays visible as a rustX switch. Ledger role tags are label-only
until the ledger is narrow, the Turn label hangs from the Turn's first row and is
the only structural chrome (Steps have no chrome, seat or navigation target),
only the selected Turn draws its rail, and the promoted initial System row sits
outside that rail and names its change only. As in Harness, a Request is a
gutter dot on the boundary above the first output of its own Step; a Request
without such output keeps its own 10px seat, and folded Turns show no Request
dots. The timeline's model span uses Harness's TTFT/decoding violets. The detail
header shows a role tag, the native Turn/Step location and an icon close.
Every control except the boundary dot sits inside its row.
Menu now keeps the trigger it captured on open until the next open, so after
a selection the keyboard returns to the split button's chevron rather than the
anchor's first button; a menu opened from outside its anchor closes onto the
anchor's `aria-haspopup` button. `Menu.tsx` is rehashed in the inventory. The
Trajectory ledger anchors a prepend on its first fully visible seat.
The search field keeps Harness's stroked hairline on its own compositing layer.
On the page layer it straddles a raster tile, so its rounded corners were
antialiased differently depending on which tile or partial repaint drew them:
12 of 30 fresh pinned-browser contexts differed (Δ1 on 7 pixels) in the focused
search capture, and other captures varied too. As its own layer it rendered
identically in 45 of 45 targeted replays.

Trace retained-input ownership repair updates generated imports to App Server v39.
Source inventory local hashes and dependency closure reflect this mechanical
protocol change; all Harness upstream pins remain unchanged.

New-conversation Sidebar visibility follows Harness `tree.ts` at
`5badb15009ae1756c3afe0ae0cef1faafc290ccc`: pin the current blank above history,
exclude blanks from search, and hide them when another Session is selected.
Harness reuses a native blank and marks it engaged on accepted input. rustX keeps
its existing deferred creation: a separate browser draft row has no Session ID,
timestamp, or native actions. Workspace selection moves this row without
resetting the input; leaving the draft removes its presentation. A native create
acknowledgement ends the draft, and the authoritative catalog supplies the real
row. Post-create admission failures retain their existing Session-scoped recovery
and are never hidden or deleted as abandoned drafts.

Trajectory overview alignment follows Harness `TrajectoryTimeline.tsx` at
`5badb15009ae1756c3afe0ae0cef1faafc290ccc`. Its two model colours represent
TTFT and decoding, not a reasoning-content boundary. The equal-width sequence
view now paints the measured TTFT/generation ratio without promoting numeric
metrics into wall-clock coordinates. Timed projections still require the native
provider bridge. Press/drag selection, hover cursor, outside-range dimming,
double-click clearing and zoomed edge panning retain native record identities
and projection-generation fencing. Exact selection edges persist until the
projection changes; changed coordinates reproject the selected native records.

The Workspace panel Start page adapts Harness `ui-sidebar-right`'s GuideBody
and its capsule styling at `5badb15009ae1756c3afe0ae0cef1faafc290ccc`. The
upstream slot registry and filesystem/terminal Host are not imported. rustX's
Product Host resolves the exact native Session/node before browsing or PTY
admission. Existing preview occurrences, resource leases and Inspector ownership
remain in `PreviewWorkspaceOwner`; the new workbench is a separate display mode.

Workspace sidebar fidelity repair: the Start page now retains GuideBody's 380px
capsules, 14px/20px insets, 56px compass, neutral watermark ink and 10% bottom
spacer. Original guide artwork, fullscreen/restore glyphs and TerminalTheme OSC
palette handling are retained with per-file provenance. The upstream DockLayout
38px strip replaces the extra title/navigation rows; guide replacement, shell
selection-and-launch, add/close tabs, keyboard navigation and shortcuts operate
on rustX Session-scoped PTYs. The two-pane presentation now uses the upstream dockkit engine, planner,
geometry, gesture handlers and component tree. It does not import Harness's
runtime or plugin store.
Browser checks cover guide geometry, light/dark terminal backgrounds, actual
shell output, focus, tabs, split/merge/resize, shortcuts and a Chinese narrow view.

### Workspace file tree and source tabs

The file browser and viewer use the Harness `5badb15009ae1756c3afe0ae0cef1faafc290ccc`
`ui-sidebar-files` FilesBody/store, document TextPreview/CodeBody, PathLabel,
FileTypeIcon/CodeFileIcon artwork, CodeToolbar/CodeCard, and code-language table.
`source-inventory.json` records each immutable source and adapted dependency.
The 38px path toolbar, 18px tree indentation, natural folder-first ordering,
separate resource tabs, persisted tree/view preferences and icon toolbar follow
those sources. Harness remotes, stores and Session authority are excluded: rustX's
existing Product Host admits every listing/read/desktop open against the native
Session and uses descriptor-relative filesystem validation. The 1 MiB text limit remains. Complete binary previews have a separate 16 MiB
Host read limit; Office conversion retains its existing stricter source and
sandbox limits.


### Sidebar document and docking source port

The same upstream commit supplies `ui-dockkit` (engine and presentation),
`ui-sidebar-right` layout persistence and shell CSS, document preview registry
matching, Markdown frontmatter/field rendering, static HTML document preparation,
image zoom controls, and the complete read-only Excel parser/rendering pipeline.
The copied dockkit behavior tests remain alongside the port. Changes at seams:

- React 19 ref initialization and existing rustX primitive/icon/locale bindings.
- The embedder uses upstream `planSettle` to merge emptied panes and reseed the
  guide; files deduplicate by workspace-relative path, Files pages by pane.
- Metadata-only layout snapshots are validated before restoration and scoped
  to the exact Product Host authority and native Session/node. Terminal tabs
  reconcile against Host-owned PTYs, never start a process by restoring metadata.
- Hidden document bodies release reads, object URLs and parser workers. Live
  native terminals retain their Host lifetime and reattach when selected.
- The copied spreadsheet worker uses Vite's module-worker bundling. An absent
  XLSX default row height remains absent instead of becoming `NaN`.
- Markdown documents opt into Host-admitted relative images and file navigation;
  conversation Markdown's inert image policy is unchanged.
- HTML copies the upstream packer, bootstrap and opaque `allow-scripts` iframe.
  Inline interaction and direct local classic JS/CSS work through scoped Host
  reads. The iframe receives no Host RPC, parent storage or same-origin access.
  General settings can select the upstream inert/static mode. Module graphs,
  CSS dependency traversal and dynamic relative fetch are not supported by DSH's
  packer either. PDF display retains rustX's bounded PDF worker owner.
- DOCX/PPTX retain OOXML admission and native source reauthorization. Linux keeps
  its existing cgroup/Bubblewrap converter. macOS uses DSH's pinned
  `@deepseek-ai/libreoffice-kit@0.1.5` native engine in a clean-environment
  Seatbelt process group: private input/output, dependency and system-font reads,
  no network or user-home access, CPU/file/descriptor and output/deadline limits.
  The native engine keeps DSH's 60-second conversion budget. Its bounded 256 MiB
  font staging is separate from the unchanged 4 MiB published PDF limit.
  macOS does not claim Linux's cgroup memory/task limits. Missing sandbox or
  engine fails closed. Legacy DOC/PPT remain outside the OOXML admission format.
  The package includes native engine licenses and source/build information under
  MPL-2.0; its optional platform payload must accompany the Host installation.


Browser coverage includes default Markdown versus explicit source/text, YAML
frontmatter, GFM tables/math, workspace-relative images and links, static HTML,
SVG, PDF, CSV/XLSX, light/dark terminal rendering, native filesystem/PTY actions,
source DockLayout floating/docking/collapse/restoration and narrow Chinese UI.
Install-closure notices include Node-only ExcelJS dependencies as well as browser
modules. The upstream `buffers@0.1.1` distribution has no published license
statement; that omission is recorded explicitly, not replaced with an inferred
license. The ExcelJS browser entry does not import that Node-only archive reader.

Conversation file references use Harness `ui-primitives/src/markdown/file-link.ts`
and the `MarkdownFileLink` delegate from `render.tsx`, retaining its URI decoding,
line fragment grammar, settled-message activation and existing file-link CSS.
The rustX Session owns the delegate for Conversation and Trajectory previews;
Product Host resolves references within the admitted native workspace with
no-follow descriptor traversal. References open the existing Workbench dock
content identity rather than synthesizing an Artifact or committed delivery.
Document-local resource links retain their own base path and resource owner.

Trajectory prompt inspection now follows Harness `TrajectoryTable.tsx`'s explicit
missing-system-prompt state and direct Markdown payload. Frozen rustX request
detail remains the sole source; current configuration never fills history.
The timeline includes projected SystemPromptCell and ContextRow input entries,
with independent display keys and their original request detail owner. Sequence
mode places the initial prompt first; timed modes use zero-duration markers at
the owning request snapshot, since no separate input clock is recorded.

### Trajectory detail presentation alignment

The detail facet matrix now follows the pinned Harness `TrajectoryTable.tsx`
`SYSTEM_PROMPT_TABS`, `SYSTEM_UPDATE_TABS`, `REQUEST_TABS`, and `detailTabs`:
prompt/catalog cells no longer inherit request summary/native tabs; request
markers expose summary/options/usage/timing; messages and individual context
rows expose summary/preview/raw content; tool source replaces the redundant
input tab. Raw content is message text, not the protocol message envelope.
Overview sections link to their corresponding facet, reasoning remains folded
inside the message preview, and attachments stay with their content. Context
reads still use the immutable native request owner but render only the selected
message ID. rustX-only lifecycle records retain native evidence inspection.

### Trajectory Harness parity: details, overview and ledger rows

Reference: `/home/caismis/Documents/codes/deepseek-harness@5badb15009ae1756c3afe0ae0cef1faafc290ccc`,
read-only: `ui-trajectory` `TrajectoryTable.tsx`/`.module.css`,
`TrajectoryTimeline.tsx`/`.module.css`, `TrajectoryToolbar.tsx`, `locales.ts`,
`trajectory-preview.ts` and `ui-primitives/src/markdown/plain-text.ts`.

The details panel is ported from Harness's: role tag and Turn · group location
(or `Request #N`), Harness tab sets per kind, the Summary fact list with source
and hierarchy links and token rows, titled overview sections that open their tab,
the usage (this request / session cumulative), timing, tool catalog, prompt diff,
source-block and program views, and Harness's own copy in both locales. rustX's
Native tab, explanatory notes, inspection-bound notices and Turn-structure
inspector are removed; native facts that Harness has no view for (Agent identity,
activation origin, exit code, managed-output locator, retry ordinal) are no longer
shown in the panel. A Diff still refuses truncated or missing prompts. The panel
uses Harness's resize handle and narrow overlay; `react-resizable-panels@4.12.4`
is no longer a dependency and its notice is removed.

The overview is ported from Harness's: a 44px lane-label column beside a track
that clips its projected domain (zooming or panning no longer draws over the
labels or past the edge), a delayed tooltip per block naming its role, recorded
range and timing, Harness's minimum selection, edge pan, hover line and turn
boundaries. The Model block shows TTFT then decoding, placed by the native phase
bridge. rustX's keyboard zoom/pan, hover hint line and the visible Actual time
switch (hidden in Harness) are removed with the `time`/`actual` projections.

Ledger rows drop rustX-only chrome: the Turn fold caret and selectable Turn
label, the per-row Collapse Calls button, lifecycle words and truncation marks.
As in Harness, double-clicking a Turn's opening row or an Assistant folds it,
and the folded summary row expands it; a tool-only Assistant reads (tool call
only), Context rows show their content, and row text is a plain-text projection of
the Markdown preview (`preview.ts`). The toolbar's Jump to latest control and the
client `latestTrace` read it drove are removed.


Manual reading highlight continuity was checked against local Harness
`5badb15009ae1756c3afe0ae0cef1faafc290ccc`, specifically
`use-chat-viewport.ts::readVisibleTurn` and `use-chat-reading.ts`.
ChatViewport now selects the preceding visible native turn region across prompts,
nested message anchors and gaps, or the first loaded region before its start.
Tail following selects the last native region even after unowned trailing content.
This adapts Harness's nonempty reading candidate policy without its approximate
binary-search candidate selection: native turn identity and exact locate anchors
remain separate, and a window containing no native region still has no selection.


Terminal model-error feedback follows local Harness at
`5badb15009ae1756c3afe0ae0cef1faafc290ccc`:
`MessageItem.tsx::TurnErrorItem`, its `MessageItem.module.css` error-row styles,
`locale.ts`, and `conversation-nodes/turn-error.ts`. The error dot, inline title,
secondary diagnostic and code are retained. Authentication uses Harness copy.
Native Attempt terminal evidence supplies normalized failures in transcript pages;
the browser neither parses provider text for classifications nor starts retries.
The row remains at the terminal cursor after partial output, later turns and
reconnection. Manual cancellation is still Stopped, without an error row.


Timeout retry presentation follows Harness `MessageItem.tsx::ModelRetryItem`,
`MessageItem.module.css`, `conversation-nodes/retry.ts`, `locale.ts` and
ui-primitives `TextShimmer` at `5badb15009ae1756c3afe0ae0cef1faafc290ccc`.
Only Web presentation consumes existing Trace summaries and explicitly disclosed
request detail. Exact native Attempt/Step/request/predecessor identities determine
which timeout and subsequent request are shown. Scheduled delays, retry ceilings
and pending retry cancellation are not exposed by this API and are not invented:
there is no countdown or maximum; a timeout alone is not labelled an active retry.
Actual started retries shimmer while attached and running. Settled or disconnected
observations stop the animation. No runtime, protocol or retry policy is changed.

Retry placement was rechecked against the same Harness revision's
`conversation-nodes/retry.ts`, `conversation-nodes/process-groups.ts` and
`chat-branch-tails.client.spec.tsx`: completed retries remain independent durable
rows with collapsed failure disclosures. rustX now seats each Step's retry before
its first retained native request publication, instead of collecting the whole
Attempt's retries beneath its completion summary. No-publication requests keep
their exact Attempt feedback seat. Retry boundaries close the preceding process
group so later reasoning and Tools cannot fold ahead of the independent retry.
Native MessageIds determine placement across
streaming, canonical settlement, clipped history and reattachment; neighboring
messages supply no ownership. Disclosure state follows the native retry chain
when its publication changes render seats, without automatic detail reads.


Appearance preferences follow local Harness `ui-theme` at
`5badb15009ae1756c3afe0ae0cef1faafc290ccc`: `AppearanceRow.tsx`,
`AppearanceRow.module.css`, `theme-settings.ts`, `locales.ts` and `boot-theme.ts`.
Light, Dark and System are three icon-over-label cubes, preserving preference
selection separately from resolved colors. The browser defaults to System and
owns one media-query listener only while that preference is active; explicit
Light/Dark preferences remain persisted overrides. The App applies the resolved
palette before its layout is painted and releases the listener on replacement.
Host theme storage and plugin lifecycle are not imported.

Sidebar recency was compared with local DeepSeek-harness commit
`5badb15009ae1756c3afe0ae0cef1faafc290ccc`, specifically
`packages/client/ui-workspace/src/client/tree.ts` (`orderByRecency`) and
`packages/api/session-controller/src/list.ts` (`applySessionListMetadata`, `updatedAt`).
The browser displays native Session summaries newest first with identity tie-breaking,
including grouped, flat, and search views. Native `SessionSummary.updated_at` now
projects the later of creation and committed ordinary human-message time, matching
Harness semantics, while `SessionSnapshot.updated_at` retains metadata semantics.
The native catalog persists `last_prompt_at`, sorts before pagination, and emits
catalog invalidations on activity so unseen Sessions can enter the current page.
Startup, composition, and storage recovery derive missed activity from the durable
message ledger; no browser clock, focus event, rename, or model output moves a row.

Connection-wait submission retains Harness's resident composer and one primary
submission gesture (local `5badb15009ae1756c3afe0ae0cef1faafc290ccc`,
`packages/client/ui-conversation/src/client/skeleton/InputBar.tsx` and
`contract/composer-submission.ts`). RustX owns the local pending draft while its
existing attachment prepares resources. The generic connecting notice does not
infer MCP status. Only the completed native attachment permits upload and turn
admission; navigation/transport replacement never replays the pending gesture.

Quiet connection correction: keep the Harness resident chat/composer layout and
render the locally submitted user bubble plus generic waiting feedback inside the
conversation. Opening alone has no connecting notice. App Server v39 adds the
read-only `session/history` window so cold stored history can render independently
of resource preparation; this is a native durable read, never a fabricated runtime
snapshot. Both clients fence late history reads against the current attachment.

Model menu layout repair: compared the local DeepSeek-harness `packages/client/ui-model-selection/src/client/ModelSelect.tsx` in-menu loading/error presentation. The rustX adapter keeps catalog-loading and unavailable-selection notices inside its existing portaled menu, so opening it does not resize the Composer or history viewport. Pending root rows are disabled and initial keyboard focus waits for usable choices. Native model mutation/read authority and the pinned source baseline remain unchanged.

Trajectory bottom fade: compared local Harness ConversationRoot.module.css,
TrajectoryView.tsx, views.module.css and TrajectoryTable.module.css. The native-bound
WebUI view now elects the resident composer overlay, retaining the fixed 36px
bg-base fade. ConversationSeat publishes its measured height to its body; ledger
and detail scroll areas reserve that height plus 16px, as Harness does. WebUI's
Chat does not reserve an outer scrollbar gutter, so the overlay keeps the same
full-width seat instead of applying Harness's outer-scroller gutter compensation.

Chat bottom-follow repair: reviewed local Harness ui-chat/chat/use-chat-scroll.ts,
use-chat-reading.ts and use-scroll-follow.ts at 5badb15009. Reader movement takes
precedence over layout/follow work. WebUI retains its one-frame anchor-based
scroll owner: upward motion exits following even within the bottom tolerance,
queued corrections sample native movement before writing, and explicit latest
or navigation actions acknowledge earlier scrolling. No timer or second scroll
writer is introduced; returning downward to the floor restores following.

Composer add menu (2026-10-08): compared local DeepSeek Harness `5badb15009`
`ui-input-trigger/MenuView.tsx`, `MenuView.module.css` and `ui-conversation/skeleton/InputBar.tsx`.
Adapted grouped, full-width icon/name/alias/description rows to RustX's existing
command availability and dispatch; file intake is a launcher action, not a native
slash command. Menu scrolling is confined to its own viewport. Existing Harness
icons are reused; missing fork/tools icons use Lucide 0.468.0 `git-fork.svg` and
`wrench.svg` (https://github.com/lucide-icons/lucide/tree/0.468.0/icons).
The upstream ISC/Feather MIT notice is shipped in `public/LICENSE-Lucide.txt`
and reproduced in `THIRD-PARTY-NOTICES.txt`. No runtime dependency was added.

## Dedicated MCP settings

Behavioral and layout reference: local ZCode
`packages/ui/src/settings/{McpSettingsSection,McpServerForm,McpServerList,SettingsResourceHeaderActions,mcpSettingsShared}.tsx`
(the shared configuration helper is `.ts`). rustX retains its native source
transactions and supports its own MCP fields rather than inventing unsupported
ZCode protocol options. No bundled ZCode servers are included.

The MCP glyph reuses the exact Lucide `Cable` path data from `lucide-react@1.17.0`,
the version pinned by ZCode's lockfile:
https://unpkg.com/lucide-react@1.17.0/dist/esm/icons/cable.mjs
The ISC license is shipped in `public/LICENSE-Lucide-1.17.txt` and the generated
third-party notices. Only the React SVG wrapper is local.


Child statistics now use App Server v39's native `agent/statistics` reading.
The Harness SubagentHeaderLineage token/duration columns accompany the existing
state rows; the child reader reuses the main conversation's statistics dock,
context meter and completed-response presentation. Native event folds retain
request usage coverage, generation evidence and activation timing. Browser code
only formats those readings and interpolates explicitly running intervals.

MCP settings presentation now follows Harness `ui-settings-models`'s
`ModelsSection.module.css` at `5badb15009ae1756c3afe0ae0cef1faafc290ccc`:
compact headings, outlined setting cards, module-filled editor and disclosure
separators. Geometry and controls use rustX's existing Harness-derived settings
vocabulary, including panel container queries and shared capsule buttons.
ZCode remains the reference for scope-local management behavior and MCP glyph;
no runtime/configuration behavior is changed by this presentation adaptation.

MCP scope switching follows ZCode `PluginScopeMenu.tsx`: a user option followed
by a separator, workspace group label, folder icons and a selected check.
The existing Menu primitive provides keyboard/portal behavior. MCP's selected
configuration owner is local to the page; the settings shell retains its
original target and navigation capabilities. All reads and edits continue through
the exact-target native settings actor.

PR #457 repair retains the scope-local list and Harness settings primitives, but
routes both definition entry points through the existing `McpDefinition` and
Settings transaction actor. JSON import only accepts environment references;
Header/Env literals are retained by key or removed, never introduced by Web
configuration writes. Configuration inspection remains read-only. The native capability lifecycle
owns Session MCP connections. The MCP list now invokes finite native diagnostics
as described below. Meter scheduling remains separate from native durable folding authority.
The client owns two current observations and a total budget of four unconfirmed
reads across scope changes. Current inventory plus a fair cursor replaces the
lossy 32-entry pending queue; selected demand gets alternating priority. Exact
activation/revision fences guard admission and publication. Unknown transport
settlement keeps its charged slot across reconnects, with an explicit unavailable
state if the bounded retirement allowance is exhausted. No native cancellation
or durable statistics authority is implemented in the browser.
The client commits attachment observation authority only at native attach
acknowledgement. Release clears it synchronously; queued Open and a retained old
target cannot recreate it, including after detach failure. Both dispatch and
publication consume the same client-owned admission proof. A
never-dispatched capacity refusal remains deferred inventory demand. Only the
request pipeline's newer capacity cut resumes admission; native errors remain
terminal and unknown transmitted outcomes remain charged. These are local
observation rules, with no native protocol or statistics-folding changes.

Repair browser acceptance follows the new Settings page order, the shared slash
model picker, and the actual `data-conversation-scroll` owner. Agent, Shell,
Settings and localized General screenshot references are refreshed for the PR's intentional
reserved scroll gutter, lineage header, running-status seat and new conversation
preferences. The strict comparator is unchanged; reference-local raster evidence is described below. A hidden
StepGroup title now stays hidden in detailed mode, and stream-to-canonical
message seats retain their disclosure identity instead of remounting.

Settings preference and resource-catalog secondary text uses the existing
readable label tokens after browser contrast checks; invalid definitions keep
a readable label with an error-colored underline. Skill visibility retains its
selection-versus-filesystem explanation. The four updated Shell references
with registered corner-arc noise retain every historical evidence pixel within
its recorded alternatives; their regions and budgets are unchanged.

Composer screenshot fixtures explicitly establish the latest-reading position
before capture: an earlier control click can otherwise leave a half-pixel
scroll offset. Dimensions remain exact. Four narrow light-theme references
have new measured raster evidence: repeated pinned-browser captures retain
byte-identical DOM geometry and computed paint styles, while shadows differ by
1–3 channel levels and eight attachment-corner pixels differ by up to 7.
The manifest contains raw pixels, capture hashes and exact row spans; tests
reject changes at every adjacent unmeasured pixel. Other references remain
strict, and neither a product CSS workaround nor a global tolerance was added.

Automatic model catalog preloading now uses the attached native snapshot for
the current selection. Only an outstanding model mutation or failed intent
requires model/snapshot reconciliation; merely mounting the control performs
no snapshot repair. Existing write acknowledgement and reread fencing remain.

Hosted final-head browser evidence exposed a second scroll offset on the active
Session shell: `overflow: hidden` allowed its scrollTop to become 11, then
fullscreen restoration reset it to zero and displaced the reading anchor. The
shell now uses `overflow: clip`; ChatViewport remains the only reading scroll
owner. A controlled overflowing descendant proves the shell rejects scrollTop
11 (the prior style fails with 11), followed by the unchanged exact reading
anchor assertion through fullscreen restoration and stream settlement.

PR #457 attachment-admission repair also measures the dark 390px Composer
attachment reference. Two CI captures and two local stable captures share the
same eight changed corner pixels; two further local captures equal the reference.
All four local captures have identical descendant/ancestor geometry and computed
paint styles. The reference-specific manifest admits exactly those eight pixels
with individual measured channel bounds. Adjacent pixels, larger deltas, layout
and dimension changes still fail. The reference image and comparator are unchanged.
The same isolated dark Composer probe also exposed a stable Context gradient
variant and model-label raster baseline variant at identical measured geometry
and styles. Their exact row spans are separately recorded. The browser test binds
this reference to the exact model/profile text, relative rectangles, font metrics,
colours and transform; real text or layout changes cannot use that allowance.

## #454 Present call row and delivered-file cards

Source-derived from local `../deepseek-harness` at
`5badb15009ae1756c3afe0ae0cef1faafc290ccc` (MIT, Copyright (c) 2026 DeepSeek):

- `packages/client/ui-deliverables/src/client/PresentRow.tsx` and
  `PresentRow.module.css` → `src/presentation/agent/PresentRow.{tsx,module.css}`:
  the present status row, declared-path summary and expandable result text over
  the shared `DisclosureRow`.
- `PresentedFileCard.tsx`, the presented-files grid of `Deliverables.tsx`, and
  `Deliverables.module.css` → `src/presentation/attachments/PresentedFileCard.tsx`
  and `Deliverables.module.css`. These provide the whole-card Preview gesture, the
  filename/description hierarchy, the file-type glyph, the separate action slot,
  and the one-row or two-column four-card summary with its toggle.
- `IconDeliverDocRegular` artwork (`ui-primitives` icons) → `IconDeliverDocOutline14`.
- `locales.ts` present/presented wording, en/zh → `tools` and `artifacts` dictionaries.

Excluded: Cordis, `ToolCallViewProps`, DSH Session events and turn-deliverables,
the Host open/reveal controller and desktop metadata, workspace path resolution,
and the Inspect pill. `src/bindings/present.ts` is the rustX-authored adapter. It
maps native foreground lifecycle to Harness phases and maps only a successful
committed Tool message's typed `deliveries` to cards. Preview and Download stay
with the existing PreviewWorkspace and original-byte owners. Inventory hashes and
dependency closures describe this final source, including the v39 import renames.

## UI optimization: MCP screenshot alignment

ZCode `29628c9acdb81b703bbd4080c207a0e7ce5e276e` remains the interaction
reference for the scope-local list, form/JSON mode switch and optional fields:
`McpServerForm.tsx`, `McpServerList.tsx`, `McpSettingsSection.tsx`,
`SettingsFormActions.tsx`, `PluginScopeMenu.tsx`, and `mcpSettingsShared.ts`.
Existing definitions have a fixed scope; a returned list keeps its own selected
owner. Mode switching and saves retain the native CAS transaction semantics.

Presentation follows Harness `5badb15009ae1756c3afe0ae0cef1faafc290ccc`:
`ModelsSection.module.css`, `SettingsRoot.module.css` and settings-form fields.
The MCP list and editor inherit the shared settings palette, with 20px page
headings, 13px fields, 36px inputs, outlined list cards, a module-filled editor,
shared capsule buttons, chevron disclosures and a sticky Save/Cancel row.
No MCP-only dark palette or list/editor background override remains. Status
indicators use theme state tokens; Refresh animates during a check and honors
reduced motion. The editor includes a return-to-list control; long endpoint or
command descriptions expose their full text on hover.
No ZCode runtime or component implementation is copied.

The generic unit shell, argument array rows, working-directory/advanced fields,
retained-secret checkboxes, redundant form explanations and list permission
metadata are absent from the dedicated MCP surface. JSON shows the named
server definition, not the internal transaction wrapper, and accepts both
named objects and `mcpServers` objects. A JSON save submits one identity through
the existing native CAS transaction and returns after its authoritative read.
Unexposed native retention metadata remains attached to edits of the same
identity. Partial tool selections keep their list switch disabled rather than
replace native grants or expose an additional selection-editor button.

The user explicitly chose to keep rustX's native rules: no timeout,
protocol-version or literal-credential authoring controls were added. Optional
environment/header JSON therefore accepts `$VARIABLE` references. Existing
native cwd and secret retention survive ordinary form edits. Invalid field
buffers block Save and JSON switching, and new drafts survive renaming without
writing unnamed or duplicate identities. The single arguments field uses
`shell-quote` 1.12.0 for reversible argv formatting; it never launches a shell or
expands environment variables. Its MIT notice is included in generated notices.

Validation includes editor/transaction regressions, desktop and narrow browser
interaction/geometry checks, and local screenshots of all four Chinese states.
The browser fixture exercises the real UI against deterministic source
responses; it does not claim a live MCP server connection or pinned-container
pixel-baseline equivalence.

MCP connectivity indicators retain ZCode's row presentation and existing Refresh
control (`McpSettingsSection.tsx`, `zcode-protocol/mcp.ts`). rustX requires an
explicit per-definition Test connection action; Refresh only reads configuration.
rustX uses
finite `mcp/probe` operations instead of ZCode's retained `mcp-status` process
connections. Green means the authored configuration passed its last handshake
and tool-list check; tooltips distinguish it from a live Agent connection.
Source revision, target and transport identity fence results. Workspace probes
use the existing Product Host registration lane; no browser path grants authority.


Composer growth repair (2026-10-10): inspected local Harness `5badb15009`
`ui-conversation/src/client/skeleton/ConversationContent.tsx` and
`ui-chat/src/client/chat/{use-chat-viewport,use-chat-reading,use-scroll-follow}.ts`.
The shared sticky composer, seat ResizeObserver and independent tail/reading
intent remain intact. rustX's native textarea now uses `field-sizing: content`
instead of collapsing its height to measure each edit. ChatViewport acknowledges
browser caret scrolling during composer input without treating it as a reader
gesture; pre-input sampling retains genuine prior reader movement. The frame
remains the sole owner of automatic scroll corrections. Harness's Lexical editor
and its transport are not imported. Regression checks use actual multiline
keystrokes, pasted capped drafts, shrinking, width reflow and detached reading in
the production App with a fixture transport, at desktop and mobile widths.

Subagent UI alignment (2026-10-10): inspected pinned local Harness
`ui-subagent/src/client/{SubagentHeaderLineage,SubagentReadOnlyComposer}` and
its client entry point and sidebar-chat integration. The header reuses the
sibling-switcher SVG and compact metrics presentation; native parent identities
supply ancestor and direct-child navigation. Hover opens after 150ms; the existing
shared Menu supplies its 200ms pointer-leave grace and keyboard/outside dismissal.
Visited native child views retain drafts and reading positions. Their input uses
the already inventoried conversation composer styling, shared sticky-seat height
observation, Enter/Shift+Enter and IME handling, native interrupt admission, and
real child statistics. Agent return events keep their native activity rendering.
No Harness session controller, Lexical editor or sidebar runtime is imported.
Native rustX does not expose child model mutation, independent child trajectory,
or Harness's explicit one-shot mode projection, so those controls are not inferred
from parent configuration. Browser validation exercises the production App with
fixture agent identities and statistics, not a live model run or pixel-baseline
claim.


Child trajectory completion (2026-10-10): Harness subagent conversations reuse
its ordinary conversation/trajectory view. rustX now exposes the same existing
Trajectory presentation and composer geometry for children, backed by native
agent/trace and agent/traceDetail over each child's durable store. The existing
bounded TraceCache owns paging, lifecycle repairs and detail epochs; a child
reader fences attachment replacement and coalesces activity refreshes. No parent
trace records or current parent model configuration fill child data. Native
child Attempt identity supplies exact current lifecycle correlation. The source
revision remains pinned; App Server v39 / Runtime Client v61 require synchronized
clients. Browser evidence uses fixture records; the real-process native suite
separately verifies durable child authority and read-only behavior after exit.

### Streaming context occupancy (2026-10-10)

Studied Harness `packages/llm/token-meter/src/usage-projection.ts`'s
`contextPressureProjectionDefinition` and the composer `ContextMeter.tsx`.
Harness retains the prompt-side measurement while a request streams, replacing
it when a newer usage sample arrives. RustX now retains the last native measured
request across unfinished and usage-free requests, with that request's frozen
model/capacity and composition. Compaction still invalidates the old reading.
This is a shared Rust Context owner change, consumed by WebUI, TUI and child
statistics, without presentation-local caching or estimated streaming tokens.
Browser plugin not available; regular Playwright verifies the production App's
context ring during running snapshots, subsequent measurement and invalidation
on desktop and mobile. Reference pin unchanged.

### Subagent call details and input roles (2026-10-10)

Studied Harness `5badb15009ae1756c3afe0ae0cef1faafc290ccc`,
`ui-tool/src/client/tool/{components/ToolDetails,models/control-details-model,toolviews/details-row}`
and `ui-subagent` composer/lineage. The expandable result caption, bounded list,
divided rows, status placement and explicit raw inspection follow that reference.
`AgentToolDetails.module.css` adapts its ToolDetails styling. The rustX-only adapter
reads recorded native ToolResult JSON: create and message operations display
receipts, lists retain recorded Agent states, and waits display their captured
activation outcomes. The live roster supplies navigation only. Manual refresh and
wait controls move to the child header; the input retains its native send/interrupt
controls and the shared composer geometry. At that revision the child model/permission controls were absent. The child capability update below replaces the committed-only transcript, input and preview paths with native ownership.
File cards without a Preview owner show metadata with a disabled preview action,
so absence of native access is not presented as a working click target. Independent Trace and statistics use their existing native read domains.
Native adopted inputs now distinguish human messages from runtime/Agent/mixed
context; late context keeps its chronological position within its native Attempt.
Browser tests use the production App and fixture transport at 1440 and 390 pixels.
The Browser plugin is unavailable in this session, so verification uses the
project's installed Playwright. Screenshots are fixture evidence, not a live model run.

### Native child capabilities (2026-10-10)

Studied Harness `tool-subagent` description/prompt separation and `ui-subagent`
`sidebar-chat/index.tsx`, which retains a child Session and reuses the full
conversation/input component. Also inspected ZCode's `SubagentSessionSidePane`
and Codex's App Server thread subscriptions. The implementation keeps rustX
execution authority in Rust: task titles are frozen and persisted at creation;
child reads reuse the child's complete Runtime Client snapshot/cursor endpoint;
active and inactive input carries native Session upload references through IPC;
file/artifact access resolves the owned child conversation and workspace.

WebUI reuses AgentComposer and AttachmentIntake, scopes both conversation and
trajectory preview resources to the Agent, and retires native observation reads
when navigation changes. No child model settings are introduced. App Server v44,
Runtime Client v66, child IPC v30 and Product Host file-read v3 are synchronized.
The Browser plugin is unavailable; the repository's pinned Playwright browser
validates real native child processes and deterministic provider streams.


Subagent catalog and sidebar audit (2026-10-10): inspected the pinned Harness
`ui-subagent/src/client/SubagentHeaderLineage.tsx`, its CSS, `SubagentReadOnlyComposer.tsx`
and `sidebar-chat/index.tsx`. The new inventoried SubagentCatalog ports the tree
hierarchy, disclosure alignment, current-title emphasis, two-row usage/duration,
150ms hover-open / 120ms hover-close with click pinning, focus restoration, tree
keyboard navigation, and a separate sidebar-chat button. Native parent IDs supply
all membership and ancestry; no upstream Session controller or transport is retained.
The existing DockLayout hosts resident child chats, including hidden-tab drafts,
with visibility scoped native watchers. The main child trajectory remains mounted
across conversation-view changes to preserve its filter and detail selection.
Native source identity and independent title label inbound agent messages, with
explicit parent-source context in child conversations. Unavailable agents receive
a read-only explanation while retaining their inaccessible draft for native repair.
No child model controls or one-shot mode are inferred. Whole-conversation child
turn-outline / locate APIs remain absent, so the main turn navigator is not projected
from a partial transcript. Source hashes and dependency closures are refreshed.

The existing base theme now retains Harness radius tokens (4/8/12/16/20/28px);
their missing definitions had invalidated catalog and other source-port radii.

### Child turn directory and history positioning (2026-10-10)

`TurnNavigator.tsx` now exposes the same Harness-derived rail presentation for
parent and child conversations. The native child reader and `SubagentChat`
composition are rustX-owned: `agent/turns` supplies the complete paged directory,
and `agent/transcript` resolves native Turn identities at child-owned read cuts.
The child watch remains independent of the bounded historical reading window.

## Composer layout inside shrinking conversation panes

Reference: Harness `5badb15009ae1756c3afe0ae0cef1faafc290ccc`,
`ui-conversation/src/client/skeleton/{InputBar.module.css,control-row-layout.ts}`,
`ui-model-selection/src/client/ModelSelect.module.css` and
`ui-permission-presets/src/client/PermissionSelect.module.css`.
The shared composer restores an anonymous inline-size control-row container,
row-relative model width, 560px compact spacing and 460px permission-label cut.
Harness's expanded-demand observer collapses model text only when the controls
cannot share a line, reacting to pane, content and font changes without React
state. Resize deliveries coalesce in an animation frame to avoid changing
observed child sizes inside their own resize delivery. Full labels remain available through trigger titles. Model and permission
menus already portal to the document, so the obsolete containment override and
viewport-based model wrapper caps are removed. Existing sticky seat measurement
continues to reserve draft, toolbar and statistics height for main and child chats.
The rounded card uses an explicit zero transform for its raster layer; native
selector menus are document portals and editor menus are card-relative.
The existing ChatViewport layout capture also identifies scroll overflow. Only
an overflowing composer's sticky seat owns a local compositing plane, keeping
its fade independent of Chromium's earlier scroll/paint promotion while
non-scrolling seats retain their normal raster. Narrow light/dark browser
regressions invalidate the fade in the sticky pose and compare exact pixels
after returning to the same reading pose; references and noise policy are unchanged.

## Single-row composer statistics

Reference: Harness `5badb15009ae1756c3afe0ae0cef1faafc290ccc`,
`ui-conversation/src/client/skeleton/InputBar.module.css` dock and
`ui-chat/src/client/chat/StatsPills.module.css` labels. The shared main/child
statistics dock uses the same non-wrapping centered row and 12px spacing.
Activity and usage labels can shrink with ellipses while the context meter
retains its ring and full percentage. Accessible names and click-open panels
preserve complete native readings at narrow pane widths. No statistics,
occupancy or streaming authority changes.

## Compaction checkpoints and occupancy

Compared with Harness `5badb15009ae1756c3afe0ae0cef1faafc290ccc`:
`ui-chat/src/client/chat/CompactionItem.tsx`, `CompactionCommandCard.tsx` and
`MessageItem.module.css`, plus `ui-conversation/src/client/skeleton/ContextMeter.tsx`
and `llm/token-meter/src/usage-projection.ts`. Manual checkpoints show `compact`
and localized replaced-history counts with an approximate token price. The whole
row uses the tertiary tone and transitions to secondary on hover, retaining the existing
summary disclosure, sticky heading and uncapped Markdown body. Native transcript
metadata resolves the canonical replacement span even in historical pages and
copied lineage; the browser never recounts visible rows or parses summary prose.
Pending/failed compaction preserves occupancy; success replaces it with the
committed primary context estimate. The panel marks that estimate with `~` while
reported cumulative billing remains unchanged. Native runtime ownership replaces
Harness projection/plugin wiring; obsolete protocol imports are removed.

## Tool detail surfaces and native Inspect navigation

Compared with Harness `5badb15009ae1756c3afe0ae0cef1faafc290ccc`:
`ui-tool/src/client/tool/components/ToolRow.tsx` and its stylesheet,
`toolviews/bash-sample.tsx`, `GenericToolCard.tsx`, and
`ui-primitives/src/TerminalBlock.tsx`, its stylesheet and `ansi.ts`.
The expanded Bash surface retains all returned lines, separate command/output
scrollers, 150px/224px bounds, the 11px code font with 18px minimum line seats,
16px radius, 4px flow indentation, sticky raw-output copy and copied feedback.
ANSI styles use upstream Anser 2.3.5 and safe React spans; source markup is text.
Cursor replay keeps only Anser-supported decorations and honors its SGR 21 bold
reset. All retained numeric tokens are normalized decimal values; indexed/RGB
components are integers in 0..255. Known color modes consume their full fixed
payload even on rejection; an unknown mode discards the rest of that SGR.
Rejected groups preserve the previous color. Both cursor and no-cursor paths
apply this policy before Anser. Each emitted opening is at most 52 characters,
independently of raw numeric-token lengths.
Generic IN/OUT sections scroll independently with a full-width divider. Business
icons remain stable on failure, summaries use native failure/stopped tones and
lifecycle announcements are visually hidden. Keyboard expansion and running copy
visibility follow the upstream flow.

The Inspect pill is pure presentation. App Server v44 / Runtime Client v66 native
readers locate the canonical occurrence across the complete indexed Journal,
including historical and exited child stores. RustX supplies bounded continuous
windows, exact record IDs and lifecycle repairs, replacing Harness runtime/store
wiring. The browser preserves that landing until Return to latest, loads exact
heavy detail, and fences attachment, navigation and visible-Conversation ownership.
Resident hidden chats suppress portal chrome, so their floating navigation cannot
escape into the active chat or Trajectory. Special Agent, present, question and
Goal rows use the same native Inspect authority.

Browser-plugin capabilities are not available in this session. Rendered QA uses
the repository's immutable Playwright 1.63 Linux image, fixtures on loopback port
5174 and the production preview on 5173. Light/dark tool detail views at desktop
and 390px verify independent scrolling, uncapped middle lines, exact full-output
copy, running transitions and keyboard expansion. Real native child-capabilities
QA verifies child and parent Inspect RPCs, selected rows, exact detail reads and
Return to latest; resident child Trajectory paging is checked at 1440px and 390px.
Screenshots are retained under `/tmp/rustx-tool-details-*` and
`/tmp/rustx-native-tool-inspect-{child,root}.png`.

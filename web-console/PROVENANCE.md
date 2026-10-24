## Background task presentation

Reference inspected read-only: local `../deepseek-harness` at immutable commit
`687f282e8625c6b8d11ff58211a90524be2e4317`, especially
`packages/client/ui-jobs/src/client/JobListAction.{tsx,module.css}` and
`packages/client/ui-tool/src/client/tool/models/terminal-card-model.ts`.
The header entry, viewport-fixed popup, active/ended groups and client-only Clear
follow that reference. The task row now follows its two-line live card, compact
settled row, square expansion/stop controls and command-copy terminal panel,
including the translucent MenuSurface material and the current tertiary-grey
ring loader (scoped to Jobs). Generic status/wait/cancel cards
and the retained-output path disclosure were removed from this menu. Only one
output panel expands at a time; expanding reads deferred native details, and
stopping uses the reference's two-press confirmation with a three-second expiry.
An accepted cancellation stays pending until native state publication. Settled
rows open by default when no live work remains; explicit folding wins. Native
Trace timing orders the groups and supplies elapsed time; absent starts produce
no synthetic timing. The native Jobs contract currently supplies bounded progress
and terminal result content, not Harness's live-output observation stream; progress
remains in the row and is never presented as shell stdout. TerminalBlock retains
explicit native lifecycle, with the reference's copyText and runStateDot options
for the menu's command-copy panel. No Harness job transport is introduced.

Chat background launches now follow the command-only settled presentation:
dispatch acceptance has its own label, command copy remains available, and no
exit outcome or output placeholder is invented. Failed launches retain their
ordinary diagnostic presentation. Historical Job controls and runtime terminal
notices share the native Job state labels and output formatting. Trace displays
acceptance separately from task settlement, retains the full native background
state vocabulary, and links a background record to its unique same-Step launch.
Harness job services, transport, event folding and model tools are excluded.

## Local Harness desktop presentation alignment

The installed DeepSeek Harness Windows desktop UI was visually inspected on
2026-10-04. The existing attributed presentation components remain the source
baseline; no brand assets, runtime code or new dependencies are imported.

- The application frame uses the reference's muted sidebar surface and rounded
  conversation panel. Browser chrome remains browser-owned.
- On narrow screens the expanded sidebar overlays the conversation, preserving
  the reading width; Escape or the backdrop dismisses it. Covered content is inert.
- Conversation title weight, default reading width and composer-context alignment
  follow the inspected desktop layout. Deliberate width preferences remain intact.
- The sidebar Extensions entry opens rustX's existing native-backed Settings
  extension inventory; it does not imply a Harness plugin runtime.
- General settings presents the existing Light and Dark choices as accessible
  theme cards, following the inspected desktop arrangement. The icons and CSS
  are locally authored; no unsupported preference is added.
- The settings modal follows the inspected 800x704 desktop geometry and plain
  dimmed backdrop; native configuration ownership and reload controls remain.
- Configuration, extension, upload and context copy uses user-facing language
  in both locales. General removes the duplicate appearance heading and the
  unrelated connection paragraph.
- Configuration cards keep source, revision and provenance details behind a
  disclosure. Invalid values and save outcomes remain visible; duplicate Tools
  group headings are removed while each form retains its own legend.
- The composer plus launcher includes file intake as its first action. Plus,
  paperclip and command glyphs reuse the already attributed Harness icon module.
  File selection keeps the existing upload owner and slash discovery remains
  command-only; the separate attachment toolbar button is removed.
- Chat/trajectory, Session actions, history navigation, attachment and command
  controls, approvals and settings retain their owners. The developer inspector
  and generic conversation collection browser have been removed.

The local hashes for adapted presentation files are updated in the
source inventory. The independent-session fork glyph additionally reuses Harness
`ui-primitives/src/icons/index.tsx` at immutable commit
`5badb15009ae1756c3afe0ae0cef1faafc290ccc`, recorded with its exact source hash
in the icon module's `additional_sources`. In-session branches retain the
original branch glyph; model and tools commands reuse the intelligence and code
glyphs. Existing source pins and dependency closures remain unchanged.

Model catalog refresh keeps the launcher enabled and visually stable while
fencing menu choices until the read finishes. First-load feedback lives inside
the popup; cached refresh does not add an inline loading label to the composer.

Updated screenshot references use exact pixel comparison. Noise evidence tied
to the previous layout is preserved with its original images in the comparator
fixtures, rather than reused as an exception for the new layout.
The narrow running-draft Composer references have newly measured rounded-shadow
noise at 15 light and 16 dark pixel sites. Each site has its own measured channel
bound; adjacent pixels, dimensions and all other references remain exact. The
manifest records independent runs, the pinned browser and decoded-image hashes.
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
`687f282e8625c6b8d11ff58211a90524be2e4317`, specifically
`packages/client/ui-chat/src/client/chat/{AssistantNodeView,AssistantMarkdown,TurnProcessNodeView,TurnTailNodeView}.tsx`.
Harness freezes released prose/reasoning on interruption, uses the stopped
process label and a quiet 11px/18px stopped tag after the partial content.
Stopped/failed whole-Turn processes stay open. They have no diagnostic JSON disclosure.
The rustX adapter renders typed publication-audit text, reasoning and refusal
through the same keyed Assistant/Markdown segments as streaming output, preserving
the rendered Markdown and open reasoning disclosures. Sparse native block indices
survive removal of Tool proposals. Empty audits and unexecuted
Tool proposals have no Chat body. Audits retain their native cursor; their
message identity suppresses stale streaming duplicates. Native terminal facts
continue to own stopped/failed labels. Copy and the native settlement time are
available for partial prose; completed-response actions are not fabricated.
Native audit data remains available through native protocol and Trajectory reads.
The adapter and grouping changes are rustX-authored; no Harness runtime or
interruption state machine is copied.

Stop mismatch ledger: the previous native audit path preserved the output but
omitted the content-level stopped tag and remounted the streamed message.
Both are repaired against the reference above. Web retains the native bounded
publication-settlement event header until its independently cut transcript row
arrives, avoiding an empty intermediate frame. The native settlement head owns
retention; attachment replacement clears these headers and historical windows
exclude them. This stores released audit facts, not expired live bytes. Explicit
native Attempt ownership seats the stopped process above its partial output,
without borrowing membership across conversations. The protocol audit retains its
own native reading anchor; only its React renderer uses the released message ID.
An unexecuted proposal without visible text/reasoning still creates no Tool row.
The native cancellation acknowledgement, terminal outcome, Tool settlement and
durable audit remain authoritative. TUI Stop behavior is unchanged.

Deferred prose, reasoning and refusal use their exact immutable audit content
references, including after reload. The browser compares actual inline UTF-8
length with the native readable range: a native inline header does not claim
that the browser's smaller live cache contains all its bytes. Already loaded
payload bytes survive the live-to-audit handoff only when the conversation,
stream, message, Attempt, Turn, request, block, representation, version and byte
end all match. Changed streams or ranges reread their own source; expired live
locators are never reread for a frozen response. Copy reads the audit's complete
text/refusal blocks, without introducing completed-response actions.
The current settled partial keeps its Copy/time footer visible, matching the
reference's latest-response action reveal policy.
Unloaded historical receipts create no placeholder Chat rows; ordinary native
history loading recovers their bodies. Explicit native message identities take
precedence over local cache paths, preventing a moved row from borrowing another
message's payload.

Browser validation uses the repository's pinned Playwright workflow because
Browser plugin not available. Flow under test: native conversation -> Stop before
output, during reasoning/text/proposal or a running foreground Tool -> retained
partial content and stopped markers -> continue and reload the same conversation.

Stop QA (2026-10-20): Web typecheck/build, i18n and source/notice checks passed;
2,960 Web tests passed (four existing skips) and 55 provider tests passed.
The pinned browser passed the seven unchanged Agent screenshot references and
both compaction flows, then both new native Stop flows after correcting their
history-loading synchronization. Stop covered empty output, reasoning, text,
39,981-byte text, proposed Tool arguments and a real running Bash process.
Both locales/themes retained partial Markdown and disclosure DOM through Stop,
discarded abandoned provider output, continued normally, and recovered the same
records and deferred body after reload plus the ordinary Load earlier gesture.
Page identity, meaningful content, no framework overlay, no application console
errors, screenshot evidence and interaction checks passed at
`http://127.0.0.1:5273`, 1440x1000 light English and 390x844 dark Chinese.
No screenshot baseline or comparison allowance was changed.

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
`WorkspaceSessionAdmission` owns fresh Product Host operation authorization.
`app/navigation/machine.ts` owns committed shell routes and exclusive preparation;
Session Lifecycle owns native attachment, Switch and settlement.
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

The rustX-only history-window explanation above Chat has been removed, including
its English and Chinese copy. Read-only inspection of Harness
`687f282e8625c6b8d11ff58211a90524be2e4317`, `ui-chat/src/client/chat/ChatView.tsx`,
confirms that its reading surface has history loading and a return-to-bottom
control without this persistent banner. Native bounded reading, loaded-window
paging and latest-tail ownership are unchanged for parent and child Chat.

The visible history controls now follow the same Harness ChatView reference:
only its compact, centered Load earlier control appears at the top, with matching
Chinese wording. The rustX-only Load later action and copy have been removed.
Native bounded newer-page reads continue when downward reading reaches the end
of the loaded historical window, before the separately observed live tail. This
continuation has no visible row, performs no eager history traversal, coalesces
pending reads, ignores programmatic scroll/anchor corrections and releases its listeners on return to latest or view disposal.
Main Chat, child Chat and immutable source history share this presentation;
native read cuts, source ownership and viewport anchoring remain authoritative.

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

## Unified canonical compaction summary presentation

CompactionMessage is the sole checkpoint disclosure. The collapsed row retains
only the Harness label and expansion hint; its expanded body shows original
Markdown. Trajectory retains the existing GFM preview projection in
src/presentation/markdown/preview.ts; the former app-local preview module and
CompactionSummary component/styles are removed. Native transcript identity, fork
boundaries and explicit source navigation remain unchanged. Both local presentation
notes and the upstream interruption/continuation references are retained.


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

Model-selection failure announcements follow Harness `ModelSelect.tsx` and
`packages/client/ui-primitives/src/Toast.tsx` / `Toast.module.css` at
`5badb15009ae1756c3afe0ae0cef1faafc290ccc`, inspected from immutable upstream
source. Rejected selections announce through a warning Toast in a body portal,
centered horizontally over the Composer and positioned near the viewport top.
The three-second hold and one-second fade do not move chat layout or take focus.
Floating UI, already used by the menu, owns positioning, viewport clamping and
layout tracking. Success/action variants are excluded; existing theme tokens
supply the surface. Catalog-load errors and reread actions remain in the menu.
Expiry hides the announcement only: native selection and reread fences remain
authoritative, repeated failures restart the announcement, and attachment
replacement retires it. Tests cover rejection, recovery without mutation replay,
catalog errors, attachment retirement and timer lifetime.

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

## Main protocol and bounded fork source integration

The main branch owns the bounded v37 navigation, outline demand scheduler and
command menu presentation. This integration retains those owners and upgrades
mandatory generated imports to the final v50 contract for the native Message window selector
and the upstream committed-delivery contract.
Fork source navigation performs one indexed native read and validates message
identity before anchoring. Existing source references and immutable upstream
pins remain; hashes/import inventories reflect the reviewed combined files.
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
when navigation changes. No child model settings are introduced. App Server v50,
Runtime Client v68, child IPC v30 and Product Host file-read v3 are synchronized.
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
The existing ChatViewport overflow measurement elects an absolute,
paint-contained background pseudo-element for the fixed 36px theme-base fade
only when sticky/flow transitions are possible. Non-scrolling seats retain
their original seat background and ordinary paint origin. The sticky seat keeps its existing geometry
and stacking; its controls, shadows and editor menus remain uncontained. The
obsolete overflow-dependent seat transform is removed. In the pinned browser, the previous gradient could retain a different
raster origin after sticky/flow promotion even with identical final geometry,
scroll offset and computed styles. The regression re-records background paint
at both origins, restores all styles, and compares exact RGBA with no noise
allowance. Both attachment themes still match their existing references. Tolerance,
masks, stabilization schedule and browser pins are unchanged.
The investigation downloaded the browser traces and actual images from hosted
run `38067804782` at `8649f8af`. Its 327x291 attachment captures differ by 1229
light / 2927 dark pixels, all within rows 1-66. Re-recording the old seat's
background at the flow origin reproduces both hosted actual images exactly in
decoded RGBA, while final DOM geometry, scroll offset and computed styles stay
unchanged. The prior passing HEAD `6e64c1f9` has the same Composer/ChatViewport
source, and isolated fresh contexts can pass; neither observation proves the
rendering independent of paint history. Paint containment belongs only to the
background so menus, narrow attachment placement and native context controls
keep their existing appearance and behavior. Full-suite inspection also caught a separate 2238-pixel, channel-delta-1
difference in the dark narrow Agent capture. Its seat is also overflowing:
327x108 at (56, 735.5), reader scroll 24 of 792 with client height 768. The old
seat initially matches the reference, then reproduces the new image exactly
after flow-origin invalidation with unchanged final geometry. Seat/pseudo
transforms and will-change do not resolve that difference. The intended
correction is one stable local background raster for that same 36px theme-base
fade. Only `agent-dark-narrow-linux.png` is intentionally revised: 2238 pixels
in rows 736-771, maximum channel delta 1; every other pixel is identical. This
reference no longer records one accidental old paint history. The two failing
attachment references remain byte-identical. Non-overflowing seats retain
their original direct background; no sticky/flow origin transition exists there.
Trajectory assertions inspect the elected background owner and retain their
pixel/geometry checks.

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

The Inspect pill is pure presentation. App Server v50 / Runtime Client v71 native
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
dependency closures describe this final source, including the v40 import renames.

### Issue #456: general Model Profiles

The Harness-derived model/profile menu (`ModelSelect`), the command panel model
rows and the adapted Models settings page now present rustX's general Model
Profiles instead of reasoning profiles: menu and label copy says "Profile", and a
profile row shows its native reasoning state and output default as published.
The Model detail page edits each profile's complete native `request_params` as
one JSON object (`RequestParamsEditor`, a rustX-authored field over the existing
TanStack/XState unit bridge); invalid, non-object or duplicate-key text stays a
local buffer that blocks submission, and formatting-only edits begin no draft.
Native Rust remains the configuration authority. No upstream source is added.
Inventory hashes and dependency closures describe this final source, including
the v50 import renames.

The #456 selection review separates configured from effective Profiles in the
Harness-derived `ModelSelect`: its Profile submenu gains a "Model default
profile" row, rows emit a typed selection intent instead of `(model, profile?)`,
and the current marker follows the configured Profile while the trigger shows
the effective one. The shared `Menu` now marks and announces the selected row of
a submenu (`aria-current`, trailing check) exactly as it does a top-level row.
The #456 stale Profile review also offers that Profile submenu, with a single
"Use model defaults" row, on a Model that declares no Profiles while its
configured Profile is pinned, so the pin can be cleared.

### PR #461: integrating the current main contracts

The main integration keeps App Server v50 and Runtime Client v68, which remain
newer than main's v39/v61, and SQLite 51's checkpoint/Model Profile contract.
Subagent IPC becomes v31 because main and this branch had different v30
vocabularies; either old peer is rejected. The current generated protocol and
consumers include main's general Model Profiles and JSON-string source authoring.
MCP retains the reviewed explicit-action editor and diagnostics ownership.

`ModelSelect` keeps main's configured/effective Profile distinction, explicit
Model-default row and typed selection intent, alongside this PR's measured
icon/text collapse and full-label title. The imported main selector reference
retained its own earlier Composer surface and always-visible model glyph. The
merged capture differs from that image by 2707 pixels, all in the Composer
controls, while its new Profile menu matches main exactly. Against this PR's
previous reference, every pixel outside the union of the old/new menu bounds
(x699-1373, y813-969) is identical. Those bounds are investigation coordinates,
not comparison exceptions. `agent-selectors-light-linux.png` is deliberately
revised to represent main's new menu within the existing PR Composer. No
product layout is changed to imitate an obsolete integrated reference, and
no screenshot threshold, mask, retry, stabilization or browser pin is changed.

The six conflicted Settings references also retained the pre-main paint owner.
Main's existing `SettingsRoot.panel` opacity layer (PR #459) is kept unchanged.
A pinned-browser diagnostic temporarily removes only that layer, then restores
it in the same page with identical measured geometry. Removing it reproduces
all six previous PR references exactly; restoring it reproduces each integrated
capture exactly. The measured changes are 6594 pixels for `locale-general-zh`,
4560 for `settings-delete-confirm-light`, 337 for `settings-read-error-light`,
6089 for `settings-shell-light`, 949 for `settings-shell-dark`, and 319 for
`mobile-settings-dark`. These six references are deliberately merged to record
main's existing stable panel paint with the retained PR surface. The changes
are caused by that inherited rendering correction, not by a new Settings
layout or a relaxed comparator. No Settings presentation source or noise
policy is changed for this reference merge.

### PR #461: independently verified Timeline fade repaint

After the main integration, the complete pinned suite found an independent
16-pixel, channel-delta-1 failure in `ledger-order-inspector-390-en-linux.png`.
The earlier-history button's 28x50 gradient, at (44, 71.5), changed after a
background invalidation with identical final geometry, styles and no animation.
Six fresh contexts reproduce the full-suite failed image exactly. A background
pseudo-element alone still permits the initial/partial-paint variants, as the
new exact repaint regression proves. The user explicitly extended the repair
scope to this verified rendering defect.

The background now owns a paint-contained pseudo-element and an independent
opacity compositing layer. The layer-2 color, 38% opaque stop, transparent end,
inherited 0.72 opacity, text geometry and font are unchanged. Containing the
whole button shifts its text raster and is rejected. Containment belongs only
to the background; inset keyboard focus and the portaled tooltip remain outside
it. The intended appearance is the same fade with a neutral, consistently
antialiased history glyph instead of colored subpixel fringes that depend on
paint ownership. This explicitly intentional rendering correction changes
references within the button's 28x50 box, including the font's grayscale
antialiasing; it is not an allowance for future mismatches.

A fresh-context diagnostic applies each candidate before the UI's first paint
and invalidates both parent and pseudo-element backgrounds. The independent
background layer is exactly stable through initial and partial paints. Native
navigation acceptance additionally asserts identical geometry and exact RGBA
before/after that invalidation with no noise allowance. All revised captures
are reviewed against their old references: dimensions and every pixel outside
the button remain identical. The 44-state same-page diagnostic reproduces each original reference with
the old paint owner exactly or with only 16/60 channel-delta-1 gradient pixels;
the independent layer returns to exactly the same RGBA after restoration in
every state. Its 37 deliberately revised references change only the 28x50
button, with 230-1303 pixels depending on the scene. The former color-fringed
glyph becomes neutral gray. Exact per-reference measurements are recorded in
`docs/evidence/pr461-rendering.json`.

The old Chinese 16-pixel noise evidence and its original reference move verbatim
to the comparator's existing frozen fixtures. Its positive/negative comparator
tests remain intact. The live allowance is removed; new tests reject a one-level
change at every formerly allowed site in both current English and Chinese
references. Other noise entries, global thresholds, retries, settle schedule
and browser pins remain unchanged. The source inventory retains the Harness
pin and records this concrete local CSS adaptation.
PR #458 follow-up: TurnNavigator's delayed hover read carries the current native
navigation intent through publication, so a newer loaded-turn jump revokes a held
preview without retiring ordinary outline reads during transcript pagination.
ModelSelect distinguishes lazy menu opening from an explicit forced catalog
reread. AgentControls retains the native model recovery gates and does not replay
a rejected selection. Held-response and cached-catalog regressions cover both
changes; the pinned Harness source and presentation styles remain unchanged.


## PR #463 integration with native child inspection

The integration keeps PR #461's native child conversation, Trace, attachment
and file-delivery presentation. Agent workspace values now carry the frozen
Session path, and retired worktree disposal and Candidate controls are absent.
The historical entries above retain the versions reviewed at those snapshots.
The combined current contracts are App Server v50, Runtime Client v71, SQLite 61,
Session catalog16, Runtime Event1, Child IPC33 and Product Host file-read4; see the [format inventory](../docs/agent-workflow-shared-workspace.md#format-and-version-inventory)
for owners and rejection behavior. Imports and generated client contracts use
App Server v50. The source inventory
records the resulting local hashes and actual dependency closure; upstream
Harness pins, licenses and source attribution remain unchanged.

## PR 458 deterministic reading and responsive repair

Turn previews share the existing single-flight outline pipeline but return only
disposable presentation data. Only explicit reads/navigation publish committed
paging. The rail retains one preview Turn and cancels it on pointer leave,
navigation, native mutation, attachment or generation replacement. Focused
marks keep their tooltip through explicit navigation; preview identities never
replace committed outline identities.

AppFrame measures responsive geometry directly from its ResizeObserver, clears
the narrow overlay on breakpoint transitions, and restores focus to the sidebar
toggle on dismissal. Tests exercise toggle, Escape, backdrop, inert central
content and pointer hit testing at 390px across desktop/mobile transitions.
The collapsed rail and intentional overlay presentation remain intact.

Native opening prompts apply the same human/delegated-Agent classification to
local and inherited provenance. Local lookup uses existing Attempt/type/range
indexes. Deterministic VM comparisons after 0, 128 and 512 unrelated future
Turns stay constant, including idle inputs, previous-Attempt steering and
ineligible external notifications. No protocol or schema version changes.

The browser repair inspected each failing reference, stable actual and available
diff, including later captures in the same test. Approved references retain the
current 856px conversation width, 12px shell inset, Composer action icons,
800 × 704 Settings panel and narrow sidebar overlay. The 57 repaired references
were captured with pinned Playwright 1.63.0 Linux Chromium; the production
comparison/stabilization helpers and rasterizer-noise manifest are unchanged.
The comparator's live evidence-binding and frozen negative-control tests pass
against the updated references. No tolerance, deadline, retry or generic mask
was added. Product speed labels remain `token/s`; assertions follow the current
English/Chinese dictionaries.

Full integrated execution exposed three history-dependent accessory circle
rasters (18–24 changed pixels, maximum channel delta 2) after context and shell
transitions. The accessory now owns a complete raster layer like the send
control. Forty-nine reviewed stable captures change only the accessory: normally
ten glyph-edge pixels by one channel level, and the disabled button's opacity
rounding by one level inside its 28px bounds. The dark context capture also
replaces the prior circle-edge raster. Geometry, clipping, comparator, browser
pins and reference-local noise evidence remain unchanged. These captures are
baseline-review evidence; only the subsequent complete strict run counts as
browser conformance.

Full-suite execution also exposed an unowned asyncio stdin observer in the
external provider fixture. The CLI now retains and settles that task for the
server lifetime. A forced-GC/EOF regression fails against the prior observer
with the same destroyed-pending-task diagnostic; all 54 provider tests and the
real native chat/child-capability browser scenarios pass with the owned task.
This changes test-process lifetime, not rustX execution or settlement ownership.

Repair-head CI run 38157397982 passed 253 browser tests but exposed one remaining
narrow Settings corner mismatch: ten pixels, maximum channel delta 6, where a
long Agent card reached the panel clip. The narrow scrollport now owns its
existing 20px bottom gutter as an outside margin rather than scrolling content
padding. Panel dimensions, superellipse corners and content scroll range remain
unchanged; content can no longer repaint behind the bottom corner. A rendered
regression checks the fixed gap before and after scrolling; the prior style
fails with 0px at both positions, while the repaired owner keeps 20px. Five
reference/actual/diff sets were individually reviewed for this intentional
bottom-gutter change. The strict comparator and noise manifest remain unchanged.


## PR #458 remaining review repair

AppFrame exposes its narrow overlay state and a presentation-only dismissal callback through the existing sidebar composition contract. Primary Session, new Conversation, Workspace and Settings navigation retire that overlay; desktop cold Composer presentation and sidebar width remain intact. A failed or superseded narrow Session admission does not publish its destination or dismiss the overlay. Focus returns through AppFrame after inert and backdrop retirement.

The Shell Navigation actor is the sole committed route and pending-intent owner. Its two states are `idle` and `preparing`; preparation retains the committed route and its controls. Typed events select Sessions, drafts, exact read-only history, executable Node transitions and command choices. Settings handoff retires preparation before sending to the independent Settings actor. Idle Settings and overlay dismissal preserve committed controls. AppFrame owns geometry: explicit overlay dismissal retires only sidebar preparation, while successful publication closes it programmatically. Already-authoritative Sessions publish immediately without attachment or snapshot refresh. Cold destinations publish only after admission and a final exact native observation check, on desktop and narrow layouts alike. A non-executable `session-draft` route preserves the existing cold local-input surface when an empty draft chooses a Session; it queues input through FirstSubmissions until native admission, never grants execution controls, and passes through only its own successful publication. Initialized Session-to-Session navigation retains the committed conversation throughout preparation. Composer binding is machine-derived and persists across same-authority reconnect and draft promotion; there is no separate React center/focus/command/transition state or top-level epoch coordinator.

Each disposable preparation actor has one correlated completion path. Final intent, connection authority, exact destination and native proof validation precede one machine publication transition. Stopped actors cannot publish routes or navigation errors. Read authorization is also retained and checked at publication, so replaced Product Host admission cannot leak a late history result or error. Committed control proofs are independent of preparation. A native first submission obtains a Shell creation publication intent while FirstSubmissions continues to own its create/attach/upload/input settlement. Settings can retire only its publication; replacing the actual committed draft also fences future native admission. Neither cleanup nor failure renews old route authority. Catalog searches retain their own disposable directory request identity and never own routes.

View Source now means history inspection. It authorizes the exact Session cwd through `admitSessionOperation`, resolves the recorded Node through `session/node`, then reads `session/history` at the exact Message with limit 32. Node Conversation, read cut and returned Message/target cursor must agree. The history surface uses the canonical transcript renderer without a SessionView or Composer; it exposes bounded cut-preserving earlier/later reads and return navigation. Its cache remains bounded to 256 entries and 8 MiB. ChatViewport remains the sole scroll writer. User scroll, newer route selection, Settings and authority replacement retire intended landing; stale results never publish a history window. Source Session attachment, selected Node, Attempt and execution state are untouched. No attach, detach, Switch, initialization or execution accompanies View Source, even for a cold or same-Session source.

Session Lifecycle retains every dispatched Switch transaction. Fresh Host authorization and caller interest gate dispatch; proven-unsent retirement preserves the original native claim. Acknowledged Switch ends authoritatively detached at its exact new Node. If the UI has retired, no follow-up attachment or Retry starts: later explicit Session opening recovers the selected Node. A transmitted refusal may follow native unload and therefore remains fail-closed; uncertainty is retained without replay. Branch/Fork storage acknowledgement remains observable through catalog reading after presentation retirement, and committed-node durability diagnostics are retained at decode time. Retry uses native returned input once; queued input has a dispatch fence, and transmitted input settles independently without replay. Browser cancellation provides no rollback semantics for any admitted native effect.

Four dark shell references deliberately retained the enabled Stop control in the preceding repair: settings-shell-dark, desktop-expanded-dark, mobile-rail-dark and mobile-expanded-dark. Their reference/actual/diff inspection showed 952 changed pixels per reference confined to Stop, with identical dimensions and surrounding pixels. This refactor retains that control contract. Screenshot comparator, noise, stabilization, timeout and retry policies remain unchanged.

Fork metadata and exact history identity use App Server v50 `session/node`, which reuses the native Session catalog's exact node map lookup. Immutable source identity and inherited transcript placement remain native facts. The matching Runtime Client v71 and SQLite v58 contracts reject prior peers/stores before dependent work. Current generated imports and local provenance hashes are synchronized.

The intentional `mobile-settings-dark-linux.png` reference change removes the obsolete sidebar backdrop beneath the Settings modal after successful Settings navigation. The reference, actual and diff were inspected: modal content and geometry are identical, and the changed pixels are the surrounding backdrop. The pinned browser generator updates this one changed reference; screenshot thresholds, noise policy and stabilization are unchanged.

Browser-lifetime Shell/Settings actors and Session admission are owned above AppSurface. The rendered projection may remount while those owners and native startup settlement continue. Native authority observation belongs to the Shell actor; React does not duplicate its route retirement. A creation acknowledgement stages its exact native identity as a non-executable Session draft; only matching attachment evidence publishes execution controls. Saved startup hints observe retained admission or failure without replaying attachment. Root owner retirement invalidates old control proofs.

Issue #467 keeps the pinned Harness presentation sources and MIT attribution while replacing observation authority with native bounded checkpoints, complete cursor transitions, deferred content ranges and exact view fencing. Command capability inspection retains native pages and reads selected fields. A deferred Todo head remains visible without inventing task counts; composed-empty and absent Todo remain distinct native facts. The corresponding local hashes and import closures are recorded in `source-inventory.json`; upstream pins, screenshot thresholds and license closure remain unchanged.

Issue #467 uses 32-item transcript/history windows independently of the 64-item native turn directory. TurnNavigator hover and ordinal page selection use the directory bound, while original transcript reads retain their smaller count and byte bounds. Its attribution, upstream pin and import closure are retained, with the changed local hash recorded in the inventory.

Issue #467 retry disclosures read the advertised original failure field at its retained native Trace cut, using the existing bounded field reader and explicit large-field windows. Their presentation retains the Harness retry attribution; attachment, source and generation ownership belong to rustX. Settings leaves disabled button opacity to the Button primitive so module import order cannot change the reviewed appearance; its other form controls retain their existing opacity. These local adaptations and the added Trace reader imports are recorded in the source inventory.

Issue #467 retains finite successful Trace detail descriptors separately from complete detail values. The existing Inspector exposes explicit original field pages and byte windows for Options, prompts, tools and Diff, using captured native cuts and exact predecessor evidence. Its presentation and localized labels retain their Harness attribution; the changed local hashes and actual import closures are recorded without changing upstream pins or license notices.

PR #478 compaction inspection retains the matched original Completed-J summary preview. The Harness-derived Inspector keeps its existing Summary and Raw Output views and marks truncated previews as deferred; the Chat marker now reads its original checkpoint summary on expansion. The reviewed local hash and treatment are updated for this presentation change; upstream pins, imports and license attribution are unchanged.

PR #478 cancellation review preserves the pinned Harness composer and submission-policy sources. The resident Root explicitly supplies a cancellation-only presentation mode when its exact live attachment remains authorized but observation is unavailable. The editor remains focusable for Escape while read-only; normal Send, draft editing, Child controls and unsupported content retain their existing rules. A new Conversation has no inferred cancellation authority. Reviewed local hashes and treatment notes for `AgentComposer.tsx` and `submission-policy.ts` are updated; upstream hashes, import closure, copyright, license notices and screenshot policies remain unchanged.

## Native Tool detail audit

Read-only reference: `deepseek-harness@687f282e8625c6b8d11ff58211a90524be2e4317`,
`ui-tool/src/client/tool/{components/ToolRow,components/ToolDetails,toolviews/details-row}`,
`models/{control-details-model,details-card-model,diff-card-model,read-card-model}`,
and the existing question, terminal, image and deliverable presentations.
The native inventory is checked against `src/tools/native` registrations: 21
fixed Tools, the optional native Todo implementation and the discovered Workflow
family (23 representative call shapes). No Harness runtime or tool contract is imported.

| Native capability | Presentation and audit outcome |
| --- | --- |
| `bash` | Existing terminal card, native command/output, copy and background acceptance distinction retained. |
| `read` | Existing bounded read card retained; failed/cancelled/uncertain calls now use diagnostic IN/OUT. |
| `write`, `edit` | Existing intended-change diffs retained and explicitly labeled; rejected/interrupted calls show diagnostics instead of suggesting applied changes. Native results do not contain Harness applied-diff metadata. |
| `glob`, `grep` | Existing bounded opaque-text search cards retained, with original recovery notices and copy. No total-match count is invented from a bounded search. Errors use diagnostic IN/OUT. |
| `read_image` | Path and existing managed-image preview share the image body; redundant arguments JSON is removed. Native artifact loading remains explicit. |
| `present` | Existing delivery row retained; delivery cards still come only from committed successful delivery facts. |
| `ask_user` | Existing question/answer presentation retained; answered results now have the same Inspect action as cancelled/interrupted results and use committed result content. |
| `job_list` | Standard row plus compact recorded list, state badges and native truncation caption. |
| `job_status`, `job_wait`, `job_cancel` | Standard row plus recorded state and copyable output; expanded row omits the repeated status. RustX-only “Recorded task result” and “Full task record” removed. Tool success does not imply Job success. |
| `subagent`, `list_agents`, `send_message`, `wait_agent`, `interrupt_agent` | Standard Tool row/details replace a separate row layout; historical receipt titles/states come from the recorded result. Current child catalog supplies navigation authority only. Acceptance never implies child completion. |
| `create_goal`, `get_goal`, `update_goal` | Compact objective, phase, recorded round count and blocked reason replace the complete execution JSON disclosure. Empty Goal is explicit. |
| `todo` | Recorded post-call tasks, statuses, dependencies and owners replace generic snapshot JSON; the Composer dock still owns current state. |
| discovered Workflow Tools | Declared output has no fixed entity schema; existing IN/OUT and Inspect remain appropriate, preserving arbitrary workflow-defined results. |

Job, Goal and Todo previews in the trajectory overview share Chat's recorded
result adapter. The existing Result tab retains original JSON/text content,
without a second raw-record disclosure. All Tools retain exact native occurrence
navigation; unavailable/unknown result formats use the existing generic view.
Copyable Job output uses the existing CodeBlock. The shared details geometry,
badges, dividers, field grid and scroll caps are adapted from the above reference;
obsolete separate Agent/Job/Goal detail styles are removed.

The inventory records the shared ToolDetails source and stylesheet, exact hashes
and imports. Browser plugin is not available in this session; rendered checks use
the repository's pinned Playwright container. Unit coverage enumerates native
registration names and exercises each disclosure/Inspect path. Browser coverage
checks English/Chinese, desktop/narrow layouts, clipboard, trajectory navigation,
framework overlays and console health; the real native Goal flow is also retained.


### Compaction Chat and trajectory alignment

The pinned Harness `ui-chat` CompactionItem, CompactionCommandCard,
GenericCommandCard and MessageItem styles provide the compact command/checkpoint
row, running shimmer, neutral completed label and summary-only disclosure.
The marker now belongs to the conversation column; the separate projection dock,
historical projection list, duplicated summaries and internal frontier/provenance
copy are removed. Summary reads remain lazy and bound to the exact native
checkpoint, activation and current attachment. A bounded summary stays explicitly
truncated when its native detail is incomplete.

Trajectory checkpoint summaries and unscoped reused history display the Harness
“Between turns” gutter label. This is presentation metadata only: original
Attempt/Step locations, record selections, native request ownership and timing
remain authoritative. Scoped summaries retain their native evidence but do not
acquire a Turn label or disappear when the Turn is folded.

Retention was checked against Harness compaction-basic config/region selection
and rustX context planning/execution. Both use token budgets rather than a fixed
message count. rustX defaults to a 4096-token recent-content target, preserving
complete call/result boundaries subject to hard request fit. The integration
regression verifies a nonzero recent target against both actual provider input
and the reconstructed frozen request: derived summary followed by the exact
original recent message. The zero-retention browser maintenance fixture does
not establish default retention behavior. No retention policy is changed by
this presentation repair.


The explicit Return to latest intent now survives delayed native history backfill,
rather than expiring after one animation frame. Reader scrolling, Turn navigation
and explicit older-history reads retire that intent. This keeps compaction
settlement and subsequent replies visible while preserving detached reading.

Large native Tool inputs retain the existing Harness ToolCard disclosure and
DiffBlock presentation. Expansion reads deferred original arguments through the
native occurrence's one-item Tool page and 64 KiB content windows, without changing
execution state. Accepted canonical calls whose execution descriptor exceeded the
inline budget read their native descriptor in place; only an actual streaming
proposal uses assembling copy. Optional display IDs do not replace the native
block occurrence. Unknown/unavailable sources remain explicit, with late reads
fenced by the original Conversation/attachment or frozen history cut.

## Workflow run and in-workflow review alignment

Reference: DeepSeek Harness `687f282e8625c6b8d11ff58211a90524be2e4317`,
`ui-workflow-run/WorkflowRunPanel` and `ui-user-questions/QuestionComposer`.
The compact module header, nested disclosure rows, status dots, clean-completion
collapse, abnormal-state expansion and focus-preserving transitions are adapted
from those surfaces. Review reuses the existing derived Question card styles,
numbered choices, explicit Submit, minimize and dismissal actions.

| Evidence mismatch | rustX presentation and native boundary |
| --- | --- |
| Full-width Current activity cards outside the reading column; no node visibility | One run panel inside the transcript column, beside its exact native Tool call when Attempt/call ownership is present; otherwise an independent native activity seat. Root and continuable Child transcripts share the renderer. |
| `Steps 9 / 10` reads as unfinished progress | Counts say **Steps used / Limit / Agent calls**. Only the native run state determines completion. A native pending Review annotates only its exact run/block/node/visit with an attention label; the underlying running execution state is unchanged. All native node kinds are shown within their exact block definition/invocation groups. A pending node in a settled run says **Not run**, without inventing a skipped/successful state. |
| Native finite headers hide nodes after the first 32 | The existing immutable Workflow source restores retained nodes at the same checkpoint cut; the browser fences replacement views. Runtime retention omissions are explicitly labeled. |
| Review subject and historical interaction envelopes shown as raw JSON | Workflow-authored values render as Markdown, fields and lists. Review submits the original instance and subject digest through `interaction/respond`; rejection requires feedback. Durable receipt rows match request/settlement only by native Attempt and interaction identity. Inspect in Trajectory retains native details. |

DSH's phases and navigable member Sessions are deliberately not imported:
rustX owns block/branch/parallel/loop/return nodes, finite Workflow activations and
reviews that pause **inside** a run. Finite activations gain no message, resume
or invented Session navigation controls. No workflow events, timestamps,
completion state or graph order are reconstructed from neighboring messages.
No workflow-definition authoring or protocol changes are introduced.

Validation flow: native Workflow Tool -> finite Agent analysis -> native Review
wait -> reload -> Accept/Reject with feedback -> native branch/return -> settled
run disclosure. Browser plugin not available; the repository's pinned Docker
Playwright runner validates desktop light English and narrow dark Chinese,
with screenshots outside the repository under `/tmp/rustx-workflow-*.png`.

## Web model and thinking effort selector alignment

Reference: DeepSeek Harness `687f282e8625c6b8d11ff58211a90524be2e4317`,
`ui-model-selection/ModelSelect`, `ui-primitives/MenuGroup` and `rank-by-name`.
The Web selector now uses the reference's single-card Model / Thinking effort
panes, compact dimensions, shared translucent menu surface, provider groups,
search above four models, keyboard return paths and selection focus treatment.

| Evidence mismatch | Repair or intentional native boundary |
| --- | --- |
| Model/Profile sideways submenus and missing current values | Two root cells show Model and Thinking effort with their current values; drilling replaces the contents of the same card. Escape/Shift+Tab return to the opening cell, then to the trigger. |
| Duplicate “Model default profile (high)” and “high” choices | The named native default appears once in both the dropdown and `/model`. Choosing that strength follows the native Model default; other strengths pin their exact native Profile. An unnamed native default or a removed Profile still has one explicit default/recovery action. No strength IDs are fabricated. |
| Binding revisions close/rebuild the menu and clear its catalog | A write keeps its pane/rows, confirmed label and fixed-size progress indicator until native acknowledgement plus authoritative reread. Revisions fence selection while retaining the last catalog for display; replacement attachments clear it. Cached refreshes add no loading paragraph to the card or Composer. The pending trigger keeps focus while native checkpoint reads temporarily revoke control admission; its option rows and dependent sends remain locked. |
| Selection failures mixed with catalog diagnostics | Selection failures use the Composer toast. A refused native mutation retains an explicit reread action before a further write; rustX requires authoritative repair rather than replay. Catalog failures retain their in-card Retry. |

The Web adapter continues to carry the complete native Summary policy and
independent overrides, and preserves native explicit same-Model selection and
NoOp acknowledgement semantics. Frozen running Attempt configurations are not
rewritten. Native Profile authoring remains in Settings. **TUI continues to
follow pi and has no changes in this work.**

Validation flow: saved Chat -> Model/Thinking effort panes -> held selection ->
held native checkpoint -> confirmed label/focus, plus actual native live profile
switching and lost-acknowledgement recovery. Browser plugin not available; the
repository's pinned Docker Playwright runner checks English desktop light and
Chinese narrow dark. Frame samples verify card/row identity and unchanged
Composer geometry and Chat scroll while each write/read is pending. Evidence
screenshots are outside the repository at `/tmp/rustx-model-*.png`.

Final visual QA also scopes the host reset's focus outline away from model menu
rows: DSH's row highlight and the trigger's own focus shadow are retained, with
no extra blue outline after a selection. The complete 19-case browser regression
passed, followed by four focused desktop/narrow rechecks after that outline
repair. The updated non-popup references change only the centered chevron;
registered dark-390 attachment/context noise pixels were compared with the old
references and are unchanged. No noise regions or tolerances were widened.

## File picker, page drop and attachment rail alignment

Reference: DeepSeek Harness `687f282e8625c6b8d11ff58211a90524be2e4317`:
`ui-conversation/skeleton/InputBar.tsx`, `ui-attachment/ComposerAttachments`,
`drop-events`, `DropOverlay`, `AttachmentRail`, `FileCard`, and the shared
`ImageLightbox`/`file-size` primitives. The reference checkout was read only.

| Reference evidence / previous mismatch | Result and native boundary |
| --- | --- |
| DSH opens the multi-file picker from the plus menu and resets its input; rustX advertised raw byte/count budgets in that row | Retain the same picker entry, editor selection and repeat-pick behavior; show the normal attachment description. |
| DSH listens across the document and shows the illustrated, blurred viewport mask; rustX listened only near the composer | Reuse the DSH document enter/leave counter, copy/blocked drop effect, viewport exit/dragend cleanup, and body-portal mask. Text-only drags remain native. |
| DSH uses a scrollbar-free paged rail, 240×64 file cards and 64×64 image thumbnails; rustX stacked a generic card, status line and separate retry controls | Reuse rail geometry, edge arrows, wheel panning, new-item reveal, file glyph/size, hover/touch removal, card-body retry and original-image lightbox. |
| DSH refuses invalid additions with a toast rather than a rejected placeholder | One intake path for picker/drop/paste refuses an invalid addition atomically and preserves the existing draft. Native safe-basename and directory admission remain authoritative. |
| DSH's InputBar explicitly applies no count/size cap to generic files; rustX imposed 8 files / 2 MiB each / 4 MiB aggregate | Remove those product limits from native preparation, receipt collection, persisted allocation validation, generated capability and clients. Transport concurrency and 64 KiB frames remain independent flow control. The server spools verified frames to anonymous temporary files rather than buffering entire large selections. |
| DSH separately projects provider-native image limits (reference defaults: 20 images, 20 MiB each, 200 MiB aggregate) | rustX uploads all selected files, including images, into the workspace and does not create provider-native image input. No unrelated image-message limit or image-limit caption is invented for this file path. Image thumbnails retain native upload indication. |
| Native transport response loss cannot prove failure | Keep Check status inside the same card; only Absent/Failed evidence permits explicit Retry with a fresh operation. No implicit retransmission or browser receipt fabrication. First-submit sealing, native authority fences and TUI's pi interaction remain intact. |

Browser plugin not available; rendered verification uses the repository's pinned
Playwright 1.63.0 Chromium container. Target flow: composer plus-menu selection or
a file drag anywhere on the page → native intake/upload → DSH rail, pending/error
feedback, explicit recovery and preserved editor text/selection. Desktop English
light and narrow Chinese dark cover the same flow; real native upload scenarios
exercise workspace bytes, source/fork ownership and more than the old file limits.


The upload capability change is mandatory App Server v53. All current Web/TUI
DTO imports and the binary carrier's control negotiation use v53; v50 artifacts
are removed and old peers are rejected. This is a native protocol update only
for other derived presentation files. Runtime Client v71 and TUI interactions
remain unchanged. The two old narrow attachment-card rasterizer-noise entries
were removed along with that card. All four new attachment references compare
exactly; no screenshot tolerance was expanded.


Final verification: 2,943 Web unit tests plus the real native Web/TUI parity
case passed (four existing skips); 87 focused attachment/comparator checks passed
after the final action-label update. The 72 focused TUI client/upload checks and
Web/TUI/protocol typechecks passed. Native upload, child input admission, App Server
and old-version refusal checks passed, as did Clippy, formatting, i18n, source
provenance and the production build. Shared child task/guidance admission now
accepts 32 files while preserving its separate text budget and cancellation fence.

The normal pinned-browser run passed all 18 selected regressions under v53. It
covered desktop English/light, narrow Chinese/dark, all four theme/width Composer
references, picker caret preservation, document drop, mixed attachment rail,
image preview/focus restoration, invalid-addition toast, named retry/status
actions, committed-control fences, read-only navigation, native retry/Fork, source
deletion/reload, and 24 files including a 6 MiB Markdown document. Target-flow
console/error-overlay and horizontal-overflow checks were clean. Only the four
intentional attachment baselines changed; both old attachment noise exceptions
were removed. Screenshots were inspected from `/tmp/rustx-upload-drop-1440.png`,
`/tmp/rustx-upload-drop-390.png`, `/tmp/rustx-upload-rail-1440.png`,
`/tmp/rustx-upload-rail-390.png`, `/tmp/rustx-upload-pending.png`, and
`/tmp/rustx-upload-failed.png`. The existing bundle-size build warning remains.

## Message Regenerate reading handoff (2026-10-20)

Reference inspection: DeepSeek Harness `687f282e8625c6b8d11ff58211a90524be2e4317`,
`ui-chat/src/client/chat/TurnTailNodeView.tsx`, `MessageIconActions.tsx` and
`ChatView.tsx`. The current Harness built-in message strip exposes Copy and Fork;
its model-request retry assembler is not a user Regenerate action. rustX retains
its native sibling-branch replay while following the user's explicit interaction
contract: click Regenerate → remove the selected response and later output from
the reading surface immediately → show the existing Harness generation chrome →
replace it with the newly accepted native replay and its streaming response.

| Mismatch | Local treatment |
| --- | --- |
| The old response stayed visible until Branch/Switch/Attach completed | Capture a finite reading prefix synchronously with the Shell intent, retaining the exact adopted User input and earlier rows. Native response/process metadata defines the cut; an unloaded input never borrows its neighbour. |
| Attachment replacement destroyed the Chat viewport and briefly exposed an empty shell | Keep the Session's viewport mounted. A new native attachment resets reading intent through the existing single-frame scroll owner, without reconstructing the surface. |
| Acknowledgement and canonical observation can arrive in either order | The Shell owns the disposable prefix through the native accepted MessageId and exact target handoff. Only that input's observed native message/descriptor retires the prefix. Failure, route replacement and authority changes retire it independently; no native effect is rolled back or replayed. |
| Send and response actions could race the native lineage preparation | Disable their existing controls while the prefix owns the reading handoff. Input replay still uses only the native returned editor content, once. |

No protocol, native execution or TUI interaction changes. The original lineage
remains accessible from Session tree. Browser plugin not available in this
session; rendered QA uses the repository's pinned Playwright Chromium container.
The flow under test is: completed response → Regenerate → hold each real native
Branch/Switch/Attach/turn-start acknowledgement → retained prompt and earlier
reading with no old response → new native streaming and settled reply.
Desktop English/light and 390px Chinese/dark runs record animation-frame checks
and screenshots outside the repository. Existing screenshot references and
comparison tolerances are unchanged.

Verification: the full Web suite passed 2,966 tests (four existing skips), then
117 focused command/retry/scroll checks passed after adding acknowledgement-order
and deferred-descriptor coverage. Typecheck, build, i18n, source/notice checks and
all 55 provider tests passed. All 23 existing Agent browser checks passed; the
final eight native/browser checks passed for upload-bearing and inherited Retry,
original lineage/Fork, both localized Regenerate frame probes, both Stop flows,
both compaction flows and turn navigation. Page identity, meaningful content,
error-overlay/console health and horizontal-overflow checks passed. Screenshots
`/tmp/rustx-retry-pending-{1440,390}.png` and
`/tmp/rustx-retry-streaming-{1440,390}.png` were visually inspected. No reference
pixels or comparator tolerances changed. The existing bundle-size warning remains.

## Session list operations (2026-10-20)

Reference: DeepSeek Harness `687f282e8625c6b8d11ff58211a90524be2e4317`,
`ui-workspace/session-actions/{PinSession,RenameSession,ForkSession,SessionId}.tsx`,
`rows/Rows{.tsx,.module.css}`, `ui-primitives/{Menu,Modal,useModalLayer}`,
`ui-workspace/navigation.ts` and `session-controller/sessions/service.ts`.
Archive is explicitly outside the user's requested scope. Permanent deletion is
rustX's native operation, not an upstream Archive action.

| Reference / rendered mismatch | Treatment |
| --- | --- |
| Row actions lacked Pin and Copy ID; extra Close view displaced the reference menu | Match Pin, Rename, Fork, Delete, Copy ID order, hover buttons and resting pin marker. Close view moves into the existing View options submenu. Pin and Copy ID do not select or attach an inactive Session. |
| Current upstream Session menu uses 144px minimum width and 34px rows; the older shared menu uses 218px and 40px | Reuse upstream `listClassName` to scope current 13px text, 14px icons, spacing and 16px radius to Session menus; other menus retain their geometry. |
| Rename had a generic Workspace form, no explicit Cancel footer and a list-refresh barrier | Adapt upstream Session dialog: selected draft, trimmed submit, composition-aware Enter, Cancel/Rename footer, inline error and native summary settlement. Focus returns to the opening control. |
| Sidebar Fork selected its source and opened an explicit boundary picker | Use the latest completed native response, paging backward at its retained read cut when necessary. Publish the child in the catalog without switching the current Session or editor. Retain the manual/generated labels copied by native Catalog authority, without a post-fork naming mutation. Explicit `/fork` and response Fork retain their native boundary pickers. |
| Delete confirmation showed low-level native inventory instead of a concise product dialog | Use the shared modal and Cancel/red-outline Delete footer with the Session title, irreversible history removal and active-work termination explanation. Native delete preview, revision check, failure and uncertain-outcome ownership remain intact. |

Intentional native difference: DSH pins are Host metadata. rustX has no native
pin API, so pins are a Web list preference, persisted per service endpoint and
native authority and shared by browser storage events. No pin authority is added
to Session execution or the TUI. A confirmed deletion removes its local pin.
The native protocol and TUI interactions are unchanged.

Browser plugin not available; rendered QA uses the repository's pinned
Playwright 1.63.0 Chromium container. The flow under test is: open the Session list
→ pin/copy without navigation → rename and cancel/confirm → inspect/cancel native
delete confirmation → Fork a running source at its last completed response while
preserving the current editor → explicitly open the child and permanently delete
it. Desktop English/light and narrow Chinese/dark screenshots are kept outside
the repository. Existing screenshot tolerances are not expanded.

Rendered review caught a browser-only focus failure: the modal could not restore
focus to a hover-only ellipsis after the mask removed hover/focus from its row.
Menu and quick-delete dialog entry now capture the always-visible title button
instead. Both cancellation paths return the keyboard to that Session row.
Double-click rename is checked on desktop; the narrow overlay uses the menu,
since selecting a title deliberately closes the overlay on its first click.

Reference updates are limited to the deletion dialog, the background row's new
hover controls, and twelve existing references with a stale model-chevron color.
The latter each differ by exactly 36 pixels; an isolated archive of pre-change
commit `fa04e72a` rendered the identical chevron as the modified tree, including
its `rgb(173, 178, 184)` computed color. This is reference repair, with no model
UI source change. No comparator or rasterizer-noise allowances changed. Stable
updated captures were reviewed before a normal comparison run.

The Sidebar completion scan also reads native deferred-message descriptors via
the existing `messageDescriptor` parser. It compares their ledger cursors with
materialized entries before selecting a boundary, so a newer completed large
body cannot be silently skipped in favor of an older short reply. No body bytes
or editor input are reconstructed, and no source/child attachment is needed.

Verification: 2,980 Web tests passed (four existing skips). After the deferred
completion fix, all 77 focused Session/Fork/navigation checks and the Web
TypeScript check passed. Production build, i18n, 301 source records, notices for
264 production packages and all 55 provider checks passed. The existing bundle
size warning remains.

Across the normal comparison and native runs, all 17 selected browser scenarios
passed: four keyboard viewports (390/820/1280/1600), six Sidebar product states,
Shell references in both themes, clipped-menu keyboard settlement, grouped/flat/
search recency, native upload-bearing Retry/explicit Fork, the two-real-Session
close-view/controller flow, and the new desktop/narrow Session action flows.
The last two native flows were rerun successfully after the deferred completion
fix. Tested URLs were `http://127.0.0.1:5273` (production preview) and
`http://127.0.0.1:5274` (isolated presentation fixtures). Page identity, meaningful
content, console/error-overlay health, actual clipboard content, modal focus,
source execution preservation and horizontal-overflow assertions passed.
Screenshots `/tmp/rustx-session-{menu,rename,delete}-{1440,390}.png` were visually
inspected. Chromium is the rendered browser authority; other engines were not
exercised for this change.

## Text-field focus paint (2026-10-20)

Reference: Harness `687f282e8625c6b8d11ff58211a90524be2e4317`,
`ui-theme/src/styles/focus.css`, `ui-primitives/src/Input.module.css`,
`ui-primitives/src/settings-form/fields.module.css`, and
`ui-workspace/src/client/rows/WorkspaceBrowser.module.css`.
The local reset forced a solid blue outline after input components had disabled
it; Settings also repeated that forced outline for text fields and React Aria
search. Use the DSH color/width-only focus defaults, preserve component-owned
outline style, and remove the two Settings text-field overrides. Editable text
controls default to no external outline; existing field/card borders, invalid
states, text selection and non-text keyboard controls remain independent.
No React, native protocol or TUI logic changes.

Browser plugin not available; QA uses the repository's pinned Playwright browser.
The flow under test is: pointer/keyboard focus of Sidebar search and Rename →
edit/cancel with stable geometry and focus → Settings provider search, endpoint
and multiline fields → Trajectory search. Desktop/light and narrow/dark evidence
is kept outside the repository. Existing screenshot tolerances are unchanged.

The Sidebar search reference is updated to its outline-free rendering. Two
Trajectory references also contained stale missing “Between turns” labels from
existing commit `faa11646`; the labels are already present in HEAD's
`TrajectoryRow.tsx`. Their refreshed differences are confined to the label/rail
column. No Trajectory source, comparison policy or noise tolerance changed.

Verification: the two outside-repository focus flows passed at 1440px/light and
390px/dark, checking pointer and Tab/Shift+Tab focus, outline paint, stable field
geometry, filtering, draft edits, cancel/return focus, endpoint and multiline
fields, and a retained action-button keyboard outline. All nine selected
existing browser regressions passed across the four keyboard viewports, Settings
pages, native desktop/narrow Session actions, Shell references and Trajectory
search/detail. The two refreshed reference cases passed a subsequent normal
strict comparison. Page identity, meaningful content, console/error-overlay
health and overflow checks passed. Screenshots
`/tmp/rustx-focus-{search,rename,provider-search,trajectory}-{1440,390}.png` and
the localized native Rename captures were inspected. Build, Web typecheck, i18n,
302 source records and 264 package notices passed; the existing large-chunk
build warning remains. URLs were `http://127.0.0.1:5273` and
`http://127.0.0.1:5274`; Chromium was the rendered browser authority.

## Model menu material (2026-10-20)

Reference: Harness `687f282e8625c6b8d11ff58211a90524be2e4317`,
`ui-theme/src/styles/design-platform.css` and
`ui-theme/src/styles/gradient-shadow-text.css`. `MenuSurface` already consumes
DSH menu fill and blur tokens, but the older local theme lacks their global
values. This leaves the ModelSelect material transparent. Port the light/dark
menu material tokens at their theme owner, keeping native model-selection
behavior and the TUI unchanged.

Browser plugin not available; use the repository's pinned Playwright browser.
Flow under test: new-conversation workspace selection → open ModelSelect root →
model and thinking-effort panes → select effort/model → retain draft and focus.
Check actual material paint, viewport containment and stable Composer geometry
in both themes at desktop/narrow widths. Existing attached-Session model-menu
checks cover pending native confirmation, keyboard navigation and geometry.

Mismatch evidence: the initial root/model/effort material computed to
`rgba(0, 0, 0, 0)` with `backdrop-filter: none`, reproducing the supplied image.
After the theme fix, all three panes consume the reference's 58% light / 45%
dark fill and `blur(40px) saturate(150%)`. The menu remains inside the viewport,
Composer geometry and draft text are unchanged, and both choices restore focus.
The workspace tab context menu also receives the same shared material and
continues to dismiss with Escape; the task-menu checks remain green.

Only `agent-selectors-light-linux.png` changed. Enabling the reference's blur
also changes Chromium's compositing and text rasterization outside the menu.
A controlled browser check disabled just the two new material tokens before
opening the same pane: its decoded RGBA screenshot exactly matched the original
checked-in reference. This proves the extra text pixels follow the restored
material, without a typography/layout change. The refreshed stable screenshot
passed the subsequent normal strict comparison. No comparator, tolerances or
noise allowances changed; the two closed-menu dark references are identical.

Verification: 20 existing model-selection unit checks, the four new-conversation
rendered flows (1440/390px, light/dark), the material/control + tab-menu flow, and
all six selected existing browser regressions passed. Those regressions cover
pending native selection, keyboard/return focus, narrow ModelSelect, reference
screenshots and task menus. Build, TypeScript, i18n, 302 source records and 264
package notices passed. The existing large-chunk build warning remains. Page
identity, meaningful content, console/error-overlay health, actual selection
and overflow checks passed. Browser URLs were `http://127.0.0.1:5273` and
`http://127.0.0.1:5274`; Chromium was the rendered authority. Evidence lives at
`/tmp/rustx-model-surface-{before,after}-<pane>-<theme>-<width>.png` and
`/tmp/rustx-model-surface-control-{old,new}.png`. Other engines and macOS were
not exercised. No protocol, React behavior or TUI changes.

## Goal diagnostics, history windows and confirmed model display (2026-10-20)

Reference: DSH `687f282e8625c6b8d11ff58211a90524be2e4317`,
`api/session-controller/src/client/sessions/session.ts` (`PAGE_MESSAGES`,
`HISTORY_PAGE_OPTIONS`, `loadOlder`, `doOpen`) and the existing Chat/Trajectory
views using that Session history. DSH requests at least 50 messages and two
Turns rather than exposing each small transport page as a reading window.
The rustX Web client now assembles initial and older reading windows from its
unchanged native bounded pages. Transcript storage retains at most 500 records
and its existing byte ceiling; Trace keeps its existing count/byte ceilings.
Metadata, cuts, body references, live updates and explicit jump ownership remain
native. Root and child reading surfaces share the threshold policy; ordinary
contiguous initial history retains live presentation.

Goal observations are model context, not ordinary Chat content. Suppress their
native typed `goal_status` messages in the current-context fallback, loaded
transcript and Message component. Goal state remains in the existing Goal dock,
and Trace diagnostics retain their context data. No matching on rendered prose.

The model-picker regression is a deferred body transition: the checkpoint can
publish model status without its growing configuration body, and AgentControls
then fell back to Workspace settings or "Choose model" before the body reread.
Retain the last confirmed display within the same attachment while the exact
new binding is read, and reconcile native mutation before reading that body.
Unconfirmed/stale configuration still cannot authorize a selection. Native
configuration, execution, persistence, and TUI behavior are unchanged.

Browser plugin not available; use the pinned repository Playwright browser.
The target flows are: open a short history with many Goal context entries → all
Chat messages load without a manual action and Goal JSON stays out of Chat →
Trajectory loads the same short conversation automatically with context rows →
return to Chat with its draft intact. Also switch to a different model in a real
native Session, observe confirmed labels across painted frames/checkpoints and
reopen its menu. Validate desktop/light and narrow/dark, page identity, content,
console/overlay health, overflow, focus, scroll and existing lifecycle guards.

Initial hydration carries an explicit presentation flag into ChatViewport. A
passive prepend keeps a following reader at the newest message; a user-owned
scroll/jump still keeps its anchor and cancels obsolete work. Automatic loading
starts on opening the reading scope and on an explicit return to latest, not on
every cache epoch. This avoids a repeated hydration loop during child observation
replacement or a live-cut rebase. The existing scroll/navigation checks and a
new hydration-following check cover that distinction.

Native model confirmation now verifies the chosen model/profile at the current
binding revision, including its deferred body, before the menu reports success.
The deferred-body regression holds those reads and checks that the menu remains
busy, the Workspace default never replaces the confirmed label, and exactly one
model mutation is dispatched. The command fixture now reflects its acknowledged
model in the native checkpoint as well as the model domain; an ACK-only fixture
cannot stand in for confirmed selection.

The existing native Chat regression is updated to expect short history to load
automatically rather than require clicks per wire chunk. The retained image
component also retains its explicit load intent through reconnect: the browser
asserts a replacement object URL under fresh authority, a single live lease,
successful decode/lightbox and the existing preview/upload checks. No attachment
implementation or screenshot/noise policy changed.

Browser verification passed all nine selected scenarios: two short Chat/Trace +
Goal flows, two real native different-model selections, two pending model/effort
flows, two child Turn-navigation flows and the native history/rich-response/
reconnect/image/lightbox/upload workflow. Real different-model tests also prove
zero provider requests: changing configuration does not generate a response.
URLs were `http://127.0.0.1:5273` (production) and
`http://127.0.0.1:5274` (fixtures), using pinned Linux Chromium at 1440px/390px,
with light/dark and English/Chinese coverage across those scenarios. Page
identity, meaningful content, console/error-overlay health, viewport bounds,
selection/checkpoint identity, focus, scrolling, drafts and reference decoding
passed. Evidence is `/tmp/rustx-goal-history-{chat,trace}-{1440,390}.png` and
`/tmp/rustx-model-confirmed-{1440,390}.png`. Other browser engines and macOS were
not exercised; the existing large-chunk build warning remains.

Final verification: all 191 active Web test files passed, with 2,987 tests passed
and four existing skips. The 77 command checks passed again with the complete
native model DTO in their checkpoint fixture. Production build, final Web
TypeScript check, i18n ownership, all 303 source records and notices for 264
production packages passed. The nine browser scenarios above passed with the
same production implementation. No native protocol, runtime or TUI source changed.

### Plain source preview uses the full sidebar reading area (2026-10-20)

Reference: Harness `ui-sidebar-documentpreview/src/client/TextPreview.module.css`
and `text/TextBody.tsx` at `687f282e8625c6b8d11ff58211a90524be2e4317`.
Harness gives the pane body the remaining height and scrolling responsibility;
plain source pages flow inside it. The local plain-text `pre` inherited the
global 360px cap and `overflow: auto`, so the actual text scroller occupied only
the top of the pane. A real-file browser reproduction measured a 924px body
containing a 360px inner scroller for 3,097px of source. The component now removes
that cap and inner overflow and resets unwrapped source breaking. The same
3,097px document flows through the full 924px/768px desktop/narrow scrollport.
The existing text inset, font, toolbar and tab state remain component-owned.

The Browser plugin/skill is unavailable in this session; QA used the repository's
pinned Playwright 1.63 Linux Chromium container against the local workspace Host
fixture at `http://127.0.0.1:5274/test/fixtures/workbench.html` (production preview
at port 5273). An external temporary script first reproduced the old geometry,
then passed the fixed flow at 1440x1000 and 390x844: open a real Markdown file,
choose Text, read long lines and the final line, restore both scroll coordinates
after switching tabs, toggle wrapping, and resize the viewport. Page identity,
meaningful source content, console/error-overlay health, panel bounds and no
page-level horizontal overflow passed. Evidence:
`/tmp/rustx-plain-text-before-1440.png`,
`/tmp/rustx-plain-text-after-{1440,390}.png`, and
`/tmp/rustx-plain-text-wrap-{1440,390}.png`.

All three existing browser regressions for source toolbar/tab state, Markdown
viewer selection and conversation file-link/source-line navigation also passed.
The focused document/view-lifetime/workspace-Host suite passed all 45 tests;
TypeScript, production build, all 303 provenance records and 264 package notices
passed. No screenshot reference/tolerance or native/TUI source changed. Other
browser engines were not exercised; the existing large-chunk build warning remains.

### History refresh stays inside the reader (2026-10-20)

Reference: Harness `session-controller/src/client/sessions/session.ts` and
`session-controller/src/history.ts` at
`687f282e8625c6b8d11ff58211a90524be2e4317`: 50 conversational messages/two
Turns per ordinary reading window, a 500-message fetch ceiling, and accumulation
through `prependWindow`. Harness Chat retains an earlier-history button for a
real omitted prefix; normal lifecycle repair does not replace history with an
internal "History was refreshed" notice.

The local mismatch was a destructive cache rebase when an unresolved response or
Tool left the latest native head, followed by a manual reload error. Root and
child readers now retain those rows and reread exact native message identities
silently. Fresh results preserve native content bindings, row identities and the
current live cursor. In-flight rereads coalesce newer observations and are fenced
by attachment, Conversation, history epoch and navigation intent. Existing rows
remain mounted while a result is pending. Passive initial hydration does not show
its older-page button. Genuine native read failures still retain the last valid
presentation and expose the existing recovery path.

Chat's global 500-row/8 MiB retention limit and Trace's global 512-row/4 MiB
retention limit were removed. Limits remain on individual native responses,
content/field reads, eight-interest lifecycle batches and the heavy detail cache.
The 500-message ceiling now measures conversational user/assistant messages per
fetch; Context and Tool-result rows do not exhaust it. Continuation progress and
opaque-cursor cycle checks replace arbitrary page-count termination. Reading
mechanism documentation now distinguishes native page admission from retained
history instead of documenting obsolete 256-row eviction.

The Browser plugin/skill is unavailable; verification used pinned Playwright 1.63
Linux Chromium at `http://127.0.0.1:5274/test/fixtures/startup.html` and production
preview at port 5273. Tested flow: open Chat, hold an older Tool/response reread,
advance the native head, keep the original User node and scroll position, release
the result, expand its successful output and edit a retained draft. Desktop
1440x1000 and narrow 390x844 passed page identity, meaningful content, error-overlay
and console health, viewport bounds, stable DOM/reading position and absence of
the internal notice/manual-recovery button. Evidence:
`/tmp/rustx-history-repair-{1440,390}.png`.

The long-history case passed with 1,000 records in its initial 500-message fetch
and all 1,120 retained after the next real history action; earliest and latest
messages remained available together. Returning to latest also preserves all
1,120 ordinary-history rows and does not reintroduce the earlier-history button;
only explicit historical navigation releases its frozen window. Root and child
unit regressions cover that distinction. Its fixture cut now reflects the actual
frontier instead of the original 80-row case. Evidence:
`/tmp/rustx-history-long-reading.png`. All eight selected browser scenarios passed,
including short Chat/Trace hydration at both widths, child navigation at both
widths, and the real native history/rich-response/reconnect/image/lightbox/upload
workflow. No screenshot reference or comparison policy changed.

The full Web suite passed 2,992 tests with four existing skips. The final focused
run passed 39 tests, including the added opaque-cursor cycle case and retention
of 669 child records; root historical navigation also retains 1,312 rows and
Trace retention exceeds 1,024 records without eviction. TypeScript, production
build, i18n ownership, all 303 source records and notices for 264 packages passed.
No native protocol, runtime or TUI source changed. Other browser engines were not
exercised; the existing large-chunk build warning remains.

After the final ordinary-return-to-latest correction, all 78 focused transcript,
history-policy, child-reader and navigation tests passed again. The correction
preserves accumulated ordinary history while explicit historical jumps still
return to their independently current live head.
All eight browser scenarios then passed again against the final production build,
including the no-reload/no-eviction assertion after returning to the latest of
1,120 loaded records. The final TypeScript and provenance checks also passed.

### Between-turn labels follow semantic membership (2026-10-20)

Reference: Harness `ui-trajectory/src/client/layout.ts`, `TrajectoryTable.tsx`
and `trajectory-search-index.ts` at
`687f282e8625c6b8d11ff58211a90524be2e4317`. Harness explicitly models standalone
compaction sections with a null Turn. The local ledger instead marked every
unowned native record between Turns, and the Inspector used the same label as a
fallback whenever a Turn was absent. This mislabeled native child activations.

One shared semantic classifier now serves the ledger, Inspector and bilingual
search. Compaction checkpoints and unscoped reused conversation material retain
the requested between-turn label. Native Subagent, Background, Workflow and Tool
activities keep their actual ownership, including absent ownership; no nearby
Turn is inferred from position or Tool-call IDs. Native records, row identities,
selection and timing are unchanged. Tests cover scoped child Turn/Step ownership,
fold/unfold selection, unscoped activities, and both search vocabularies.

The Browser plugin/skill is unavailable; QA used pinned Playwright 1.63 Linux
Chromium at `http://127.0.0.1:5274/test/fixtures/trajectory.html` and the startup
fixture, with the production preview on port 5273. Desktop 1440x844 English/dark
and narrow 390x844 Chinese/light passed semantic labels, selection, close/focus
restoration, search without extra detail reads, meaningful page identity,
console/error-overlay health and viewport bounds. Evidence:
`/tmp/rustx-trajectory-child-section-{1440,390}.png` and
`/tmp/rustx-trajectory-between-section-{1440,390}.png`.

Before-edit screenshot runs exposed stale references that omitted even the
already-requested User/compaction gutter labels. Comparing the before-edit actual
with the fixed actual changed only x=0..71, y=583..624: 1,560 pixels at 1440 and
1,379 at 390, removing the two incorrect native activity labels. After visual
inspection, eight ledger/Inspector references were refreshed for the legitimate
User/compaction labels. Their reference changes are confined to x=0..71,
y=153..564 (x=0..30 for the narrow Inspector overlay). All four strict screenshot
replays passed without changing comparison tolerances. The existing child
navigation regression now checks automatic history hydration instead of waiting
for the obsolete manual earlier-page action; it still verifies child-owned
trace/detail reads, draft retention and parent/child switching at both widths.

All 138 focused trajectory/timing tests and ten selected browser scenarios passed,
including English/light and Chinese/dark native compaction continuation. TypeScript,
production build, i18n ownership, all 303 source records and notices for 264
packages passed. No native protocol, runtime or TUI source changed. Other browser
engines were not exercised; the existing large-chunk build warning remains.

### Agent interactions use Tool and Context instead of activation activity (2026-10-20)

Reference: Harness `ui-trajectory/src/client/index.ts` and
`trajectory-message-definitions.ts` at
`687f282e8625c6b8d11ff58211a90524be2e4317`. Its registered trajectory definitions
publish messages, requests, Tools and compaction; messages from an Agent enter as
Context. There is no separate Agent activation activity row. The prior local fix
removed an incorrect between-turn label but still exposed this extra native
lifecycle dimension as a selectable row and Tools-lane span.

The shared Web projection now excludes native `subagent` activation records
before constructing sections, as it already excludes interaction lifecycle
audits. Ledger rows, Inspector targets, bilingual search, both timeline modes
and Turn/Step membership therefore agree, including activation-only pages.
Obsolete activation-specific narrow labels and the timeline style were removed.
Actual delegation/control Tool calls and delivered Context messages retain their
native identities, scope and detail reads. Trace cache facts and Agent controls
remain intact; records are not relabeled as invented Context.

Browser plugin not available: QA used the pinned Playwright 1.63 Linux Chromium
container, fixture origin `http://127.0.0.1:5274` and production preview origin
`http://127.0.0.1:5273`. The dedicated `/test/fixtures/trajectory.html?agent-interactions`
flow includes running/completed/cancelled/failed activations with and without
scope: open trajectory, search the actual Tool, inspect its recorded result,
close and restore keyboard focus, search the delivered Context, then apply a
lifecycle refresh. Desktop 1440x844 English/dark and narrow 390x844 Chinese/light
passed meaningful page identity, console/error-overlay health, viewport bounds,
unchanged native record count and absence of activation rows/spans/search hits.
Evidence: `/tmp/rustx-agent-trajectory-{1440,390}.png`.

The real native continuation scenario also passed: create a child, wait and
interrupt its initial activation, resume the same Agent, wait for success, return
to the parent and inspect the original delegation Tool result. Parent trajectory
retains the Tool and delivered Context without the two activation audit rows.
Evidence: `/tmp/rustx-agent-native-trajectory.png`. Existing child navigation,
draft retention, receipts, independent child trajectory and native compaction
continuation passed alongside the full trajectory browser suite: 56 scenarios.

All four before-edit ledger/Inspector screenshot scenarios passed. After visual
review of the intended row removal and timeline redistribution, 13 affected
references were refreshed, with no comparison tolerance/noise-policy changes.
Pixel differences are confined to timeline y=72..121, removed/moved activity rows
and their old bottom boundary at y=573..642; the two resize references also
receive the existing User/compaction between-turn gutter labels at x=0..71.
All strict screenshot replays passed as part of the 56-scenario run.

All 139 focused trajectory/timing tests, TypeScript, production build, i18n
ownership, 303 source records and notices for 264 packages passed. No native
protocol, runtime or TUI source changed. Other browser engines were not exercised;
the existing large-chunk build warning remains.

### Composer editor focus keeps the neutral Harness card edge (2026-10-20)

Reference: Harness `ui-conversation/src/client/skeleton/InputBar.module.css` at
`687f282e8625c6b8d11ff58211a90524be2e4317`. Its card retains the neutral border-l2
elevation stroke while editing. The local textarea outline was already removed,
but a separate `card:has(input:focus-visible)` override recolored the whole card
blue. That override is removed from the shared composer stylesheet; new, parent
and child editors keep the same card edge and shadow before and after focus.

Browser plugin not available: an outside-repository Playwright script used the
pinned 1.63 Linux Chromium container at the startup fixture on port 5274 (production
preview on port 5273). All four before/after scenarios passed at 1440x844 and
390x844 in light and dark themes, exercising new-conversation, parent and child
editors. The flow is workspace selection or child navigation, pointer focus,
draft editing, Tab/Shift+Tab focus return and action-button keyboard focus.
After the fix, computed card paint and geometry remain unchanged by editor
focus, the draft survives, and action buttons keep their keyboard outlines.
Page identity, meaningful content, console/error-overlay health and viewport
bounds passed. Evidence: `/tmp/rustx-composer-focus-{before,after}-{hero,root,child}-{1440,390}-{light,dark}.png`.

The existing Agent browser suite passed all 23 scenarios, including strict
screenshots, growth/scrolling, restored drafts, send/queue/steer, uploads, model
menus, both locales' keyboard policies and preference reload. Sixteen focused
composer references were updated after visual review: only the rounded card
edge changes, dimensions are identical and the central interior has zero changed
pixels. Screenshot tolerance and noise policies are unchanged. TypeScript,
production build, all 303 source records and notices for 264 packages passed.
No native protocol, runtime or TUI source changed. Other browser engines were
not exercised; the existing large-chunk build warning remains.

### Child observation recovery and continuable composer parity (2026-10-20)

Reference: Harness `ui-conversation/src/client/skeleton/InputBar.tsx`,
`ui-subagent/src/client/SubagentReadOnlyComposer{.tsx,.module.css}` and
`ui-primitives/src/ConnectionIndicator{.tsx,.module.css}` at
`687f282e8625c6b8d11ff58211a90524be2e4317`. Continuable children retain Send as
the primary action and expose an independent Stop immediately before it. The
shared rustX composer now follows that rule for child views, including empty or
admission-blocked editors; root primary behavior is unchanged. Double Escape is
fenced to the exact child activation and root attachment. Model/permission seats
use the exact child checkpoint and are read-only under the current native child
API; no parent model/catalog is substituted. Unavailable children use the compact
neutral read-only frame and localized title/body.

A failed live child read previously bypassed the post-read registry check and
printed the raw RPC exception in the chat flow. Runtime Client now rechecks the
same activation even after a live read fails. Only a proven registry transition
to Inactive or Unavailable authorizes `resync_required` to retained native
history. An IPC failure while still Active remains `observation_unavailable`;
it cannot fabricate completion, trigger execution, or substitute a historical
answer. The original total read budget and inspection capacity remain intact.
Real Unix tests close the inspection connection during initial and following
reads both before and after actual child settlement, then verify canonical
retained content and exactly one model execution.

Web observation failures use compact amber connection feedback above the child
composer, adapted from Harness ConnectionIndicator. Localized retry/connecting
states retain coherent content, the resident editor and its draft. Technical
diagnostics remain available in an explicit Details disclosure. Retrying resets
only the child read cursor under the existing validated source owner; it never
replays Send, Stop or model work. Error ownership is invalidated with the view,
activation or attachment. Existing native header controls remain available.
Four newly attributed source records distinguish the small adapted presentation
from rustX-owned lifecycle, observation and mutation authority; historical pins
remain unchanged.

All 10 native child-observation boundary tests (including four physical close
cases), 107 focused frontend checks, Rust formatting, TypeScript, production
build, i18n ownership, 307 source records and notices for 264 packages passed.
No protocol shape or TUI presentation changed. The existing large-chunk build
warning remains.

Browser plugin not available: QA used the repository's pinned Playwright 1.63
Linux Chromium container at fixture origin `http://127.0.0.1:5274` and production
preview origin `http://127.0.0.1:5273`. All 40 scenarios passed, including the real
native interrupt/wait/resume workflow, existing root strict screenshots and
keyboard policies, child breadcrumbs/aside navigation, retained drafts,
independent child trajectory and four controlled read-failure/recovery flows.
The recovery flows cover 1440x900 English and 390x900 Chinese in both themes:
enter child, retain evidence and draft, fail an actual typed read, disclose and
close diagnostics, retry one cursor-free read while preserving the same editor
node, then receive fresh canonical content without execution/control replay.
Page identity, error-overlay/console health and viewport bounds passed.
Visually inspected desktop dark and narrow light failure states and narrow dark
recovered state: the compact warning sits above the card, Send and Stop fit the
same row, and the native model/permission seats collapse on narrow layouts.
Evidence: `/tmp/rustx-child-{recovery,reconnecting,recovered}-{dark,light}-{1440,390}.png`.
No screenshot references or comparison policies changed. Other browser engines
were not exercised.

### Direct-parent tasks use the child UserMessage seat (2026-10-20)

Reference: the supplied DSH child-view screenshot and Harness
`ui-chat/src/client/chat/MessageItem.tsx` (`UserMessageNodeView` /
`UserStyleBubble`) at `687f282e8625c6b8d11ff58211a90524be2e4317`. The reference
shows the parent task as an expanded, right-aligned user bubble with ordinary
timestamp/copy actions. The local predicate admitted only Human sources, so a
child's actual `Agent(parent)` task and follow-up guidance instead became folded
Context rows. The shared native adapter now uses the exact receiver's direct
parent identity for this presentation decision. Body, whitespace, flow spacing,
actions and current-message visibility reuse the existing UserMessage seat.
No canonical source, native role, turn ownership or trajectory classification is
rewritten. Sibling/child reports and injected Context keep their own presentation.
Nested-child tests require a match to the direct parent, and canonical-head to
journal settlement retains the same bubble and Copy button nodes.

The child model selector introduced in the previous change is removed at the
user's explicit request, including when a native child checkpoint contains model
settings. The shared ordinary composer placeholder replaces the extra inactive
resume prompt; its obsolete bilingual dictionary entries are removed. The exact
native permission label and independent Send/Stop behavior remain in the child
seat. These changes apply to both the main conversation and sidebar child view.

TypeScript, i18n ownership, production build, all 81 focused frontend checks,
307 source records and notices for 264 packages passed. Historical source pins
and screenshot comparison policies are unchanged. No native or TUI source changed.

Browser plugin not available: QA used the pinned Playwright 1.63 Linux Chromium
container, fixture origin `http://127.0.0.1:5274`, production preview origin
`http://127.0.0.1:5273`. All 44 browser scenarios passed. The real native child
launch/interrupt/wait/resume workflow now asserts that its delegated task uses
the parent-message UserMessage article and has Copy, with no child model control.
Four new reference-driven flows cover 1440x1000 English and 390x1000 Chinese in
both themes: enter a child, read initial task and follow-up guidance, verify
right alignment/wrapping/native timestamps, copy the exact original multiline
text to the clipboard, switch to trajectory and back with the draft preserved,
return to the parent with its draft preserved, then open the same child in the
sidebar. Both child mounts have user bubbles and no model selector, while the
parent's returned Agent report remains contextual. Page identity, meaningful
content, console/error-overlay health and viewport bounds passed.

Visually inspected main desktop dark, main narrow light and sidebar desktop
dark screenshots against the supplied reference: shared rounded user bubble,
ordinary message footer, preserved line breaks and neutral composer. Evidence:
`/tmp/rustx-parent-bubbles-{main,aside}-{dark,light}-{1440,390}.png`.
Existing root strict references, child recovery and child trajectory checks also
passed; no screenshot baselines changed. Other browser engines were not tested.
The existing large-chunk build warning remains.

### Chat file attachments use the Harness user-content stack (2026-10-20)

Reference: the supplied `compose.yml` comparison and Harness
`ui-chat/src/client/chat/UserContent.tsx` / `MessageItem.module.css` at
`687f282e8625c6b8d11ff58211a90524be2e4317`. The old native adapter rendered every
user block through the text bubble, including a generic AttachmentCard with a
bold filename and generic File attachment label. User content now separates an
ordered attachment row above ordinary text, reusing the existing rounded file
tile, 28px FileTypeIcon, filename, uppercase extension and 8px stack gap. Empty
text bodies are omitted for attachment-only messages. Multiple file tiles wrap
and long names retain their full title while truncating visually.

The native adapter partitions by original block indices rather than filtering
the underlying content array: deferred text after attachments still reads its
advertised exact field. This applies to Human and direct-parent child messages
in both conversation mounts. Managed file artifacts use the same tile and open
the existing original-source preview from the card without eager byte reads;
the preview retains its native download path. Tool artifact presentations retain
their existing resource controls. First-submit pending files use the same tile
and actual local File size. Historical uploaded-file references currently expose
only batch identity and filename, so their card omits size rather than inventing
0B, deriving a host path or retaining a browser-only historical size cache.

All 94 focused frontend checks, TypeScript, i18n ownership, production build,
307 source records and notices for 264 packages passed. No native protocol,
message source, runtime or TUI source changed. Historical source pins remain
intact; the two shared presentation records add the exact UserContent baseline.

Browser plugin not available: validation used the pinned Playwright 1.63 Linux
Chromium container, fixture origin `http://127.0.0.1:5274` and production preview
origin `http://127.0.0.1:5273`. All 51 final scenarios passed. The new flows cover
1440x1000 English and 390x1000 Chinese in both themes: open the conversation,
verify 240x64 file cards with 28px icons above the independent text bubble,
copy only the original text, inspect file-only wrapping/long names, preserve the
root draft while entering a child, then open that child in the sidebar. Page
identity, meaningful content, error-overlay/console health, geometry and viewport
bounds passed. Visual review covered desktop dark, narrow light and narrow dark
sidebar states. Evidence: `/tmp/rustx-message-files-{dark,light}-{1440,390}.png`
and `/tmp/rustx-message-files-{child,aside}-{dark,light}-{1440,390}.png`.

Existing picker/drop/rail, uncertain upload recovery, first-submit ACK loss and
remount ownership, native child execution and parent-message flows also passed.
Two stale startup test locators were updated from the removed global Check status
button to the existing filename-specific card action; their original operation,
receipt, no-replay and explicit continuation assertions remain intact.

Seven before-update Agent comparisons rejected the intentional layout change.
After reviewing actual/diff evidence against Harness, nine affected references
were refreshed and every strict comparison passed in the final replay. All PNG
dimensions are unchanged; pixel differences stay inside the conversation content
region (desktop x=436..1276, narrow x=123..362), with zero changed pixels in the
sidebar or top chrome. The input/toolbar screenshots remain unchanged. No noise
allowances or comparison tolerances changed. Other browser engines were not
exercised; the existing large-chunk build warning remains.

### Compaction markers retain their historical positions (2026-10-20)

Reference: Harness `ui-chat/src/client/conversation-nodes/compaction.ts` and
`command.ts` at `687f282e8625c6b8d11ff58211a90524be2e4317`. The automatic marker
uses the committed checkpoint event's sequence, and manual compaction shares the
conversation node sequence. The native adapter previously appended ContextSeat
below every transcript node, so the latest checkpoint followed every later reply.

The original native activation now captures a typed transcript anchor in its
finite, integrity-checked display header. It observes pending inputs, publication
audits and terminal controls as well as canonical messages. Inherited activations
use the destination's translated canonical prefix, not the source's transcript
coordinate. Projection directory/detail reads retain the same original anchor;
Ledger frontiers and Journal sequences never become browser ordering guesses.
This is display metadata; it does not create a canonical message or alter the
model's compacted history, compaction admission, or TUI interaction logic.

Chat reads checkpoint metadata from the existing native projection directory,
scoped to the exact attached observation. Each checkpoint has its own stable
historical seat after the corresponding response tail. Later replies and later
compactions preserve earlier markers and open summary state. Markers outside a
loaded window, or inside its unloaded gap, do not attach to unrelated content.
Process groups split at the marker boundary so later reasoning/Tools remain below
it. ContextSeat now renders only live operation/receipt feedback; completed
checkpoint summaries no longer occupy the permanent bottom seat. Summary styling
and lazy full-summary inspection use the existing Harness presentation primitives.

Validation passed: 57 focused frontend checks; 17 native anchor, original-head,
receipt, integrity and observation-boundary checks; WebUI and protocol TypeScript;
i18n ownership; production build; 307 source records and notices for 264 packages.
The native tests explicitly distinguish accepted-but-not-canonical T positions
from Ledger positions, retain the first anchor after a later commit, and translate
an inherited activation into a different destination T position.

Browser plugin not available: visual/interaction QA used the pinned Playwright
1.63 Linux Chromium container at fixture origin `http://127.0.0.1:5274` and
production preview origin `http://127.0.0.1:5273`. All 33 scenarios passed,
including the existing strict Agent/Composer references without baseline changes.
The compaction matrix covers English/Chinese, light/dark and 1440/390 widths:
collapsed/expanded summaries, subsequent prompts/replies, a second checkpoint,
preserved historical ordering and usable disclosure without horizontal overflow.
The real native flow covers manual and automatic compaction, continuation,
questionnaires, reconnect, full page reload and interrupted output. It verifies
both checkpoints survive and the automatic marker remains before the subsequent
answer after reload. Desktop dark and narrow light captures were visually
reviewed; the native narrow dark flow was also reviewed. Evidence:
`/tmp/rustx-compaction-position-{en,zh}-{light,dark}-{1440,390}.png` and
`/tmp/rustx-compaction-zh-dark.png`. Other browser engines were not exercised.
The existing large-chunk build warning remains.

### Live trajectory inspection keeps its resident content (2026-10-20)

Reference: Harness `ui-trajectory/src/client/TrajectoryTable.tsx` at
`687f282e8625c6b8d11ff58211a90524be2e4317` keeps the selected record/detail pane
resident while its semantic cells update. The rustX adapter instead superseded
all reader-owned detail on each native checkpoint/domain event, then invalidated
cached bodies for changes to timing, status, usage or Turn placement. The detail
hook also hid its loaded body while reading a replacement cut. Together these
paths repeatedly removed the right-hand content and its interactive state.

Ordinary overlapping live updates now preserve the original native detail page,
reader and content cut when its content selectors are unchanged. Native payload
changes mark the detail stale and cause one background read; the old content
remains displayed until the new descriptor and all required fields are ready.
A per-read token lets native changes queue one successor read without repeatedly
cancelling the in-flight read. Root and child readers share this cache behavior.
The Inspector keeps its tab, JSON tree, reasoning disclosure, focus and scroll
position. Failed replacement reads preserve content and expose the existing
error presentation. New selections, native interval epochs, attachment changes,
explicit resynchronization and child retirement still fence obsolete readers.

Native summary reads have a separate freshness check: a response launched before
a newer native Trace head cannot overwrite that head, and one coalesced read
repairs its interests. This does not revoke the resident detail's immutable
source merely because the live summary advanced. No runtime execution, provider,
native protocol, or TUI interaction logic changed.

Validation passed: 245 focused tests covering root/child live inspection, finite
cache limits, delayed response ordering, deferred native fields, original Tool
and message sources, cut replacement, resync, attachment replacement and child
retirement; TypeScript, i18n ownership, production build, 307 source records and
notices for 264 production packages also passed. A new delayed-summary regression
failed against the intermediate implementation before the ordering guard was
added, then passed with the rest of the final checks.

Browser plugin not available: rendered QA used pinned Playwright 1.63 Linux
Chromium, fixture origin `http://127.0.0.1:5274` and production preview origin
`http://127.0.0.1:5273`. All 14 final browser scenarios passed. The new flow opens
Session A at `test/fixtures/startup.html?existing&models&trace-live`, selects the
Trajectory request and Options, runs 50 native updates while summary/header/field
replies are held, then completes the new source. The body stays readable until
the replacement is ready. A MutationObserver saw zero panel/tree detachments and
zero loading-state appearances; tab focus, scrollTop=80 and the original tree DOM
survived. Detail reads stayed at one for lifecycle changes and two for one actual
body update; stale summary replies coalesced one follow-up read. Tests cover both
themes at 1440x1000 and 390x1000, with English desktop and Chinese narrow UI.

The existing eight native/Harness visual-parity combinations and desktop/narrow
child trajectory navigation also passed, with no screenshot baseline changes.
Page identity, meaningful content, error overlays, console health, interaction
state and horizontal viewport bounds passed. Desktop dark and narrow light
captures were visually reviewed. Evidence:
`/tmp/rustx-trace-live-{before,after}-{dark,light}-{1440,390}.png`; these show the
resident view before and after the stress flow, not a before-fix capture. Other
browser engines were not exercised. The existing large-chunk build warning
remains.

The real native `trajectory-integration.spec.ts` workflow also passed: Settings
save/reread, busy admission, exact adoption and immutable historical Trace
inspection through a running App Server. This brings the final browser validation
to 15 passing scenarios, including the 14 presentation/stress/child scenarios.

## Office sidebar admission regression

The reported PPTX failed before conversion: its ordinary binary printer-settings
part was rejected as executable binary content, and its web hyperlinks were
rejected as external resources. Admission now distinguishes the standard OOXML
printer-settings location and inert HTTP/HTTPS/mailto hyperlinks. Other binary
parts, macros, ActiveX, embedded packages, external images/templates/workbooks,
dangerous link schemes, ZIP traversal, expansion and CRC violations remain
rejected. Both default and per-part active content declarations are checked, so
active MIME declarations cannot hide behind a printer-settings filename. The
existing isolated converter and PDF viewer still have no network/link-action
authority. Source bytes and document sizes are unchanged.

The Files sidebar now shares the document preview's classified, localized failure
mapping instead of showing a raw WorkspaceHostError. The existing toolbar refresh
retries the read. DSH's Office provider and OfficeBody at
`687f282e8625c6b8d11ff58211a90524be2e4317` were inspected; the existing Host
authorization/conversion boundary and shared PDF presentation remain the owners.
The original reported file was read without modification and successfully
converted to a complete 10-page PDF. Its bytes are not committed. New DOCX/PPTX
fixtures are original MIT content generated by the existing Python fixture tool.

Validation passed: 116 focused tests, with four macOS-only cases skipped on Linux;
TypeScript, i18n, production build, 307 source records and 264 production package
notices. Six existing advanced-document/workbench browser scenarios passed. The
new Office tests initially used an incomplete accessible button name; the test
selector was corrected, and both final English and Chinese scenarios passed.
They render DOCX/PPTX through real Host conversion, check selectable PDF text and
canvas output, force a classified failure, refresh back to a rendered document,
and verify zero external requests and zero uncaught page errors.

Browser plugin not available: rendered verification used pinned Playwright 1.63
Linux Chromium with fixture origin `http://127.0.0.1:5274` and production preview
origin `http://127.0.0.1:5273`. Both 1440x1000 screenshots were visually reviewed:
`/tmp/rustx-office-sidebar-{en,zh}.png`. No screenshot references or comparison
tolerances changed. Other browser engines and macOS conversion were not exercised
on this Linux host. The existing large-chunk build warning remains.

## Office and PDF continuous-reader alignment

The Office screenshot reference uses a continuous PDF scrollport, fit-width pages,
12 px page gaps and floating zoom controls. The previous rustX viewer instead had
native form controls and one selected page. `PdfPreview` now adapts DSH's PDF page
presentation and CSS at `687f282e8625c6b8d11ff58211a90524be2e4317`, reusing the
existing source-derived `ZoomViewport` and `ZoomControls`. PDF and converted Office
share this surface. Single-viewer files no longer show an empty viewer choice;
the extra derived-preview paragraph and obsolete single-page/text-layer paths
were removed. Tab-local fit/fixed zoom and scroll metadata replace the page picker.
Geometry uses the source's 96/72 CSS scale while admission remains in PDF points.

Native Worker ownership, serial render settlement, finite page/text/pixel limits,
source authorization and converter isolation remain rustX-owned. Page geometry is
held independently from viewport-local bitmaps. Zoom replacements publish their
complete bitmap and matching text together. Leaving the reading window releases
backing stores; closing a preview retires every owned page and Worker.

Rendered QA found and fixed three integration mismatches:

- The zoom menu's minimum width was applied to its anchor. It now uses Menu's
  `listClassName`; source-sized buttons override the native Button padding. The
  floating toolbar measures 132 px in pinned DPR=1 Chromium (28/58/28 px controls),
  including device-rounded hairline borders. The menu keeps its compact 112 px
  width token without widening the trigger.
- Standard-font rendering requested resources that were not supplied. The lazy
  reader now bundles exact-version PDF.js standard fonts and CMaps through the
  source-derived binary resource factory, without network fallback. The Adobe,
  Foxit and Liberation data licenses are included in generated production notices.
- The original presentation's normal knockout text/card groups needed more than
  eight temporary canvases; the ninth request was only 12x8 px. The count budget
  is now 128 while the 16M aggregate scratch pixels and 4M per-canvas pixels stay
  enforced. Completed page renders also clear their temporary surfaces. An
  intermittent rapid-switch failure was separately traced to `reader.cancel()`
  without the Error reason required by PDF.js 6. Cancellation now marks the
  SDK controller closed before late Worker chunks arrive, with a regression
  assertion for the Error reason. No global error suppression was added.

Validation passed: 131 tests across 13 focused files; TypeScript, i18n ownership,
production build, 311 source records and notices for 267 production entries. All
13 final browser scenarios passed, including real native dual-PDF/Office workspace
lifetime, rapid tab/pane changes, narrow layouts, original downloads, selection
and copy after zoom, localized failure/refresh, continuous page scrolling,
fit-width/fixed zoom and restored reading positions. The copy check waits for the
matching completed bitmap/text replacement. No screenshot references or comparison
tolerances changed. The lazy PDF chunk includes the font/CMap data; the existing
large-chunk build warning remains.

Browser plugin not available: rendered QA used pinned Playwright 1.63 Linux
Chromium, fixture origin `http://127.0.0.1:5274` and production preview origin
`http://127.0.0.1:5273`. The Office matrix covers English/Chinese, light/dark and
1440x1000/390x1000. Desktop light and narrow dark captures were visually reviewed:
`/tmp/rustx-office-continuous-zh-{light-1440,dark-390}.png`.

The user's original PPTX was copied only into the ephemeral, authorized fixture
workspace and opened through the real Workbench Host conversion. All 10 pages
were scrolled and rendered without alerts, uncaught errors or external requests.
At 1900x1000 the pages fit at about 65%, with a measured 12 px gap; offscreen
backing stores were released while geometry remained. The reference capture was
visually reviewed: `/tmp/rustx-office-reference-aligned.png`. The user's bytes
were not changed or committed. Other browser engines and macOS conversion were
not exercised here. No native protocol, Agent execution or TUI logic changed.

### Workflow one-shot child observability (2026-10-20)

Reference: DSH `687f282e8625c6b8d11ff58211a90524be2e4317`,
`ui-workflow-run/WorkflowRunPanel` and `ui-subagent/SubagentReadOnlyComposer`.
The target flow is: invoke a native Workflow -> open a running Agent node ->
inspect that child's Chat/Trajectory -> retain the record on settlement ->
return to the original parent conversation. Browser plugin not available;
rendered checks use the repository's pinned Playwright 1.63 Linux Chromium,
production preview at `http://127.0.0.1:5273`, and fixture origin at port 5274.

Mismatch ledger:
- Workflow nodes previously rendered only text/status. DSH's running member
  link is now adapted to the exact native `child_agent_id` + activation pair.
  It becomes non-navigable on settlement, retaining an already-focused button
  until blur. Parent conversations and visited child records remain resident.
- Native read routing previously admitted only durable, continuable Agents.
  Inspection now resolves Workflow-owned executions from their real Registry
  ownership for observation, content, history, Trace, delivery and statistics.
  No durable Agent record is created; message, resume and interrupt admission
  still reject these finite executions. They remain outside the native Agent
  inventory and do not create synthetic parent Trace lifecycle activities.
- Child input is displayed in the existing user bubble; adjacent canonical
  text blocks now have a paragraph boundary, retaining original block indices.
  DSH's localized one-shot record title/body replaces the composer. No model
  selector or follow-up input is added. Narrow breadcrumbs reserve space for
  the selected child title instead of allowing the parent title to consume it.
- rustX keeps native block/node topology, review gates and execution budgets.
  DSH Session ownership, event assembly and executable workflow semantics are
  excluded; the TUI interaction logic is unchanged.

Initial verification: 110 focused WebUI tests, a native Registry ownership/
control regression, WebUI and protocol TypeScript, i18n, production build,
311 source records and 267 notice entries passed. The first three real native
browser cases passed at 1440x1000/light and 390x1000/dark in English/Chinese;
source-driven final settlement and existing child regression checks follow.
No screenshot reference or comparator tolerance was changed.

Final rendered validation passed: all 12 browser cases, including the existing
four-theme/viewport parent-message and observation-recovery matrices. The new
real native `web_workflow_observation` case executes a permitted `read`, opens
its Result detail from the child-owned Trace, then commits `workflow_output`.
The selected inspector DOM, Result tab and contents remain resident across
native settlement; the header becomes Completed. The Trace reader now follows
its exact observation cursor and is not replaced by a lifecycle-only change.
`workflow_output` remains a reserved terminal protocol, never a fabricated Tool
execution. The initial settlement probe incorrectly assumed an ordinary Tool
row for that protocol; it was replaced by the actual native `read` scenario.
The browser selector also uses the Tool inspector's existing Result tab.

Final page identity, meaningful content, no framework overlay, relevant console
health, real interaction and screenshot checks passed. Screenshots reviewed:
`/tmp/rustx-workflow-child-{chat,trace,completed}.png` and
`/tmp/rustx-workflow-child-zh.png`. The desktop screenshot shows the real read
result; the narrow dark screenshot shows the selected breadcrumb, ordinary
user bubble and localized read-only record frame without horizontal overflow.
Verification commands: focused eight-file Vitest run (110 tests), native
`a_workflow_child_closes_native_lifecycle_without_parent_inbound`, WebUI/protocol
TypeScript, i18n, provenance/notices, production build and the repository pinned
browser runner. No reference baselines, comparison tolerances or TUI interaction
logic changed. Other browser engines and operating systems were not exercised.

### Workflow execution path (2026-10-20)

Reference: the same DSH revision's `ui-workflow-run/workflow-definition.ts`
adds members only on actual Agent start. rustX now appends its own typed node
records only after native cancellation/budget admission, under the same lock
that orders parallel admissions. Block entry no longer pre-registers graph
nodes. Unselected branches and nodes cancelled before admission have no row;
the existing native cut preserves start order through waits, settlement and
reconnect without browser clocks or Journal reconstruction.

Chat displays one chronological list across concrete blocks and Loop iterations,
with scoped paths/instance markers on nested nodes. Definition-block grouping
and the unused "Not run" labels were removed. Exact native identities keep rows
and focused one-shot links stable during updates; review state and consumed
execution budgets retain their independent meaning. TUI formatting, expansion,
focus and navigation code is unchanged.

The rendered target flow is: start Workflow -> see only its admitted child ->
wait at Review -> reconnect with the same ordered path -> accept/reject -> see
only the chosen successor in start order. Browser plugin not available; use the
repository's pinned Playwright Linux Chromium with production preview at
`http://127.0.0.1:5273` and fixture origin at port 5274.

Validation passed: 124 native Workflow/read-source tests, 72 WebUI tests across
five focused files, 56 TUI projection tests, WebUI TypeScript, i18n, production
build, 311 source records and notices for 267 production entries. The native
tests compare real nested Loop/Parallel start facts against display order, test
both branch choices and reject phantom rows before/cancelled admission. The
React checks cover interleaved/repeated identities and focused child settlement.
Edited Rust files pass rustfmt; the repository-wide formatting check still
reports the existing unrelated signature in `runtime_client/projection.rs`.

All three real native browser cases passed, including English/light desktop
(1440x1000), Chinese/dark narrow (390x1000), accept/reject, reconnect, child Chat
and Trace inspection, and terminal collapse. Page identity, meaningful content,
absence of framework overlays, relevant console health and user interactions
passed. Completed screenshots were visually reviewed:
`/tmp/rustx-workflow-completed-en.png` and
`/tmp/rustx-workflow-completed-zh.png`. No screenshot baselines or comparator
tolerances changed. Other browser engines and operating systems were not tested.

### Pre-PR self-review (2026-10-20)

The complete branch was reviewed against `origin/main`, including native child
inspection/control separation, compaction transcript anchors, history repair,
model confirmation, response retry, upload settlement and document lifetimes.
The review found that every retained Composer registered document drag listeners,
including hidden parent/child views. A failing reproduction produced two portal
overlays for one drag. Document drops now belong to the visible targeted or
focused Composer; otherwise the first visible owner claims the event once.
Hidden/inert owners and already-handled events cannot upload or render a mask.
Native upload admission, receipts and uncertain settlement are unchanged.

The regression checks actual parent/child resident switching and exact upload
preparation counts, as well as direct target/focus selection in the unit suite.
The full unit pass also exposed outdated Workflow disclosure, retained Trace-cut
and PDF owner fixtures. They now assert the current single ordered node list,
coherent immutable read cut and page-size loading contract. Native inspection
checks and nested-path assertions were separated to satisfy strict lint without
disabling rules; immutable checkpoint display conversion borrows its input.

Office acceptance now waits for the real Host conversion response before checking
rendered pages. This fixes the cold-start race without increasing assertion
timeouts or altering screenshot comparators. All four locale/theme/viewport
Office cases passed after this change. The other 25 selected browser cases
passed, including attachment intake, resident child navigation, Stop, retry,
session actions, model confirmation, compaction, history refresh and live Trace.

Browser plugin not available: validation uses the same pinned Playwright 1.63
Linux image, fonts and read-only Node modules as `scripts/browser-tests.sh`,
preview origin `http://127.0.0.1:5273` and fixture origin at port 5274. The current
all-feature Rust executables were reused while the complete native test command
owned Cargo's lock, avoiding a feature-changing rebuild during its execution.
The drop-ownership screenshot was visually reviewed:
`/tmp/rustx-review-drop-owner.png`. No reference image changed.

Verified: 3050 WebUI tests (4 existing skips), 1180 TUI tests (1 existing skip),
41 dev-launcher tests, all four TypeScript surfaces, production build, i18n,
311 source records, 267 dependency notice entries, repository-wide rustfmt and
all-target/all-feature Clippy. Full native results are recorded in the PR.

The affected native module pass completed with 324 tests passing and one existing
skip, including Workflow ordering/inspection, App Server transport, Runtime Client
projection and the shared Workflow wire round-trip. Its shared JSON sample now
includes the nullable `child_agent_id` field, matching native serialization; the
six TUI Workflow projection tests passed against that updated sample.

The local all-target/all-feature safety-net run was not a clean pass: 4389 tests
passed, four were ignored, the stale sample failed, and two outer liveness guards
expired. The sample was repaired and passed; the unchanged model-selection and
large-write progress cases passed individually. A repeated combined run again
expired the model-selection guard and a delivery-access guard, and was stopped.
The unchanged delivery-access case passed individually as well. No product
deadline or test assertion was weakened. Per `tests/README.md`, outer watchdog
expiry is a harness failure rather than a semantic verdict; the final PR is
promoted to FULL verification on its committed head.

The final protocol regeneration/check passed with no generated-file drift;
all-target/all-feature Clippy and repository-wide formatting also passed after
the shared fixture correction.

### PR #488 review corrections (R1–R7)

The retained Harness references above remain unchanged (read-only checkout
`687f282e8625c6b8d11ff58211a90524be2e4317`). These changes repair rustX adapters:
Workflow inspection now keeps ended children navigable and routes abandoned
terminal publication to retained evidence without claiming success; retry prefixes
truncate inline and deferred history consistently; fork receipts survive naming
failure and generated titles respect native Unicode scalar bounds. The product
`/tools` directory automatically hydrates native collection/value continuations
at one attachment/read cut. PDF render deadlines begin when the serialized slot
actually starts, and queued cancellation stays independent. Compaction preview
truncation remains explicit in Summary and Raw output.

Regression coverage includes publication-failure injection through native
inspection, settled/failed/cancelled child navigation, deferred replay inputs and
outputs, bounded/deferred tool directories with attachment replacement, Unicode
fork naming and post-commit refusal, controlled PDF queue timing/cancellation, and
truncated historical summaries. Browser plugin not available; rendered verification
uses the repository's pinned Linux Playwright runner. No upstream checkout or
screenshot tolerance is changed by these review corrections.

Review validation: full WebUI **3071 passed, 4 skipped**; affected native modules
**319 passed, 1 ignored**; pinned Chromium **12 passed** (Workflow/Review,
settled-child close/reopen/reload, retry/Fork/tools, compaction, continuous
Office/PDF pages). The native publication-failure inspection and protocol prose
contract tests pass. TypeScript, i18n, production build, provenance/notices,
formatting and strict all-target/all-feature Clippy pass. Full CI remains an
independent exact-head merge gate; the preceding head's broader CI was not green.

### PR #488 acceptance review closeout (V1–V2)

The Harness reference pins and execution boundaries above remain unchanged.
Acceptance now follows the click-to-drill Model menu and `menuitemradio` choices,
and closes a Session using its exact native identity, including duplicate titles.
The shared menu exposes each existing item identity as a data attribute without
changing its role, label, styling or action. Keyboard acceptance retains actual
focus, editor/caret reachability and the approved neutral composer edge.

Native distant-reading acceptance accounts separately for the two initial,
bounded older pages and the direct Turn windows; it still forbids intervening
prefix traversal. Source cancellation is measured after an explicitly selected
Session finishes its initial reading window, rather than mistaking that passive
fill for a stale source landing. Request logs and canceled-source geometry are
attached to the browser report. The Trace file-reference fixture now serves the
finite native inspection head and original bounded content instead of the
obsolete raw-detail response. Background-task resize acceptance observes Floating
UI's settled bounds, and the add-menu test constrains its height enough to prove
real keyboard scrolling rather than assuming padding overflow requires it. This
stronger test exposed a genuine short-viewport clipping issue: the menu fit used
the window top and could hide its first row behind the conversation header.
Following the inspected Harness MenuView header-clearance convention at
`687f282e8625c6b8d11ff58211a90524be2e4317`, it now fits to the actual conversation
scrollport top and remeasures on resize/scroll. Keyboard wrapping must reveal
both the first and last options in the visible conversation area.

Browser plugin not available; verification uses the immutable Linux Playwright
image and fonts recorded above. Eight stable screenshot references were reviewed
against their prior captures: seven contain only the intended ModelSelect chevron
paint change, and the Workspace override reference removes the previously
rejected blue text-input outline. No comparator, noise allowance, timeout or
product behavior is weakened. The full Web suite passes 3071 tests with four
existing skips; TypeScript and strict native Clippy pass. Ordinary native
delegation passes locally and in the preceding exact-head Linux boundary job;
its failure path now records the last authoritative snapshot and all three
completion predicates. Final rendered acceptance and the final exact-head FULL
CI result are recorded in PR #488.

## PR #488: deferred history, message blocks, terminal Trace and upload receipts

The four follow-up review boundaries retain rustX's native owners and the existing
Harness presentation pins. Transcript aggregation now keeps both native inline and
deferred positions, their exact content bindings and collection frontiers. A newer
representation replaces its former representation at the same transcript position.
Normal Chat seats consume canonical message block continuation before segmenting
assistant replies; the first 32 attachments no longer hide their following text.
Reads use bounded native pages at their captured live/retained cut, including child
activation scope, and reuse the existing canonical collection/content validation.
Live continuations stay on the live checkpoint owner; frozen readers keep their
retained cut. A failed live cut retries after the native checkpoint advances,
without rereading already successful immutable blocks. They do not add generic
pagination controls or transport metadata to normal Chat.

Child Trace detail consumes only an owner-confirmed live-to-durable resync, with
one original absolute deadline and the same child/activation. Resident inspectors
keep successful immutable content while lifecycle changes, terminal child refresh
(including already-completed records) or explicit selection allow failed reads to recover. Wire upload preparation admits exactly one file per
operation (also reflected in the generated v53 schema); Composer multi-selection
continues through independent queued operations. Native Session allocation and
receipt authority are unchanged, and a lost acknowledgment remains read-repairable.
The catalog/SQLite documentation versions are corrected to 16/74.

Regression coverage uses the production AppServerClient, AgentReading, AgentTranscript,
AgentTraceReader and Trajectory components; native tests cover the actual child
TraceDetail read across settlement and reject an under-budget 2,400-file prepare
whose complete Ready record would exceed the wire budget before allocation. The
existing real commit/lost-ACK test also checks the complete single-file response
with maximum admitted filename and request ID. Browser plugin not available;
rendered verification uses the repository's pinned Playwright 1.63 Linux container,
including a real native 32-file/33-block submission and reload, desktop/mobile Chat
reading, upload interactions and resident Trace inspection. Screenshot tolerances
and Harness source pins are unchanged.

## Child message continuation ownership

Native-backed Chat continuation now restarts unfinished reads when a resident
Child view regains visibility, while retaining successful immutable blocks and
rejecting late results from its former effect lifetime. Frozen Child messages
and Tool descriptors use their original durable window cut with the Registry's
conversation/activation identity; the live observation advances independently.
The Registry serves those historical pages from the Child's own durable store.
No Harness state owner, protocol, loading surface or visual styling is added.

QA uses the production transcript/provider/client with controlled native reads,
a real Registry/Child boundary test and the existing pinned Playwright browser
for Chat/Trace switching at desktop and narrow widths. Browser plugin not available.

Read-cut identity includes every native owner, source frontier, publication and
collection generation. Comparisons use closed field identities rather than JSON
property order, and normalize omitted/`null` optional native selectors. Regression
responses include the native serialized selector fields instead of echoing only
the client request shape.

A combined frozen window/live tail selects its source per canonical message.
Historical membership comes from the selected native inline/deferred positions;
new tail messages retain their live checkpoint and revision-based error recovery.
Live canonical block counts discover tail continuations without borrowing live
Tool lifecycle state for historical rows. Regressions cover both short/long old
messages alongside a long new message, and expired live reads inside a frozen view.

The native Chat acceptance waits for post-reconnect Trace history to create an
actual scrolling range before asserting detached reading during settlement.
A short native head at scrollTop zero is still following the tail; passive
history assembly may correctly move that viewport. The assertion after an
explicit detached gesture remains exactly scrollTop zero.


### File preview loading

The loading contracts were compared with DeepSeek Harness
`687f282e8625c6b8d11ff58211a90524be2e4317`: workspace text pages, bounded
complete-byte reads, multipart responses, and Host-owned Office conversion.
The [research note](../docs/dsh-file-preview-research.md) records immutable sources.
No additional upstream implementation source is copied. Existing renderer pins,
licenses and attribution remain unchanged.

Conversation XLSX previews now compose the existing attributed browser Excel
parser/viewer, with worksheet selection retained through its public hooks. Both
spreadsheet surfaces use the same limits. The old Host workbook projection and
inspection table are removed. PDF wrappers distinguish original and converted
input budgets. Paged text and complete-byte transport preserve rustX's committed
delivery identity, descriptor checks, authority fencing and physical settlement.
The source inventory records updated local hashes and dependency closures for
the existing adapted files.

The shared XLSX preview archive uses rustX-owned ZIP admission before expanding
any member: bounded directory count, names, declared sizes and ratios; matching
local headers and descriptors; fixed-output fflate inflation with exact length
and CRC checks. ExcelJS receives a stored ZIP rebuilt from admitted bytes. The
existing transitive `crc-32` 1.2.2 package is declared directly for checksum
validation. No additional Harness source is copied for this admission boundary.
The archive also admits actual worksheet coordinates, row/column extents, merged
rectangles, data-validation ranges and named-range matrices before ExcelJS can materialize them. It
uses the existing SAX parser and the same aggregate cell-area budget, covers all
worksheet entries recognized by ExcelJS, and retains ordinary merge rendering.
The lazy dependency prebundle includes CRC and SAX parsing so opening CSV/XLSX
does not force a development-server navigation after the preview is already open.

## Issue #491 — Workflow library and editor

The Workflows library and editor are original rustX code built from existing
theme tokens and primitives (Button, Menu, Modal, icons); no Harness source was
copied. Their layout follows the two approved Issue #491 design references
(`rustx_harness_workflows_list.png`, `rustx_harness_workflows_editor.png`).

Canvas interaction uses `@xyflow/react` **12.12.0** (MIT, © webkid GmbH), pinned
exactly. It adds this production closure, each MIT/ISC licensed and inventoried
in `public/THIRD-PARTY-NOTICES.txt`: `@xyflow/system` 0.0.83, `classcat` 5.0.5,
`zustand` 4.4.0, `use-sync-external-store` 1.2.0 and the `d3-color`,
`d3-dispatch`, `d3-drag`, `d3-ease`, `d3-interpolate`, `d3-selection`,
`d3-timer`, `d3-transition` and `d3-zoom` 3.x modules (their `@types/*`
packages are type-only). The library loads only in the lazily imported editor
chunk. The canvas hides React Flow's on-canvas attribution badge through
`proOptions.hideAttribution`; the MIT license permits this, and attribution is
retained in the shipped notices instead.

React Flow owns only viewport, selection, dragging and connection gestures.
Program state, validation, serialization and file I/O remain native; no React
Flow JSON is persisted, and the browser-local view store holds only node
coordinates and viewports.


## Issue #495 protocol review: mandatory App Server v57

The native `ModelCapabilities` object now serializes `jsonSchemaOutput`, which
is incompatible with the closed v53 schema. Web and TUI therefore negotiate
App Server v57 and reject v53 peers before loading Model or configuration data.
Only the v57 generated schema and TypeScript pair remains. Existing strict
validation stays in place. Derived source changes are protocol import paths;
the source inventory records their current hashes and dependency paths without
changing the pinned Harness baseline or licenses.

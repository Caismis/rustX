# Harness presentation ownership

Base: rustX `533cd341a3cd899e5fa9fdc693899cd35cd9a15b`.
Upstream: https://github.com/deepseek-ai/deepseek-harness at
`ddefc45fbc7f8e46dd73185e68295696d1297887` (current master release HEAD inspected
at implementation start; immutable source, never a runtime dependency).

Harness owns the presentation baseline; rustX owns product/runtime semantics.
The shell decision from #345 remains in force. #346 completes the Agent seats;
[AGENT-ARCHITECTURE.md](AGENT-ARCHITECTURE.md) records their ownership.

| Surface | Classification | Treatment |
| --- | --- | --- |
| Theme/typography/reset/elevation/scrollbars | A direct reuse | Current sheets; remove rustX accent overrides |
| Button/Input/Pill/StateDot/Menu/HoverCard/Tooltip | A direct reuse | Preserve DOM/CSS/interaction; local imports |
| Modal | A direct reuse, accessibility adaptation | Preserve mask/panel; focus containment/restoration |
| Markdown/code | A bounded reuse | Audited incremental machinery and native-safe links; current styles |
| AppFrame/column solver | A environment adaptation | Measured grid/drag, 280/56px Sidebar, 1024px collapse; React state/content seats replace plugin injection |
| SidebarRoot | B Harness UI + rustX binding | Preserve brand/new/rail/region/footer; callbacks replace registry |
| Workspace browser/rows | B Harness UI + rustX binding | Compact tree/search/hover/menu; native DTO projections; archive becomes native deletion preview |
| SettingsRoot | B Harness UI + rustX binding | Modal/nav/content seats; existing CFG3 editors; no Settings Controller |
| Conversation/composer/tools/interactions/model/permission | B Harness UI + rustX binding | Pinned Agent presentation, bounded native adapters; see AGENT-ARCHITECTURE.md |
| Connection/Inspector/CFG3 scopes | C rustX extension | Harness primitives/panel seats; native product semantics |

## Boundary

WEB-12 uses one Session identity/header with Chat/Trajectory tabs, a bounded
Session actions menu (tree navigation and advanced lifecycle), and the existing
Inspector corner/side-panel seat. The former duplicate title/connection bar and
permanent attach/resync/detach/unload toolbar are removed. Paths identify the
execution location without attachment suffixes; Sidebar tooltips use product copy.
Sidebar is the sole Session selector. The top Session strip is deleted, not hidden.
`openViews` tracks real browser controller ownership/restoration (maximum 32), not
visual tabs. Sidebar row menus expose Close view; switching focus retains background
observation and never releases, cancels or unloads work. Chat/Trajectory retain tabs.
Sidebar View options also offers explicit Close all views for unlisted/restored
entries; missing catalog rows cannot make the finite capacity unmanageable.
`sessionDisplayTitle` uses explicit name > native SessionSummary.preview > New session.
The preview is a server-owned persisted display projection, computed from the
canonical root user message after its commit and stored in the session catalog;
null means no projection has been published, not a client-side derivation gap.
No UUID fallback or automatic LLM naming exists. After canonical user-message
observation, an exact `session/summary(SessionId)` read refreshes unnamed labels;
only success (including no preview) completes the check. Failed reads remain retryable
at authoritative refresh/reconnect, with concurrent reads coalesced, and convergence
holds because publication follows the canonical commit. No draft or
admission can generate a title. `session/list` owns fuzzy paginated browsing, never
identity lookup. `view.summary` is a replaceable exact observation across pages,
not a second catalog. It may retain its last value across transport loss for stable
presentation, but each new open-view attachment epoch must establish authoritative
summary freshness through an exact read. The first canonical-user preview check is
separate: only a successful read begun after that history exists completes it,
including preview=None. A successful rename establishes a metadata read floor and
starts a new exact observation after its acknowledgement; pre-rename reads cannot
publish or satisfy that repair. Ordinary reads still coalesce. Read-order and
connection fences reject stale observations.
See [the complete classification](PRODUCT-SURFACE.md).

`bindings/session-product.ts` is a pure projection, shared by Session status and
Sidebar observation. It carries no transitions, timers or guessed settlement.
Transport reconnect, attachment open and snapshot refresh remain distinct actions.
Inspector accepts observations and `ProtocolLog` only, so its controls cannot
dispatch runtime operations. Unresolved evidence stays in the client; reading,
filtering, clearing or pausing the log cannot acknowledge it.
Selected sections filter operation evidence by Session; separate global/other
diagnostics never attribute another Session's uncertainty to the selected one.

`presentation/` has no imports of app, bindings, workspaces, client or protocol.
`app/` and `workspaces/` bind its props to the typed App Server client.
Native Session cwd/history/execution remain App Server owned. Product Host owns
registration/classification/authorization only. WorkspaceSessionNavigation and
NavigationEpoch fence browser continuations; AppServerClient owns authoritative
reconnect/resync. Unmount/collapse/search/navigation never cancel committed work.
Only widths, collapse, menus, theme, search text and section selection are local.

## Exclusions and deviations

No official brand marks, desktop integrations, Cordis/Host module loader,
frontend provisional Session identities, archive store, schedule, Workspace
ownership or content-search protocol. Native metadata search/paging replace
controller queries. Delete uses native preview/revision confirmation. Inspector
occupies the right-panel seat. Narrow Settings navigation accommodates CFG3's
larger section set. No generic plugin platform or compatibility exports.

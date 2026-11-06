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
| AppFrame/column solver | A environment adaptation | Permanent 56px primary rail, resizable 280px Home sidebar, 1024px overlay breakpoint and retained right-panel seat |
| HomeSidebar / PrimaryNavigationRail | B adapted Home sidebar + C original rail | Home owns brand/new/workspace/session browsing; the separate rail binds to the committed Shell destination |
| Workspace browser/rows | B Harness UI + rustX binding | Compact tree/search/hover/menu; native DTO projections; archive becomes native deletion preview |
| SettingsRoot | B Harness UI + rustX binding | Independent page/nav/content seats; existing CFG3 editors; no modal visibility authority or Settings Controller |
| Conversation/composer/tools/interactions/model/permission | B Harness UI + rustX binding | Pinned Agent presentation, bounded native adapters; see AGENT-ARCHITECTURE.md |
| Connection/CFG3 scopes | C rustX extension | Harness primitives/panel seats; native product semantics |
| Workflows library and editor (#491) | C rustX extension | One primary-rail destination; content seat without the Home sidebar; React Flow canvas only, native file semantics |

## Boundary

WEB-12 uses one Session identity/header with Chat/Trajectory tabs, a bounded
Session actions menu (tree navigation and advanced lifecycle), and the file
preview/workbench side-panel seat. The former duplicate title/connection bar and
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
null means no projection has been published — a legitimately empty projection or an
unrepaired publication gap — not a client-side derivation gap.
No UUID fallback or automatic LLM naming exists. After canonical user-message
observation, an exact `session/summary(SessionId)` read refreshes unnamed labels;
a read completes the check only when it is also causally after every
`session/summaryInvalidated` observed for that Session. Failed reads remain retryable
at authoritative refresh/reconnect, with concurrent reads coalesced per generation and
invalidation epoch; convergence after publication comes from the native invalidation,
never from inferring settlement out of canonical history. No draft or
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
Transport reconnect, attachment open and bounded checkpoint refresh remain distinct actions.
The developer inspector, raw protocol log and generic conversation collection
browser have been removed. Unresolved evidence stays in the client and remains
actionable through Connection Settings. Session status and Sidebar warnings
never attribute another Session's uncertainty to the selected one.

`presentation/` has no imports of app, bindings, workspaces, client or protocol.
`app/` and `workspaces/` bind its props to the typed App Server client.
Native Session cwd/history/execution remain App Server owned. Product Host owns
registration/classification/authorization only. WorkspaceSessionAdmission owns fresh Product Host authorization. The XState v5
ShellNavigationMachine owns the sole committed route and one disposable pending
intent. Browser-lifetime Shell/Settings actors and Session admission belong to
`App`; `AppSurface` is their disposable projection. Remounting that surface does
not retire an admitted startup continuation or recreate route authority. A
lifetime callback actor observes native authority directly. Its correlated completion and native proof guard publish routes atomically;
preparation and failure preserve committed controls. Settings is a committed Shell route. Its separate actor owns only the exact
configuration target, authorized section, detail and disposable owner lookup.
Leaving Settings fences that lookup and its inspection, never the configuration
transaction actors. An unresolved owner renders a pending/unavailable page, not
the previous owner's authoring UI. AppServerClient
and Session Lifecycle own authoritative reconnect/resync and admitted mutation
settlement. Fork View Source publishes a read-only history route using authorized
exact native Node and Message reads; it never attaches or switches the source.
The browser-session Composer snapshot retains unsent text, selection and restored
uploads across history unmounts; the existing AttachmentIntakes owner retains
local Files. History and Return preserve this draft binding while each route
publication replaces control authority. Native attachment/generation replacement
keeps recoverable local text/Files but retires receipts and upload operations;
restoration never replays them. Another conversation destination replaces the draft; primary pages retain it.
ChatViewport owns its exact anchor landing and scroll cancellation. View Source
registers cancellation in the initiating click stack against the exact Shell
pending intent. A correlated CANCEL cannot retire a newer or committed route. Unmount/collapse/search/navigation never cancel committed work.
Only widths, collapse, menus, theme, search text and section selection are local.

## Primary navigation and Home retention (#536)

`ShellNavigationMachine` owns Home conversation routes, Workflows, Scheduled
Tasks and Settings. The permanent rail derives selection from that committed
route; a pending authorization or Workflow leave request cannot select its
prospective destination. Home's last conversation route remains in the same
actor, with its logical editor binding, local files and native operation fences.
The retained Home and Settings presentations stay mounted and hidden/inert on
other pages. Native lifecycle, configuration and submission owners keep settling;
React page visibility cannot attach, detach, cancel, create, send or replay work.

Returning Home reads fresh Product Host authorization and current Session
lifecycle evidence. It never issues Open. A same-authority disconnected Session
returns as a non-executable local draft; an uncertain or released attachment
does not relinquish that editor's text or files. Only existing lifecycle recovery
can establish executable controls; Home never reacquires a released claim.
Deleted, unauthorized or unavailable targets fall
back to an ordinary empty Home without native Session creation. Authority
replacement clears remembered identities. A newer gesture stops the old return
preparation, so its later authorization result cannot publish.

A returned New Conversation resumes observation of its original binding's
`FirstSubmissions` owner, including creation acknowledgements arriving during
Home authorization. If creation finishes during that wait, Home re-reads the
binding and validates the Session against fresh lifecycle and Workspace evidence;
a Session deleted during authorization returns to the ordinary empty Home.
The correlated observer publishes the known Session and
its completed attachment without creating, opening or sending again. A newer
editor retires this observation; the old acknowledgement cannot replace it.
The observer also follows native readiness: an already-admitted owner can stay
quiet while a same-claim resync suspends controls, then finish publication when
those controls recover. Both subscriptions retire with the preparation actor.
A known Session stays publishable while its first input is failed, uncertain or
paused: explicit upload recovery never creates or attaches that Session again.
Discarding that input clears its text/files while retaining the known Session.
Failures without a Session, replacement navigation and authority retirement
still end publication interest.
Same-editor attachment and creation promotions retain its control fence as well
as its binding. Generation, authority and exact native attachment observations
remain the execution fences; explicit editor/Node/history replacement retires
the prior control owner.

Every external exit from Workflows passes through Shell's correlated leave
request and `workflowSurfaceMachine`. The latest request replaces the previous
one, cancellation retires its callback, and confirmation publishes once. The
Workflow editor owns unsaved/unsettled edit protection; Shell owns destination
commit. No presentation callback maintains a competing route.

`AppFrame` composes four regions: fixed primary rail, optional Home sidebar,
content, optional Home workbench. Collapsing Home releases its entire track.
The workspace browser stays resident without an alternate collapsed UI. Below
1024px its overlay starts beside the primary rail, leaves the rail interactive,
and dismisses with Escape/backdrop/toggle. Programmatic publication closes only
geometry; dismissal never cancels committed native work. Width/collapse hints
are browser preferences bound from `app/`; presentation has no storage access.
Other primary pages have no Home preview track. The right-panel subtree retains
its selected files, bounded document bodies and HTML browsing contexts for return.
Page activation gates workbench interaction separately from tab-body retention;
tab closure, target replacement and existing content budgets still retire bodies.
Existing native ownership,
configuration authorization, revisions/CAS and transaction convergence are unchanged.

## Exclusions and deviations

No official brand marks, desktop integrations, Cordis/Host module loader,
frontend provisional Session identities, archive store, schedule, Workspace
ownership or content-search protocol. Native metadata search/paging replace
controller queries. Delete uses native preview/revision confirmation. File previews
and the workbench occupy the right-panel seat. Narrow Settings navigation accommodates CFG3's
larger section set. No generic plugin platform or compatibility exports.

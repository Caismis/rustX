# WEB-RESET-01 — presentation ownership decision

Base: rustX `533cd341a3cd899e5fa9fdc693899cd35cd9a15b`.
Upstream: https://github.com/deepseek-ai/deepseek-harness at
`ddefc45fbc7f8e46dd73185e68295696d1297887` (current master release HEAD inspected
at implementation start; immutable source, never a runtime dependency).

Harness owns the presentation baseline; rustX owns product/runtime semantics.
This record precedes implementation; the inventory records the final closure.

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
| Conversation/composer/tools/interactions | B deferred interior integration | Retain native content inside new frame, no Agent experience rewrite |
| Connection/Inspector/CFG3 scopes | C rustX extension | Harness primitives/panel seats; native product semantics |

## Boundary

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

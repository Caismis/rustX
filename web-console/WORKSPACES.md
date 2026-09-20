# Product Host Workspace navigation

Workspace registrations are Product Host navigation/authorization metadata.
`SessionPersistentState.cwd` is the single durable rustX cwd authority. A Host
registration is neither a runtime container nor a trusted-project configuration
layer. Host navigation authorization is separate from native User < Workspace configuration.

## Concrete local Host

`host/workspaces.ts::LocalWorkspaceHost` runs in Node. An operator provides a finite
set of absolute roots and one rustX endpoint. Construction canonicalizes roots;
resolution rechecks physical identity. The browser submits opaque registration or
configured-location handles, never paths to authorize. `host/http.ts` exposes the
small typed `src/workspaces/host.ts` contract using same-origin JSON POSTs.

Normal local development uses [the dev launcher](../DEVELOPMENT.md), which owns
the ephemeral config and metadata. The following describes the distinct case of
an independently managed Product Host with persistent operator-owned metadata.
The launcher carrier authenticates browser requests before the Workspace middleware.
Its bootstrap DTO contains only native transport admission material, never roots.
Browser authentication cannot add roots; Remote Settings cannot grant filesystem access.
Create its JSON file outside the repository/runtime root, e.g.:

```json
{
  "endpoint": "ws://127.0.0.1:8080/",
  "picker": true,
  "metadataFile": "/home/me/.local/state/rustx-web/workspaces.json",
  "roots": [
    { "id": "project-a", "cwd": "/home/me/projects/a", "displayName": "Project A" }
  ]
}
```

Create the metadata parent directory first. Start the loopback local carrier:

```sh
RUSTX_WORKSPACE_HOST_CONFIG=/absolute/path/host.json pnpm --dir web-console dev
# Or, after building:
RUSTX_WORKSPACE_HOST_CONFIG=/absolute/path/host.json pnpm --dir web-console preview
```

The first start registers the configured roots. Subsequent starts retain Host
names/order/unregistrations in the metadata file, published through atomic rename.
One Host instance owns this file. Metadata contains only registration ID, authorized
location handle and display name; array order is Workspace order. It contains no
Session IDs, trust flags or configuration blobs. Roots remain operator configuration,
not browser state. Restart after changing the authorized root set; registrations
referring to removed locations must be removed from the operator-owned metadata.

With `picker: true`, **Add Workspace** offers only configured authorized locations,
including roots previously unregistered. With `picker: false`, Add is absent and the
UI explains capability absence. Without a configured Host, native existing Sessions
remain listable as ungrouped, new attachments and creation are unavailable, and
Host absence is visible.
There is no arbitrary path field or localStorage authorization fallback.

`HttpWorkspaceHost` is injectable into `App`. A remote Product Host can implement
this same concrete contract behind its authenticated same-origin deployment route.
This local adapter supplies no remote authentication, multi-user ACL, OS picker,
recursive directory browser or OS filesystem sandbox. Host routing and browser
binding share `endpointIdentity` / `sameEndpoint`, based
on `URL.href`; equivalent URL spellings identify the same process endpoint.
Authorization uses the endpoint captured by the current native connection, not an
uncommitted browser connection field. Per-user deployments retain
`user A -> Host A + rustX A` and `user B -> Host B + rustX B`.

## Native projections and lifetimes

`session/list` still returns at most 32 searchable summaries per Web page. Native
`SessionCatalog::list_page` projects cwd directly from durable Session state; no
runtime attachment or transcript download discovers grouping. The App Server adds
a same-page residency observation from the native manager. Runtime observations
are replaceable, not execution authority; unloaded, detached, loaded/attached,
running, pending inbound and disconnected/stale observations have distinct labels.
Only a current attached native snapshot supplies running/pending markers.

The Host's `classifyLocations` classifies a bounded page by exact canonical root
identity. `config.roots` is authorization; registrations are navigation metadata
covering some of those roots. An authorized root without a registration produces
`{ authorized: true }`: its Session is ungrouped and may still be opened. Outside
or unavailable roots produce `{ authorized: false }`: their Sessions remain durable
and listable, but this Web Product Host refuses new attachment/cold resume. Aliases
resolve on the Host; descendants are not implicitly authorized. No second durable
Session-to-Workspace map exists. Search uses the native bounded summary query;
query/navigation epochs suppress obsolete responses. There is no transcript index.

Workspace selection only changes navigation. It does not attach every Session,
unload, cancel, change cwd, rewrite history or alter trust. Session opening attaches
only that Session, including cold resume, after current Host admission. Switching
focus leaves unrelated work
alone. Unregister removes metadata only; its Sessions remain durable and visible
ungrouped. Session deletion, detach and unload remain separate explicit operations.
`WorkspaceSessionNavigation` owns the Web admission policy. The client's single
attachment entry point calls this injected policy for every new attachment,
including saved-tab restoration/reconnect, sidebar Open/Fork, toolbar Attach,
creation and Fork/branch/retry/tree continuations. No policy means refusal. Before
attach, it reads `settings/read` from the native durable owner, classifies that
current cwd through the Host for the current connection endpoint, and checks
navigation/generation fences. Summary classifications only describe the list;
they never authorize attachment. Saved localStorage openViews are hints, not admission.
Already-attached focus does not require a new runtime claim or hot revocation.
Fork additionally checks its source's current cwd before creating a child; its
result still passes normal attachment admission.

App's `focusSession` transition publishes one Session/Workspace pair. Every Session
focus path, including Sidebar selection, restored focus, command results, close/deletion
fallback and reconnect, clears the previous Workspace while reading current native
cwd and Host classification. Only its current continuation publishes the matching
registration. Authorized-unregistered focus has no Workspace. `/new` consumes this
explicit focus context and refuses when it is empty; it never falls back to Session
cwd or a previously selected Workspace.

Native `session/name` owns renaming. Sidebar Fork opens the existing native exact
boundary chooser and `session/fork`; TypeScript never clones Session/history state.

Create and `/new` resolve the selected Host registration to an explicit cwd before
`session/create`. NavigationEpoch and connection generations fence resolution,
creation, attachment and Fork continuations. Supersession never cancels an operation
already committed on the server; the result remains discoverable through the list.

## Configuration authority

Host registration authorizes product navigation and exact Workspace routing; it is
not configuration precedence. Native CFG3 resolves User < Workspace with no trust
gate. Both source scopes are editable in structured Settings; Effective is a
read-only native projection. Invalid configuration fails native resolution instead
of silently skipping Workspace content. Save persists source and transfers application to the native coordinator;
context changes await explicit Session adoption. Host metadata introduces no configuration layer,
credentials or resource definitions. See [Web Settings](../docs/web-settings.md).

## Harness browser presentation (#345)

The Sidebar uses the upstream compact project/Session rows, hover cards, menus,
search results and collapsed rail. **New Session** opens the registered Workspace
picker; a project's **New Session** button uses that authorized registration.
Project title selection changes navigation context; its chevron expands/collapses
rows. **View options** switches Flat/Grouped view and refreshes the native list.
Search is native Session metadata search, paged in 32-row windows. It never scans
browser-owned history. Rename, Fork and Delete are in each Session's menu; Delete
opens the existing native preview and revision-checked confirmation.

Current authoritative pending interactions (approval/review/question) outrank the
running marker. Queued inbound input alone is not a pending interaction. Detached,
unloaded or stale snapshots cannot claim current activity. A Host classification
result is paired with the exact Session summary array it classified and is thrown
away on replacement. Collapse and search selection remain disposable; there is no
persisted Session-to-Workspace membership map. Unregister changes Host metadata
only, retaining authorized ungrouped Sessions and committed native operations.

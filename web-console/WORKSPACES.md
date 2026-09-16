# Product Host Workspace navigation

Workspace registrations are Product Host navigation/authorization metadata.
`SessionPersistentState.cwd` is the single durable rustX cwd authority. A Host
registration is neither a runtime container nor a trusted-project configuration
layer. Host authorization and native project trust are separate facts.

## Concrete local Host

`host/workspaces.ts::LocalWorkspaceHost` runs in Node. An operator provides a finite
set of absolute roots and one rustX endpoint. Construction canonicalizes roots;
resolution rechecks physical identity. The browser submits opaque registration or
configured-location handles, never paths to authorize. `host/http.ts` exposes the
small typed `src/workspaces/host.ts` contract using same-origin JSON POSTs.

Create an operator-owned JSON file outside the repository/runtime root, e.g.:

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
remain navigable as ungrouped, creation is unavailable, and Host absence is visible.
There is no arbitrary path field or localStorage authorization fallback.

`HttpWorkspaceHost` is injectable into `App`. A remote Product Host can implement
this same concrete contract behind its authenticated same-origin deployment route.
This local adapter supplies no remote authentication, multi-user ACL, OS picker,
recursive directory browser or OS filesystem sandbox. A Host endpoint must match
the rustX connection before its registrations resolve. Per-user deployments retain
`user A -> Host A + rustX A` and `user B -> Host B + rustX B`.

## Native projections and lifetimes

`session/list` still returns at most 32 searchable summaries per Web page. Native
`SessionCatalog::list_page` projects cwd directly from durable Session state; no
runtime attachment or transcript download discovers grouping. The App Server adds
a same-page residency observation from the native manager. Runtime observations
are replaceable, not execution authority; unloaded, detached, loaded/attached,
running, pending inbound and disconnected/stale observations have distinct labels.
Only a current attached native snapshot supplies running/pending markers.

The Host groups a bounded page by exact canonical root identity. Aliases resolve
on the Host; descendants are not silently allocated to a parent registration.
Unavailable/unregistered roots produce **Ungrouped Sessions**. No second durable
Session-to-Workspace map exists. Search uses the native bounded summary query;
query/navigation epochs suppress obsolete responses. There is no transcript index.

Workspace selection only changes navigation. It does not attach every Session,
unload, cancel, change cwd, rewrite history or alter trust. Session opening attaches
only that Session, including cold resume. Switching focus leaves unrelated work
alone. Unregister removes metadata only; its Sessions remain durable and visible
ungrouped. Session deletion, detach and unload remain separate explicit operations.
Native `session/name` owns renaming. Sidebar Fork opens the existing native exact
boundary chooser and `session/fork`; TypeScript never clones Session/history state.

Create and `/new` resolve the selected Host registration to an explicit cwd before
`session/create`. NavigationEpoch and connection generations fence resolution,
creation, attachment and Fork continuations. Supersession never cancels an operation
already committed on the server; the result remains discoverable through the list.

## Trust source and configuration authority

`settings/read.project_trusted` is a read-only projection from
`UserConfigManager::project_trusted`, using the same canonical location identity and
native trust store as source resolution. Null/failure/not-yet-read/stale is unknown,
never trusted. This is current **source** trust at observation time. Runtime resource
activation remains the admitted native generation; existing native resource/source
activation projections (including `untrusted`) retain their meaning.

The prior resolver rejected all untrusted runtime admission. WEB-07 separates cwd
use from project source activation at that owner: untrusted cold resolution skips
project `rustx.toml`, project Agents/Skills/Workflows/Python Tools and implicit
project instructions. User configuration, User resources, built-ins and explicitly
authorized Session selections remain their existing authorities. Untrusted project
bytes are not parsed. Reload retains admitted project authority and document/Skill
roots: navigating or later granting trust cannot activate a previously inert runtime.
Fresh/cold native resolution applies an explicit external trust change. Trust grant
and revoke remain native CLI operations; there is no Web trust mutation or store.

Workspace Settings is disabled for unknown/untrusted native source state. For an
observed trusted source, the entry displays read-only source/lifetime information.
The structured editors are owned by WEB-08/WEB-09. Host metadata introduces no
`rustx.local.toml`, precedence layer, overrides, credentials or resource definitions.

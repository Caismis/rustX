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
remain listable as unclassified, new attachments and creation are unavailable, and
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
runtime attachment or transcript download discovers grouping. The App Server does not add residency authority to these summaries. Runtime
observations are replaceable, not execution authority; detached, attached, running,
pending inbound and disconnected/stale observations have distinct labels.
Only a current attached native snapshot supplies running/pending markers.

The Host's `classifyLocations` classifies a bounded page by exact canonical root
identity. `config.roots` is authorization; registrations are navigation metadata
covering some of those roots. An authorized root without a registration produces
`{ authorized: true }`: its Session is ungrouped and may still be opened. Outside roots produce `{ authorized: false, reason: 'denied' }`; filesystem
failures produce `{ authorized: false, reason: 'unavailable' }`: their Sessions remain durable
and listable, but this Web Product Host refuses new attachment/cold resume. Aliases
resolve on the Host; descendants are not implicitly authorized. No second durable
Session-to-Workspace map exists. Search uses the native bounded summary query;
query/navigation epochs suppress obsolete responses. There is no transcript index.

Workspace selection only changes navigation. It does not attach every Session,
unload, cancel, change cwd, rewrite history or alter trust. Session opening attaches
only that Session, including cold resume, after current Host admission. Switching
focus leaves unrelated work
alone. Unregister removes metadata only; its Sessions remain durable and visible
ungrouped. Session deletion and view detach remain separate explicit operations.
`WorkspaceSessionNavigation` owns the Web admission policy. The client's single
attachment entry point calls this injected policy for every new attachment,
including saved-tab restoration/reconnect, sidebar Open/Fork, toolbar Attach,
creation and Fork/branch/retry/tree continuations. No policy means refusal. Before
attach, it reads `session/settings` from the native durable owner, classifies that
current cwd through the Host for the current connection endpoint, and checks
navigation/generation fences. Summary classifications only describe the list;
they never authorize attachment. Saved localStorage openViews are hints, not admission.
Already-attached focus does not require a new runtime claim or hot revocation.
Fork additionally checks its source's current cwd before creating a child; its
result still passes normal attachment admission.

App's `focusSession` selects a Session identity. Its Workspace is read from the
shared display association owner; reconnect and catalog refresh retain compatible
confirmed evidence. Authorized-unregistered focus has no Workspace. `/new` may
use the displayed registration as navigation intent, but must resolve it freshly
before creation; display evidence never supplies a cwd or operation permission.

Native `session/name` owns renaming. Sidebar Fork opens the existing native exact
boundary chooser and `session/fork`; TypeScript never clones Session/history state.

Create and `/new` resolve the selected Host registration to an explicit cwd before
`session/create`. NavigationEpoch and connection generations fence resolution,
creation, attachment and Fork continuations. Supersession never cancels an operation
already committed on the server. For first submission, decoded creation ACK
publishes a client-owned Session continuation before navigation. The Conversation
uses the native identity directly, before catalog refresh; attach/readiness gates
upload and admission. Unrelated navigation prevents further dispatch, retains
acknowledged facts and pending input, and never hijacks a newer route. See
[startup ownership](../docs/issue-419/ownership.md).

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
search results and collapsed rail. **New Conversation** opens the center draft route. Workspace selection stays
browser draft state until first submit; only that submit creates a native Session.
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
only, retaining authorized unclassified Sessions and committed native operations.

Unclassified Sessions remain reachable through the bounded “Sessions outside registered Workspaces” disclosure or native metadata search. They are never a synthetic Workspace.

## Session display association owner (UX-04)

`src/workspaces/associations.ts::WorkspaceAssociations`, created once by `App`, is
the sole Session-to-Workspace **display** projection. Sidebar grouping and selected
Session Workspace presentation consume its same immutable snapshot. It contains
no authorization bit, Session summaries, execution snapshots, persistent storage,
configuration, or background synchronization. Row-local activity subscriptions remain
independent. New-conversation Workspace selection is explicit navigation intent in
`App.center`, not a fabricated Session membership. Metadata rereads do not reset drafts
or navigation. Fork preparation still performs fresh admission before its callback.

Evidence identity is `(native authority, authorityRevision, endpoint, Host authority,
Session ID, native cwd)`. `AppServerHost` allocates an opaque UUID for its lifetime;
mandatory `initialize.authority_id` exposes it, unchanged across connections. The Web
client invokes its existing authority retirement path before resynchronizing if this
identity changes, including at the same URL. Endpoint identity is the existing
`URL.href` normalization; a URL alone never establishes process identity. The local
Product Host generates its own per-instance `WorkspaceCatalog.authorityId`; display
classification submits that identity and a replacement Host refuses it with the
HTTP-preserved `authority_replaced` kind. That definitive observation immediately
retires display evidence, even if the next explicit catalog read fails. The refusal
does not start an automatic retry loop. Definitive Host replacement invalidates
pending navigation continuations; the client also rejects admission results from
a replaced admission callback. Both are refusal fences, never grants from display
evidence. Host-object
replacement also constructs a new projection. None of these identifiers grant access.

Native `session/list` summaries own the visible page's IDs/cwds; selected off-page
summaries/settings come from the existing native view owner. Same-authority disconnect
retains the existing native summary page and view observations. The display owner
therefore need not invent another Session catalog. Confirmed native deletion emits a
client-owned retirement notification and removes its display evidence; page absence
alone is not deletion. A changed cwd immediately loses incompatible evidence.

Request freshness additionally captures connection generation, the local catalog
invalidation revision, an exact request token, and disposal state. Reconnect changes
request generation, **not** compatible evidence identity. Explicit refresh, selection,
reconnect, and committed registration changes reread the catalog. Both catalog success
and failure are fenced before publication. A committed unregister first removes the
registration association and invalidates all older reads; a failed subsequent reread
cannot resurrect it. Complete successful catalogs also remove missing registrations.
There is no Host push feed: changes made outside this browser become known at those
read boundaries. Native Session metadata invalidations still use their existing owner.

Classification keeps the positional Host contract: immutable `{id,cwd}` vectors are
captured before awaiting, cardinality/shape and Workspace references are validated,
and complete results publish atomically through that vector, never a current array.
A reordered page cannot change correlation. Malformed responses are unavailable
observations. Superseded successes **and failures** are ignored. HTTP decoding and
fresh attachment admission validate the same result shape. Classification never
fetches or activates every Session, changes cwd, writes configuration, registers a
root, trusts a project, creates a Session, or calls a model.

Bounds: 128 recently demanded Session identities (one entry per ID/current cwd), no
cached pages, one current 32-summary page plus one off-page selected Session, Host
batches of at most 32, at most two active classification operations (each makes at
most two sequential batches), two active catalog reads, and one coalesced latest
queued demand per lane. Equivalent active reads share one request. Title/status-only
updates do not reclassify. Eviction is insertion/LRU-on-demand bounded; unrelated
retained metadata survives page changes. Each owner has one native-state subscription
and one deletion subscription; disposal releases both and aborts read resources.
Non-cooperative injected Hosts still cannot exceed the active-call bounds. Loaded-page
counts are not global totals (and are not displayed as such).

| Observation | Display | Operation admission |
| --- | --- | --- |
| Same-authority refresh/reconnect | Keep confirmed group; refreshing/disconnected feedback | Fresh native settings and Host classification |
| Catalog/network/filesystem unavailable | Keep compatible evidence; unavailable feedback | No permission from display; missing roots fail closed |
| No classification or changed cwd | Pending/unclassified surface, not confirmed ungrouped | Fresh checks required |
| Authorized without registration | Confirmed ungrouped, existing disclosure | Authorized root may still admit |
| Registration removed | Remove registration immediately; ungrouped disclosure | Root authorization remains independent |
| Definitive denial | Retire association; revoked observation | Refuse attachment |
| Native/Host/endpoint replacement | Retire incompatible evidence and late callbacks | New authority must independently admit |

The current Host result is `{authorized:true, workspaceId?:string}` or
`{authorized:false, reason:'denied'|'unavailable'}`. Filesystem failure is unavailable;
a canonical-root mismatch is denial. Neither result permits action. Exact root
semantics remain unchanged: descendants are not recursively authorized, and symlink
changes never retain usable permission. `WorkspaceSessionNavigation`, Workspace
resolution, native configuration writes, and operation-specific checks remain the
security-bearing owners. Configuration mutation lanes and write acknowledgement /
independent reread semantics are unchanged.

Reference study: DeepSeek Harness `639ed015397290b3745d163aafe02ffee4aa3f84`,
`packages/api/workspace-controller/src/{feed.ts,client/model.ts,client/index.ts,client/service.ts}`
and `packages/client/ui-workspace/{README.md,src/client/navigation.ts,src/client/tree.ts}`.
Adapted behavior: one membership projection and preservation of the last complete
observation across carrier loss until a matching baseline replaces it. Deliberately
excluded: DSH storage membership, persisted `workspace_id`, Remote feed framework,
Session creation/navigation policy, and framework services. The new association owner
is locally authored, with no copied DSH source. Existing presentation notices and
source lineage remain in place; modified descendants retain their inventory records.

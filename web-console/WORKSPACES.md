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
That environment-neutral module owns Workspace/configuration DTOs, pure result
validation, and the single `WorkspaceHostError` class. Its runtime syntax must be
loadable by native Node type stripping; generated protocol imports remain type-only.
Both Node Host modules import that contract directly, preserving `instanceof`
identity. `src/workspaces/http-host.ts` alone owns the browser `HttpWorkspaceHost`
adapter and its `carrierFetch` dependency; browser composition imports it directly.
The neutral module never re-exports the browser adapter. Dev's transitive compiler
scope enforces `erasableSyntaxOnly`, and a clean native child regression exercises
real Host classification and HTTP authority-error serialization.

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
result is correlated through captured Session ID/cwd pairs and fenced by request,
generation, catalog revision and authority identity. Cloned or reordered summary
arrays do not invalidate compatible evidence. Collapse and search selection remain disposable; there is no
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
Product Host generates its own per-instance `WorkspaceCatalog.authorityId`. Product
Host authority scope is that process identity bound to the catalog's normalized
endpoint (`sameEndpoint`): a change of either advances the authority epoch, so the
same Host process reporting another endpoint retires every observation and proof
taken under the old scope, while equivalent spellings (`ws://LOCALHOST:80`,
`ws://localhost/`) confirm it. Classification refuses an endpoint outside the
observation's scope. Display
classification submits that identity and a replacement Host refuses it with the
HTTP-preserved `authority_replaced` kind. That definitive observation immediately
retires display evidence, even if the next explicit catalog read fails. The refusal
does not start an automatic retry loop. `WorkspaceAuthority`, created once alongside
navigation by `App`, owns the current Product Host catalog authority and invalidation
epoch. `WorkspaceAssociations` subscribes only to retire incompatible display reads
and evidence and establish the replacement catalog baseline. It never invalidates
`NavigationEpoch` or supplies an operation fence. Host-object replacement constructs
new authority and display owners. None of these identifiers alone grant access.

Registration writes have two distinct Product Host authority fences. The dispatch
fence requires every `adoptWorkspace`, `renameWorkspace`, `reorderWorkspace`, and
`removeWorkspace` call to supply a `WorkspaceAuthorityScope` containing the expected
`authorityId` and endpoint. HTTP carries it as `scope` alongside the operation's
arguments, for example `{scope: {authorityId: "host-A", endpoint: "ws://localhost/"},
id: "W"}` for `/product-host/remove`. Missing/malformed scopes are refused before
dispatch. `LocalWorkspaceHost`, which owns the immutable instance authority ID,
checks that ID and the normalized endpoint immediately before changing registrations.
Remove checks inside its registration lane after waiting for any native configuration
operation. The check and registration commit have no asynchronous gap; metadata is
committed by atomic rename before success returns. A mismatch is a definite
`WorkspaceHostError` with kind `authority_replaced`, zero registration changes and
no metadata rewrite, preserved by HTTP. There is no retry against the replacement.
Browser preflight cannot enforce this boundary: an A-scoped request may arrive at B
even while the browser's A observation still appears current.

The settlement fence separately prevents a legitimately executed A mutation's late
response from changing B's display. `captureMutation()` returns the captured dispatch
scope together with `current()` and `commit()` for display settlement. Sidebar
metadata dialogs and the Composer Add Workspace picker retain that capability and
their picker inputs from opening, rather than recapturing replacement authority at
confirmation. Composer observes catalogs through the shared authority owner and
commits through the shared display owner before its own navigation continuation.
Neither owner performs the Host write: UI orchestration passes the scope to the Host.
Host metadata mutation authority, native App Server authority, display association
evidence, and native operation admission remain separate. These metadata preconditions
do not change read, configuration, resolution or native admission APIs.

`WorkspaceSessionNavigation` independently observes the Host authority, captures its
epoch and the native endpoint, reads current native `session/settings`, and classifies
the exact returned cwd with that authority ID. An authorized result returns an
operation proof, not a cached permission bit; the initial classification only decides
whether to queue the operation. When the operation reaches the front of native RPC
backpressure, `AppServerClient` reserves a bounded dispatch slot and calls the proof's
`validate`, which observes the current Product Host again and **classifies the captured
exact cwd again** under that fresh observation and the captured normalized endpoint.
Only a fresh `authorized: true` passes; `unavailable` (a configured root deleted) and
`denied` (a root path now resolving to another physical directory) refuse with no RPC
sent, even when the Host process and its `authorityId` never changed. The client then
checks the captured Host epoch, native generation and endpoint, navigation
continuation and injected admission callback identity synchronously before socket
send. Callback identity and Host authority are separate fences. Fork admission uses the
same dispatch proof. There is no display owner prerequisite, persistent permission,
automatic registration, or second admission mode.

**Linearization point.** The operation admission linearization read is the fresh
Product Host classification of the exact native cwd under the current Product Host
authority and normalized endpoint scope, performed while holding the bounded native
dispatch reservation; the synchronous send check fences any locally observed
replacement between that read and `socket.send`. Host policy changes after an
operation is dispatched are not retroactive cancellation of that operation.

**Captured cwd.** Final validation reclassifies the cwd captured from the initial
`session/settings` read instead of rereading native settings. Native
`SessionPersistentState.cwd` is fixed at `session/create`/`session/fork` on every App
Server path: the protocol has no settings-replacement method, and the only production
settings writer (configuration adoption in `SessionRuntimeManager`) copies the lineage
settings and replaces `model` alone. A future wire method that can change a Session's
cwd must extend this admission proof.

**Capacity.** A validation holds one of the eight ordinary RPC slots, so the final
classification stays adjacent to send and validation never returns to "validate, then
queue". At most two validations run at once; waiting validations hold nothing and never
block requests behind them. Product Host validations reserve only ordinary RPC
capacity and cannot consume the independent lifecycle-control capacity.
`turn/cancel` uses the separate, bounded two-request control lane: ordinary RPC
saturation cannot delay an otherwise admissible exact cancellation, while a full
control lane refuses new local control admission before transmission. Each validation
has the RPC deadline from reservation; retirement (Host replacement, navigation,
disconnect, callback replacement or timeout) aborts its reads and releases its
reservation exactly once, and a late Host answer can neither send nor release again.

Native `session/list` summaries own the visible page's IDs/cwds; selected off-page
summaries/settings come from the existing native view owner. Same-authority disconnect
retains the existing native summary page and view observations. The display owner
therefore need not invent another Session catalog. Confirmed native deletion emits a
client-owned retirement notification and removes its display evidence; page absence
alone is not deletion. A changed cwd immediately loses incompatible evidence.

Request freshness additionally captures connection generation, the local catalog
invalidation revision, the classification invalidation epoch, and disposal state.
Each in-flight classification has a separate unique read ID and captures the display
scope, native generation, revision, exact catalog reference, Host observation,
immutable `{id,cwd}` rows and AbortController. Current demand excludes satisfied
evidence and rows covered by still-valid reads. Uncovered rows can use a free second
slot while an unrelated old page remains pending; completion releases only its own
read ID and schedules uncovered current demand. Invalidated reads are aborted and
cannot publish. Compatible off-page results may populate retained evidence, but
cannot overwrite a changed cwd, revive an evicted entry, or settle newer evidence.
Reconnect changes
request generation, **not** compatible evidence identity. Explicit refresh, reconnect, and committed registration changes reread the catalog.
Selection only changes bounded page-plus-selected demand: satisfied rows cause no
catalog or classification read, and an unsatisfied off-page selection reads only its
cwd. Host replacement invalidates incompatible evidence through the authority owner. Both catalog success
and failure are fenced before publication. `captureMutation()` gives both UI mutation
paths a scoped capability backed by the authority owner's observation of the current
Product Host scope, independently of the current display catalog baseline. The
capability can commit only while that exact `(authorityId, normalized endpoint)`
scope remains current, the current native endpoint matches that normalized scope,
and the display owner has not been disposed;
an obsolete completion changes neither display evidence nor metadata navigation.
`refresh()` accepts no deletion evidence. Navigation changes and compatible catalog
observations do not retire the mutation capability. Native App Server authority
replacement retires display evidence and its catalog baseline, but does not retire a
still-current Product Host mutation capability. Every accepted completion advances
the catalog/classification invalidation fences and starts or queues a post-commit
catalog observation. A retained compatible catalog receives the removal delta
immediately, with matching confirmed associations cleared/refreshed; a failed reread
cannot resurrect that removal. Without a display catalog, the completion still
invalidates pre-commit reads and schedules the fresh observation, without fabricating
a baseline or retaining a stale one. Late pre-commit catalog success or failure cannot
publish over the committed fact. Complete successful catalogs also remove missing registrations.
Neither missing-registration invalidation nor explicit unregister completes a new
classification. Missing catalog membership removes the old confirmation immediately;
explicit unregister may retain confirmed ungrouped presentation. Both leave the
current classification revision unsatisfied. Only a complete current Host response
can establish fresh classification, including authorized-but-unregistered or denied.
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

The snapshot status projects only the current bounded page, selected off-page
Session, and connection/catalog observation. Disconnected status takes precedence,
then unavailable catalog, then pending/refreshing catalog. With a ready catalog,
current unavailable demand outranks pending, then refreshing, then ready; definitive
denial is settled, not a pending read. Revision mismatch represents unsatisfied
work even when queued behind the two-read bound. Inactive cached pending/unavailable
entries cannot report current activity or failure. They remain retained, and returning
to an unchanged classified identity can reuse its valid evidence.

Native process identity is not durable-store identity: a new UUID proves neither a
new store nor the same store. Endpoint/cwd/Session ID/title coincidence proves neither.
Old deletion obligations move into the client's existing bounded detached evidence,
never the replacement's active views or operation preconditions. See
[connection ownership](CONNECTION.md) for explicit browser-only handling.

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

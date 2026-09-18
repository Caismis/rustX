# rustX Web console

Harness owns the presentation baseline; rustX owns all runtime/product semantics.
The ordinary Session surface follows the [WEB-12 product audit](PRODUCT-SURFACE.md):
one identity/view header, contextual product status, and a Session actions menu.
Exact runtime facts live in the read-only Developer Inspector. Normal opening and
restoration use the existing native client attachment path without lifecycle buttons.
The normal Agent path uses the integrated Conversation, Composer, specialized
Tool cards, interaction takeover and model/permission controls. See
[Agent architecture](AGENT-ARCHITECTURE.md) and [#346 validation](RESET-346-VALIDATION.md).
There is no legacy Agent mode or compatibility component API.


The rustX Full Web client: a Vite/React product over the native App Server.
Start with the canonical [local development launchers](../DEVELOPMENT.md).
The [Full Web dogfooding guide](DOGFOODING.md) and [conformance map](CONFORMANCE.md)
cover strict acceptance fixtures.
**Reuse the UI; keep runtime and protocol authority in rustX.**

Selected DeepSeek Harness source is checked in under `src/presentation/`, pinned to
`ddefc45fbc7f8e46dd73185e68295696d1297887`. Read [PROVENANCE.md](PROVENANCE.md)
and the per-file [source inventory](source-inventory.json). Building or running this
package never fetches Harness and requires no Harness Host or Agent.

## Install, check and build

Use Node 24+ and the exact pnpm in `package.json` (Corepack can install it).
From this directory:

```sh
corepack enable
corepack install
pnpm install --frozen-lockfile
pnpm typecheck
pnpm test
pnpm check:provenance
pnpm build
pnpm dev
```

Open `http://127.0.0.1:5173`. `pnpm preview` serves the production `dist/` at
`http://127.0.0.1:4173`. Both bind loopback. There is no frontend lint configuration;
strict TypeScript, deterministic tests and production build are the frontend checks.
The separate CI job also runs the real-browser integration below. Rust-only jobs do
not depend on Node; generated protocol and TUI checks have their own jobs.

## Normal local development

Use the [canonical development launcher](../DEVELOPMENT.md):

```sh
pnpm --dir dev web -- \
  --config /absolute/path/rustx.toml \
  --runtime-root /absolute/path/runtime \
  --workspace /absolute/path/workspace
```

It owns the real App Server, ephemeral transport credential, exact-root Product
Host configuration, and Vite carrier. Open the printed browser URL and enter the
printed endpoint plus the contents of the private token file in **Connect**.
The native settings resolver remains authoritative; no fake provider starts.
Choose an authorized Workspace or open a listed native Session. The browser never
supplies arbitrary cwd authority or supplies configuration authority.

Direct `pnpm dev`/`preview` remain component-only commands for focused UI work or
an independently managed runtime/Host. The [Host contract](WORKSPACES.md) describes
that operator-owned integration. Use the launcher for complete local composition.

Authentication is #36's **local/trusted, single writable controller** boundary.
The browser sends subprotocols `rustx.app-server.v7` and `rustx-token.<token>` in its
WebSocket handshake. No arbitrary authorization header, URL credential, login,
OAuth, tenancy, BFF or production hosting layer is introduced. Use the matching
App Server transport token, never a provider key. Provider/MCP credentials are
resolved by rustX and never entered, stored or requested here. The dedicated socket
token stays in page memory; reload requires entering it again. Only the safe
endpoint and up to 32 navigation IDs are stored in localStorage. They are hints to
read server state, not persisted conversation or interaction state.

See [App Server transport/protocol](../docs/app-server-protocol.md) for supported
bind/authentication semantics and deployment boundaries. No browser UI can redact
secrets a developer intentionally includes in a prompt/Tool result; the inspector
shows that actual application traffic. It never records handshake credentials or
requests provider/MCP configuration.

## Ownership and recovery

- `src/presentation/`: generic theme, primitives, responsive layout and incremental
  Markdown. No imports of app, bindings, client or generated protocol are allowed.
- `src/app/components/`: extracted product sidebar, bubbles, composer, Tool and
  interaction cards. Questionnaire DTOs and draft encodings belong here and in bindings.
- `src/app/` and `src/bindings/`: render generated native snapshots into these
  components and turn gestures into typed native operations. Canonical messages
  come from `snapshot.messages`; current activity comes from `snapshot.attempt`.
  An in-flight message with an already committed ID is suppressed. No Harness
  event model, fake V3 Session log, optimistic conversation or event reducer exists.
- `src/client/`: one WebSocket, generated `protocol/app-server/v7.ts` unions,
  correlation IDs, initialize/capabilities, bounded requests, native routing,
  replaceable snapshots, connection/attachment fences and wire observer. Rust DTOs
  remain authoritative. The shared generator normalizes schema `$ref` siblings
  before TypeScript compilation so canonical message fields are not lost; it does
  not alter the Rust schema or public wire contract.

Each Session has its own target tuple (SessionId, ConversationId, runtime
incarnation, attachment ID), cursor and snapshot. A/B notifications cannot share a
projection. Successful attach installs the native authoritative snapshot and atomic
subscription. Events invalidate that Session's cache: one in-flight snapshot read
and one dirty bit coalesce bursts. `resyncRequired` causes snapshot followed by
subscribe at the returned cursor. No unbounded event queue or browser replay log
exists. This intentionally favors clear ownership over token-by-token rendering.

Session selection lives only in the Sidebar. The center header identifies the
current Session and offers bounded actions, Inspector, and Chat/Trajectory views;
there is no top Session tab strip or alternate picker. Close view is in each open
Session's Sidebar row menu; the 32-view bound is actionable there. Internal
`openViews` represents actual browser controller ownership, never a hidden tab UI.
Sidebar View options → Close all views also releases unlisted/restored views, so
missing catalog entries cannot strand capacity. This is explicit, never eviction;
server-owned work continues exactly as for closing one view.

Every ordinary Session label uses `sessionDisplayTitle`: explicit native name >
native `SessionSummary.preview` > **New session**. Session IDs stay in Inspector.
Manual naming remains `session/name`; no LLM call, generated title, truncation or
first-message summarizer exists in React. After authoritative canonical user-message
observation, a previously unnamed view reads exact native `session/summary` metadata.
Only success marks the first-message check complete, including a file-only `preview: null`.
Failures remain retryable on authoritative refresh/reconnect; concurrent reads coalesce.
Drafts, accepted inbound and optimistic submission never supply preview text.
`session/list` is fuzzy, bounded catalog browsing, never exact identity resolution.
`view.summary` retains replaceable exact metadata for off-page/restored views without
changing the Sidebar page or owning catalog membership. Read-order/generation fences
prevent older observations overwriting newer summaries. Rename rereads the exact summary.

Every new connection has a new generation. Socket callbacks, resolved request
continuations and attachment workers are fenced. A fresh attach/snapshot replaces
stale observations; old attachment work cannot overwrite a new incarnation, even
within the same connection. Native unload may close an attachment before its reply;
its acknowledgement applies only to the attachment lifecycle that requested it.

**Disconnect** closes only the socket and stays disconnected. An unexpected loss
marks observations stale. **Reconnect** is explicit: initialize, list, reattach
Sessions whose current `attachmentIntent` is `wanted`, then replace snapshots.
There is no automatic connection loop. Selection controls focus; `openViews`
tracks bounded browser views; `attachmentIntent` records desired controller ownership;
`attachment`/target and uncertain-operation diagnostics record server observations.
Open/Attach sets intent to `wanted`. Detach, unload and closing a view set it to
`released` immediately, before any RPC acknowledgement or failure. Server results
never change intent. Loss marks an observed route stale regardless of intent.

Closing a view through its Sidebar row menu removes its open-view hint immediately
and sends only **session/detach**
when an observed target is available. If attach is in flight, the release waits for
its exact target; an explicit reopen waits for release before acquiring a fresh
attachment. These operations are serialized per Session, bounded to 64 queued or
running changes, and fenced by connection generation. They are never retried.
A detach failure/uncertain result stays diagnostic without reopening the view.
Switching Sessions and React unmounting alone remain presentation-only.

Disconnect, closing a view, switching focus, and React unmounting never cancel,
answer, unload or delete. Detach releases only external controller/subscription
ownership, so work, loaded runtimes and pending interactions survive. Explicit
**Unload runtime**, available only in **Session actions → Advanced Session controls**,
is the native shutdown operation and may settle active work.
After lost detach/unload acknowledgement, released intent prevents reconnect from
attaching or cold-loading the Session to discover the outcome. Only a later
explicit **Open Session** sets wanted intent again. A fresh attach
cannot by itself prove the previous mutation's outcome, so uncertainty remains.

The endpoint/openViews navigation hints retain only wanted views for automatic
page-reload restoration. A released Session remains in the Sidebar catalog, without
remaining a resume hint; no persisted observation or request state is introduced.
On a fresh page the Session remains available through the native list
for explicit Open. Opening or resuming resolves through rustX's canonical
configuration owners.

Request IDs provide correlation only. There are at most eight transmitted and 64
pending/queued calls, below the transport's native work bound. The 30-second
response deadline closes the transport rather than resending. A transmitted
side-effect with no acknowledgement is **outcome uncertain**, including create,
delete, turn/cancellation, interaction and lifecycle operations. Unsent calls are
discarded. Reconnect reads authority first; no mutation is automatically replayed.
Non-interaction uncertainty remains a diagnostic even when a snapshot is suggestive:
the protocol cannot prove request identity. The product warning and Inspector
retain that unresolved evidence. Up to 64 unresolved mutations can be retained; new mutations are
refused before diagnostics would be silently dropped. After inspecting evidence and
verifying affected work, Connection → Review uncertain operations permits explicit
browser-local acknowledgement of non-interaction notices. This sends no RPC,
asserts no outcome, and never retries; Inspector remains read-only.

Pending interactions come only from authoritative snapshots, including routed
Subagent interactions. Approval, Questionnaire (all six native answer shapes and
partial submissions) and Review can be answered/cancelled. Controls are disabled
while stale, responding, acknowledged-but-refreshing, or uncertain. A lost
settlement acknowledgement is never resent. Native absence removes obsolete
controls and the corresponding uncertainty. A still-pending uncertain response
remains disabled; this client does not infer retry safety from a snapshot. A fresh
page has no saved request/Promise to replay: it only offers the interactions rustX
still publishes. Exactly-one settlement remains runtime-owned.

## Inspector and supported presentation

The inspector exposes Session/Conversation identity, attachment/runtime incarnation,
connection generation, cwd, observed residency, safe model/settings generation,
active attempt, pending interactions, Subagents, Workflows and background counts.
Native Tool calls/results use extracted disclosure cards. Current Todo, Goal and
pending inbound state render as the composer context docks ([COMPOSER.md](COMPOSER.md)).
Workflow, Subagent and background activity keep named progress cards; execution
identities and raw Agent status are in Inspector. Live versus last-observed values are labelled. Unknown residency is not
inferred as unloaded.

The raw log observes actual incoming/outgoing JSON-RPC text **before adaptation**.
Responses retain their correlated method and Session where known. Filters match
method/Session substrings and message kind. **Copy JSON** copies the visible entries
with metadata and original text. **Pause log** freezes presentation only; protocol
processing and the bounded live ring continue. Resume shows the latest ring;
clear resets retention/counts and respects pause. Pause is not backpressure.

Limits: 300 entries, 1 MiB of retained UTF-16 text and 32 KiB per entry. A paused
view can retain one additional bounded snapshot of that ring. Dropped and truncated
counts are exposed (while paused the displayed counts are frozen until resume).
Truncated entries are explicitly labelled raw prefixes and need not parse as full
JSON; the copy envelope itself is valid JSON. Metadata is bounded by entry count.
The log is disposable diagnostics, never durable history. No fetch or remote
telemetry is used by it.

Supported native gestures: list/create/open, delete preview and revision-checked
delete, Send/Queue/Steer and cancel, Goal pause/resume/edit, typed slash commands,
native Fork/Branch/Retry and Session tree navigation, explicit model selection and configuration-owned approval policy,
answer/decline/cancel interactions and contextual reconnect. Closing a view releases
its controller without stopping work; advanced unload remains explicit. Resync,
detach and cold attach remain native client capabilities, not permanent product buttons.
Commands are client grammar, never server command strings. Unsupported slash input
is refused without prompt fallback. Retry creates a native branch and executes its
returned input once; original canonical history remains unchanged. Fork relies on
#319 for independent destination upload ownership. Lost mutation responses remain
uncertain and are never replayed; navigation/reconnect fences late continuations.
Queue and Steer expose the shared native inbound mailbox. Exact pending edits
and removal use WEB-06 revisioned operations; native semantics provide no reorder action.

See [COMPOSER.md](COMPOSER.md) for the catalog and interaction contract and
[CHAT.md](CHAT.md) for historical boundaries, cold-lineage settings and retry order.
Workspace navigation uses the Product Host. Provider/model and MCP definitions
have native typed structured editors. Skill/Agent/Workflow content, terminal,
browser canonical history and Harness runtime authority are outside Web v1.
Assistant text uses the incremental Markdown foundation; reasoning/Tool/other
blocks retain disclosures. Native transcript paging and typed attachment cards
are described in CHAT.md.

## Reproducible real-server browser fixture

No real model credentials are needed. This uses the repository's external provider
emulator over real HTTP/SSE, with the ordinary App Server and native Tools:

```sh
# repository root
cargo build --bins
cd test-support/fake-provider
uv sync --frozen
cd ../../web-console
pnpm exec playwright install chromium
pnpm test:e2e
```

`pnpm test:e2e` builds the frontend and serves its production output on port 5173
(which must be free). The shared Playwright acceptance suite drives concurrent Sessions, streaming, browser loss,
interactions, detached publication, cold settings resolution and the wire inspector.
Named provider gates and explicit observations establish ordering; there are no
race-proof sleeps. The provider scenario's final report **and exit status** must
succeed. Missing browser, Rust binary or emulator is a failure, never a silent skip.
Runtime screenshots and failure traces go to ignored `test-results/`. The ten deterministic
#345 shell references are checked in under `test/e2e/shell.spec.ts-snapshots/`.

## Manual dogfooding procedure

Follow [DOGFOODING.md](DOGFOODING.md). It supplies the Product Host configuration,
named local provider scenarios, exact prompts/gates, editor/CAS checks, and
keyboard/responsive checks. The fixture must be paired with its printed Host
configuration; running an unconfigured Web server intentionally fails closed.

See [VALIDATION.md](VALIDATION.md) for executed checks, environment limits and
which observations were automated versus manually inspected.

## Harness presentation shell

[SHELL-ARCHITECTURE.md](SHELL-ARCHITECTURE.md) records the #345 reset. Direct imports
from `presentation/` form one Harness-derived foundation. AppFrame accepts Sidebar,
main, right-panel and overlay seats. Its measured columns retain the upstream
280px Sidebar, 56px rail, 1024px responsive collapse and right-panel constraints.
The Sidebar Settings entry opens the shared modal/navigation frame; existing CFG3
editors supply its content. Inspector uses the right panel and is closed initially.
Connection settings are reachable from the Sidebar and Settings frame.

Menus, HoverCards, Tooltips and Modals own only presentation/focus state. The
workspace adapter projects native list/cwd, Host classification and current
snapshot status. Only AppServerClient and rustX's native owners execute product
operations. No old shell, alternate primitives or compatibility exports remain.

`MarkdownText` receives `{ text, streaming?, labels? }`. Canonical content is always
plain text from rustX. The parser freezes all but the final two blocks and caches
React elements with source-offset keys. An open top-level code fence advances a
second frontier: only its last completed line/current partial line is reparsed.
Completed highlighted lines retain tokens/grammar state and React groups of 32.
Non-append text resets the generation. Settling performs one full math-aware parse,
resolving references across freeze boundaries and replacing streaming caches.

Memory invariant: one current source value (references may share it), one current
frozen/tail AST, current rendered prefix/tail, and code-prefix/token caches, all
O(current document size). There is no per-chunk AST or rendered-tree history.
Immutable arrays copy references, not every historical tree. The open-fence
frontier retains a constant number of code-value strings, not one per chunk.
A single ever-growing paragraph/list, nested fence or single unterminated code
line can still require reparsing the unstable tail; this is not a universal
constant-time parser. Append detection compares the current source prefix.

Streaming TeX intentionally stays literal; settled TeX uses untrusted KaTeX.
Reference links/footnotes crossing a frozen boundary may stay literal until
settlement, matching the pinned architecture. Raw HTML is text, links allow only
HTTP(S)/mailto, and images are inert alt text. No workspace/file actions are exposed.
Unknown code languages remain plain. Clipboard denial does not report success.

The normal real-server acceptance test still runs against the production build.
Additional browser contracts use an isolated Vite fixture on port 5174, excluded
from the production entry tree, to exercise native focus and layout at 390/900/1440px.
See [PROVENANCE.md](PROVENANCE.md) for the source/closure audit and
[RESET-345-VALIDATION.md](RESET-345-VALIDATION.md) for the shell baseline and
[RESET-346-VALIDATION.md](RESET-346-VALIDATION.md) for integrated Agent evidence.

## CFG3 Settings

Settings has **Effective | User | Workspace** views. Effective is read-only and
reports the loaded generation. User and Workspace edit structured authored
semantic units, including independent Provider/Model identities, Root capability
selections, Plugins, MCP definitions and complete named-Agent resources.

Save commits one revision-fenced source mutation and leaves the loaded runtime
unchanged. Reload publishes one coherent generation or reports typed busy/failure
with the old generation authoritative. Conflicts preserve the draft and original
revision; uncertain outcomes trigger rereads without mutation replay. The bound
User config path and fixed User resource root are displayed separately.

See [the complete Settings contract](../docs/web-settings.md) and
[configuration reference](../docs/configuration.md). Browser regressions exercise
Save/Reload, CAS external edits, named-Agent editing and desktop/mobile views.

The [Settings/native auxiliary architecture](SETTINGS-ARCHITECTURE.md) documents CFG3 ownership, draft recovery, resource cards, appearance and bounded artifact preview.

Reset #347 validation and browser evidence: [acceptance record](RESET-347-VALIDATION.md).

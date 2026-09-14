# rustX Developer Web Console

A separate Vite/React browser application for App Server dogfooding (#289).
**Reuse the UI; keep runtime and protocol authority in rustX.**

Selected DeepSeek Harness source is checked in under `src/presentation/`, pinned to
`c291e7961a515f6d7af9304e7fd1d257929aef26`. Read [PROVENANCE.md](PROVENANCE.md)
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
pnpm build
pnpm dev
```

Open `http://127.0.0.1:5173`. `pnpm preview` serves the production `dist/` at
`http://127.0.0.1:4173`. Both bind loopback. There is no frontend lint configuration;
strict TypeScript, deterministic tests and production build are the frontend checks.
The separate CI job also runs the real-browser integration below. Rust-only jobs do
not depend on Node; generated protocol and TUI checks have their own jobs.

## Connect to your App Server

Build the binaries from the repository root and use your canonical rustX model
catalog/user settings (see the root README and [configuration](../docs/launch-configuration.md)):

```sh
cargo build --bins
# Generate a dedicated socket credential, independent of provider credentials.
python3 -c 'import os,secrets; p="/tmp/rustx-console-token"; fd=os.open(p,os.O_CREAT|os.O_EXCL|os.O_WRONLY,0o600); os.write(fd,secrets.token_urlsafe(32).encode()); os.close(fd)'
target/debug/rustx app-server \
  --user-settings /absolute/path/settings.toml \
  --runtime-root /absolute/path/runtime \
  --listen ws://127.0.0.1:8080 \
  --token-file /tmp/rustx-console-token
```

Read the token file locally. Enter `ws://127.0.0.1:8080/` and that token in the
console, then **Connect**. The client sends `initialize` protocol v1, checks native
capabilities, lists Sessions, and attaches saved open views. Initialization failures
and missing capabilities are visible. Enter an explicit absolute **Session cwd**
to create a Session, or open a listed Session. rustX validates paths, trust and
configuration; this UI does not author provider/MCP configuration or grant trust.

Authentication is #36's **local/trusted, single writable controller** boundary.
The browser sends subprotocols `rustx.app-server.v1` and `rustx-token.<token>` in its
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

- `src/presentation/`: extracted shell, sidebar, message bubbles, composer, Tool
  disclosures, Approval card and Questionnaire form. Local state is disclosure,
  form drafts and selection only. Unsupported upstream controls were removed.
- `src/app/` and `src/bindings/`: render generated native snapshots into these
  components and turn gestures into typed native operations. Canonical messages
  come from `snapshot.messages`; current activity comes from `snapshot.attempt`.
  An in-flight message with an already committed ID is suppressed. No Harness
  event model, fake V3 Session log, optimistic conversation or event reducer exists.
- `src/client/`: one WebSocket, generated `protocol/app-server/v1.ts` unions,
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

Every new connection has a new generation. Socket callbacks, resolved request
continuations and attachment workers are fenced. A fresh attach/snapshot replaces
stale observations; old attachment work cannot overwrite a new incarnation, even
within the same connection. Native unload may close an attachment before its reply;
its acknowledgement applies only to the attachment lifecycle that requested it.

**Disconnect** closes only the socket and stays disconnected. An unexpected loss
marks observations stale. **Reconnect** is explicit: initialize, list, reattach
Sessions whose current `attachmentIntent` is `wanted`, then replace snapshots.
There is no automatic connection loop. Three facts stay separate: React tabs own
visibility; `attachmentIntent` records this browser's desired controller ownership;
`attachment`/target and uncertain-operation diagnostics record server observations.
Open/Attach sets intent to `wanted`. Detach, unload and closing a tab set it to
`released` immediately, before any RPC acknowledgement or failure. Server results
never change intent. Loss marks an observed route stale regardless of intent.

Closing a tab removes presentation immediately and sends only **session/detach**
when an observed target is available. If attach is in flight, the release waits for
its exact target; an explicit reopen waits for release before acquiring a fresh
attachment. These operations are serialized per Session, bounded to 64 queued or
running changes, and fenced by connection generation. They are never retried.
A detach failure/uncertain result stays diagnostic without reopening the tab.
Switching tabs and React unmounting alone remain presentation-only.

Disconnect, closing a tab, switching focus, and React unmounting never cancel,
answer, unload or delete. Detach releases only external controller/subscription
ownership, so work, loaded runtimes and pending interactions survive. Explicit
**Unload runtime** is the native shutdown operation and may settle active work.
After lost detach/unload acknowledgement, released intent prevents reconnect from
attaching or cold-loading the Session to discover the outcome. Only a later
explicit **Open / Attach / cold resume** sets wanted intent again. A fresh attach
cannot by itself prove the previous mutation's outcome, so uncertainty remains.

The existing endpoint/tab navigation hints retain only wanted views for automatic
page-reload restoration. A detached tab can remain visible on this page without
remaining a resume hint; no new persisted intent, observation or request state is
introduced. On a fresh page the Session remains available through the native list
for explicit Open. **Attach / cold resume** resolves through rustX's canonical
configuration owners.

Request IDs provide correlation only. There are at most eight transmitted and 64
pending/queued calls, below the transport's native work bound. The 30-second
response deadline closes the transport rather than resending. A transmitted
side-effect with no acknowledgement is **outcome uncertain**, including create,
delete, turn/cancellation, interaction and lifecycle operations. Unsent calls are
discarded. Reconnect reads authority first; no mutation is automatically replayed.
Non-interaction uncertainty remains a diagnostic even when a snapshot is suggestive:
the protocol cannot prove request identity. Acknowledging its notice changes no
server state. Up to 64 unresolved mutations can be retained; new mutations are
refused before diagnostics would be silently dropped.

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
Native Tool calls/results use extracted disclosure cards. Todo, Goal, Workflow,
Subagent, inbound and background/status values use simple JSON disclosure where
specialized Harness semantics do not fit. Live versus last-observed values are
labelled. Unknown residency is not inferred as unloaded.

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
delete, start/steer/cancel, answer/decline/cancel interactions, resync, detach,
unload/cold attach, disconnect/reconnect. No workspace manager, editor, terminal,
provider setup, file navigation/upload, Harness commands, queue/retry, fork,
policy preset, plugin controls or unsupported status actions remain. Conversation
text is plain text; reasoning/Tool/other blocks have disclosures. Rich Markdown,
image rendering and a separate historical transcript browser are outside this
bounded console; canonical server-projected messages remain visible.

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
(which must be free). One focused Playwright scenario drives concurrent Sessions, streaming, browser loss,
interactions, detached publication, cold settings resolution and the wire inspector.
Named provider gates and explicit observations establish ordering; there are no
race-proof sleeps. The provider scenario's final report **and exit status** must
succeed. Missing browser, Rust binary or emulator is a failure, never a silent skip.
Desktop/mobile screenshots and failure traces go to ignored `test-results/`.

## Manual dogfooding procedure

Run `pnpm dogfood:server` after building binaries and syncing the emulator as above.
In a second terminal run `pnpm dev`. The fixture prints endpoint, token-file path,
explicit workspaces A/B, user-settings path, and provider-control URL. Read the
transport token file, enter it in the browser and connect. The fixture's private
configuration uses only a fake provider key and sets bash approval to `always`.
It grants trust for its two temporary workspaces using the public rustX CLI.

Follow this exact order because the provider is scripted:

1. Create A and B using their printed absolute cwds. Keep both tabs open.
2. In A send **Long action in A**. Wait for **A is running.** and inspect the native
   active attempt. The provider is held at `finish-a`.
3. In B send **Use B while A runs** and observe **B stayed responsive.** Return to A.
4. Disconnect, reconnect, then reload the page. Re-enter the transport token and
   connect. The active A projection and both tab IDs return from authoritative state.
5. Close A's tab and verify its Session-list row says detached. Release the provider
   gate (replace `<control>` with the printed URL):
   `curl -X POST <control>/gates/finish-a/release`.
   Reopen A from the Session list and observe **A is running. A finished.** once in
   committed conversation. Closing the view released its controller, not its work.
   Repeat close/open on A and B; each closed row should become detached.
6. Send **Approval please** in A. Disconnect/reload while **Allow once** is pending,
   reconnect, then allow. The real bash Tool returns `console-approved`, followed by
   **Approval completed.** Open its Tool card and inspect the IN/OUT values.
7. Send **Questionnaire please**. Reload/reconnect, choose **Keep native**, submit,
   and observe **Questionnaire completed.**
8. Send **Publish while detached**. Wait for **Preparing a question.**, then explicitly
   Detach A. Release `<control>/gates/publish-question/release` via POST. Await
   `<control>/observations/await?kind=response_completed&count=7&timeoutMs=30000`.
   Attach A; the question created with no attached browser must appear. Answer
   **Keep native** and observe **Detached question completed.**
9. Filter/copy actual `session/attach`, `interaction/respond` and snapshot traffic;
   exercise pause/resume and clear. Observe attachment IDs and connection generations.
10. Edit only the printed **server** settings file: change `fixture/console-model` to
    `fixture/second-model`. Resync while loaded; the safe model still says
    `console-model`. Explicitly unload, then cold attach. It now says `second-model`,
    and canonical history/cwd are still present. The browser did not author settings.
11. Optionally unload B and exercise its native delete preview/confirmation. Session
    deletion may report native blockers, stale revision or durability diagnostics;
    those are shown without converting them into success.
12. Ctrl+C the fixture terminal. It reports whether all eight required provider
    requests matched and finished and then removes its temporary state. An early
    stop intentionally returns an unsuccessful report.

See [VALIDATION.md](VALIDATION.md) for the exact checks and separately recorded
browser dogfooding actually performed for this implementation.

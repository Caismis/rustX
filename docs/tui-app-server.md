# The TUI as an App Server client

`rustx-tui` is a client of the App Server protocol. It is not the owner of
Session, ConversationRuntime, execution, interaction, cancellation, residency or
persistence semantics — the App Server is, and the terminal renders a projection
of what that server publishes.

```text
                     one App Server protocol (v1)
                                |
                    one typed TUI client layer
                      /                     \
                     /                       \
              stdio JSONL                 WebSocket
        TUI-owned child process      existing / remote server
```

## Two modes, one protocol

```sh
# local self-hosted: the TUI spawns and owns its App Server
rustx-tui --binary /usr/local/bin/rustx \
          [--user-settings ~/.config/rustx/settings.toml] \
          [--models ~/.config/rustx/models.toml] \
          [--runtime-root ~/.local/state/rustx/app-server] \
          [--cwd /work/project] [--config /work/project/rustx.toml]

# existing / remote: the TUI connects to a server someone else runs
rustx-tui --connect ws://127.0.0.1:8080 --token-file /private/user/socket-token \
          --cwd /srv/project
```

Local mode spawns exactly one `rustx app-server --listen stdio` child and speaks
the App Server protocol over JSONL on its pipes. Remote mode opens one WebSocket
and speaks the same protocol over text messages, using the transport's
[dedicated credential](app-server-protocol.md#dedicated-websocket-credential):
the client offers `rustx.app-server.v1` and `rustx-token.<token>` as
subprotocols, and refuses to proceed unless the server selects the former.

There is no loopback WebSocket for ordinary local use. Unification is a property
of the protocol, not of the socket: after the transport is bound, nothing above
it knows which mode it is in.

### Where each option belongs

Three groups, deliberately not mixed:

| Group | Options | Scope |
| --- | --- | --- |
| Mode | `--binary`, `--connect`, `--token-file` | which App Server, and who runs it |
| Process bindings | `--user-settings`, `--models`, `--runtime-root` | the App Server **process**; local mode only |
| Session settings | `--cwd`, `--config`, `--model`, `--name`, `--skill`, `--no-automatic-skills`, `--no-builtin-tools`, `--no-direct-tools`, `--tools`, `--exclude-tools` | `session/create` inputs |

A Session cwd belongs to the App Server's filesystem namespace. In local
self-hosted mode, omitted `--cwd` defaults to the TUI process cwd because the
owned child shares that filesystem. Every remote launch requires an explicit
absolute `--cwd` on the **server host**, including launches using `--session`
since `/new` can create Sessions later. The client passes that path unchanged;
it never supplies its own cwd, home directory, or token-file location as a
remote default. Missing or relative remote cwd fails before connecting.

Process bindings are rejected against `--connect`: the external server already
owns its process configuration.

Routing (`--session <id> [--node <id>]`, `--resume`) selects which Session the
terminal opens on. It is client focus and nothing more; the App Server has no
global active Session.

## The client layers

```text
tui/src/protocol/app-server.ts   generated #288 DTOs, re-exported and derived
tui/src/app-server/client.ts     the one typed client: initialize, ids,
                                 correlation, notification dispatch, settlement
tui/src/app-server/session.ts    one attached Session: snapshot, subscribe,
                                 resync, fold, and its typed operations
tui/src/app-server/host.ts       the connection, the durable Session catalog,
                                 and who owns the server process
tui/src/app-server/transport.ts  complete messages in, complete messages out
```

Every wire type comes from `protocol/app-server/v1.ts`, generated from the Rust
DTOs in `src/app_server/protocol.rs`. `tui/src/protocol/app-server.ts` re-exports
those types and derives the ones the generator inlines; it transcribes nothing.
A Rust DTO change regenerates the TypeScript and fails `pnpm typecheck` at every
use site.

Transport differences end below the typed client. `StdioTransport` and
`WebSocketTransport` deliver untrusted parsed JSON and connection lifetime;
neither knows what a Session, a Turn, an approval or a retry is.

Transport framing and JSON parsing produce untrusted values. Before correlation
or notification delivery, the single App Server client validates each value
against the Rust-generated `v1.schema.json`, compiled once by its protocol
decoder. Invalid envelopes or nested DTOs terminate the connection as
`protocol_error`; no partial message reaches host, Session, or UI code.

A total `Record<MethodName, ResponseLossClass>` policy classifies every generated
method. Reads fail with the connection; side-effecting requests have unknown
outcomes and are never replayed. Initialize and subscription changes are
connection-local and must be established anew after reconnecting. New protocol
methods cannot compile until their response-loss semantics are classified.

## Resume bootstrap

`--resume` reads the durable Session catalog and shows the picker with no
focused Session and no attachment. Catalog order is deterministic Session-id
order, not recency. Browsing does not load, control, cancel, or unload a Session.
Selecting a row acquires only that Session's attachment/controller; a conflict
is reported for that explicit selection and leaves the picker available to
choose again. An unrelated controlled Session cannot block browsing.

If the initial catalog is empty, startup explicitly creates one Session with
the launch's Session settings, attaches it, and opens normal presentation.
Ordinary startup also creates/attaches one Session; `--session ID` attaches that
exact identity directly. During unfocused browsing there is no transcript or
runtime interaction/model/footer projection and runtime input is disabled.
Esc closes the picker; Enter reopens it and Ctrl+C exits. Reconnecting before
selection refreshes the host/catalog without attaching anything. Reconnecting
after focus reattaches the known Session/node and repairs from server state.

## Session switching is focus

The Session picker reads `server/diagnostics` alongside the durable catalog and
shows loaded/loading/unloading/unloaded state plus native root activity. These
are server observations, not a client scheduler.

Showing a different Session attaches to it, or reuses the attachment this
connection already holds. That is all it does.

```text
Session A starts long-running work
        |
TUI changes focus to Session B      <- no process replacement
        |                              no cancellation, no unload, no detach of A
Session B is usable
        |
Session A keeps executing in the same App Server process
        |
TUI switches back to A
        |
an authoritative snapshot repairs the visible projection
```

One local App Server child keeps many Sessions loaded and running at once. The
`restart_required` / one-active-process model is gone: there is no client path
that replaces a process because the visible Session changed.

The server permits one live conversation node per Session. Tree navigation to
another node requires an explicit **Unload and open node** confirmation. That
user action invokes native `session/unload` for this Session, then attaches the
selected node. It never unloads another Session. Ordinary A/B focus changes
retain both attachments and never offer or perform unload. `/unload <session-id>`
is an explicit command for an attached background Session, including when the
user wants to release it before native deletion. Durable history remains.

## Process ownership

Ownership is established by **who spawned the process**, and is never inferred
from a Session id, an endpoint address, loopback versus remote, or the transport
type.

| | local self-hosted | existing / remote |
| --- | --- | --- |
| Spawned by | this TUI | someone else |
| On exit | signals SIGTERM, closes stdin, and waits for server-owned drain and exit | closes this socket |
| Effect on the server | the process this TUI owns ends | none |
| Effect on other Sessions | they end with the process | none |
| Work in flight | settled by native drain; forced termination reports unproven settlement | continues |

Local exit requests the server-owned drain before the process ends. Transport
detach has no execution authority. Persistent execution
across TUI exit is what an externally managed App Server is for — and that is the
same server the Developer Web Console (#289) connects to, so both can be pointed
at one `rustx app-server --listen ws://…` for dogfooding.

#291 is merged: the App Server owns runtime drain, its deadline, and residency
policy. The TUI sends the owner signal and observes exit; it defines no second
semantic shutdown state machine. A failed startup uses bounded process termination
to avoid leaving its child behind.

## Losing a response

> Losing a response does not prove that a side-effecting request was never
> accepted.

The client never replays a request. When a connection dies with requests in
flight, every pending request settles exactly once, and a request that could have
changed server state settles as an **unknown outcome** rather than a failure —
because "it did not happen" is a claim the client is not entitled to make. That
covers `turn/start`, `turn/steer`, `turn/cancel`, `interaction/respond`,
`interaction/cancel`, `session/create`, `session/delete`, rename, fork, branch,
unload and every settings mutation. Reads are not in that set: reissuing one
after reconnecting changes nothing.

Recovery is always the same shape, and never a resend:

```text
new transport -> initialize -> attach -> authoritative snapshot -> repair
recovery failure -> Ctrl+R retries recovery; Ctrl+C exits
```

The terminal attempts remote recovery once automatically, preserving the focused
Session/node and unsubmitted editor text. A failed attempt leaves input disabled
until explicit recovery. Pending mutations are never copied to the new client.

A disconnect cancels no turn, settles no interaction, answers no approval or
Questionnaire, unloads no Session and edits no history. Accepted work continues
while this client is not watching.

For a local child, an unexpected pipe or process death is a **connection and
process failure** and is reported as one. Nothing fabricates a settled attempt,
an answered interaction or a completed tool to make the screen look tidy.

## Identity and stale fencing

Session ID, Conversation ID, runtime incarnation, attachment ID, projection
cursor and JSON-RPC request ID are six distinct domains. Every notification
repeats the full `AttachmentTarget`, and an attachment folds an event only when
all four of its identity domains match — so an event addressed to a superseded
incarnation cannot reach the projection that replaced it.

Responses are fenced the same way. An authoritative read issued against one
attachment may still be in flight when that attachment is replaced or released;
installing its result afterwards would overwrite current truth with stale truth,
so the projection's epoch is checked before any install. The same rule governs
presentation: an overlay, a picker or a transient message belongs to the Session
that opened it.

Cursors, revisions and incarnations are exact `u64` domains carried as canonical
decimal **strings**. They are compared numerically through `BigInt`, never
lexicographically — `"10" < "5"` as text, and a client that ordered cursors that
way would silently drop every event past cursor 9.

## What the terminal still does

The migration changed connectivity and lifecycle, not the product: streaming
conversation display, Markdown rendering, tool cards and background execution
views, Approval, Questionnaire and MCP interaction surfaces, Subagent views,
Workflow views, Todo, Goal, Session naming/history/tree/delete flows, and the
model and settings controls all work through the new boundary.

One capability was removed rather than migrated: opening a subagent's child
conversation as a read-only attachment. That was a Runtime Client *process*
capability (`rustx --inspect-conversation`), and the App Server exposes no method
for attaching to a conversation that is not a Session. Selecting a subagent now
opens its authoritative `subagent/status` detail instead.

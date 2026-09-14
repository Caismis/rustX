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
          [--cwd /srv/project]
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

A Session's cwd is a Session selection. The App Server's own launch directory is
never substituted for it, in either mode. Process bindings are rejected against
`--connect`: a remote App Server was launched by someone else and already has its
own, and a flag that pretends to configure a server it cannot reach is worse than
no flag.

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
`WebSocketTransport` deliver complete protocol messages and connection lifetime;
neither knows what a Session, a Turn, an approval or a retry is.

## Session switching is focus

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

Detach and `session/unload` still exist as deliberate product actions. Neither is
what navigation does.

## Process ownership

Ownership is established by **who spawned the process**, and is never inferred
from a Session id, an endpoint address, loopback versus remote, or the transport
type.

| | local self-hosted | existing / remote |
| --- | --- | --- |
| Spawned by | this TUI | someone else |
| On exit | closes the child's stdin; the child sees EOF, detaches and exits; SIGTERM/SIGKILL only if it overstays its grace | closes this socket |
| Effect on the server | the process this TUI owns ends | none |
| Effect on other Sessions | they end with the process | none |
| Work in flight | may be lost, because the owned process is deliberately ending | continues |

Losing in-flight work on a normal local exit happens because the process ends,
not because detaching a transport is execution authority. Persistent execution
across TUI exit is what an externally managed App Server is for — and that is the
same server the Developer Web Console (#289) connects to, so both can be pointed
at one `rustx app-server --listen ws://…` for dogfooding.

#291 owns graceful runtime drain and residency policy for that owned process.
This client integrates with the shutdown boundary that exists today and defines
no second shutdown state machine.

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
```

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

# `rustx-tui`

The rustX reference terminal client: a Pi-TUI presentation layer over Runtime
Client Protocol v1.

## The one architectural rule

**Pi TUI is only the terminal input/output projection of rustX.**

It is not a runtime, not a session owner, not an agent framework, not a model
framework, not a tool runtime, not a capability authority, and not a second
source of conversation truth.

```text
user keyboard                          rustX Runtime
     |                                       |
     v                                       v
Pi Editor / controls              Runtime Client snapshot/events
     |                                       |
     v                                       v
rustX intent/command layer        rustX presentation projection
     |                                       |
     v                                       v
Runtime Client request            rustX presentation components
     |                                       |
     v                                       v
rustX Runtime                         Pi rendering primitives
                                             |
                                             v
                                          terminal
```

Pi provides terminal mechanics. rustX provides semantics. The dependency
direction is never reversed:

```text
rustX Runtime semantics
        -> Runtime Client Protocol
        -> rustX TypeScript projection
        -> rustX TUI presentation
        -> @earendil-works/pi-tui primitives
```

### The test for the layering

> If `@earendil-works/pi-tui` were replaced tomorrow with another terminal
> rendering library, would rustX Runtime Client semantics, protocol handling,
> presentation reduction, model state, tool state, background state, and
> command semantics remain valid?

Yes. Pi is imported by four files, all of them presentation: `src/ui/app.ts`,
`src/ui/components/model-selector.ts` (a `Component`), `src/ui/components/
transcript.ts` (one type import), and `src/commands/autocomplete.ts` for the
`AutocompleteProvider` interface it implements. Everything below `src/ui/` is
plain TypeScript over protocol values.

No suite needs a terminal. Framing, RPC, presentation projection, session
lifecycle, the model invariant, tool correlation, the process owner, and the
real `rustx` integration never reach `@earendil-works/pi-tui` at all, directly
or transitively; the presentation suites reach it only to render strings.

## Requirements

- **Node**: the nvm LTS line (`.nvmrc` says `lts/*`). The package declares
  `"engines": { "node": ">=22.19.0" }`, which is what `@earendil-works/pi-tui`
  requires.
- **Package manager**: pnpm, via Corepack. The pinned version is recorded in
  `packageManager`.

```sh
nvm install --lts
nvm use --lts
corepack enable
pnpm --dir tui install --frozen-lockfile
```

`npm install`, `npm ci`, `yarn`, and `bun` are not the project workflow; the
dependency graph and lockfile are owned by pnpm.

## Running

The TUI owns the lifecycle of the `rustx` child process and nothing else. Build
the binary first, then point the client at it and at the runtime's own
configuration paths:

```sh
cargo build --bin rustx

pnpm --dir tui start \
  --binary "$PWD/target/debug/rustx" \
  --models  "$PWD/examples/local-runtime/models.json" \
  --session "$PWD/examples/local-runtime/session.json" \
  --workspace "$PWD/examples/local-runtime/workspace" \
  --runtime-root "$PWD/examples/local-runtime/.rustx"
```

The four runtime paths are passed straight through. **The client never opens,
parses, or interprets any of them** — `models.json` is a runtime-owned model
authority, and reading it here would create a second one. Provider credentials
are resolved by the Rust process from the environment it inherits. For the
complete copyable configuration and Python-tool example, see
[`examples/local-runtime/README.md`](../examples/local-runtime/README.md).

## Startup sequence

```text
spawn rustx
  -> initialize(v1)
  -> authoritative snapshot + cursor
  -> install the presentation projection
  -> subscribe_events(after cursor)
  -> interactive
```

## Owners

| Module | Owns | Does not own |
| --- | --- | --- |
| `runtime/child-process.ts` | spawn, stdio, bounded stderr tail, stdin close, wait, fallback termination | anything semantic; it never reads stdout |
| `protocol/jsonl.ts` | LF framing, CRLF, the 8 MiB bound in encoded bytes | protocol meaning |
| `runtime/connection.ts` | request ids, the pending RPC map, correlation, event delivery, ordered writes, terminal settlement | conversation state |
| `runtime/session.ts` | attach, snapshot install, subscribe, resync repair, shutdown | agent semantics |
| `presentation/projection.ts` | the ephemeral render cache | canonical history, authority of any kind |
| `presentation/tools.ts` | the `ToolCallId` correlation used for display | tool lifecycle, which it only reads |
| `commands/` | slash-command parsing, dispatch to canonical operations | parallel runtime semantics |
| `ui/components/` | the semantic presentation grammar | every fact it displays |
| `ui/preferences.ts` | reasoning visibility and expanded cards | anything the runtime owns |
| `ui/` | Pi components and rendering | every fact it displays |

## Commands

`/help` `/model` `/tools` `/skills` `/status` `/debug` `/reasoning` `/expand`
`/cancel` `/approve` `/quit`

Each either renders projection state, changes a client display preference, or
invokes exactly one canonical Runtime Client operation. `/model` opens the
searchable selector over `model_catalog_get` and applies a choice through
`model_set`, while `/model show` renders the projection's own model view;
`/tools` and `/skills` read the capability projection; `/status` prints the
runtime's own Agent Status rendering; `/debug` shows bounded diagnostics and
never a credential; `/approve` sends a finite typed response to one
runtime-owned Approval interaction. The TUI never edits the displayed tool
arguments or keeps a local approval outcome.

`/reasoning [on|off]` and `/expand [<tool-call-id>|all|none]` are the two
commands that touch nothing but the screen. They send no request, and they are
also bound to keys:

| Key | Effect |
| --- | --- |
| `ctrl+c` | cancellation intent for the active attempt, or quit when idle |
| `ctrl+o` | expand or collapse the most recent tool card |
| `ctrl+t` | show or hide model reasoning |

There is deliberately **no** `!bash`, no `@file` attachment, no client-side
file read, and no client-side Skill execution. Shell, file, and Skill behaviour
must travel through the real rustX tool and capability path, and rustX has not
yet defined a client-facing attachment contract.

## Native approvals

Pending approvals arrive as runtime-owned `interaction_pending` events and in
the authoritative `snapshot.pending_interactions` view. The presentation
reducer sorts and replaces those facts like every other projection value;
reconnect/resync discards local assumptions and rebuilds the list from the
snapshot. The renderer shows the immutable tool identity, mode, reason, and
validated arguments. `/approve <interaction-id> allow` or
`/approve <interaction-id> deny [reason]` sends only the typed Runtime Client
`interaction_respond` request. It never invokes a tool, mutates arguments,
infers an outcome from detach/EOF, or callbacks into the Agent Loop.

## The model selector

`/model` opens a searchable overlay over the catalog `model_catalog_get`
published. It fuzzy-filters on the model reference, marks the configured model
as current, and shows the highlighted entry's effective capability, context
window, output limit, and reasoning profiles — each exactly as the catalog
published it. A reasoning-capable model that declares no profiles is shown as
exactly that: there is no universal off/low/medium/high, and inventing one
would make this client a second model-configuration authority. The overlay also
states the configured/effective pair and, while an attempt is running, that the
running attempt keeps the model it froze.

## The model invariant

`snapshot.model` is the session's *desired* configuration.
`snapshot.attempt.model` and the `attempt_started` event carry the *immutable*
model an already-admitted attempt froze. While an attempt runs on A and the
session is switched to B, the UI shows both truthfully and the running attempt
never visually mutates to B. The next admission shows B.

This is proven in `test/model-invariant.test.ts` through the pure reducer and
end to end over the transport, and again against the real binary in
`test/integration.test.ts`.

## The semantic presentation model

The transcript is a grammar of semantic components, not a log of protocol
records:

```text
user            ▌ the question, verbatim
assistant text  ordinary Markdown, no banner
reasoning       dimmed, or one `Thinking…` marker when hidden
refusal         explicitly a refusal, never an answer
tool_call       one correlated tool card
tool result     folded into that card, not repeated
```

Canonical block order is preserved exactly: `reasoning, text, tool_call, text`
renders in that sequence, streaming and committed alike, and streaming text
renders identically to the committed text so nothing reflows on commit.

### One tool call is one visual entity

rustX publishes three different facts about one logical call:

```text
assistant tool_call block   committed conversation content
foreground execution        attempt-scoped execution lifecycle
tool result message         committed conversation content
```

Their semantic ownership stays separate. `presentation/tools.ts` joins them for
*display*, keyed by the runtime's own `ToolCallId` — never by tool name,
argument equality, list position, timing, or adjacency, so two concurrent calls
of the same tool with the same arguments stay two cards. The card renders at
the assistant block that asked for the call and evolves in place:

```text
◇ Bash · preparing        ->  ◐ Bash · running · 40/900  ->  ✓ Bash · ok · 2.8s · exit 0
```

### Renderers may format, never decide

> **Tool identity may select a presentation renderer.
> Tool identity may never select or infer execution semantics.**

A stable `ToolId` picks a specialized renderer — Bash, Read, Grep, Glob, Edit,
Write — so a shell call reads as `$ cargo test --all` instead of argument JSON.
A renderer formats already-authoritative facts and is never handed the
lifecycle, so it cannot express an opinion about it: running, success, failure,
denial, cancellation, timeout, interruption, progress, duration, exit code, and
truncation all come from the Runtime Client. Nothing reads a status out of
output text, infers running from an absent result, or infers cancellation from
missing output. A renderer that does not recognise a shape returns nothing and
the generic renderer takes over, so unknown, MCP, and Python tools stay fully
usable.

### Visual collapse is not runtime truncation

A collapsed card shows a bounded preview and says how many lines are hidden.
Expanding re-renders facts the client already holds: no re-execution, no
filesystem access, no network. The runtime's own `TruncationState` is a
different fact, reported separately and always — expanding never undoes it.

### Reasoning visibility is not reasoning configuration

The TUI consumes only canonical Runtime Client reasoning blocks. Provider
spellings such as `reasoning` and `reasoning_content` never enter the
TypeScript protocol or presentation layer. Whether reasoning is *drawn* is a
client preference (`/reasoning`, `ctrl+t`); when hidden it collapses to a
`Thinking…` marker rather than becoming assistant text. What rustX *asks a
provider for* is `SessionModelConfig.reasoningProfile` / `reasoningEnabled`,
which only `model_set` changes.

### Working status is proven, never timed

The spinner names a phase only when a projection fact proves it — a pending
interaction, a running or assembled foreground execution, the kind of the
latest streamed block, the attempt phase. There is no timer and no inactivity
threshold, and no state is invented for a lifecycle rustX does not publish.

## Testing

```sh
pnpm --dir tui typecheck
pnpm --dir tui test
```

Everything is deterministic: scripted byte and record sequences, a data
barrier rather than a delay for readiness, and no `setTimeout` used to
establish an ordering. The presentation suites drive projection facts directly
and assert on normalized strings — `transcript.test.ts`,
`tool-correlation.test.ts`, `tool-card.test.ts`, `model-selector.test.ts`,
`status.test.ts`, and `reconstruction.test.ts`, which rebuilds the whole
visible UI from one fresh snapshot.

`test/integration.test.ts` drives the **real** `rustx` binary over the real
stdio/JSONL transport against a local SSE provider fixture (no credentials, no
network). It skips itself with a clear reason when `target/debug/rustx` has not
been built; set `RUSTX_BINARY` to point elsewhere.

## Licensing

See [`THIRD_PARTY_NOTICES.md`](./THIRD_PARTY_NOTICES.md).
`@earendil-works/pi-tui@0.82.1` is consumed as a published MIT dependency; no
Pi source is copied into this repository.

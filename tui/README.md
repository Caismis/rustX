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

Yes. Pi is imported by exactly two files: `src/ui/app.ts`, and
`src/commands/autocomplete.ts` for the `AutocompleteProvider` interface it
implements. Everything below that is plain TypeScript over protocol values.

Eight of the nine test suites — framing, RPC, presentation projection, session
lifecycle, the model invariant, rendering, the process owner, and the real
`rustx` integration — do not reach `@earendil-works/pi-tui` at all, directly or
transitively. The ninth (`commands.test.ts`) touches it only through the
autocomplete interface and `fuzzyFilter`. No suite needs a terminal.

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

pnpm --dir tui start -- \
  --binary "$PWD/target/debug/rustx" \
  --models  "$PWD/models.json" \
  --session "$PWD/session.json" \
  --workspace "$PWD/workspace" \
  --runtime-root "$PWD/.rustx"
```

The four runtime paths are passed straight through. **The client never opens,
parses, or interprets any of them** — `models.json` is a runtime-owned model
authority, and reading it here would create a second one. Provider credentials
are resolved by the Rust process from the environment it inherits.

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
| `commands/` | slash-command parsing, dispatch to canonical operations | parallel runtime semantics |
| `ui/` | Pi components and rendering | every fact it displays |

## Commands

`/help` `/model` `/tools` `/skills` `/status` `/debug` `/cancel` `/quit`

Each either renders projection state or invokes exactly one canonical Runtime
Client operation. `/model` goes through `model_catalog_get` and `model_set`;
`/tools` and `/skills` read the capability projection; `/status` prints the
runtime's own Agent Status rendering; `/debug` shows bounded diagnostics and
never a credential.

There is deliberately **no** `!bash`, no `@file` attachment, no client-side
file read, and no client-side Skill execution. Shell, file, and Skill behaviour
must travel through the real rustX tool and capability path, and rustX has not
yet defined a client-facing attachment contract.

## The model invariant

`snapshot.model` is the session's *desired* configuration.
`snapshot.attempt.model` and the `attempt_started` event carry the *immutable*
model an already-admitted attempt froze. While an attempt runs on A and the
session is switched to B, the UI shows both truthfully and the running attempt
never visually mutates to B. The next admission shows B.

This is proven in `test/model-invariant.test.ts` through the pure reducer and
end to end over the transport, and again against the real binary in
`test/integration.test.ts`.

## Testing

```sh
pnpm --dir tui typecheck
pnpm --dir tui test
```

Everything is deterministic: scripted byte and record sequences, a data
barrier rather than a delay for readiness, and no `setTimeout` used to
establish an ordering.

`test/integration.test.ts` drives the **real** `rustx` binary over the real
stdio/JSONL transport against a local SSE provider fixture (no credentials, no
network). It skips itself with a clear reason when `target/debug/rustx` has not
been built; set `RUSTX_BINARY` to point elsewhere.

## Licensing

See [`THIRD_PARTY_NOTICES.md`](./THIRD_PARTY_NOTICES.md).
`@earendil-works/pi-tui@0.82.1` is consumed as a published MIT dependency; no
Pi source is copied into this repository.

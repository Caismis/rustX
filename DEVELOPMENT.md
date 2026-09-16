# Local development

Use Node 24+, the pinned pnpm (Corepack), and the repository's Rust toolchain.
The native runtime currently supports Unix (Linux/macOS; use WSL on Windows).
From the repository root:

```sh
cargo build --bins
pnpm --dir dev install --frozen-lockfile
pnpm --dir tui install --frozen-lockfile
pnpm --dir web-console install --frozen-lockfile
```

Configure rustX through its native configuration commands and
[launch contract](docs/launch-configuration.md). The launcher never reads user
settings or provider/MCP credentials, grants project trust, or merges configuration.
Build explicitly after native changes; the launcher never builds implicitly.
`--binary /absolute/path/rustx` selects another executable. Missing binaries produce
an actionable error. Native paths/values pass through unchanged; relative native
paths resolve from the command's working directory (pnpm runs scripts in `dev/`).
Use absolute paths for settings, runtime roots, and Workspaces.

## Canonical commands

**App Server** — real native process, with source bindings passed through:

```sh
pnpm --dir dev app-server -- \
  --user-settings /absolute/path/settings.toml \
  --runtime-root /absolute/path/runtime
```

This defaults to `--listen stdio`, with owned pipes and unmodified protocol stdout.
It stays in the foreground until stdin closes or the owner stops it. For an external
WebSocket client, pass native `--listen ws://127.0.0.1:8080 --token-file /private/token`.
The native parser validates these arguments and the native exit status propagates.
For headless/integration use without pnpm, use `target/debug/rustx app-server`
with an explicit native transport; see [App Server](docs/app-server-protocol.md).

**TUI** — existing TUI composition root and `AppServerHost.spawnLocal`:

```sh
pnpm --dir dev tui -- \
  --user-settings /absolute/path/settings.toml \
  --workspace /absolute/path/workspace
```

TUI arguments pass through to its own parser. `--workspace` supplies the Session's
initial cwd through `session/create`; native `SessionPersistentState.cwd` remains
its durable authority. `--runtime-root`, `--models`, `--resume`, and other documented
[TUI options](tui/README.md) remain available. The TUI owns and reaps its stdio
App Server child. Direct `pnpm --dir tui start --connect ...` serves the distinct
case of attaching to an externally owned App Server.

**Web** — complete normal local composition:

```sh
pnpm --dir dev web -- \
  --user-settings /absolute/path/settings.toml \
  --runtime-root /absolute/path/runtime \
  --workspace /absolute/path/workspace \
  --workspace /absolute/path/another-workspace
```

At least one explicit absolute Workspace root is required. The launcher starts the
real App Server on `ws://127.0.0.1:0`, waits for its bound-listener announcement,
and starts the normal Vite carrier with one ephemeral Product Host configuration.
Vite selects its own free loopback port and reports readiness through IPC only
after `listen()` completes. There are no readiness sleeps or port reservations.
The summary prints the browser URL, native endpoint, token **file path**, and exact
Workspace roots. Open the URL, enter that endpoint and the contents of the token
file in **Connect**. The transport token stays in page memory; it is neither logged
nor persisted in browser storage. Never enter a provider key in this screen.

The Product Host retains all Workspace navigation/authorization decisions. Only
explicit exact roots are authorized; descendants are not implicitly admitted.
Native project trust remains separate. Dev Host names/order/registrations are
intentionally ephemeral for each composition. Persistent operator-owned Host
metadata belongs to a separately managed Host, not launcher scratch.

## Ownership and shutdown

`dev/src/launcher.ts` is the sole development composition owner. It owns direct
native/TUI/carrier processes, sequencing, readiness, and its `rustx-dev-*` scratch
root. `web-console/scripts/dev-carrier.ts` adapts Vite's listening/close APIs to
owner IPC; it never starts an App Server or supplies a second composition.

The first terminal cause synchronously fences every subsequent spawn and scratch
allocation. SIGINT, SIGTERM, startup failures and unexpected child exits converge
on the same settlement promise. Cleanup stops and waits for all children before
removing scratch, then reports one exit status. Direct executables avoid package
manager grandchildren. Each child gets an owned process group; graceful native
shutdown or carrier/TUI IPC comes first, followed by bounded escalation for a stuck
group. Native runtime/tool settlement remains native-owned.

The process owner also observes process-group disappearance after escalation;
delivering a kill signal alone is not treated as settlement.

Scratch contains one random transport token (0600), one Host config (0600), and the
Host's ephemeral registration metadata, beneath a private temporary directory
(0700). User settings, runtime roots, Workspace files and persistent Host metadata
are never launcher scratch and are never removed. Launcher SIGINT/SIGTERM return 130/143;
the TUI retains its own raw-key Ctrl+C behavior and ordinary clean exit status.
A native standalone/TUI exit preserves its status. A Web child exiting unexpectedly,
even successfully, ends the composition with a failure status.

## Focused component work and acceptance fixtures

- `pnpm --dir web-console dev` and `preview` serve only the Web component, for
  focused presentation work or attachment to an independently managed Host/runtime.
  They do not compose a local runtime. Normal local work uses `pnpm --dir dev web`.
- [Strict Web dogfooding](web-console/DOGFOODING.md) and `pnpm --dir web-console
  test:e2e` are acceptance fixtures. Their fake providers, scripted scenarios,
  trust setup and test Workspaces remain fixture-only. The normal launcher imports
  none of these. Both reuse only the bounded native listener-announcement parser;
  fixture lifecycle and provider assertions stay fixture-owned. The browser suite also exercises the real development launcher
  without fake providers or a Workspace Host proxy.
- `protocol/app-server` owns protocol generation/checking only. Launchers do not
  change protocol semantics or generated files.

## Validation

```sh
pnpm --dir dev typecheck
pnpm --dir dev test
pnpm --dir tui typecheck
pnpm --dir tui test
pnpm --dir web-console typecheck
pnpm --dir web-console test
pnpm --dir web-console build
pnpm --dir web-console exec playwright test dev-launcher.spec.ts
```

The dev package runs directly under Node type stripping and has no build artifact.
Launcher tests use controlled readiness/reap promises and real process handshakes;
they prove terminal races without sleep-based coordination. See
[CI](.github/workflows/ci.yml) and [native test guide](tests/README.md) for the full
Rust, provider-emulator, browser acceptance, formatting and Clippy gates.

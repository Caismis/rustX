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
configuration or provider/MCP credentials, or merges configuration.
Build explicitly after native changes; the launcher never builds implicitly.
`--binary /absolute/path/rustx` selects another executable. Missing binaries produce
an actionable error. Native paths/values pass through unchanged; native parsers own validation.
User configuration and runtime-root bindings require absolute paths. Use an
absolute Workspace path too (pnpm runs scripts in `dev/`).
`--config` rebinds only the User document; User resources stay at `~/rustx/.agents`.

## Canonical commands

**App Server** — real native process, with source bindings passed through:

```sh
pnpm --dir dev app-server -- \
  --config /absolute/path/rustx.toml \
  --runtime-root /absolute/path/runtime
```

This defaults to `--listen stdio`, with owned pipes and unmodified protocol stdout.
Launcher stdin EOF is an owner shutdown request: the launcher explicitly requests
native shutdown and waits for settlement (exit 0 if EOF wins). For an external
WebSocket client, pass native `--listen ws://127.0.0.1:8080 --token-file /private/token`.
In explicit WebSocket mode, stdin is not the ownership lifetime; process signals
and child exit govern launcher lifetime. Exact `app-server -- --help` delegates
to native help without adding transport arguments. The native parser validates
these arguments and the native exit status propagates.
For headless/integration use without pnpm, use `target/debug/rustx app-server`
with an explicit native transport; see [App Server](docs/app-server-protocol.md).

**TUI** — existing TUI composition root and `AppServerHost.spawnLocal`:

```sh
pnpm --dir dev tui -- \
  --config /absolute/path/rustx.toml \
  --workspace /absolute/path/workspace
```

TUI arguments pass through to its own parser. `--workspace` supplies the Session's
initial cwd through `session/create`; native `SessionPersistentState.cwd` remains
its durable authority. `--runtime-root`, `--model`, `--resume`, and other documented
[TUI options](tui/README.md) remain available. The TUI owns and reaps its stdio
App Server child. Direct `pnpm --dir tui start --connect ...` serves the distinct
case of attaching to an externally owned App Server.

**Web** — complete normal local composition:

```sh
pnpm --dir dev web -- \
  --config /absolute/path/rustx.toml \
  --runtime-root /absolute/path/runtime \
  --workspace /absolute/path/workspace \
  --workspace /absolute/path/another-workspace
```

At least one explicit absolute Workspace root is required. The launcher starts the
real App Server on `ws://127.0.0.1:0`, waits for its bound-listener announcement,
and starts the normal Vite carrier with one ephemeral Product Host configuration.
Vite selects its own free loopback port and reports readiness through IPC only
after `listen()` completes. There are no readiness sleeps or port reservations.
The launcher prints one authenticated startup URL and opens the default browser.
The browser exchanges its launch credential for an HttpOnly session, redirects to
clean `/`, and automatically connects to the exact native App Server. Reload
repeats authenticated bootstrap without credential entry. Use `--no-open` to print
the URL without opening a browser; SSH launches also suppress automatic opening.
Browser opener failure leaves the composition running and points to the printed URL.
The opener receives only a bounded desktop environment, never provider/MCP credentials.
It observes OS handoff, not browser lifetime. Only Windows waits for the short-lived
PowerShell launcher; Linux/macOS/WSL return after `open()` accepts the URL.
App Server credentials never appear in the URL or browser persistent storage.

For the minimal local launch, use:

```sh
pnpm --dir dev web -- --workspace /absolute/path/to/project
```

Externally managed App Servers use **Settings → Connection → Remote App Server**.
Select Remote explicitly and enter its endpoint and transport token there. Failed
local startup never chooses Remote; failed Remote never chooses Local. Remote
attachment grants no Product Host filesystem authority. See [connection security](web-console/CONNECTION.md).

The Product Host retains all Workspace navigation/authorization decisions. Only
explicit exact roots are authorized; descendants are not implicitly admitted.
Native configuration resolution remains separate from Host navigation authorization. Dev Host names/order/registrations are
intentionally ephemeral for each composition. Persistent operator-owned Host
metadata belongs to a separately managed Host, not launcher scratch.

## Ownership and shutdown

`dev/src/launcher.ts` is the sole development composition owner. It owns direct
native/TUI/carrier processes, sequencing, readiness, and its `rustx-dev-*` scratch
root. `web-console/scripts/dev-carrier.ts` adapts Vite's listening/close APIs to
owner IPC; it never starts an App Server or supplies a second composition.

The first terminal cause synchronously fences every subsequent spawn and scratch
allocation. SIGINT, SIGHUP, SIGTERM, stdio owner EOF, startup failures and unexpected
child exits converge on the same settlement promise. Cleanup stops and waits for all children before
removing scratch, then reports one exit status. Direct executables avoid package
manager grandchildren. Each child gets an owned process group; graceful native
shutdown or carrier/TUI IPC comes first, followed by bounded escalation for a stuck
group. Native runtime/tool settlement remains native-owned.

The process owner also observes process-group disappearance after escalation;
delivering a kill signal alone is not treated as settlement.

Scratch contains one random transport token (0600), one Host config (0600), a private
Web bootstrap config (0600, referencing the existing transport-token file), and the
Host's ephemeral registration metadata, beneath a private temporary directory
(0700). User configuration, runtime roots, Workspace files and persistent Host metadata
are never launcher scratch and are never removed. Launcher SIGINT/SIGHUP/SIGTERM
return 130/129/143;
the TUI retains its own raw-key Ctrl+C behavior and ordinary clean exit status.
A native standalone/TUI exit preserves its status. A Web child exiting unexpectedly,
even successfully, ends the composition with a failure status.

## Focused component work and acceptance fixtures

- `pnpm --dir web-console dev` and `preview` serve only the Web component, for
  focused presentation work or attachment to an independently managed Host/runtime.
  They do not compose a local runtime. Normal local work uses `pnpm --dir dev web`.
- [Strict Web dogfooding](web-console/DOGFOODING.md) and `pnpm --dir web-console
  test:e2e` are acceptance fixtures. Their fake providers, scripted scenarios,
  isolated CFG3 sources and test Workspaces remain fixture-only. The normal launcher imports
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

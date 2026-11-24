# Installing rustX

Build natively on Linux or macOS. Use the Linux build under WSL. The
archives target the OS and architecture named in their filenames. Builds
use the locked Rust toolchain and dependencies. The single-machine bundle
also needs Corepack, Node.js 24, and the native build tools for its Web
carrier dependencies. The packager invokes the pinned pnpm release. See [DEVELOPMENT.md](../DEVELOPMENT.md).

```sh
python3 release/package.py rustx-server --out /absolute/path/to/packages
python3 release/package.py rustx-client --out /absolute/path/to/packages
python3 release/package.py rustx --out /absolute/path/to/packages
```

`all` builds the three layouts in one command. The Server and Client use
separate Cargo target directories. Each archive has a sibling SHA-256 file
and an internal `manifest.json` with its source commit, Execution Protocol
revision and installed file hashes, modes and links. Verify the checksum
and extract into a directory you control. Keep the installation directory
intact when adding its `bin` to `PATH` or creating a symlink into it.

| Archive | Entry point | Other executables |
| --- | --- | --- |
| `rustx-server` | `bin/rustx-server` | `bin/workflow-renderer` |
| `rustx-client` | `bin/rustx-client` | `bin/bash-supervisor`, `bin/interactive-supervisor` |
| `rustx` | `bin/rustx` | All of the above companions, with the local Client named `bin/rustx-execution-client` |

The Server-only layout can serve committed history, Server-side tools and
model work before an Execution Client is present. Its `app-server --help`
lists transport and bootstrap options. Starting an independent Client
requires an enrollment issued by the Server for the corresponding Session.
The Client owns Workspace filesystem access, supervisors, stdio MCP and
Managed Python. Python/uv requirements belong to the Client workflows
that use them. The Client has neither Provider configuration nor a
canonical Session store.

## Standalone Server and Client

Configure `app_server.environment_roots` for the authorized absolute
Workspace paths before enrolling. For a loopback remote example, run the
Server as follows, using a private directory for the bootstrap file.

```sh
/path/to/rustx-server/bin/rustx-server app-server --listen stdio \
  --execution-workspace /absolute/workspace \
  --execution-ws-listen 127.0.0.1:0 \
  --execution-bootstrap-out /private/directory/execution.json
```

Use the private bootstrap with the separately installed Client.

```sh
/path/to/rustx-client/bin/rustx-client --binding-file /private/directory/execution.json \
  --workspace /absolute/workspace
```

See `rustx-client --help` and the [Execution Protocol](execution-protocol.md)
for the transport, enrollment, and recovery contracts. Keep the bootstrap
private when transferring it to the Client machine. A stale or incompatible
bootstrap requires operator attention. Use the existing Session identity
when recovering an enrolled Session.

## Single-machine bundle

```sh
/path/to/rustx/bin/rustx web --workspace /absolute/workspace \
  --local-execution-workspace /absolute/workspace
/path/to/rustx/bin/rustx tui --session "$SESSION_ID" \
  --local-execution-workspace /absolute/workspace
```

The explicit local Workspace option enrolls and launches a trusted sibling
Client through the paired Unix protocol. It creates a durable minimal
Session before dependent configuration is loaded. Configure the environment
root first. The installed Web catalog lists explicitly configured locations.
The TUI example attaches to the specified enrolled Session. The Web launcher
serves the built assets together with the existing local Workspace Host. Web
and TUI need `node` on `PATH`. Run
`rustx app-server --help` for direct, headless startup with these same
installed binaries. On Linux, local recovery checks also require `bwrap`.

The Server and single-machine archives include the existing third-party
App Server example at `share/examples/app-server/connect.mjs`. With Node.js
24, run it with the App Server WebSocket URL and the protected App Server
token file. It uses the public control protocol independently of the
Execution Client connection.

Normal shutdown fences execution and reaps the owned sibling. Local
diagnostics distinguish an unconfirmed execution domain from a cleanup
receipt. Existing committed history remains available if physical settlement
is unconfirmed.
The protocol's same-instance recovery and physical-settlement rules govern
any subsequent execution admission.

## Verifying extracted packages

Run `release/conformance.py` with Python 3.12 or later and one archive of
each layout. It checks the archive checksums, file inventory, version agreement,
standalone help with a restricted `PATH`, and installed Web and TUI entry points.
The Web check starts an enrolled local Client and exercises the built assets and
loopback browser authentication from a temporary directory outside the checkout.
It runs the packaged third-party example and then starts the TUI in a PTY
to check that it can render the Session catalog and disconnect independently
of the Web owner.
The local Fake Provider fixtures require no paid credentials.

```sh
python3 release/conformance.py /absolute/path/to/packages/*.tar.gz
```

To replay the real-process checks against the extracted executables, prepare
and pass the current `process` test harness. On Linux, add `--mount-proof` to
include the Client-only filesystem and Managed Python cases. Use the native
platform dependencies described in [the test index](../tests/README.md).

```sh
PYTHONPATH=test-support python3 - <<'PYCODE'
from prepare_native_tests import prepare
_, harnesses = prepare(['prepare_native_tests.py', '--', 'cargo', 'test', '--locked',
                        '--all-features', '--test', 'process', '--no-run'])
print(harnesses[0])
PYCODE
python3 release/conformance.py /absolute/path/to/packages/*.tar.gz \
  --process-harness /absolute/path/to/process-test-executable --mount-proof
```

See the [installed-product evidence](exec-04-evidence.md) for tested boundaries
and outstanding acceptance gates. The paired Linux I/O probe takes the unpacked Client and reports selected
bytes, bounded framed traffic, RPC counts, memory high-water marks, and
startup, scan, and settlement times.

```sh
python3 test-support/measure_execution_reads.py /path/to/rustx-client/bin/rustx-client
```

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
separate Cargo target directories. Use paths outside the checkout or Git-ignored
directories without tracked files for `--out`, `--server-target`, and
`--client-target`. The bundle builds its Web and TUI assets
in a temporary source tree, so it can run alongside a live checkout preview.
Each archive has a sibling SHA-256 file
and an internal `manifest.json` with its source commit, Execution Protocol
revision and installed file hashes, modes and links. Verify the checksum
and extract into a directory you control. Keep the installation directory
intact when adding its `bin` to `PATH` or creating a symlink into it.

| Archive | Entry point | Other executables |
| --- | --- | --- |
| `rustx-server` | `bin/rustx-server` | `bin/workflow-renderer` |
| `rustx-client` | `bin/rustx-client` | `bin/bash-supervisor`, `bin/interactive-supervisor`, `bin/python-guard/` |
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
/path/to/rustx/bin/rustx web --workspace /absolute/workspace
/path/to/rustx/bin/rustx tui --workspace /absolute/workspace
```

Each new Session in the installed Web or self-hosted TUI starts a sibling
Client over the same paired Unix Execution Protocol. The Server reserves a
Session identity, authenticates the Client, and reads the bound configuration
before admitting the Session. A clean restart or a fork uses the same
admission when requested as a live Session. When restoring a Session, the
load owns the new Client until the runtime is admitted. A failed load
reaps that Client and releases its slot before reporting the result. When
settlement is unconfirmed, the recovery fence stays in place.
Concurrent requests for different conversations in that Session share the
first admission outcome and the Session still allows one resident
conversation at a time. A failed conversation load leaves a previously
admitted Client available to the Session.

Cold history and Session management reads do not start a Client. A durable
physical fence must settle before the same Session identity can be admitted
again. The launcher grants each exact Workspace passed at startup. Deleting
the Session or draining its Server revokes execution and settles the owned
Client. If all eight local Session Client slots are active, new Sessions
receive an explicit capacity error. The standalone Server can create
ordinary Sessions without launching Clients.

Web can re-register a configured exact root after it is removed from the
navigation. Browsing and registering descendants additionally require an
`app_server.environment_roots` grant. The picker uses a private minimal
binding, verifies the selected directory revision, and settles its temporary
Client before returning. The picker starts no Agent or Conversation. Home
Skills uses the same private Client capacity for read-only catalog and
instruction requests and reports Client authority.

Before a Session is selected, an authorized local Workspace can also use a
private Client for MCP Test Connection, the MCP tool catalog, and Workflow
Tools discovery. Tool discovery can start configured stdio MCP processes
and prepare and run Managed Python packages. These requests require the
Workspace to have no pending execution recovery record. An explicitly
selected Session uses only that Session's binding.

Each Server allows one private Workspace Client at a time across these
requests and the picker. Catalog requests already admitted by the Server wait
in order for that slot. Their handshake and listing deadlines start after
admission to the slot. Grant revocation or Server shutdown cancels the wait.
The picker reports busy immediately if the slot is occupied.

Web and TUI need Node.js 24 on `PATH`. Run `rustx app-server --help` for
headless startup with the installed binaries. On Linux, local recovery
checks also require `bwrap`.

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
The verifier checks the built Web assets and loopback browser authentication
from a temporary directory outside the checkout. It runs the packaged
third-party example and exercises both the connected TUI and the self-hosted
TUI's new Session and tools in a PTY. For the installed browser regression,
install Chromium with `pnpm --dir web-console exec playwright install chromium`
and pass `--browser-proof`. The Chromium run creates and deletes real Sessions,
verifies the two Client processes settle, and checks root registration with
and without native browse authorization.
The local Fake Provider fixtures require no paid credentials.

```sh
python3 release/conformance.py /absolute/path/to/packages/*.tar.gz
```

To replay the real-process checks against the extracted executables, prepare
and pass the current `process` test harness. On Linux, add `--mount-proof` to
include the Client-only filesystem and Managed Python cases. Pass the
`rustx-execution-client` package's `bootstrap` test executable with
`--client-harness` to exercise same-instance reattachment against the
installed Client. Use the native
platform dependencies described in [the test index](../tests/README.md).

```sh
PYTHONPATH=test-support python3 - <<'PYCODE'
from prepare_native_tests import prepare
_, harnesses = prepare(['prepare_native_tests.py', '--', 'cargo', 'test', '--locked',
                        '--all-features', '--test', 'process', '--no-run'])
print(harnesses[0])
PYCODE
python3 release/conformance.py /absolute/path/to/packages/*.tar.gz \
  --browser-proof --process-harness /absolute/path/to/process-test-executable --mount-proof
```

See the [installed-product evidence](exec-04-evidence.md) for tested boundaries
and outstanding acceptance gates. The paired Linux I/O probe takes the unpacked Client and reports selected
bytes, bounded framed traffic, RPC counts, memory high-water marks, and
startup, scan, and settlement times.

```sh
python3 test-support/measure_execution_reads.py /path/to/rustx-client/bin/rustx-client
```


The macOS packager builds the optimized Server with 16 Cargo codegen units
for the `rustx` package. The partitioning bounds the native runner's single
LLVM-unit workload during the release build.

For the native macOS release check, run the same packager and verifier through
`python3 release/verify_macos.py --out /absolute/path/to/packages`. The
local run needs the Fake Provider fixture initialized with
`uv sync --frozen` in `test-support/fake-provider`. The verifier
checks strict Clippy, compiles the native lib test target, builds the
three archives, and runs the installed process and Client reattachment
cases. On GitHub, dispatch
`CI` with `installed_macos=true` for the desired release branch. This job
runs only by explicit dispatch and has its own result and installation
manifests.

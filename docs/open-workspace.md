# Open workspace on the Host desktop

The current Session header provides **Open workspace** and an application menu.
The primary action uses the first available application in this fixed order:
file manager, terminal, Visual Studio Code. The menu offers only discovered apps.
No Agent starts and no model or Tool request is issued by this action. Drafts,
history and running work are unchanged.

The desktop belongs to the **Product Host**, which may differ from the computer
running your browser. The local development launcher provisions
`nativeFilesystem: "shared"` because it launches the App Server and Host in the
same filesystem namespace. Independently managed Hosts must explicitly attest
that mapping in their operator-owned Host configuration and configure the native
transport connection. Do not set it for an App Server in a remote/container
namespace. Matching path strings alone do not establish a mapping. No access is
granted by this action, and no browser-machine fallback exists.

Supported discovery catalog:

| Host | File manager | Terminal | Editor |
| --- | --- | --- | --- |
| Linux desktop | `xdg-open` on PATH | `gnome-terminal` on PATH | `code` on PATH |
| macOS | `/usr/bin/open` (Finder) | system Terminal bundle via `open -a` | standard `/Applications/Visual Studio Code.app` CLI |
| Windows | `explorer.exe` under SystemRoot | — | — |

SSH sessions, Linux without DISPLAY/WAYLAND_DISPLAY, Windows service sessions,
unsupported platforms and missing mapping report unavailable. These checks and
verified executables establish eligibility, not proof of a usable GUI. Install a
missing application and restart the Host to rebuild its lifetime catalog. Checking
applications again is explicit; failures never trigger automatic discovery loops
or replay a launch. Discovery executes no subprocesses, visits at most 32 absolute
PATH directories for a closed set of names, and does not recursively scan disks.
Cached executable paths are reverified before every launch and removed when gone.

“Launcher started” means the operating system acknowledged spawning the launcher;
it does not assert that the external application displayed a window or that a
later launcher exit succeeded. Spawn failures and nonzero exits observed within the fixed one-second launch
window are reported in launch details. The window bounds observation only; it
never kills a running application.
After a lost HTTP acknowledgement, check the Host desktop before opening again.
Navigation, disconnect and closing the menu never kill a handed-off application.

## Ownership and admission

`ProductHostWorkspaces` exposes `desktopCatalog(scope)` and
`openWorkspace(scope, { session_id, active_node }, application)`. The browser
supplies identities, never a path, command or argument array. Existing carrier
browser authentication and Host/Origin protection cover both JSON POST routes.
Malformed/unbounded requests fail before discovery, resolution or launch.

`LocalWorkspaceHost` verifies its authority ID and endpoint, bounds outstanding
launches to one, resolves a discovered app, and calls existing native
`session/summary`. That read is the **native Session admission point**: the
existing Session catalog rejects unknown/deleting Sessions and the Host checks
the exact active-node identity. It reads the canonical, immutable Session cwd
already projected by the runtime. It neither attaches nor activates a Session.
A retirement that wins this read rejects the operation. A retirement after this
read cannot retract the admitted desktop intent; this is a snapshot admission,
not an atomic transaction spanning the native process and OS desktop.

After the read, the Host revalidates its scope and uses the same exact-root
classification as existing Workspace admission. Registration removal does not
revoke configured root authorization. Missing roots or changed canonical
resolution reject the operation. The final Host check and the adapter's verified
argument-array spawn have **no intervening await**. Host closure that wins that
check has zero launch effects; closure after spawn does not kill the application.
Filesystem mutations by other processes remain outside that JavaScript boundary.

`host/desktop.ts` owns platform discovery, safe argv and a small injectable process
boundary. Only an allowlist of desktop environment variables reaches the child;
provider credentials, transport tokens and NODE_OPTIONS are excluded. The child
is detached with ignored stdio and unreferenced after spawn. There are no discovery
children to clean up; native metadata connections have handshake/request bounds
and close after every operation. No desktop API enters Rust, and no new Session
or Workspace owner, storage, protocol schema, feature flag or compatibility path
is introduced.

Deterministic tests gate native reads, catalog responses and launch responses;
they assert zero launch before admission, exact target/path/argv, authority
replacement, missing paths/apps, errors, filtered environment and no replay.
Mocked platform boundaries do not constitute actual macOS/Windows GUI smoke tests.

# Open workspace on the Host desktop

The current Session header provides **Open workspace** as a compact split button:
the folder icon opens the workspace and the chevron opens the application menu.
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

SSH sessions and Linux without DISPLAY/WAYLAND_DISPLAY report unavailable.
On macOS, the Host must run as the non-root owner of `/dev/console` **and** its
current launchd bootstrap must report `Aqua`. The adapter reads console ownership
and invokes only `/bin/launchctl managername` (no shell, empty environment, 1 KiB
output bound, one-second execution bound). Missing/inaccessible console state,
root, another console user, non-Aqua context or a failed query reports `headless`.
This deliberately excludes background/remote Hosts even when `/usr/bin/open`
exists. It can reject working custom launch arrangements; run the Host from the
logged-in desktop session. The checks establish desktop-session eligibility, not
proof that a particular GUI application will display a window. The launchd rule
uses Apple's documented [current bootstrap manager name](https://github.com/apple-oss-distributions/launchd/blob/main/man/launchctl.1).
Unsupported platforms (including Windows) and missing mapping report unavailable.

Ordinary primary/menu reads reuse the application catalog and reverify retained
executables. **Check applications again**, in the menu or unavailable/failure
feedback, invalidates that cache and performs exactly one bounded rediscovery.
Newly installed or recovered applications can therefore appear without restarting
the Host. Refresh never launches an application. Application discovery executes no
subprocesses, visits at most 32 absolute PATH directories for a closed set of names,
and does not recursively scan disks. The separate macOS eligibility query runs
once per catalog request. There are no discovery timers, polling, watchers or
automatic retry loops. Cached executable paths are reverified before every launch
and removed when gone. Failures never automatically replay a launch.

“Launcher started” means the operating system acknowledged spawning the launcher;
it does not assert that the external application displayed a window or that a
later launcher exit succeeded. Spawn failures and nonzero exits observed within the fixed one-second launch
window are reported in launch details. The window bounds observation only; it
never kills a running application.
After a lost HTTP acknowledgement, check the Host desktop before opening again.
Navigation, disconnect and closing the menu never kill a handed-off application.

## Ownership and admission

`ProductHostWorkspaces` exposes `desktopCatalog(scope, refresh)` and
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
boundary. rustX supplies only an allowlisted environment to the desktop launcher.
The operating system/runtime may add platform-owned environment variables. rustX
credentials, provider secrets, transport tokens and runtime injection variables
such as NODE_OPTIONS are not supplied. The child
is detached with ignored stdio and unreferenced after spawn. Application discovery
creates no children; the short macOS eligibility query is bounded and reaped.
Native metadata connections have handshake/request bounds
and close after every operation. No desktop API enters Rust, and no new Session
or Workspace owner, storage, protocol schema, feature flag or compatibility path
is introduced.

Deterministic tests gate native reads, catalog responses and launch responses;
they assert zero launch before admission, exact target/path/argv, authority
replacement, missing paths/apps, errors, filtered environment and no replay.
The `Desktop adapter and Host (macOS Node)` CI job on `macos-latest` executes
`test/desktop-host.test.ts` using Node 24 and the pinned dependencies. These exercise actual macOS filesystem,
canonicalization, HTTP and harmless Node child-process launch boundaries, plus the
native console/bootstrap eligibility check. Deterministic fakes cover GUI argv,
missing apps and session evidence. The full Linux Web job runs the same tests and
browser acceptance. Neither lane opens Finder/Terminal or proves GUI completion;
real interactive GUI smoke remains a separate, unexecuted check.

### External file-opening identity

Opening a Workspace file in a desktop application is a best-effort pathname
operation. The Host authorizes the native Session location and validates the
relative path using descriptor-relative traversal at admission: absolute escapes,
`..`, and symlink traversal are rejected. This guarantees admission of an
allowed Workspace location, not the identity of the file eventually used by the
external application. Another local process can rename or replace the path after
admission; the application may resolve that path after the launcher returns.
Keeping a descriptor open or rechecking the pathname cannot guarantee identity
through that external boundary. No file-object identity or desktop sandbox is
promised. A new opening request independently repeats Host admission. Descriptor
based Workspace reads retain their stronger object-bound read contract.

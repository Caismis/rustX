# Native execution environment

Native Agent Bash, Workflow Bash, and Subagent Bash inherit the environment of
the host process that launched rustX. The Workspace supplies the working
directory. It does not replace `HOME`.

The composition order is:

1. The captured host environment.
2. User and Workspace `[environment]` overrides, including `PATH`, `HOME`,
   `LANG`, and `LC_ALL`.
3. Removal of shell startup controls (`BASH_ENV`, `ENV`, `SHELLOPTS`,
   `BASHOPTS`, and exported Bash functions) and rustX supervisor control
   variables. Overrides cannot put those keys back.
4. Workspace Python selection.

`PATH` or `HOME` values that are not Unicode fail that composition. Other
non-Unicode host values are omitted because the supervisor receives the
command environment as JSON in an owner-only private snapshot. The native
process owner retains the snapshot through physical settlement; only its path
is passed to the supervisors, avoiding per-string OS environment limits. An authorized command can still read ordinary
credentials that were already in the host environment. This is not an
operating-system sandbox.

## Workspace Python

Each Workspace has one default environment at `<workspace>/.venv`, shared by
its sessions. Native Bash uses it for `python` and `python3`. The runtime does
not rewrite the command string, so an absolute interpreter path keeps its
ordinary meaning.

- A valid existing virtual environment is reused after its interpreters are
  probed. `uv` is not required for reuse. A missing environment is left absent
  if uv is missing; rustX does not install uv or use another environment creator.
- A missing environment is created only with `uv` 0.10.0 or newer, at the
  final path, without `--clear`, `--seed`, or `--allow-existing`. `uv venv`
  performs interpreter discovery and automatic download under uv's effective
  download/offline policy; `uv python find` is not a prerequisite. Bootstrap
  `uv` commands pass `--directory` pointing at a private directory outside the
  Workspace tree, so a `.python-version` on the Workspace or its parents
  cannot select the interpreter. An explicit `--python` request also excludes
  pins above the private directory; without root requirements it is `default`.
  User and system `uv.toml` files still apply, together with
  offline, proxy, certificate, index, and cache settings from the captured
  environment. There is no installer, `python -m venv` fallback, `uv sync`,
  or package seed. At most 64 preparations run at once. A further Workspace is
  left for a later schedule, and shutdown joins every preparation that was
  started.
- A symlink, file, or invalid directory at `.venv` is left in place and
  reported internally as a conflict. After acquiring creation capacity, the
  runtime classifies the target again immediately before dispatch. A directory
  added during the wait enters validation; even an empty directory is never
  initialized. A new root symlink or file is a conflict before uv starts.
- The Workspace-root `.python-version` is the explicit interpreter request.
  A path request must resolve to that interpreter. An implementation prefix
  such as `cpython-` or `pypy-` must match `sys.implementation.name`; an
  unknown or trailing qualifier is rejected. With no pin, the root
  `pyproject.toml` `[project].requires-python` specifier is passed to
  `uv venv --no-project --directory <outside the Workspace> --python <request>`. A relative
  path request is made absolute from the Workspace root before that command
  sees it. `requires-python` is parsed before probing. When both root files
  specify versions, pep440_rs constructs their conjunction, preserving
  implementation and partial/exact-version semantics, and uv selects over
  that joint request. A fixed path interpreter must satisfy both inputs before
  creation and retain its identity across the capacity wait. A fixed path can
  name an executable/launcher or a Python installation/virtual-environment
  directory. Directory requests select `bin/python` when present, otherwise
  `bin/python3` only when the preferred directory entry is absent. A selected
  invalid entry, including a dangling or cyclic link, is diagnosed rather than
  skipped; inspection errors also stop preparation.
  This order is fixed across uv versions and also applies to reuse without uv.
  Bare relative directory names emitted by `uv python pin` also resolve from
  the Workspace root; restoring such a directory invalidates an earlier
  unresolved-request conflict without requiring a root version-file edit.
  The authored path, selected entry, and probed base executable are stamped. Creation passes that
  validated executable to uv; adoption compares the requested invocation's
  base executable with the environment's, so macOS launchers can resolve to
  their actual interpreter without accepting a different Python binary.
  File requests are probed through their resolved invocation path while the
  authored alias remains in the witness. Directory entries retain their
  invocation path so their virtual-environment context remains effective.
  Missing, unusable, or changed path-pin interpreters use the existing
  30-second Unavailable cooldown in both creation and adoption. Installing or
  repairing that external interpreter can therefore recover without rewriting
  root version files. Malformed root inputs and invalid or incompatible
  `.venv` contents remain conflicts.
  Ready retains the first fixed-interpreter witness across both probes and
  publication, and both probes must report the same actual executable even
  when launcher configuration changes without altering tracked metadata.
  Its authored path, selected entry, and actual executable stamps are checked on
  cache hits, together with the directory-entry selection. Replacing or
  redirecting an entry, or adding a preferred entry, invalidates Ready even
  when the installation-directory and old base-executable stamps are unchanged.
  The selected
  interpreter is
  checked against both inputs before it is ready. The readiness identity is
  snapshotted after the environment exists and before those probes. That same
  snapshot must match the original root and version inputs. Every physical
  request carries the originally admitted root device/inode to the supervisor,
  including after waiting for a concurrency permit. A pin,
  project, or interpreter change during a probe does not publish Ready.
  Version files use the shared 1 MiB bounded reader, which opens with
  `O_NONBLOCK` and verifies that the opened descriptor is a regular file. FIFO,
  linked FIFO and oversized inputs fail before any subprocess starts; links
  to ordinary files remain supported. Unreadable version pins are conflicts, and exact pins retain prerelease
  and postrelease qualifiers. Changes to `pyvenv.cfg`, environment-root
  identity, interpreter targets or permissions invalidate cached readiness.
  Neither root file is edited.
- When the environment is not ready, `python` and `python3` resolve to a
  guard that exits 127. The guard is a private directory ahead of the host
  `PATH`, so `/usr/bin/env python3` uses it and `git`, `ls`, and `cargo` do
  not. If the Workspace state directory cannot be written, that directory is
  claimed in the temporary directory, `XDG_RUNTIME_DIR`, `/dev/shm`, or the
  directory containing the rustX executable. If every one of those is
  unwritable, `PATH` still starts with the `python-guard` directory installed
  beside that executable. `cargo build` places it next to `rustx` and the test
  binaries using atomic file replacement, and a deployment copies that
  directory with the executable.
  Lookup does not use the build checkout. Both scripts must be executable and
  match the bytes embedded in the binary. Every cache hit is revalidated; a
  deleted or damaged temporary guard is replaced. If no writable resource and
  no valid deployed guard remain, native Bash fails explicitly at composition
  instead of inheriting an unguarded host PATH. Other native tools and excluded
  execution paths remain available. They
  participate in executable lookup, so `sh`, `/usr/bin/env python3`, and
  `#!/usr/bin/env python3` exit 127 with the guard diagnostic. Other commands
  keep the host `PATH`. The failure is not reported as a non-Unicode `PATH`
  or `HOME`.
- Registration queues the authorized directory only when the Product Host
  attests a shared filesystem. The location id is not a path. Waiting for
  the cross-process lock observes runtime shutdown and does not stay blocked
  in `flock`.

Registration and session creation schedule this work and do not wait for it.
Read-only listing does not create an environment. The first native Bash use
observes the current selection immediately and does not wait either.

Python activation cleanup removes `PYTHONHOME`, `PYTHONPATH`, `PYTHONSTARTUP`,
`PYTHONEXECUTABLE`, `__PYVENV_LAUNCHER__`, and inherited venv/Conda state, both
for bootstrap and after native Bash configuration overrides.

`uv` itself is resolved from the host `PATH` before the Workspace environment
is activated. Bootstrap removes `UV_PROJECT`, `UV_PROJECT_ENVIRONMENT`,
`UV_VENV_CLEAR`, and `UV_VENV_SEED`; operational download, offline, mirror,
proxy, certificate and cache policy remains in force. User commands are not rewritten, and the runtime does not
inject `UV_PROJECT_ENVIRONMENT` into them.

## Excluded execution

Managed Python, manual terminals, generic App Server `command/exec`, and MCP
servers do not enter the Workspace environment. Managed Python, generic
execution, and MCP retain their `ToolEnvironment::child_environment` contract.
Manual terminals retain the existing Desktop environment allowlist; the
terminal process owner passes it through the same private snapshot protocol
and retains that snapshot until the outer supervisor exits.

A subagent receives the parent's captured host snapshot in a mode-0600 file
inside its private runtime root. It does not recapture the environment of the
supervisor that spawned it.

## Residual limits

Stdio EOF only detaches a client. The process/host drain owns cancellation and
joining of preparation, alongside its other runtime resources. Physical probes
inherit the Owner cancellation signal, including children derived after
cancellation. The subagent process joins its preparation before exiting.

The lock coordinates rustX processes on one machine. Shutdown stops waiting
for it. It does not stop an unrelated program from changing `.venv`, and it
is not an NFS lock. The post-permit target check does not claim a general
filesystem transaction against arbitrary external mutations after dispatch. Readiness
identity tracks the Workspace root and the device, inode, mtime, length and
permissions of its version files, `.venv`, `pyvenv.cfg`, and the resolved
`.venv/bin/python` plus `python3` targets. For a path pin it also retains the
authored interpreter path and the probed actual executable. An in-place edit
that preserves all tracked metadata can remain cached until another input
changes.
One process admits at most two uv or interpreter probes at a time; that cap
does not span processes. One Workspace keeps a single preparation task until
it finishes, and a process retains a bounded number of finished tasks and Workspace records, including Ready records. Evicting
a record only drops cached readiness; later use revalidates the existing
environment without rebuilding it. A launcher that starts rustX
with a short `PATH` is a launcher configuration problem: a running host does
not observe later changes made in a parent terminal. `uv` is optional and is
installed separately; creation requires 0.10.0 or newer. The environment is
frozen on the native Bash invocation before permission approval, so a later
Ready publication does not change a command that was already prepared.

Command-environment transport currently needs an owner-only named temporary
file for every supervised launch, including small foreground Bash commands.
This is independent of lazy output spill. Failure to allocate that snapshot
refuses launch; moving it to a retained private FD or control-frame transport
would require a separate supervisor transport change.

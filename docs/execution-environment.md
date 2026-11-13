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
command environment as JSON. An authorized command can still read ordinary
credentials that were already in the host environment. This is not an
operating-system sandbox.

## Workspace Python

Each Workspace has one default environment at `<workspace>/.venv`, shared by
its sessions. Native Bash uses it for `python` and `python3`. The runtime does
not rewrite the command string, so an absolute interpreter path keeps its
ordinary meaning.

- A valid existing virtual environment is reused after its interpreters are
  probed. `uv` is not required for reuse.
- A missing environment is created only with `uv` 0.10.0 or newer, at the
  final path, without `--clear`, `--seed`, or `--allow-existing`. Bootstrap
  `uv` commands pass `--directory` pointing at a private directory outside the
  Workspace tree, so a `.python-version` on the Workspace or its parents
  cannot select the interpreter. A pin above that private directory can still
  be walked. User and system `uv.toml` files still apply, together with
  offline, proxy, certificate, index, and cache settings from the captured
  environment. There is no installer, `python -m venv` fallback, `uv sync`,
  or package seed. At most 64 preparations run at once. A further Workspace is
  left for a later schedule, and shutdown joins every preparation that was
  started.
- A symlink, file, or invalid directory at `.venv` is left in place and
  reported internally as a conflict.
- The Workspace-root `.python-version` is the explicit interpreter request.
  A path request must resolve to that interpreter. An implementation prefix
  such as `cpython-` or `pypy-` must match `sys.implementation.name`; an
  unknown or trailing qualifier is rejected. With no pin, the root
  `pyproject.toml` `[project].requires-python` specifier is passed to
  `uv python find --no-project --directory <outside the Workspace>`. A relative
  path request is made absolute from the Workspace root before that command
  sees it. The selected interpreter is
  checked against both inputs before it is ready. The readiness identity is
  snapshotted after the environment exists and before those probes. A pin,
  project, or interpreter change during a probe does not publish Ready.
  Neither root file is edited.
- When the environment is not ready, `python` and `python3` resolve to a
  guard that exits 127. The guard is a private directory ahead of the host
  `PATH`, so `/usr/bin/env python3` uses it and `git`, `ls`, and `cargo` do
  not. If the Workspace state directory cannot be written, that directory is
  claimed in the temporary directory, `XDG_RUNTIME_DIR`, `/dev/shm`, or the
  directory containing the rustX executable. If every one of those is
  unwritable, composition still succeeds: Bash rejects the `python` and
  `python3` command names with the guard diagnostic, and other commands keep
  the host `PATH`. The failure is not reported as a non-Unicode `PATH` or
  `HOME`.
- Registration queues the authorized directory only when the Product Host
  attests a shared filesystem. The location id is not a path. Waiting for
  the cross-process lock observes runtime shutdown and does not stay blocked
  in `flock`.

Registration and session creation schedule this work and do not wait for it.
Read-only listing does not create an environment. The first native Bash use
observes the current selection immediately and does not wait either.

`uv` itself is resolved from the host `PATH` before the Workspace environment
is activated. Bootstrap does not follow `UV_PROJECT_ENVIRONMENT` or
`UV_VENV_CLEAR`. User commands are not rewritten, and the runtime does not
inject `UV_PROJECT_ENVIRONMENT` into them.

## Excluded execution

Managed Python, manual terminals, generic App Server `command/exec`, and MCP
servers do not enter the Workspace environment. They keep the explicit
baseline from `ToolEnvironment::child_environment`.

A subagent receives the parent's captured host snapshot in a mode-0600 file
inside its private runtime root. It does not recapture the environment of the
supervisor that spawned it.

## Residual limits

The lock coordinates rustX processes on one machine. Shutdown stops waiting
for it. It does not stop an unrelated program from changing `.venv`, and it
is not an NFS lock. Readiness
identity is the device, inode, mtime, and length of the Workspace root, its
version files, and `.venv/bin/python` plus `python3`. A replacement that keeps
the same length and timestamp can remain cached until another input changes.
One process admits at most two uv or interpreter probes at a time; that cap
does not span processes. One Workspace keeps a single preparation task until
it finishes, and a process retains a bounded number of finished tasks, probe
cancellation signals, and non-ready records. A launcher that starts rustX
with a short `PATH` is a launcher configuration problem: a running host does
not observe later changes made in a parent terminal. `uv` is optional and is
installed separately; creation requires 0.10.0 or newer. The environment is
frozen on the native Bash invocation before permission approval, so a later
Ready publication does not change a command that was already prepared.

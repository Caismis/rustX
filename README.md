# rustX

rustX is a standalone Rust execution runtime for durable, tool-using LLM
agents.

> Status: pre-alpha. The architecture may change incompatibly before 1.0 when
> a cleaner abstraction is the better design.

Supported platforms are Linux and macOS. Windows is currently unsupported.
Process supervision has platform-specific guarantees; this README summarizes
the product, while the exact Linux and macOS contracts live in the
[architecture](docs/architecture.md) and [invariants](docs/invariants.md)
documents.

## What works today

rustX currently supports:

- durable multi-turn conversations with provider-independent canonical
  messages and history;
- OpenAI Chat Completions, OpenAI Responses, and Anthropic Messages;
- a tool-using Agent Loop with context assembly, compaction, and reasoning
  streams;
- native Read, Write, Edit, Grep, Glob, and Bash tools, plus Skills, MCP tools,
  and custom Python tools;
- a `todo` task list the model keeps as it works — an optional Native Agent
  Extension, enabled by default — drawn as a live panel above the editor and
  printed in full by `/todos`;
- cancellation, recovery, runtime supervision, and background tool
  execution;
- runtime-owned model selection and switching;
- native Sessions with resume, rename, clone, fork, and tree workflows;
- Runtime Client protocol over stdio/JSONL and the `rustx-tui` reference
  terminal client.

These are current capabilities of the pre-alpha repository, not a promise of
production maturity.

## Quick start

Use [`rustx init`](docs/configuration-diagnostics.md#minimal-initialization) with
explicit provider/model declarations, or author your model once in the host configuration directory:
`$XDG_CONFIG_HOME/rustx`, or `$HOME/.config/rustx` when XDG_CONFIG_HOME is
unset (Linux and macOS). Put explicit provider/model declarations in
`models.toml` and select one in `settings.toml`:

```toml
[model]
model = "example/demo-model"
```

Use your declared provider/model identity. The
[minimal catalog example](examples/local-runtime/minimal/models.toml) shows the required endpoint,
credential source, protocol, limits and capabilities; its endpoint is a placeholder.
The [launch contract](docs/launch-configuration.md) documents all locations,
field ownership, precedence, path semantics, defaults and trust.
`rustx config check` diagnoses configuration offline; `rustx config show --sources`
explains the redacted prospective next launch. Only explicit `doctor --probe`
may connect or spawn diagnostic targets. See the [command and exit contract](docs/configuration-diagnostics.md).

Build the runtime and install the reference TUI:

```sh
cargo build --bin rustx
pnpm --dir tui install --frozen-lockfile
./target/debug/rustx --workspace /path/to/project --trust grant
pnpm --dir tui start --binary "$PWD/target/debug/rustx" --workspace /path/to/project
```

When launched from the project, `--workspace` is unnecessary. A project
`rustx.toml` is optional, and runtime state defaults to the user state directory.
Native-only startup needs neither Python nor MCP. The
[advanced resource example](examples/local-runtime/README.md) also demonstrates
optional managed Python tools and fixed Workflows.

Startup begins on a fresh/unused empty Session. Earlier Sessions remain reachable
through `/resume`; `--continue` and `--session` request explicit selection.
`--name` only names the selected Session. Failed launch resolution or composition
cannot publish another active Session.

## Runtime and reference client

`rustx` is the runtime. `rustx-tui` is a reference client and presentation
layer.

The TUI spawns `rustx`, communicates with it through the Runtime Client protocol
over stdio/JSONL, and projects runtime snapshots and events into a terminal
interface. Model, Session, tool, capability, context, and execution semantics
remain owned by the Rust runtime; the TUI does not implement a parallel
runtime or session system. See [`tui/README.md`](tui/README.md) for the
user-visible command surface.

## Filesystem and native tools

`--workspace` establishes the runtime's authoritative execution cwd and the
conventional project/source tree. It is not a general filesystem sandbox for
native Read, Write, Edit, Grep, or Glob.

Project-authored Agent resources use the workspace-owned `.agents/` namespace:
Skills and Python tools retain their discovery semantics, while Subagents and
native Workflows are admitted explicitly by `rustx.toml`. The configured
runtime root is runtime-owned/generated state outside the workspace and
is not a project-resource fallback. `.agents/skills/` is the canonical project
layout; the two automatic Skill sources are `global` (`~/.agents/skills`) and
`workspace` (`<workspace>/.agents/skills`), selected by `[skills].sources`,
with `workspace` shadowing `global` for the same Skill identity. Explicit
`--skill` paths are a separate launch authority and take precedence over both.
The root Agent automatically sees every eligible Skill in that catalog minus
`agent.disabled_skills`; named Agents select identities explicitly, and every
Agent loads a Skill's contents lazily. Paths are resolved by the
[launch resolver](docs/launch-configuration.md).

For those native file tools, relative paths resolve from the execution cwd and
absolute paths are valid host filesystem paths. `.` and `..` are resolved
lexically before filesystem or symlink behavior. Runtime-owned
`ManagedToolOutput` can be inspected through supported read/search paths, but
model-originated Write/Edit cannot mutate that managed output. Grep and Glob
remain in-process, and their `.gitignore` behavior is unchanged.

This is a user-facing path model, not a general security-sandbox guarantee.
See the [architecture](docs/architecture.md) and
[invariants](docs/invariants.md) documents for the exact native-tool
contracts.

### Isolated subagent local overlays

An isolated Git-worktree subagent may receive explicitly selected local files
through `<workspace>/.worktreeinclude`. The manifest is resolved only at the
authoritative logical workspace root, and each nonblank line is one exact path
relative to that workspace. Leading and trailing whitespace is ignored, and a
line whose first non-whitespace character is `#` is a comment. Version 1 does
not support globs, negation, escaping, directory recursion, absolute paths, or
`..` traversal.

Every selected path must exist as an individual regular file, remain inside
the logical workspace, contain no symlink component, be untracked, and be
classified as ignored by Git. The manager acquires each source beneath a
stable logical-workspace handle with no-symlink traversal and retains the
validated file handle through freezing; no later pathname reopen supplies
overlay bytes. Duplicate normalized/canonical destinations are rejected. A
manifest may select at most 64 files and 8 MiB of content in total; the
manifest itself is limited to 64 KiB. Missing or ineligible files fail
isolated acquisition instead of being skipped.

The workspace manager freezes all selected bytes from those retained handles
before creating the child worktree, then materializes and byte-verifies them
through the child logical-workspace authority before child ownership can
commit. Later parent edits therefore cannot change the acquired overlay.
These files are local execution inputs, not source synchronization: they do
not copy dirty tracked or arbitrary untracked state, and overlay-only edits in
the child remain ignored by ordinary Git settlement and do not create a
source-worktree handoff.

## Native Sessions

Sessions are runtime-owned. The reference TUI currently exposes `/new`,
`/resume`, `/session`, `/name`, `/clone`, `/fork`, and `/tree` for creating,
resuming, inspecting, naming, cloning, forking, and branching Session graphs.
A name is display metadata only: an unnamed Session is listed by its first
message, and a Session is always resolved by identity, never by name.
The TUI invokes the canonical Runtime Client operations; it does not maintain
a separate Session implementation. See [`tui/README.md`](tui/README.md) for
argument hints and interaction details.

## Architecture and development

Normative detail remains in the owning documents:

- [Architecture](docs/architecture.md)
- [Runtime Invariants](docs/invariants.md)
- [Process-death conformance](docs/process-death-conformance.md)
- [Development Plan](docs/development-plan.md)
- [Repository Policy](docs/repository-policy.md)

See [CONTRIBUTING.md](CONTRIBUTING.md) for contributor checks and pull-request
workflow.

## License

MIT
External sources are opt-in: see [source activation and credentials](docs/source-activation.md).
Native-only startup needs no Python, `uv`, MCP executable/endpoint or external
source secret. `--no-tools` controls model exposure; source disabling controls preparation.

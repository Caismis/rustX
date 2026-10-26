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
- a `todo` task list the model keeps as it works — an optional closed Rust-owned
  Plugin, off unless explicitly enabled — drawn as a live panel above the editor and
  printed in full by `/todos`;
- cancellation, recovery, runtime supervision, and background tool
  execution;
- runtime-owned model selection and switching;
- native Sessions with resume, rename, clone, fork, and tree workflows;
- finite background Jobs and durable child Agents with message, activation wait, interrupt and resume controls;
- App Server as the sole external product control protocol over stdio/JSONL
  and WebSocket, including the `rustx-tui` terminal client.

These are current capabilities of the pre-alpha repository, not a promise of
production maturity.

## Quick start

Use [`rustx init`](docs/configuration-diagnostics.md#minimal-initialization) with
explicit Provider/Model declarations, or author `~/rustx/rustx.toml` using the
[minimal example](examples/local-runtime/minimal/rustx.toml). Workspace configuration
lives in `<workspace>/rustx.toml`; Agent resource definitions live in the User and
Workspace `.agents` roots. Resource existence grants no Root capability.

The [CFG3 reference](docs/configuration.md) documents paths, the complete typed
schema, exact semantic overlay units, resource shadowing and Save/Reload behavior.
`rustx config check` validates offline; `rustx config show --sources` explains
prospective resolution. Default durable storage is `~/rustx/runtime`.

See [Local development](DEVELOPMENT.md) for the canonical App Server, TUI, and complete Web launchers. Build the runtime and install the reference TUI:

```sh
cargo build --bins
pnpm --dir dev install --frozen-lockfile
pnpm --dir tui install --frozen-lockfile
pnpm --dir dev tui -- --workspace /absolute/path/to/project
```

Supply `--workspace` explicitly with the development launcher. A project
`rustx.toml` is optional, and runtime state defaults to `~/rustx/runtime`.
Native-only startup needs neither Python nor MCP. The
[advanced resource example](examples/local-runtime/README.md) also demonstrates
optional managed Python tools and fixed Workflows.

`rustx` itself starts no Session: clients control it only through
`rustx app-server`, and a bare `rustx` prints help. The TUI starts on a new
Session; earlier Sessions remain reachable through `/resume`, and the TUI's
`--session` requests explicit selection. A failed Session composition publishes
nothing.

## Runtime and reference client

`rustx` is the runtime; `rustx app-server` is its only external product
control protocol. `rustx-tui` is a reference client and presentation layer.

The TUI spawns `rustx`, communicates with it through the App Server protocol
over stdio/JSONL, and projects runtime snapshots and events into a terminal
interface. Model, Session, tool, capability, context, and execution semantics
remain owned by the Rust runtime; the TUI does not implement a parallel
runtime or session system. See [`tui/README.md`](tui/README.md) for the
user-visible command surface.

The [App Server protocol](docs/app-server-protocol.md) is the unified,
transport-neutral client boundary. Direct `rustx app-server --listen stdio` and
`rustx app-server --listen ws://127.0.0.1:8080 --token-file <path>` serve
headless/integration use. Local TUI owns a stdio App Server child; remote TUI
connects to an externally managed WebSocket server. Connection detach is separate
from owner-driven process shutdown.

The [Developer Web Console](web-console/README.md) uses this App Server directly.
It adapts a pinned subset of DeepSeek Harness presentation source for concurrent
Sessions, native interactions and a bounded raw JSON-RPC inspector. No Harness
backend is required.

## Filesystem and native tools

`--workspace` establishes the runtime's authoritative execution cwd and the
conventional project/source tree. It is not a general filesystem sandbox for
native Read, Write, Edit, Grep, or Glob.

Agent resources have exactly two roots: User `~/rustx/.agents` and Workspace
`<workspace>/.agents`. Workspace same-name resources shadow the complete User
resource, including malformed higher definitions. Resource existence grants no
Root authority. Root Native Tools use the explicit `agent.tools.builtin` whitelist;
MCP and Managed Python Tools use `agent.tools.sources`. Named Agents own independent
complete Tool profiles.

For MCP and Managed Python sources, `"all"` grants source-wide access to both
ordinary Tools and advertised MCP Resources. Exact Tool-name arrays grant only
those Tools. A negotiated, authorized Resources source automatically composes
`list_mcp_resources`, `list_mcp_resource_templates`, and `read_mcp_resource` once
per Agent, including when the source publishes no Tools. Resource contents enter
context only through an explicit Tool call.

Skill roots are `~/rustx/.agents/skills` and `<workspace>/.agents/skills`, with
whole-package Workspace shadowing. Root and named Agents each select `skills` as
`"all"`, an exact name array, or `[]`. This controls prompt visibility, not filesystem
authorization. The model receives absolute collection roots and reads package
contents progressively. There is no configurable source-selection layer.
The [CFG3 reference](docs/configuration.md) defines the complete ownership model.

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

### Shared Agent and Workflow workspace

Every Agent and Workflow runs in the Session's workspace. Agents see the
parent's uncommitted state and their own effects in place; failure,
cancellation and a rejected Review never roll anything back. rustX allocates
no Git worktree, branch or copy for an Agent or Workflow, and Session deletion
never removes project files. The five Agent controls (`subagent`,
`list_agents`, `send_message`, `wait_agent`, `interrupt_agent`) are composed
automatically for the Root Agent when `agent.agents` admits at least one Agent.
See [the shared-workspace contract](docs/agent-workflow-shared-workspace.md).

Root and named-Agent Plugins all default off, including Todo, Goal and Agent Status.
Only an explicitly authored enabled Plugin is composed.

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
- [App Server protocol and generated clients](docs/app-server-protocol.md)
- [Runtime Invariants](docs/invariants.md)
- [Process-death conformance](docs/process-death-conformance.md)
- [Development Plan](docs/development-plan.md)
- [Repository Policy](docs/repository-policy.md)

See [CONTRIBUTING.md](CONTRIBUTING.md) for contributor checks and pull-request
workflow.

## License

MIT
External sources materialize on admitted demand: see [source demand and credentials](docs/source-activation.md).
Native-only startup needs no Python, `uv`, MCP executable/endpoint or external
source secret. Definitions and discovery are inert; Agent/Workflow selection
produces finite admitted demand before connection or Python preparation.

For typed generation diagnostics and offline Agent/Workflow inspection, see
[capability inspection](docs/capability-inspection.md) and the [CFG3 example](examples/local-runtime/README.md).

Public CLI syntax and stream rules are documented in [the CLI contract](docs/implementation/issue-396-cli.md). Use `rustx --help` or nested `--help` for the generated grammar.

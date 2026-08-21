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
- cancellation, recovery, runtime supervision, and background tool
  execution;
- runtime-owned model selection and switching;
- native Sessions with resume, rename, clone, fork, and tree workflows;
- Runtime Client Protocol v1 over stdio/JSONL and the `rustx-tui` reference
  terminal client.

These are current capabilities of the pre-alpha repository, not a promise of
production maturity.

## Quick start

The runtime takes four explicit startup paths; it does not discover
configuration from the current directory or from `~/.rustx`. Configure
`examples/local-runtime/models.json` and
`examples/local-runtime/session.json` in place, or copy the complete
`examples/local-runtime/` directory and adjust the paths below.

Set the credential referenced by the example catalog, then build the runtime
and install the locked TUI dependencies:

```sh
export RUSTX_EXAMPLE_API_KEY='replace-me'

cargo build --bin rustx

nvm install --lts
nvm use --lts
corepack enable
pnpm --dir tui install --frozen-lockfile
```

Launch the reference client with the example paths:

```sh
pnpm --dir tui start \
  --binary "$PWD/target/debug/rustx" \
  --models "$PWD/examples/local-runtime/models.json" \
  --session "$PWD/examples/local-runtime/session.json" \
  --workspace "$PWD/examples/local-runtime/workspace" \
  --runtime-root "$PWD/examples/local-runtime/.rustx"
```

The example endpoint is a placeholder. Replace it with the endpoint for the
selected provider before making a model request. The full configuration
contract and custom Python-tool example are in
[`examples/local-runtime/README.md`](examples/local-runtime/README.md).
The example's discovered Python tool also requires `uv` on `PATH`.

## Runtime and reference client

`rustx` is the runtime. `rustx-tui` is a reference client and presentation
layer.

The TUI spawns `rustx`, communicates with it through Runtime Client Protocol v1
over stdio/JSONL, and projects runtime snapshots and events into a terminal
interface. Model, Session, tool, capability, context, and execution semantics
remain owned by the Rust runtime; the TUI does not implement a parallel
runtime or session system. See [`tui/README.md`](tui/README.md) for the
user-visible command surface.

## Filesystem and native tools

`--workspace` establishes the runtime's authoritative execution cwd and the
conventional project/source tree. It is not a general filesystem sandbox for
native Read, Write, Edit, Grep, or Glob.

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

## Native Sessions

Sessions are runtime-owned. The reference TUI currently exposes `/new`,
`/resume`, `/session`, `/name`, `/clone`, `/fork`, and `/tree` for creating,
resuming, inspecting, naming, cloning, forking, and branching Session graphs.
The TUI invokes the canonical Runtime Client operations; it does not maintain
a separate Session implementation. See [`tui/README.md`](tui/README.md) for
argument hints and interaction details.

## Architecture and development

Normative detail remains in the owning documents:

- [Architecture](docs/architecture.md)
- [Runtime Invariants](docs/invariants.md)
- [Development Plan](docs/development-plan.md)
- [Repository Policy](docs/repository-policy.md)

See [CONTRIBUTING.md](CONTRIBUTING.md) for contributor checks and pull-request
workflow.

## License

MIT

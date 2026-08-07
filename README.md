# rustX

rustX is a standalone Rust execution runtime for durable, tool-using LLM agents.

> Status: pre-alpha. The architecture is intentionally allowed to break before 1.0 when a cleaner abstraction is available.

## Goals

- Build a deterministic, durable agent execution kernel in Rust.
- Keep the agent loop independent from provider SDK types, storage implementations, UI protocols, and control-plane schemas.
- Support multi-turn conversations, tool use, reasoning streams, context compaction, skills, MCP tools, and custom Python tools.
- Make cancellation, recovery, event ordering, and capability revisions explicit runtime semantics.
- Optimize for long-term architectural evolution rather than compatibility with previous runtimes.

## Non-goals

- Compatibility with Agno, legacy Python runtimes, old database schemas, or previous internal APIs.
- Provider-hosted tools.
- Native Google model protocols in the first implementation.
- Control-plane concerns such as billing, user management, visual builders, or Fleet UI.

## Model protocols

The first runtime targets three model interaction protocols:

- OpenAI Chat Completions
- OpenAI Responses
- Anthropic Messages

Provider SDKs are implementation details of model adapters. Their types must not cross into the runtime core.

## Capability model

The runtime capability set consists of:

- Native platform tools
- Skills
- MCP tools
- Custom Python tools

A running attempt observes an immutable capability revision for its entire lifetime.

## Core architecture

```text
Runtime Manifest
      |
      v
Capability Snapshot
      |
      v
Agent Kernel
  |       |
  |       +--> Tool Plane
  |             |- Native tools
  |             |- MCP
  |             `- Python tools
  |
  +--> Context Engine
  |       |- Context assembly
  |       |- Compaction
  |       `- Provider context compilation
  |
  `--> Model Plane
          |- OpenAI Chat adapter
          |- OpenAI Responses adapter
          `- Anthropic Messages adapter

All execution facts
      |
      v
Runtime Event Journal
      |
      +--> Local diagnostics
      `--> External projections such as AG-UI
```

## Development strategy

The executor is built and validated locally before production integration. The early development loop is:

```text
Terminal input
  -> Message blocks
  -> Context compiler
  -> Real or mock model
  -> Agent loop
  -> Tool execution
  -> Compaction
  -> Continued multi-turn conversation
```

Container integration, production storage, orchestration, and control-plane integration come after the local runtime semantics are stable.

See [Architecture](docs/architecture.md), [Development Plan](docs/development-plan.md), [Runtime Invariants](docs/invariants.md), and [Repository Policy](docs/repository-policy.md).

## Repository governance

All repository content must be written in English. Non-trivial work should use the repository Issue Forms and be linked from a focused pull request. Merge-ready pull requests must pass formatting, Clippy, and test checks. During pre-1.0 development, breaking changes are preferred over compatibility shims when they improve the architecture.

See [CONTRIBUTING.md](CONTRIBUTING.md) for the contributor workflow.

## License

MIT

# Development Plan

This plan prioritizes proving the execution kernel locally before integrating rustX into production infrastructure.

## Milestone 0 — Repository foundation

Deliverables:

- Minimal Rust crate
- Formatting and linting baseline
- Architecture documentation
- Runtime invariants
- Deterministic test structure

Exit criteria:

- `cargo check`, `cargo fmt --check`, and `cargo clippy` can be introduced without restructuring the repository.
- The module boundaries are explicit and documented.

## Milestone 1 — Canonical runtime model

Implement runtime-owned types for:

- `SystemMessageBlock`
- `UserMessageBlock`
- `AgentMessageBlock`
- `ToolMessageBlock`
- Content blocks
- Tool definitions, calls, and results
- `ModelRequest`
- `ModelEvent`
- `RuntimeEvent`
- `RuntimeManifest`

Implemented in M1 as the Layer 0 contracts described in
[`docs/architecture.md`](architecture.md) section 2.1, with deterministic
serialization fixtures under `tests/fixtures/m1/` and round-trip contract
tests in `tests/m1_contracts.rs`.

Exit criteria:

- No provider SDK type appears in the canonical model.
- Types serialize deterministically where persistence is required.

## Milestone 2 — Model execution

Implement model adapters for:

- OpenAI Chat Completions
- OpenAI Responses
- Anthropic Messages

Responsibilities:

- Streaming normalization
- Tool-call normalization
- Reasoning normalization with provider continuation state
- Usage normalization
- Error normalization
- Cancellation propagation

Exit criteria:

- A local CLI can stream a single real model response through canonical `ModelEvent` values.
- Adapter tests cover text, tool calls, reasoning, usage, errors, and cancellation.

## Milestone 3 — Agent loop

Implement the attempt and turn state machines.

Features:

- Multi-turn conversation
- Model -> tool -> model loop
- Sequential and parallel tool batches
- Deterministic tool-result ordering
- Attempt termination rules
- Turn-boundary inbound-message drain point
- Mock model executor for deterministic tests

Exit criteria:

- A deterministic fixture can execute a complete multi-turn tool-using agent without network access.
- A live local session can hold a normal multi-turn conversation.

## Milestone 4 — Context engine and compaction

Implement:

- Context assembly
- Token accounting
- Provider-context compilation
- Automatic compaction threshold
- Valid cut-point detection
- No cuts at tool-result boundaries
- Split-turn prefix summaries
- Incremental summary updates
- Context checkpoints
- Compact-and-retry flow

Exit criteria:

- A long local session can compact multiple times and continue correctly.
- Compaction never rewrites or deletes canonical history.
- Deterministic fixtures cover normal compaction and split-turn compaction.

## Milestone 5 — Native tool plane

Implement the canonical tool registry and executor contract.

Initial native tools:

- Read
- Write
- Edit
- Glob
- Grep
- Bash

Bash requirements:

- Full `/bin/bash`
- Foreground and background execution
- Separate process groups
- stdout/stderr capture
- Timeouts
- TERM -> grace period -> KILL
- Large-output truncation with durable full output
- Explicit execution environment instead of inherited process environment

Exit criteria:

- Tool batches work through the same agent loop used by mock tools.
- Foreground Bash cancellation is reliable.

## Milestone 6 — Skills

Implement:

- Skill package discovery
- `SKILL.md` loading
- Scripts, references, and assets layout
- Shared skill Python environment
- Shared skill Node environment
- Dependency materialization
- Skill execution through native file/Bash capabilities

Exit criteria:

- A skill can instruct the model to read its instructions and execute Python, Node, or shell scripts against the local workspace.
- Multiple skills can coexist in one shared environment with deterministic environment identity.

## Milestone 7 — External tool plane

### MCP

Use the Rust MCP SDK behind a rustX-owned executor boundary.

Implement:

- Server connection lifecycle
- Tool discovery
- Tool execution
- Progress
- Cancellation
- Deferred application of `tools/list_changed` until the runtime is quiescent

### Custom Python tools

Implement:

- Immutable tool version manifest
- One `uv` virtual environment per tool version digest
- Schema validation
- Process execution
- Result normalization

Exit criteria:

- MCP and Python tools are indistinguishable from native tools to the agent kernel.

## Milestone 8 — Runtime events and durability

Implement interfaces for:

- Runtime event writer
- Message store
- Context checkpoint store

Development backend:

- JSONL / filesystem storage is acceptable for local validation.

Semantics:

- Append-only runtime events
- Persist-before-publish ordering
- Stable event sequence numbers
- Crash reconciliation
- Unresolved tool-call handling

Exit criteria:

- A local session can be reconstructed from durable facts after process restart.

## Milestone 9 — Cancellation and runtime supervision

Implement:

- Hierarchical cancellation tokens
- Attempt-owned foreground processes
- Conversation-owned background processes
- Graceful draining
- Runtime idle state
- Capability mutation guard
- Capability revision snapshots

Exit criteria:

- Capability changes are rejected while the conversation runtime is busy.
- Attempt cancellation does not incorrectly terminate conversation-owned background work.

## Milestone 10 — Local runtime product

Build an interactive CLI for sustained manual testing.

Suggested commands:

- `/context`
- `/messages`
- `/events`
- `/tools`
- `/skills`
- `/compact`
- `/cancel`
- `/reset`

Run long-session soak tests covering:

- Many conversation turns
- Repeated compaction
- Tool failures
- Model errors
- Cancellation
- MCP reconnects
- Python tool execution
- Background Bash

Exit criteria:

- rustX can run as a standalone local agent runtime for extended sessions without relying on production control-plane services.

## Milestone 11 — Production integration boundary

Only after the local executor is stable, implement:

- Runtime command protocol
- HTTP/control interface
- Production event-store adapter
- AG-UI event projection
- Universal runtime image
- Orchestrator integration
- Control-plane integration

## Testing strategy

Three test layers are required:

1. Unit tests for pure state machines, transformations, ordering, and validation.
2. Deterministic runtime fixtures using mock model and tool executors.
3. Live integration tests using real model endpoints, MCP servers, and process environments.

Core semantics must never depend exclusively on nondeterministic live-model tests.

## Development priority

The shortest validated path is:

```text
single model stream
-> canonical messages
-> multi-turn loop
-> fake tool loop
-> runtime events
-> compaction
-> native tools
-> skills
-> MCP
-> Python tools
-> recovery
-> production integration
```

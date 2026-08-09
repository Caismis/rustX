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
- Turn-boundary inbound-message drain point (implemented by Issue #22 as a
  safe-boundary mailbox drain)
- Mock model executor for deterministic tests

Exit criteria:

- A deterministic fixture can execute a complete multi-turn tool-using agent without network access.
- A live local session can hold a normal multi-turn conversation.

M3's sequential/parallel tool-batch scheduling and deterministic
tool-result ordering are implemented by the M5 tool plane PR: a
`Sequential` invocation is an exclusive scheduling barrier, adjacent
`Parallel` invocations execute concurrently as one group, and canonical
results are committed in model call order.

## Milestone 4 — Context engine and compaction

Implemented in PR #21 (see [`docs/context-engine.md`](context-engine.md)):

- Context assembly and the explicit `ContextProjection` boundary
- Token accounting with explicit provenance (provider-reported vs.
  deterministic estimate) and a pluggable `TokenEstimator` (default
  `ceil(bytes / 4)`); the anti-loop progress rule compares deterministic
  estimates on both sides
- Provider-context compilation into canonical `ModelRequest.messages`
- Automatic compaction at the derived soft input limit
  (`window - reserve - max_output_tokens`, checked arithmetic)
- Valid structural cut-point detection with a tool-call/result edge index
- No cuts at tool-result boundaries; no orphan tool messages
- Recent-token retention by token target, not message count, measured over
  conversation content only (tool definitions never satisfy the target)
- Whole-turn-before-split cut priority; split-turn prefix summarization
  with projection-only agent slices
- Incremental summary updates from a previous compaction checkpoint, with
  absorbed-checkpoint summary-source suppression
- `ContextCheckpoint` and the `ContextCheckpointStore` abstraction
  (in-memory development/test implementation; M8 owns the durable backend)
- Bounded compact-and-retry on `ContextWindowExceeded` (exactly one retry
  per model turn)
- Continuation invalidation after successful compaction; explicit failure
  when the continuation-owning turn is pinned by system context
- Mandatory Agent Status projection: explicit `FreshInboundTurn` identity
  with a mandatory canonical-order validation and an explicit
  `InitialTurnTrigger` (fresh inbound vs pure continuation), structured
  section composition with reserved ids and registration-frozen section
  identities, the mandatory temporal section (clock + IANA timezone), the
  canonical deterministic renderer over structured extension facts, the
  ephemeral `AgentStatusAttachment` as a Layer 0 `model/types.rs` contract,
  adapter-owned wire placement, full token accounting, projection
  fingerprinting, fresh-inbound compaction protection, and a
  `ContextPreparationFailed`/`ContextCompactionFailed` failure distinction
- Agent Status integration with Issue #22 inbound batching: one drained
  batch becomes one fresh inbound turn with exactly one status snapshot
  targeting the final message
- Opt-in live repeated-compaction validation (`tests/m4_live.rs`)

The M1 `ContextManifest` gained `context_window_tokens` (additive pre-1.0
contract change; fixture and round-trip tests updated).

Issue #7 (M4: context engine and Agent Status) is **completed**; Issue #27
owns the deferred live multi-compaction/TUI verification, and Issue #8 (M5)
owns the background-execution Agent Status integration, which is implemented
by the M5 tool plane PR as a runtime-owned built-in section.

Issue #22 (inbound batching) is implemented in the Issue #22 PR: the
conversation inbound mailbox foundation (`src/runtime/inbound.rs`),
including the canonical `UserMessageBlock.timestamp`, the shared
`InboundSequence` domain, atomic enqueue, the finite watermark-bounded
drain, safe-boundary agent-loop integration, and the deterministic
mailbox/race/agent-loop/M4/provider test coverage. The remaining
cross-issue acceptance work — Agent Status integration with the drained
batch — is implemented by the Agent Status PR (issue-7/agent-status).
Background runtime producers are implemented by Issue #8 (M5), and mailbox
persistence/recovery remains later milestone work.

Exit criteria:

- A long local session can compact multiple times and continue correctly.
- Compaction never rewrites or deletes canonical history.
- Deterministic fixtures cover normal compaction and split-turn compaction.
- Fresh inbound material is never compacted before a successful model
  invocation observes it; preserving it or failing explicitly with
  `CannotFit` are the only two outcomes.

Deferred to later milestones: durable checkpoint/event storage (M8),
conversation summarization in the CLI (M10), and any provider fallback or
routing. Parallel tool scheduling is implemented by the M5 tool plane PR;
the turn-boundary mailbox drain is implemented in the Issue #22 PR as a
safe-boundary contract.

## Milestone 5 — Native tool plane

Implemented in the M5 tool plane PR (Issue #8):

- The canonical `ToolExecutor` contract and validating `ToolRegistry`
  (definition/executor ownership, registration validation, deterministic
  model-facing ordering, preflight boundary with JSON Schema validation)
- Two independent policy axes: `ToolExecutionPolicy`
  (foreground/background ownership) and `ToolConcurrencyPolicy`
  (sequential/parallel batch scheduling)
- The compiled `ModelToolDefinition` with the reserved `__rustx_`
  invocation namespace (`__rustx_execution` for model-selectable tools)
- `ToolExecutionId` and the conversation-owned
  `ConversationBackgroundRegistry` with the dispatch ownership commit,
  lifecycle state machine, cancel-vs-complete linearization, and
  exactly-once terminal inbound publication
- The `background_task` runtime intrinsic (status and idempotent cancel)
- The runtime-owned Agent Status `background_execution` built-in section
- Native Read, Write, Edit, Glob, Grep, and Bash tools plus the workspace
  boundary, artifact store, and explicit tool environment
- The concrete bounded `NativeToolPolicies` configuration: each ordinary
  native tool independently selects its `NativeToolPolicy` (execution +
  concurrency axes; foreground-only sequential by default), with
  `background_task` fixed foreground-only sequential outside the
  configurable set
- One canonical conversation mailbox owned by the conversation tool
  runtime, drained by the Agent Loop at every safe boundary; a configured
  mailbox must belong to the runtime's own conversation (construction-time
  identity check)
- Artifact storage structurally disjoint from the model workspace
- Deterministic foreground/background scheduling through the agent loop
  with structural batch settlement

Bash requirements:

- Full `/bin/bash`
- Foreground and background execution
- One per-invocation supervisor process unit (outer reaper-of-last-resort
  plus inner session/group leader; both subreapers)
- stdout/stderr/combined capture
- Timeouts
- `TERM -> grace period -> KILL` driven by the invocation supervisor
- Complete lifecycle ownership: shell-parent exit is not the Bash
  settlement boundary. The invocation settles naturally only when the
  shell's terminal status is known, the supervisor reached the kernel
  child-wait terminal state (every owned child reaped; `ECHILD`), and the
  capture is settled; a live owned descendant (pipes held or redirected
  away) keeps the invocation active under the same deadline and
  cancellation until the supervisor's terminal state or
  cancellation/timeout/process-control failure settles it
- Reuse-safe process-group ownership: `TERM`/`KILL` are issued by the
  inner supervisor with `killpg` against its own process group, whose
  numeric id is its own pid — provably allocated while it lives; the final
  signal is the last `killpg`, after which the anchor is released by the
  reap and no further signal exists
- Kernel-mediated descendant ownership: shell descendants that outlive the
  shell are reparented into the invocation supervisor's child domain
  (`PR_SET_CHILD_SUBREAPER`), and the terminal child-set point is the
  supervisor's `waitpid(-1)` loop returning `ECHILD` — never a `/proc`
  membership scan, which is not a linearizable ownership snapshot
- Process-control failures (supervisor setup, shell spawning,
  waiting/reaping, signaling, IPC) are explicit failed results; if
  ownership of a numeric process group can no longer be proven, no further
  signal is issued and the invocation fails explicitly
- Explicit artifact-capture failures instead of silent success
- Large-output truncation with durable full output artifacts
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

The M5 tool plane PR implements the concrete ownership seams required by
the native tool plane: the shared runtime `CancellationSignal`, attempt-owned
cancellable foreground executions (including Bash process-group
termination), conversation-owned background executions with the
`background_task` cancel path, and explicit runtime shutdown cancellation of
active background work. Remaining M9 work:

- Hierarchical runtime supervisor tree and generic process supervision
- Quiescent runtime state machine and graceful draining
- Capability mutation guard and revision snapshots

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

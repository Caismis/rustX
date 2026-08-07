# Architecture

## 1. Architectural objective

rustX is an execution kernel, not an agent application framework and not a control plane. Its responsibility is to execute an immutable runtime manifest, produce durable execution facts, and expose stable runtime-owned contracts to higher-level systems.

The architecture is layered so that external SDKs, storage backends, process managers, and UI protocols can change without rewriting the agent kernel.

## 2. Layer model

### Layer 0: Domain and protocol types

This layer contains runtime-owned data contracts only:

- Message blocks and content blocks
- Model requests and model events
- Tool definitions, calls, and results
- Runtime events
- Runtime manifest
- Attempt, turn, and capability identifiers

It must not depend on provider SDKs, MCP SDKs, databases, HTTP frameworks, or process implementations.

### Layer 1: Agent kernel

The kernel owns deterministic execution semantics:

- Attempt state machine
- Turn lifecycle
- Model -> tool -> model loop
- Tool batch ordering
- Turn-boundary inbound message draining
- Attempt termination rules
- Retry and compaction decision points

The kernel operates only on rustX canonical types and interfaces.

### Layer 2: Context engine

The context engine owns what the model sees:

- Context assembly
- Token accounting
- Context checkpoints
- Pi-style compaction
- Valid compaction cut-point selection
- Split-turn summaries
- Provider-context compilation

Compaction is a projection of durable conversation history. It must never delete or rewrite the canonical history.

### Layer 3: Model plane

The model plane implements protocol adapters:

- OpenAI Chat Completions
- OpenAI Responses
- Anthropic Messages

The first implementation uses native Rust SDKs where practical. SDK-specific request, response, stream, error, and tool types terminate at the adapter boundary.

Each adapter converts between provider SDK types and rustX canonical `ModelRequest` / `ModelEvent` types.

### Layer 4: Tool plane

The tool plane exposes a single runtime-owned execution contract and multiple executor implementations:

- Native tools
- MCP tools
- Custom Python tools
- Platform communication tools such as durable message sending

Execution implementations may depend on `rmcp`, process APIs, `uv`, or other libraries. The agent kernel may not.

### Layer 5: Skill plane

Skills are filesystem/workflow packages. A skill may include:

- `SKILL.md`
- scripts
- references
- assets
- Python dependency declarations
- Node dependency declarations

All active skills in one conversation share one Python environment and one Node environment. Skills use the same native Bash execution capability as the agent.

### Layer 6: Runtime services

This layer owns execution infrastructure:

- Cancellation hierarchy
- Runtime event writer
- Message store interface
- Context checkpoint store interface
- Capability revision management
- Capability mutation guard
- Process supervision
- Background shell session management
- Crash reconciliation

### Layer 7: Interfaces and projections

The outermost layer exposes the runtime to humans and other systems:

- Local interactive CLI
- Runtime command interface
- HTTP control interface
- Runtime event streaming
- AG-UI projection

AG-UI is an output projection, not the internal durable event model.

## 3. Dependency rule

Dependencies point inward.

```text
Interfaces / projections
        |
Runtime services
        |
Model / Tool / Skill implementations
        |
Context engine
        |
Agent kernel
        |
Domain and protocol types
```

Forbidden dependencies include:

```text
Agent kernel -> OpenAI SDK
Agent kernel -> Anthropic SDK
Agent kernel -> rmcp
Agent kernel -> database client
Agent kernel -> HTTP framework
Agent kernel -> control-plane schema
```

## 4. Message model

The canonical conversation model contains four message classes:

```text
SystemMessageBlock
UserMessageBlock
AgentMessageBlock
ToolMessageBlock
```

Semantics:

- `SystemMessageBlock`: trusted instructions or runtime context.
- `UserMessageBlock`: inbound information supplied to the current agent. The source may be a human, another agent, the control plane, or an external system.
- `AgentMessageBlock`: model output produced by the current agent.
- `ToolMessageBlock`: result of a tool call produced by the current agent.

Identity and provenance are metadata. Message role does not encode real-world identity.

Agent-to-agent communication uses a durable mailbox model. A `send_message` tool result reports only whether delivery was durably accepted or rejected. The recipient later receives the content as a `UserMessageBlock`.

## 5. Turn model

A turn is:

```text
one model response
+ all tool calls emitted by that response
+ all corresponding tool results
```

Tool execution may be parallel or sequential. Runtime execution events may follow actual completion order, while canonical tool-result ordering follows the original tool-call order for deterministic context construction.

Inbound mailbox messages may arrive at any time but are injected only at safe turn boundaries.

## 6. Durability model

The runtime distinguishes execution events from conversation messages:

```text
RuntimeEvent = execution fact
MessageBlock = model-context fact
```

Runtime events are append-only. In production, events must be persisted before being published to external subscribers.

Partial model deltas are execution facts. A canonical `AgentMessageBlock` is committed only when a complete model response has been assembled.

## 7. Recovery model

Runtime process memory is disposable.

Recovery uses durable state:

- committed message blocks
- runtime events
- capability revision
- context checkpoints
- workspace state

An unresolved tool call after a crash is never automatically replayed unless the tool explicitly declares an idempotent replay policy. The safe default is to commit an interrupted/unknown tool result and allow the model to decide what to do next.

## 8. Compatibility policy

Before 1.0, rustX intentionally does not preserve compatibility with previous runtimes or flawed abstractions. Breaking changes are preferred when they materially improve correctness, separation of concerns, or long-term maintainability.

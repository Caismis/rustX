/**
 * Protocol-shaped fixtures for the deterministic client suites.
 *
 * These build well-formed Runtime Client values so tests read as protocol
 * scripts rather than as object literals. They are test data only: no fixture
 * here decides semantics, and the real-child integration test exercises the
 * same shapes against bytes the Rust runtime actually wrote.
 */

import type {
  AssistantContentBlock,
  AttemptModelView,
  CapabilityView,
  CatalogModelView,
  ForegroundToolExecution,
  MessageBlock,
  RuntimeClientAttempt,
  InteractionRequest,
  ModelInvocationView,
  RuntimeClientBackgroundExecution,
  RuntimeClientSnapshot,
  SessionModelView,
  ToolExecutionResult,
  UserMessageBlock,
} from "../../src/protocol/types.ts";

export function approvalInteraction(
  id = "attempt-1-interaction-1",
): InteractionRequest {
  return {
    id,
    conversation_id: "conv-test",
    attempt_id: "attempt-1",
    turn: 2,
    kind: {
      type: "approval",
      call_id: "call-1",
      tool_id: "tool-bash",
      tool_name: "bash",
      origin: "builtin",
      mode: "foreground",
      arguments: { command: "printf original" },
      reason: "native policy requires approval",
    },
  };
}

export function invocation(
  model: string,
  overrides: Partial<ModelInvocationView> = {},
): ModelInvocationView {
  return {
    model,
    protocol: "openai_chat_completions",
    contextWindow: 128_000,
    modelMaxOutputTokens: 4_096,
    maxOutputTokens: 4_096,
    reasoningEnabled: false,
    requestParams: {},
    capabilities: {
      inputModalities: ["text"],
      outputModalities: ["text"],
      toolCalls: true,
      reasoning: false,
    },
    declaredCapabilities: {
      inputModalities: ["text"],
      outputModalities: ["text"],
      toolCalls: true,
      reasoning: false,
    },
    ...overrides,
  };
}

export function attemptModel(
  model: string,
  overrides: Partial<ModelInvocationView> = {},
): AttemptModelView {
  return { primary: invocation(model, overrides), summary: { mode: "session" } };
}

export function sessionModel(
  model: string,
  overrides: Partial<ModelInvocationView> = {},
): SessionModelView {
  return {
    configured: { model },
    effective: invocation(model, overrides),
    summary: { mode: "session" },
  };
}

export function capabilities(revision: number): CapabilityView {
  return {
    revision,
    tools: [
      {
        id: "tool-bash",
        name: "bash",
        description: "Run a shell command in the workspace.",
        input_schema: { type: "object" },
        execution_policy: "model_selectable",
        concurrency_policy: "sequential",
        replay_policy: "never",
        origin: "builtin",
      },
      {
        id: "tool-mcp-search",
        name: "search",
        description: "Search an indexed corpus.",
        input_schema: { type: "object" },
        execution_policy: "foreground_only",
        concurrency_policy: "parallel",
        replay_policy: "idempotent",
        origin: { mcp: { server_id: "corpus" } },
      },
    ],
    skills: [
      {
        id: "skill-review",
        version_id: "skill-review@1",
        name: "review",
        description: "Review a change set.",
      },
    ],
  };
}

export function snapshot(
  overrides: Partial<RuntimeClientSnapshot> = {},
): RuntimeClientSnapshot {
  return {
    conversation_id: "conv-test",
    shutting_down: false,
    messages: [],
    inbound: { pending: [] },
    pending_interactions: [],
    background: [],
    context: { compaction_count: 0 },
    capabilities: capabilities(1),
    model: sessionModel("alpha/model-a"),
    ...overrides,
  };
}

/**
 * A bare `UserMessageBlock`.
 *
 * The mailbox events (`inbound_enqueued`) and the pending-inbound view carry
 * this shape, which has no `role` tag: only the canonical `MessageBlock` enum
 * is role-tagged.
 */
export function inboundBlock(id: string, text: string): UserMessageBlock {
  return {
    id,
    content: [{ type: "text", text }],
    source: "human",
    kind: "message",
  };
}

export function userMessage(id: string, text: string) {
  return { role: "user" as const, ...inboundBlock(id, text) };
}

export function runtimeInbound(id: string, text: string) {
  return {
    role: "user" as const,
    id,
    content: [{ type: "text" as const, text }],
    source: "runtime" as const,
    kind: "message" as const,
  };
}

export function assistantMessage(id: string, text: string) {
  return {
    role: "assistant" as const,
    id,
    content: [{ type: "text" as const, text }],
  };
}

export function toolResult(
  overrides: Partial<ToolExecutionResult> = {},
): ToolExecutionResult {
  return {
    status: { type: "success" },
    content: [{ type: "text", text: "ok" }],
    duration_ms: 12,
    ...overrides,
  };
}

export function backgroundExecution(
  executionId: string,
  state: RuntimeClientBackgroundExecution["state"],
  overrides: Partial<RuntimeClientBackgroundExecution> = {},
): RuntimeClientBackgroundExecution {
  return {
    execution_id: executionId,
    tool_id: "tool-background",
    tool_name: "background_task",
    state,
    ...overrides,
  };
}

/** One canonical assistant tool-call block. */
export function toolCallBlock(
  callId: string,
  toolId: string,
  name: string,
  args: unknown,
) {
  return {
    type: "tool_call" as const,
    id: callId,
    tool_id: toolId,
    name,
    arguments: args,
  };
}

/** One committed assistant message with arbitrary canonical blocks. */
export function assistantBlocks(
  id: string,
  content: AssistantContentBlock[],
): MessageBlock {
  return { role: "assistant", id, content };
}

/** One committed canonical system message, with its authority. */
export function systemMessage(
  id: string,
  authority: "platform" | "agent" | "runtime" | "skill" | "fleet",
  text: string,
): MessageBlock {
  return {
    role: "system",
    id,
    authority,
    content: [{ text }],
  };
}

/** One committed canonical tool result message. */
export function toolMessage(
  id: string,
  callId: string,
  toolId: string,
  result: ToolExecutionResult = toolResult(),
): MessageBlock {
  return {
    role: "tool",
    id,
    tool_call_id: callId,
    tool_id: toolId,
    result,
  };
}

/** One attempt view with a foreground execution list. */
export function attemptView(
  overrides: Partial<RuntimeClientAttempt> = {},
): RuntimeClientAttempt {
  return {
    attempt_id: "a1",
    phase: { type: "running" },
    turn: 1,
    model: attemptModel("alpha/model-a"),
    foreground: [],
    ...overrides,
  };
}

/** One foreground execution slot. */
export function foreground(
  callId: string,
  toolId: string,
  name: string,
  state: ForegroundToolExecution["state"],
): ForegroundToolExecution {
  return { call_id: callId, tool_id: toolId, name, state };
}

/** One catalog entry. */
export function catalogModel(
  model: string,
  overrides: Partial<CatalogModelView> = {},
): CatalogModelView {
  return {
    model,
    protocol: "openai_chat_completions",
    contextWindow: 128_000,
    maxOutputTokens: 8_192,
    declaredCapabilities: {
      inputModalities: ["text"],
      outputModalities: ["text"],
      toolCalls: true,
      reasoning: false,
    },
    effectiveCapabilities: {
      inputModalities: ["text"],
      outputModalities: ["text"],
      toolCalls: true,
      reasoning: false,
    },
    credentialSource: { type: "environment", variable: "RUSTX_KEY" },
    ...overrides,
  };
}

/**
 * Protocol-shaped fixtures for the deterministic client suites.
 *
 * These build well-formed Runtime Client values so tests read as protocol
 * scripts rather than as object literals. They are test data only: no fixture
 * here decides semantics, and the real-child integration test exercises the
 * same shapes against bytes the Rust runtime actually wrote.
 */

import type {
  AttemptModelView,
  CapabilityView,
  ModelInvocationView,
  RuntimeClientBackgroundExecution,
  RuntimeClientSnapshot,
  SessionModelView,
  ToolExecutionResult,
  UserMessageBlock,
} from "../../src/protocol/types.ts";

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

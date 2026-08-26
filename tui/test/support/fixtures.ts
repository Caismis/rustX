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
  RuntimeClientTranscriptCursor,
  RuntimeClientCursor,
  RuntimeClientTranscriptPage,
  SessionNodeView,
  SessionView,
  SessionModelView,
  ToolExecutionResult,
  UserMessageBlock,
} from "../../src/protocol/types.ts";

/** Test-only constructors for the two numeric wire cursor domains. */
export function runtimeCursor(value: number): RuntimeClientCursor {
  return value as RuntimeClientCursor;
}

export function transcriptCursor(value: number): RuntimeClientTranscriptCursor {
  return value as RuntimeClientTranscriptCursor;
}

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

export function questionnaireInteraction(
  id = "attempt-1-interaction-question-1",
): InteractionRequest {
  return {
    id,
    conversation_id: "conv-test",
    attempt_id: "attempt-1",
    turn: 3,
    kind: {
      type: "questionnaire",
      questionnaire: {
        questions: [
          {
            question: "Which environment should I use?",
            header: "Environment",
            options: [
              { label: "staging", description: "A safe test environment." },
              { label: "production", description: "The live environment." },
            ],
            multi_select: false,
          },
        ],
      },
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
  const tools = [
    {
      id: "tool-bash",
      name: "bash",
      description: "Run a shell command in the workspace.",
      input_schema: { type: "object" },
      execution_policy: "model_selectable" as const,
      concurrency_policy: "sequential" as const,
      approval_policy: "never" as const,
      replay_policy: "never" as const,
      origin: "builtin" as const,
    },
    {
      id: "tool-mcp-search",
      name: "search",
      description: "Search an indexed corpus.",
      input_schema: { type: "object" },
      execution_policy: "foreground_only" as const,
      concurrency_policy: "parallel" as const,
      approval_policy: "always" as const,
      replay_policy: "idempotent" as const,
      origin: { mcp: { server_id: "corpus" } },
    },
  ];
  return {
    revision,
    tools,
    available_tools: tools,
    skills: [
      {
        id: "skill-review",
        version_id: "skill-review@1",
        name: "review",
        description: "Review a change set.",
        location: ".rustx/skills/review/SKILL.md",
      },
    ],
  };
}

export function snapshot(
  overrides: Partial<RuntimeClientSnapshot> = {},
): RuntimeClientSnapshot {
  const messages = overrides.messages ?? [];
  const transcript: RuntimeClientTranscriptPage =
    overrides.transcript ?? {
      entries: messages.map((message, index) => ({
        cursor: transcriptCursor(index + 1),
        item: { type: "message" as const, message },
      })),
    };
  return {
    conversation_id: "conv-test",
    shutting_down: false,
    effective_approval_mode: "policy",
    approval_mode_revision: 0,
    messages,
    transcript,
    inbound: { pending: [] },
    pending_interactions: [],
    background: [],
    context: { compaction_in_progress: false, compaction_count: 0 },
    capabilities: capabilities(1),
    model: sessionModel("alpha/model-a"),
    ...overrides,
  };
}

export function sessionView(
  overrides: Partial<SessionView> = {},
): SessionView {
  return {
    id: "session-1",
    created_at: "2026-08-21T00:00:00Z",
    updated_at: "2026-08-21T00:00:00Z",
    active_node: "node-1",
    active_conversation_id: "conv-test",
    node_count: 1,
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

export function contextUserMessage(
  id: string,
  text: string,
  context: "runtime_tool_observation" | "extension_environment" | "agent_status" = "agent_status",
) {
  return {
    role: "user" as const,
    id,
    content: [{ type: "text" as const, text }],
    source: "runtime" as const,
    kind: { context } as const,
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

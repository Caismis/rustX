/**
 * Runtime Client Protocol v1 — the TypeScript mirror of the wire contract.
 *
 * These declarations describe the JSON rustX already speaks. They are a
 * *transcription* of the Rust types in `src/runtime_client/`, never a second
 * definition of the semantics:
 *
 * - the runtime owns every value here; the client fills in only the request
 *   parameters the protocol asks it for (a request id, an inbound content
 *   block, a desired `SessionModelConfig`);
 * - nothing in this file interprets a value. `requestParams` stays opaque
 *   provider-owned JSON, capability sets are read as published, and tool
 *   arguments/results are carried, not parsed for meaning;
 * - unknown variants must stay representable, so unions that rustX may extend
 *   are read through explicit discriminator checks rather than exhaustive
 *   assumptions.
 *
 * Field casing follows the Rust `serde` attributes exactly: the protocol
 * envelope, snapshot, and event types are snake_case; the model-configuration
 * types (`SessionModelConfig`, `SessionModelView`, `AttemptModelView`,
 * `ModelInvocationView`, `ModelCatalogView`, `ModelCapabilities`) are
 * camelCase.
 */

export const RUNTIME_CLIENT_PROTOCOL_VERSION_V1 = 1;

// ---------------------------------------------------------------------------
// Identities
//
// Every runtime identity is a transparent string on the wire. They are kept as
// distinct type aliases so a reader can see which domain a value belongs to;
// the client never constructs one.
// ---------------------------------------------------------------------------

export type ConversationId = string;
export type AgentId = string;
export type AttemptId = string;
export type MessageId = string;
export type ToolCallId = string;
export type ToolExecutionId = string;
export type ToolId = string;
export type SkillId = string;
export type SkillVersionId = string;
export type McpServerId = string;
export type ToolVersionId = string;

/** A monotonic capability revision. */
export type CapabilityRevision = number;
/** A position in the Runtime Client observation stream. */
export type RuntimeClientCursor = number;
/** A mailbox-assigned inbound sequence. Not a cursor. */
export type InboundSequence = number;
/** An attachment-scoped request id. Allocated by the connection alone. */
export type RequestId = number;
/** A canonical `provider-id/model-id` reference. */
export type ModelRef = string;
/** A reasoning profile identity declared by the catalog. */
export type ReasoningProfileId = string;
/** A stable index of one content block within a canonical message. */
export type ContentBlockIndex = number;

/**
 * Opaque provider-owned request parameters.
 *
 * rustX preserves these exactly as configured and never interprets them; so
 * does this client. Nothing may branch on a key such as `temperature` or
 * `thinking` — that would make the client a second provider-semantics owner.
 */
export type RequestParams = Record<string, unknown>;

// ---------------------------------------------------------------------------
// Canonical message content
// ---------------------------------------------------------------------------

export interface TextBlock {
  text: string;
}

export interface FileReference {
  [key: string]: unknown;
}

export interface ImageReference {
  [key: string]: unknown;
}

export type UserSource =
  | "human"
  | "fleet"
  | "external_system"
  | "runtime"
  | { agent: { agent_id: AgentId } };

export type InboundKind = "message" | "compaction_summary";

export type UserContentBlock =
  | ({ type: "text" } & TextBlock)
  | ({ type: "image" } & ImageReference)
  | ({ type: "file" } & FileReference);

export interface ReasoningBlock {
  text?: string;
  provider_state?: unknown;
}

export interface RefusalBlock {
  text: string;
}

export interface ToolCall {
  id: ToolCallId;
  tool_id: ToolId;
  name: string;
  arguments: unknown;
}

export interface ToolCallStart {
  id: ToolCallId;
  tool_id: ToolId;
  name: string;
}

export type AgentContentBlock =
  | ({ type: "text" } & TextBlock)
  | ({ type: "reasoning" } & ReasoningBlock)
  | ({ type: "tool_call" } & ToolCall)
  | ({ type: "refusal" } & RefusalBlock)
  | ({ type: "image" } & ImageReference);

export interface SystemMessageBlock {
  id: MessageId;
  authority: "platform" | "agent" | "runtime" | "skill" | "fleet";
  content: TextBlock[];
}

export interface UserMessageBlock {
  id: MessageId;
  content: UserContentBlock[];
  source: UserSource;
  kind?: InboundKind;
  timestamp?: string;
}

export interface AgentMessageBlock {
  id: MessageId;
  content: AgentContentBlock[];
}

export interface ToolMessageBlock {
  id: MessageId;
  tool_call_id: ToolCallId;
  tool_id: ToolId;
  result: ToolExecutionResult;
}

export type MessageBlock =
  | ({ role: "system" } & SystemMessageBlock)
  | ({ role: "user" } & UserMessageBlock)
  | ({ role: "agent" } & AgentMessageBlock)
  | ({ role: "tool" } & ToolMessageBlock);

// ---------------------------------------------------------------------------
// Tool execution
// ---------------------------------------------------------------------------

export type CancellationReason =
  | "user_requested"
  | "runtime_shutdown"
  | "parent_cancelled";

export type ToolExecutionStatus =
  | { type: "success" }
  | { type: "failed"; error: string }
  | { type: "cancelled"; reason: CancellationReason }
  | { type: "timed_out" }
  | { type: "interrupted" };

export type ToolResultContent =
  | ({ type: "text" } & TextBlock)
  | { type: "json"; value: unknown }
  | ({ type: "file" } & FileReference)
  | ({ type: "image" } & ImageReference);

export interface TruncationState {
  truncated: boolean;
  original_bytes?: number;
}

export interface ToolExecutionResult {
  status: ToolExecutionStatus;
  content?: ToolResultContent[];
  duration_ms: number;
  exit_code?: number;
  artifacts?: FileReference[];
  truncation?: TruncationState;
}

export interface ToolProgress {
  message?: string;
  completed?: number;
  total?: number;
}

export type ToolExecutionPolicy =
  | "foreground_only"
  | "background_only"
  | "model_selectable";

export type ToolConcurrencyPolicy = "sequential" | "parallel";

export type ToolReplayPolicy = "never" | "idempotent";

/**
 * Where a tool comes from.
 *
 * This is a *label*. It may pick an icon or a group heading and nothing more:
 * execution semantics are Rust-owned, so no branch may key behaviour to a
 * tool's origin or name.
 */
export type ToolOrigin =
  | "builtin"
  | { mcp: { server_id: McpServerId } }
  | { python: { tool_version_id: ToolVersionId } };

export type BackgroundLifecycle =
  | "starting"
  | "running"
  | "cancelling"
  | "succeeded"
  | "failed"
  | "cancelled";

export const BACKGROUND_TERMINAL_STATES: ReadonlySet<BackgroundLifecycle> =
  new Set<BackgroundLifecycle>(["succeeded", "failed", "cancelled"]);

// ---------------------------------------------------------------------------
// Model configuration (camelCase on the wire)
// ---------------------------------------------------------------------------

export type Modality = "text" | "image" | "file";

export type ModelProtocol =
  | "openai_chat_completions"
  | "openai_responses"
  | "anthropic_messages";

export interface ModelCapabilities {
  inputModalities: Modality[];
  outputModalities: Modality[];
  toolCalls: boolean;
  reasoning: boolean;
}

export interface ModelInvocationView {
  model: ModelRef;
  protocol: ModelProtocol;
  contextWindow: number;
  modelMaxOutputTokens: number;
  maxOutputTokens: number;
  reasoningProfile?: ReasoningProfileId;
  reasoningEnabled: boolean;
  requestParams?: RequestParams;
  /** What the runtime can actually deliver today. Display this. */
  capabilities: ModelCapabilities;
  /** What the catalog claims. Useful only to explain an absent capability. */
  declaredCapabilities: ModelCapabilities;
}

/**
 * The compaction summary model policy.
 *
 * Note the casing: this is a tagged enum whose struct-variant fields are
 * snake_case, unlike the surrounding camelCase model-configuration types.
 */
export type SummaryModelPolicy =
  | { mode: "session" }
  | {
      mode: "explicit";
      model: ModelRef;
      reasoning_profile?: ReasoningProfileId;
      request_params?: RequestParams;
      max_output_tokens?: number;
    };

export type SummaryModelView =
  | { mode: "session" }
  | ({ mode: "explicit" } & ModelInvocationView);

/**
 * The authoritative desired session model configuration.
 *
 * This one shape is the session state, the `model_get` result, and the
 * `model_set` parameter: an update is a whole-state replacement, never a
 * patch. A client that wants to change one field sends back the complete
 * configuration it read.
 */
export interface SessionModelConfig {
  model: ModelRef;
  reasoningProfile?: ReasoningProfileId;
  requestParams?: RequestParams;
  maxOutputTokens?: number;
  summaryModel?: SummaryModelPolicy;
}

export interface SessionModelView {
  configured: SessionModelConfig;
  effective: ModelInvocationView;
  summary: SummaryModelView;
}

/**
 * The immutable model snapshot one admitted attempt froze.
 *
 * Deliberately distinct from {@link SessionModelView}: while an attempt
 * admitted on model A runs and the session has been switched to B, this
 * reports A and the session view reports B.
 */
export interface AttemptModelView {
  primary: ModelInvocationView;
  summary: SummaryModelView;
}

/**
 * Where a provider credential comes from — the *kind*, and for an environment
 * reference the variable *name*. Never a credential value.
 */
export type CredentialSourceView =
  | { type: "literal" }
  | { type: "environment"; variable: string };

export interface ReasoningProfileView {
  id: ReasoningProfileId;
  enabled: boolean;
}

export interface CatalogModelView {
  model: ModelRef;
  protocol: ModelProtocol;
  contextWindow: number;
  maxOutputTokens: number;
  declaredCapabilities: ModelCapabilities;
  effectiveCapabilities: ModelCapabilities;
  reasoningProfiles?: ReasoningProfileView[];
  defaultReasoningProfile?: ReasoningProfileId;
  /** The credential *source*, never a credential value. */
  credentialSource: CredentialSourceView;
}

export interface ModelCatalogView {
  models?: CatalogModelView[];
}

// ---------------------------------------------------------------------------
// Snapshot read model
// ---------------------------------------------------------------------------

export type ModelFinishReason =
  | { type: "stop" }
  | { type: "tool_calls" }
  | { type: "length" }
  | { type: "content_filter" }
  | { type: "refusal" }
  | { type: "other"; reason: string };

export type AttemptLimit = "max_turns" | "max_tool_calls" | "max_runtime_seconds";

export type ModelErrorKind =
  | "invalid_request"
  | "authentication"
  | "rate_limit"
  | "timeout"
  | "transport"
  | "provider_error"
  | "context_window_exceeded"
  | "cancelled"
  | "unsupported";

export interface RuntimeErrorShape {
  type: string;
  message?: string;
  name?: string;
}

export type RuntimeClientAttemptFailure =
  | {
      type: "model";
      kind: ModelErrorKind;
      message: string;
      retry_after_ms?: number;
    }
  | { type: "runtime"; error: RuntimeErrorShape };

export type RuntimeClientOutcome =
  | { type: "completed"; finish_reason: ModelFinishReason }
  | { type: "cancelled"; reason: CancellationReason }
  | { type: "timed_out" }
  | { type: "limit_exceeded"; limit: AttemptLimit }
  | { type: "failed"; error: RuntimeClientAttemptFailure };

export interface UsageDetails {
  reasoning_tokens?: number;
  cached_input_tokens?: number;
}

export interface ModelUsage {
  input_tokens: number;
  output_tokens: number;
  total_tokens: number;
  details?: UsageDetails;
}

export type RuntimeClientAttemptPhase =
  | { type: "admitted" }
  | { type: "running" }
  | { type: "settled"; outcome: RuntimeClientOutcome };

export type InFlightBlock =
  | { type: "text"; block_index: ContentBlockIndex; text: string }
  | { type: "reasoning"; block_index: ContentBlockIndex; text: string }
  | { type: "refusal"; block_index: ContentBlockIndex; text: string }
  | {
      type: "tool_call";
      block_index: ContentBlockIndex;
      call_id: ToolCallId;
      tool_id: ToolId;
      name: string;
      arguments: string;
    };

export interface InFlightAgentMessage {
  message_id: MessageId;
  blocks?: InFlightBlock[];
}

export type ForegroundToolState =
  | { type: "assembled"; arguments: string }
  | { type: "running"; arguments: string; progress?: ToolProgress }
  | { type: "settled"; arguments: string; result: ToolExecutionResult };

export interface ForegroundToolExecution {
  call_id: ToolCallId;
  tool_id: ToolId;
  name: string;
  state: ForegroundToolState;
}

export interface RuntimeClientAttempt {
  attempt_id: AttemptId;
  phase: RuntimeClientAttemptPhase;
  turn: number;
  last_usage?: ModelUsage;
  in_flight?: InFlightAgentMessage;
  foreground?: ForegroundToolExecution[];
  /** The immutable model this attempt froze at admission. */
  model: AttemptModelView;
}

export interface InboundItemView {
  sequence: InboundSequence;
  message: UserMessageBlock;
}

export interface InboundDrainView {
  watermark: InboundSequence;
  count: number;
}

export interface InboundDiagnostics {
  pending?: InboundItemView[];
  last_drain?: InboundDrainView;
}

export interface RuntimeClientBackgroundExecution {
  execution_id: ToolExecutionId;
  tool_id: ToolId;
  tool_name: string;
  state: BackgroundLifecycle;
  progress?: ToolProgress;
  result?: ToolExecutionResult;
}

export interface RuntimeClientStatusFact {
  label: string;
  value: string;
}

export type RuntimeClientStatusSection =
  | {
      type: "temporal";
      current_time: string;
      timezone?: string;
      inbound_message_time: string;
    }
  | {
      type: "background_executions";
      executions: RuntimeClientBackgroundExecution[];
    }
  | { type: "facts"; facts: RuntimeClientStatusFact[] };

export interface AgentStatusView {
  attempt_id: AttemptId;
  turn: number;
  target_message_id: MessageId;
  sections?: RuntimeClientStatusSection[];
  /** The canonical rendering, derived from the same composition as sections. */
  rendered: string;
}

export interface RuntimeClientTool {
  id: ToolId;
  name: string;
  description: string;
  input_schema: unknown;
  execution_policy: ToolExecutionPolicy;
  concurrency_policy: ToolConcurrencyPolicy;
  replay_policy: ToolReplayPolicy;
  origin: ToolOrigin;
}

export interface RuntimeClientSkill {
  id: SkillId;
  version_id: SkillVersionId;
  name: string;
  description: string;
}

export interface CapabilityView {
  revision: CapabilityRevision;
  tools?: RuntimeClientTool[];
  skills?: RuntimeClientSkill[];
}

export interface RuntimeClientSnapshot {
  conversation_id: ConversationId;
  messages: MessageBlock[];
  attempt?: RuntimeClientAttempt;
  inbound: InboundDiagnostics;
  background?: RuntimeClientBackgroundExecution[];
  status?: AgentStatusView;
  capabilities: CapabilityView;
  /** The session's *desired* model. Never the running attempt's model. */
  model: SessionModelView;
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

export type RuntimeClientEvent =
  | {
      type: "attempt_started";
      attempt_id: AttemptId;
      /**
       * The immutable model the attempt froze at admission. The event is
       * self-contained, so an incremental client never infers the active
       * attempt's model and never needs a second `snapshot_get`.
       */
      model: AttemptModelView;
    }
  | {
      type: "attempt_settled";
      attempt_id: AttemptId;
      outcome: RuntimeClientOutcome;
    }
  | {
      type: "assistant_message_started";
      attempt_id: AttemptId;
      message_id: MessageId;
    }
  | {
      type: "assistant_text_delta";
      attempt_id: AttemptId;
      message_id: MessageId;
      block_index: ContentBlockIndex;
      delta: string;
    }
  | {
      type: "assistant_reasoning_delta";
      attempt_id: AttemptId;
      message_id: MessageId;
      block_index: ContentBlockIndex;
      delta: string;
    }
  | {
      type: "assistant_refusal_delta";
      attempt_id: AttemptId;
      message_id: MessageId;
      block_index: ContentBlockIndex;
      delta: string;
    }
  | {
      type: "tool_call_started";
      attempt_id: AttemptId;
      message_id: MessageId;
      block_index: ContentBlockIndex;
      call: ToolCallStart;
    }
  | {
      type: "tool_call_arguments_delta";
      attempt_id: AttemptId;
      message_id: MessageId;
      block_index: ContentBlockIndex;
      call_id: ToolCallId;
      arguments_delta: string;
    }
  | {
      type: "tool_call_assembled";
      attempt_id: AttemptId;
      message_id: MessageId;
      block_index: ContentBlockIndex;
      call: ToolCall;
    }
  | {
      type: "tool_execution_started";
      attempt_id: AttemptId;
      tool_call_id: ToolCallId;
      tool_id: ToolId;
    }
  | {
      type: "tool_execution_progress";
      attempt_id: AttemptId;
      tool_call_id: ToolCallId;
      tool_id: ToolId;
      execution_id?: ToolExecutionId;
      progress: ToolProgress;
    }
  | {
      type: "tool_execution_settled";
      attempt_id: AttemptId;
      tool_call_id: ToolCallId;
      tool_id: ToolId;
      result: ToolExecutionResult;
    }
  | {
      type: "message_committed";
      attempt_id?: AttemptId;
      message: MessageBlock;
    }
  | {
      type: "agent_status_composed";
      attempt_id: AttemptId;
      turn: number;
      target_message_id: MessageId;
      status: AgentStatusView;
    }
  | {
      type: "inbound_enqueued";
      sequence: InboundSequence;
      message: UserMessageBlock;
    }
  | {
      type: "inbound_drained";
      watermark: InboundSequence;
      count: number;
      message_ids: MessageId[];
    }
  | {
      type: "background_execution_updated";
      execution: RuntimeClientBackgroundExecution;
    }
  | { type: "capability_published"; capabilities: CapabilityView }
  | { type: "session_model_changed"; model: SessionModelView }
  | { type: "runtime_shutdown" };

export interface RuntimeClientProtocolEvent {
  cursor: RuntimeClientCursor;
  event: RuntimeClientEvent;
}

// ---------------------------------------------------------------------------
// Requests, results, errors
// ---------------------------------------------------------------------------

export type RuntimeClientRequest =
  | { method: "initialize"; id: RequestId; protocol_version: number }
  | { method: "submit_inbound"; id: RequestId; content: UserContentBlock[] }
  | { method: "cancel_current_attempt"; id: RequestId }
  | { method: "snapshot_get"; id: RequestId }
  | {
      method: "subscribe_events";
      id: RequestId;
      after_cursor: RuntimeClientCursor;
    }
  | { method: "capability_get"; id: RequestId }
  | { method: "model_catalog_get"; id: RequestId }
  | { method: "model_get"; id: RequestId }
  | { method: "model_set"; id: RequestId; config: SessionModelConfig }
  | {
      method: "background_status";
      id: RequestId;
      execution_id: ToolExecutionId;
    }
  | {
      method: "background_cancel";
      id: RequestId;
      execution_id: ToolExecutionId;
    }
  | { method: "detach"; id: RequestId }
  | { method: "shutdown"; id: RequestId };

export type RuntimeClientMethod = RuntimeClientRequest["method"];

/** A request without its id: the connection is the sole id allocator. */
export type RuntimeClientRequestBody =
  | Omit<Extract<RuntimeClientRequest, { method: "initialize" }>, "id">
  | Omit<Extract<RuntimeClientRequest, { method: "submit_inbound" }>, "id">
  | Omit<
      Extract<RuntimeClientRequest, { method: "cancel_current_attempt" }>,
      "id"
    >
  | Omit<Extract<RuntimeClientRequest, { method: "snapshot_get" }>, "id">
  | Omit<Extract<RuntimeClientRequest, { method: "subscribe_events" }>, "id">
  | Omit<Extract<RuntimeClientRequest, { method: "capability_get" }>, "id">
  | Omit<Extract<RuntimeClientRequest, { method: "model_catalog_get" }>, "id">
  | Omit<Extract<RuntimeClientRequest, { method: "model_get" }>, "id">
  | Omit<Extract<RuntimeClientRequest, { method: "model_set" }>, "id">
  | Omit<Extract<RuntimeClientRequest, { method: "background_status" }>, "id">
  | Omit<Extract<RuntimeClientRequest, { method: "background_cancel" }>, "id">
  | Omit<Extract<RuntimeClientRequest, { method: "detach" }>, "id">
  | Omit<Extract<RuntimeClientRequest, { method: "shutdown" }>, "id">;

export type RuntimeClientResult =
  | {
      type: "initialized";
      attachment_id: string;
      conversation_id: ConversationId;
      agent_id: AgentId;
      snapshot: RuntimeClientSnapshot;
      cursor: RuntimeClientCursor;
    }
  | {
      type: "inbound_accepted";
      message_id: MessageId;
      inbound_sequence: InboundSequence;
    }
  | { type: "attempt_cancellation_accepted"; attempt_id: AttemptId }
  | {
      type: "snapshot";
      snapshot: RuntimeClientSnapshot;
      cursor: RuntimeClientCursor;
    }
  | { type: "subscribed"; after_cursor: RuntimeClientCursor }
  | { type: "capability"; capabilities: CapabilityView }
  | { type: "model_catalog"; catalog: ModelCatalogView }
  | { type: "model"; model: SessionModelView }
  | { type: "model_set"; model: SessionModelView }
  | {
      type: "background_status";
      execution: RuntimeClientBackgroundExecution;
    }
  | {
      type: "background_cancel_accepted";
      execution: RuntimeClientBackgroundExecution;
    }
  | { type: "detached" }
  | { type: "shutdown_accepted" };

export type RuntimeClientError =
  | { type: "unsupported_protocol_version"; supported: number; requested: number }
  | { type: "attachment_in_use"; existing_attachment_id: string }
  | { type: "not_attached" }
  | { type: "invalid_request"; message: string }
  | { type: "no_current_attempt" }
  | { type: "unknown_background_execution"; execution_id: ToolExecutionId }
  | {
      type: "resync_required";
      after_cursor: RuntimeClientCursor;
      earliest_serviceable: RuntimeClientCursor;
    }
  | { type: "runtime_shutdown" }
  | { type: "invalid_state"; message: string }
  | { type: "invalid_model_configuration"; message: string }
  | { type: "projection_exhausted" }
  | { type: "runtime_failure"; message: string };

export interface RuntimeClientResponse {
  id: RequestId;
  result?: RuntimeClientResult;
  error?: RuntimeClientError;
}

/** One record rustX writes on its transport output stream. */
export type RuntimeClientOutboundRecord =
  | RuntimeClientResponse
  | RuntimeClientProtocolEvent;

/**
 * Classifies one decoded outbound record.
 *
 * A notification structurally carries no request id, and a response always
 * carries one — that is the protocol's own discriminator, so no heuristic is
 * needed.
 */
export function isProtocolEvent(
  record: RuntimeClientOutboundRecord,
): record is RuntimeClientProtocolEvent {
  return (
    typeof (record as RuntimeClientProtocolEvent).cursor === "number" &&
    typeof (record as RuntimeClientProtocolEvent).event === "object" &&
    (record as RuntimeClientProtocolEvent).event !== null
  );
}

/** A human-readable rendering of one typed protocol error. */
export function describeProtocolError(error: RuntimeClientError): string {
  switch (error.type) {
    case "unsupported_protocol_version":
      return `the runtime speaks Runtime Client Protocol v${error.supported}, this client asked for v${error.requested}`;
    case "attachment_in_use":
      return `another client is attached (${error.existing_attachment_id})`;
    case "not_attached":
      return "the request arrived without an admitted attachment";
    case "invalid_request":
      return `invalid request: ${error.message}`;
    case "no_current_attempt":
      return "no attempt is currently cancellable";
    case "unknown_background_execution":
      return `unknown background execution ${error.execution_id}`;
    case "resync_required":
      return `the stream after cursor ${error.after_cursor} is no longer serviceable (earliest ${error.earliest_serviceable})`;
    case "runtime_shutdown":
      return "the runtime is shutting down and no longer admits inbound work";
    case "invalid_state":
      return `invalid state: ${error.message}`;
    case "invalid_model_configuration":
      return `the model configuration was rejected: ${error.message}`;
    case "projection_exhausted":
      return "the runtime observation stream is exhausted";
    case "runtime_failure":
      return `runtime failure: ${error.message}`;
    default:
      // A future runtime may add a category. Report it rather than crash.
      return `unrecognized protocol error: ${JSON.stringify(error)}`;
  }
}

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
 * - Runtime Client Protocol v1 event discriminators are a closed vocabulary at
 *   the connection boundary: an unknown event is a protocol error, not a
 *   presentation fact; other open values remain opaque or are checked at
 *   their owning boundary.
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
export type TurnId = string;
export type MessageId = string;
export type ToolCallId = string;
export type ToolExecutionId = string;
export type SubagentId = string;
export type ToolId = string;
export type SkillId = string;
export type SkillVersionId = string;
export type McpServerId = string;
export type ToolVersionId = string;
export type SessionId = string;
export type SessionNodeId = string;

/** A monotonic capability revision. */
export type CapabilityRevision = number;
/** A position in the Runtime Client observation stream. */
declare const runtimeClientCursorBrand: unique symbol;
export type RuntimeClientCursor = number & {
  readonly [runtimeClientCursorBrand]: "runtime-client-cursor";
};
/** A position in the durable derived transcript. Not a Runtime Client cursor. */
declare const runtimeClientTranscriptCursorBrand: unique symbol;
export type RuntimeClientTranscriptCursor = number & {
  readonly [runtimeClientTranscriptCursorBrand]: "transcript-cursor";
};
/** An immutable Conversation Surface revision selected for a seed. */
export type SurfaceRevision = number;
/** A mailbox-assigned inbound sequence. Not a cursor. */
export type InboundSequence = number;
/** An attachment-scoped request id. Allocated by the connection alone. */
export type RequestId = number;
/** A canonical `provider-id/model-id` reference; the model ID may contain `/`. */
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

export type InboundKind =
  | "message"
  | "compaction_summary"
  | {
      context:
        | "runtime_tool_observation"
        | "extension_environment"
        | "agent_status";
    };

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

export type AssistantContentBlock =
  | ({ type: "text" } & TextBlock)
  | ({ type: "reasoning" } & ReasoningBlock)
  | ({ type: "tool_call" } & ToolCall)
  | ({ type: "refusal" } & RefusalBlock)
  | ({ type: "image" } & ImageReference);

export interface UserMessageBlock {
  id: MessageId;
  content: UserContentBlock[];
  source: UserSource;
  kind?: InboundKind;
  timestamp?: string;
}

export interface AssistantMessageBlock {
  id: MessageId;
  content: AssistantContentBlock[];
}

export interface ToolMessageBlock {
  id: MessageId;
  tool_call_id: ToolCallId;
  tool_id: ToolId;
  result: ToolExecutionResult;
}

export type MessageBlock =
  | ({ role: "user" } & UserMessageBlock)
  | ({ role: "assistant" } & AssistantMessageBlock)
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
  | { type: "denied"; reason: string }
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

export type ToolApprovalPolicy = "never" | "always";

/** Runtime-wide approval control. `full_access` only bypasses approval. */
export type ApprovalMode = "policy" | "full_access";

export type ToolInvocationMode = "foreground" | "background";

export type ToolReplayPolicy = "never" | "idempotent";

/**
 * Where a tool comes from.
 *
 * A *presentation* fact:
 *
 * > Tool identity and origin may select presentation.
 * > Tool identity and origin may never select or infer execution semantics.
 *
 * So an origin or a `ToolId` may pick a label, a group heading, or a
 * specialized presentation renderer — the reason a Bash call reads as
 * `$ cargo test --all` instead of argument JSON. It may never decide whether
 * a call is running, succeeded, failed, was denied, cancelled, timed out or
 * interrupted, may never change what is executed or how, and may never alter
 * approval, concurrency, or replay behaviour. Those are Rust-owned and reach
 * the client only as published facts.
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

/** The lifecycle states of a subagent child (Issue #60). */
export type SubagentState =
  | "running"
  | "cancelling"
  | "publishing_terminal"
  | "succeeded"
  | "failed"
  | "cancelled";

export const BACKGROUND_TERMINAL_STATES: ReadonlySet<BackgroundLifecycle> =
  new Set<BackgroundLifecycle>(["succeeded", "failed", "cancelled"]);

// ---------------------------------------------------------------------------
// Native interaction projection
// ---------------------------------------------------------------------------

export type InteractionId = string;

export type ApprovalDecision =
  | { type: "allow" }
  | { type: "deny"; reason: string };

export type QuestionAnswer =
  | { type: "choice"; value: string }
  | { type: "free_text"; value: string };

export type InteractionResponse =
  | {
      type: "approval";
      decision: ApprovalDecision;
    }
  | {
      type: "question";
      answer: QuestionAnswer;
    };

export type InteractionRequest = {
  id: InteractionId;
  conversation_id: ConversationId;
  attempt_id: AttemptId;
  turn: number;
  kind:
    | {
        type: "approval";
        call_id: ToolCallId;
        tool_id: ToolId;
        tool_name: string;
        origin: ToolOrigin;
        mode: ToolInvocationMode;
        arguments: unknown;
        reason: string;
      }
    | {
        type: "question";
        prompt: string;
        choices?: string[];
        allow_free_text: boolean;
      };
};

export type InteractionOutcome =
  | { type: "answered"; response: InteractionResponse }
  | { type: "cancelled"; reason: CancellationReason }
  | { type: "unavailable" };

/** The bounded by-value subject retained by the durable interaction audit. */
export type InteractionSubject =
  | {
      type: "approval";
      call_id: ToolCallId;
      tool_id: ToolId;
      tool_name: string;
      arguments_digest: string;
      reason: string;
    }
  | {
      type: "question";
      prompt: string;
      choices?: string[];
      allow_free_text: boolean;
    };

/** The terminal value retained by the durable interaction audit. */
export type InteractionSettlement =
  | { type: "approved" }
  | { type: "denied"; reason: string }
  | { type: "answered"; answer: QuestionAnswer }
  | { type: "cancelled"; reason: CancellationReason };

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
// Native Session product view
// ---------------------------------------------------------------------------

export type SessionNodeOrigin =
  | { type: "new" }
  | {
      type: "clone";
      source_session: SessionId;
      source_node: SessionNodeId;
      source_surface_revision: SurfaceRevision;
    }
  | {
      type: "fork";
      source_session: SessionId;
      source_node: SessionNodeId;
      source_surface_revision: SurfaceRevision;
      source_user_message: MessageId;
    };

export interface SessionNodeView {
  id: SessionNodeId;
  parent?: SessionNodeId;
  conversation_id: ConversationId;
  origin: SessionNodeOrigin;
}

export interface SessionView {
  id: SessionId;
  name: string;
  created_at: string;
  updated_at: string;
  active_node: SessionNodeId;
  active_conversation_id: ConversationId;
  node_count: number;
}

export interface SessionSummaryView {
  id: SessionId;
  name: string;
  updated_at: string;
  active_node: SessionNodeId;
  active: boolean;
}

export interface SessionUserMessageBoundaryView {
  surface_revision: SurfaceRevision;
  message: UserMessageBlock;
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

export interface InFlightAssistantMessage {
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
  in_flight?: InFlightAssistantMessage;
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

/** The Runtime Client view of one subagent child (Issue #60). */
export interface RuntimeClientSubagent {
  subagent_id: SubagentId;
  child_agent_id: AgentId;
  child_conversation_id: ConversationId;
  profile: string;
  state: SubagentState;
  detail?: string;
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
  approval_policy: ToolApprovalPolicy;
  replay_policy: ToolReplayPolicy;
  origin: ToolOrigin;
}

export interface RuntimeClientSkill {
  id: SkillId;
  version_id: SkillVersionId;
  name: string;
  description: string;
  location: string;
}

export interface CapabilityView {
  revision: CapabilityRevision;
  /** The active model-visible Tools; provider requests use exactly this set. */
  tools?: RuntimeClientTool[];
  /** Every currently available Tool, including inactive Tools. */
  available_tools?: RuntimeClientTool[];
  /** The model-visible Skill catalog; hidden runtime Skills are omitted. */
  skills?: RuntimeClientSkill[];
  /** Typed availability of every evaluated optional capability source. */
  sources?: CapabilitySourceView[];
}

/** The identity of one optional capability source (Issue #81). */
export type CapabilitySourceDescriptor =
  | { type: "python" }
  | { type: "mcp"; server_id: McpServerId };

/** The availability of one optional capability source. */
export type CapabilitySourceStateView =
  | { type: "ready" }
  | { type: "unavailable"; reason: string };

export interface CapabilitySourceView {
  source: CapabilitySourceDescriptor;
  state: CapabilitySourceStateView;
}

export type TokenMeasurementSource = "provider_reported" | "estimated";

export interface TokenMeasurement {
  input_tokens: number;
  source: TokenMeasurementSource;
}

export interface RuntimeClientCompactionView {
  /** The compaction generation, derived from Conversation Surface history. */
  generation: number;
  /**
   * The identity of the committed canonical compaction summary. Its content
   * is an ordinary Message Ledger fact in `RuntimeClientSnapshot.messages`.
   */
  summary_message_id: MessageId;
  /** The Conversation Surface revision the rewrite established. */
  surface_revision: number;
  tokens_before: TokenMeasurement;
  estimated_tokens_after: number;
}

export interface RuntimeClientContextView {
  compaction_in_progress: boolean;
  compaction_count: number;
  latest_compaction?: RuntimeClientCompactionView;
}

export interface RuntimeClientSnapshot {
  conversation_id: ConversationId;
  shutting_down: boolean;
  effective_approval_mode: ApprovalMode;
  pending_approval_mode?: ApprovalMode;
  approval_mode_revision: number;
  messages: MessageBlock[];
  /** The bounded newest page of durable transcript history. */
  transcript: RuntimeClientTranscriptPage;
  attempt?: RuntimeClientAttempt;
  inbound: InboundDiagnostics;
  /** Live runtime-owned interactions; never client-owned approval truth. */
  pending_interactions: InteractionRequest[];
  background?: RuntimeClientBackgroundExecution[];
  subagents?: RuntimeClientSubagent[];
  status?: AgentStatusView;
  context: RuntimeClientContextView;
  capabilities: CapabilityView;
  /** The session's *desired* model. Never the running attempt's model. */
  model: SessionModelView;
}

export type RuntimeClientTranscriptItem =
  | { type: "message"; message: MessageBlock }
  | { type: "publication_audit"; audit: PublicationAudit }
  | {
      type: "interaction_requested";
      event_id: string;
      timestamp: string;
      attempt_id: AttemptId;
      turn_id: TurnId;
      interaction_id: InteractionId;
      subject: InteractionSubject;
    }
  | {
      type: "interaction_settled";
      event_id: string;
      timestamp: string;
      attempt_id: AttemptId;
      turn_id: TurnId;
      interaction_id: InteractionId;
      settlement: InteractionSettlement;
    };

export interface RuntimeClientTranscriptEntry {
  cursor: RuntimeClientTranscriptCursor;
  item: RuntimeClientTranscriptItem;
}

export interface RuntimeClientTranscriptPage {
  entries: RuntimeClientTranscriptEntry[];
  next_cursor?: RuntimeClientTranscriptCursor;
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/**
 * One consolidated block of a settled publication audit.
 *
 * `proposed_tool_call` is a model proposal the runtime committed for release.
 * It is never evidence that the call was authorized, started, or executed.
 */
export type PublicationAuditBlock =
  | { kind: "text"; block_index: ContentBlockIndex; text: string }
  | { kind: "reasoning"; block_index: ContentBlockIndex; text: string }
  | { kind: "refusal"; block_index: ContentBlockIndex; text: string }
  | {
      kind: "proposed_tool_call";
      block_index: ContentBlockIndex;
      call_id: ToolCallId;
      tool_id: ToolId;
      name: string;
      /** The released raw argument text, exactly as far as it was released. */
      arguments: string;
      /** Whether the proposal finished assembling before the stream ended. */
      complete: boolean;
    };

/** Why a publication stream settled without canonical acceptance. */
export type PublicationAuditKind = "unaccepted" | "incomplete";

/** The bounded immutable audit of one settled publication stream. */
export interface PublicationAudit {
  stream_id: string;
  attempt_id: AttemptId;
  turn_id: string;
  request_id: string;
  message_id: MessageId;
  kind: PublicationAuditKind;
  content: PublicationAuditBlock[];
  settled_at: string;
}

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
      type: "attempt_turn_updated";
      attempt_id: AttemptId;
      turn: number;
    }
  | {
      type: "attempt_usage_updated";
      attempt_id: AttemptId;
      usage: ModelUsage;
    }
  | { type: "interaction_pending"; interaction: InteractionRequest }
  | {
      type: "interaction_settled";
      interaction_id: InteractionId;
      outcome: InteractionOutcome;
    }
  | {
      type: "interaction_audit_requested";
      transcript_cursor: RuntimeClientTranscriptCursor;
      audit: {
        event_id: string;
        timestamp: string;
        attempt_id: AttemptId;
        turn_id: TurnId;
        interaction_id: InteractionId;
        subject: InteractionSubject;
      };
    }
  | {
      type: "interaction_audit_settled";
      transcript_cursor: RuntimeClientTranscriptCursor;
      audit: {
        event_id: string;
        timestamp: string;
        attempt_id: AttemptId;
        turn_id: TurnId;
        interaction_id: InteractionId;
        settlement: InteractionSettlement;
      };
    }
  | {
      type: "approval_mode_changed";
      effective_approval_mode: ApprovalMode;
      pending_approval_mode?: ApprovalMode;
      revision: number;
    }
  | {
      type: "context_compaction_started";
      attempt_id?: AttemptId;
    }
  | {
      type: "context_compaction_failed";
      attempt_id?: AttemptId;
      error: string;
    }
  | {
      type: "context_compacted";
      attempt_id?: AttemptId;
      context: RuntimeClientContextView;
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
      /**
       * The in-flight Assistant publication settled without ever becoming a
       * canonical Assistant message (Issue #108).
       *
       * The carried audit is what the runtime durably committed **for
       * release**: an upper bound on what this client may have displayed,
       * never proof that anything was perceived. Its `proposed_tool_call`
       * entries are model proposals that were never authorized and never
       * executed, so they must never be presented as the
       * `tool_execution_started` / `tool_execution_settled` Tool Plane facts,
       * or imply side effects. The audit is a noncanonical derived transcript
       * item, not a Message Ledger message.
       */
      type: "assistant_publication_settled";
      attempt_id: AttemptId;
      audit: PublicationAudit;
      transcript_cursor: RuntimeClientTranscriptCursor;
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
      transcript_cursor?: RuntimeClientTranscriptCursor;
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
      transcript_cursor?: RuntimeClientTranscriptCursor;
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
  | {
      type: "subagent_updated";
      subagent: RuntimeClientSubagent;
    }
  | { type: "capability_updated"; capabilities: CapabilityView }
  | { type: "session_model_changed"; model: SessionModelView }
  | { type: "runtime_shutdown" };

/** Whether a User message is a hidden Context fact rather than transcript content. */
export function isHiddenContextMessage(message: {
  role?: unknown;
  kind?: unknown;
}): boolean {
  const isUserMessage = message.role === undefined || message.role === "user";
  return (
    isUserMessage &&
    typeof message.kind === "object" &&
    message.kind !== null &&
    "context" in message.kind
  );
}

function isWireTranscriptCursor(value: unknown): value is RuntimeClientTranscriptCursor {
  return (
    typeof value === "number" &&
    Number.isSafeInteger(value) &&
    value >= 0
  );
}

/** Checks the one visible/hidden transcript-cursor contract at the wire boundary. */
export function hasTranscriptCursorContract(
  message: unknown,
  transcriptCursor: unknown,
): boolean {
  if (typeof message !== "object" || message === null) {
    return false;
  }
  if (isHiddenContextMessage(message as { role?: unknown; kind?: unknown })) {
    return transcriptCursor === undefined;
  }
  return isWireTranscriptCursor(transcriptCursor);
}

/**
 * Validates a transcript-visible message before presentation reduction.
 *
 * Hidden Context messages may omit the cursor and never enter the ordinary
 * transcript. Every other message must carry the durable cursor allocated by
 * `transcript_order`; contradictory hidden-with-cursor facts fail closed.
 */
export function validateTranscriptCursorContract(
  message: { role?: unknown; kind?: unknown },
  transcriptCursor: RuntimeClientTranscriptCursor | undefined,
): RuntimeClientTranscriptCursor | undefined {
  if (isHiddenContextMessage(message)) {
    if (transcriptCursor !== undefined) {
      throw new Error(
        "hidden Context message must not carry a durable transcript cursor",
      );
    }
    return undefined;
  }
  if (!isWireTranscriptCursor(transcriptCursor)) {
    throw new Error(
      "visible transcript message is missing a valid durable transcript cursor",
    );
  }
  return transcriptCursor;
}

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
  | { method: "compact_context"; id: RequestId }
  | { method: "reload_resources"; id: RequestId }
  | {
      method: "interaction_respond";
      id: RequestId;
      interaction_id: InteractionId;
      response: InteractionResponse;
    }
  | { method: "snapshot_get"; id: RequestId }
  | {
      method: "transcript_page_get";
      id: RequestId;
      before_cursor?: RuntimeClientTranscriptCursor;
      limit: number;
    }
  | {
      method: "subscribe_events";
      id: RequestId;
      after_cursor: RuntimeClientCursor;
    }
  | { method: "capability_get"; id: RequestId }
  | { method: "model_catalog_get"; id: RequestId }
  | { method: "model_get"; id: RequestId }
  | { method: "model_set"; id: RequestId; config: SessionModelConfig }
  | { method: "approval_mode_set"; id: RequestId; mode: ApprovalMode }
  | {
      method: "session_list";
      id: RequestId;
      query?: string;
      offset: number;
      limit: number;
    }
  | { method: "session_get"; id: RequestId }
  | {
      method: "session_tree_get";
      id: RequestId;
      node_offset: number;
      history_offset: number;
      limit: number;
    }
  | { method: "session_name"; id: RequestId; name: string }
  | { method: "session_new"; id: RequestId }
  | {
      method: "session_select";
      id: RequestId;
      session_id: SessionId;
      node_id?: SessionNodeId;
    }
  | { method: "session_clone"; id: RequestId }
  | {
      method: "session_fork";
      id: RequestId;
      surface_revision: SurfaceRevision;
      message_id: MessageId;
    }
  | {
      method: "session_tree_branch";
      id: RequestId;
      surface_revision: SurfaceRevision;
      message_id: MessageId;
    }
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
  | {
      method: "subagent_status";
      id: RequestId;
      subagent_id: SubagentId;
    }
  | {
      method: "subagent_cancel";
      id: RequestId;
      subagent_id: SubagentId;
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
  | Omit<Extract<RuntimeClientRequest, { method: "compact_context" }>, "id">
  | Omit<Extract<RuntimeClientRequest, { method: "reload_resources" }>, "id">
  | Omit<
      Extract<RuntimeClientRequest, { method: "interaction_respond" }>,
      "id"
    >
  | Omit<Extract<RuntimeClientRequest, { method: "snapshot_get" }>, "id">
  | Omit<Extract<RuntimeClientRequest, { method: "transcript_page_get" }>, "id">
  | Omit<Extract<RuntimeClientRequest, { method: "subscribe_events" }>, "id">
  | Omit<Extract<RuntimeClientRequest, { method: "capability_get" }>, "id">
  | Omit<Extract<RuntimeClientRequest, { method: "model_catalog_get" }>, "id">
  | Omit<Extract<RuntimeClientRequest, { method: "model_get" }>, "id">
  | Omit<Extract<RuntimeClientRequest, { method: "model_set" }>, "id">
  | Omit<Extract<RuntimeClientRequest, { method: "approval_mode_set" }>, "id">
  | Omit<Extract<RuntimeClientRequest, { method: "session_list" }>, "id">
  | Omit<Extract<RuntimeClientRequest, { method: "session_get" }>, "id">
  | Omit<Extract<RuntimeClientRequest, { method: "session_tree_get" }>, "id">
  | Omit<Extract<RuntimeClientRequest, { method: "session_name" }>, "id">
  | Omit<Extract<RuntimeClientRequest, { method: "session_new" }>, "id">
  | Omit<Extract<RuntimeClientRequest, { method: "session_select" }>, "id">
  | Omit<Extract<RuntimeClientRequest, { method: "session_clone" }>, "id">
  | Omit<Extract<RuntimeClientRequest, { method: "session_fork" }>, "id">
  | Omit<Extract<RuntimeClientRequest, { method: "session_tree_branch" }>, "id">
  | Omit<Extract<RuntimeClientRequest, { method: "background_status" }>, "id">
  | Omit<Extract<RuntimeClientRequest, { method: "background_cancel" }>, "id">
  | Omit<Extract<RuntimeClientRequest, { method: "subagent_status" }>, "id">
  | Omit<Extract<RuntimeClientRequest, { method: "subagent_cancel" }>, "id">
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
  | { type: "context_compacted"; context: RuntimeClientContextView }
  | {
      type: "resources_reloaded";
      resource_revision: number;
      capability_revision: CapabilityRevision;
    }
  | { type: "interaction_response_accepted"; interaction_id: InteractionId }
  | {
      type: "snapshot";
      snapshot: RuntimeClientSnapshot;
      cursor: RuntimeClientCursor;
    }
  | { type: "transcript_page"; page: RuntimeClientTranscriptPage }
  | { type: "subscribed"; after_cursor: RuntimeClientCursor }
  | { type: "capability"; capabilities: CapabilityView }
  | { type: "model_catalog"; catalog: ModelCatalogView }
  | { type: "model"; model: SessionModelView }
  | { type: "model_set"; model: SessionModelView }
  | {
      type: "approval_mode_set";
      effective_approval_mode: ApprovalMode;
      pending_approval_mode?: ApprovalMode;
      revision: number;
    }
  | {
      type: "session_list";
      sessions: SessionSummaryView[];
      next_offset?: number;
    }
  | { type: "session"; session: SessionView }
  | {
      type: "session_tree";
      session: SessionView;
      nodes: SessionNodeView[];
      next_node_offset?: number;
      branchable_messages: SessionUserMessageBoundaryView[];
      next_history_offset?: number;
    }
  | {
      type: "session_changed";
      session: SessionView;
      /** Transient fork/tree editor content; not canonical history. */
      editor_content?: UserContentBlock[];
      restart_required: boolean;
    }
  | {
      type: "session_committed_restart_required";
      session: SessionView;
      /** Restore only after restart confirms this Session/node. */
      editor_content?: UserContentBlock[];
      diagnostic: string;
    }
  | {
      type: "background_status";
      execution: RuntimeClientBackgroundExecution;
    }
  | {
      type: "background_cancel_accepted";
      execution: RuntimeClientBackgroundExecution;
    }
  | {
      type: "subagent_status";
      subagent: RuntimeClientSubagent;
    }
  | {
      type: "subagent_cancel_accepted";
      subagent: RuntimeClientSubagent;
    }
  | { type: "detached" }
  | { type: "shutdown_completed" };

export type RuntimeClientError =
  | { type: "unsupported_protocol_version"; supported: number; requested: number }
  | { type: "attachment_in_use"; existing_attachment_id: string }
  | { type: "not_attached" }
  | { type: "invalid_request"; message: string }
  | { type: "no_current_attempt" }
  | { type: "resource_reload_busy"; reason: string }
  | { type: "interaction_not_pending"; interaction_id: InteractionId }
  | { type: "interaction_invalid_response"; message: string }
  | { type: "interaction_audit_failed"; interaction_id: InteractionId }
  | { type: "approval_mode_inactive" }
  | { type: "approval_mode_durability_failed"; message: string }
  | { type: "unknown_background_execution"; execution_id: ToolExecutionId }
  | { type: "unknown_subagent"; subagent_id: SubagentId }
  | {
      type: "resync_required";
      after_cursor: RuntimeClientCursor;
      earliest_serviceable: RuntimeClientCursor;
    }
  | { type: "runtime_shutdown" }
  | { type: "invalid_state"; message: string }
  | { type: "invalid_model_configuration"; message: string }
  | { type: "projection_exhausted" }
  | { type: "runtime_failure"; message: string }
  | { type: "session_failure"; message: string }
  | { type: "session_restart_required"; message: string };

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
 * Checks only the discriminator of one Runtime Client Protocol v1 event.
 *
 * The connection owns structural protocol validation, so this deliberately
 * does not validate the event payload. Once this returns true, the reducer
 * may receive the event as a known v1 fact.
 */
export function isKnownRuntimeClientEvent(
  value: unknown,
): value is RuntimeClientEvent {
  if (typeof value !== "object" || value === null) {
    return false;
  }

  switch ((value as { type?: unknown }).type) {
    case "attempt_started":
    case "attempt_settled":
    case "attempt_turn_updated":
    case "attempt_usage_updated":
    case "interaction_pending":
    case "interaction_settled":
    case "interaction_audit_requested":
    case "interaction_audit_settled":
    case "approval_mode_changed":
    case "context_compaction_started":
    case "context_compaction_failed":
    case "context_compacted":
    case "assistant_message_started":
    case "assistant_text_delta":
    case "assistant_reasoning_delta":
    case "assistant_refusal_delta":
    case "tool_call_started":
    case "tool_call_arguments_delta":
    case "tool_call_assembled":
    case "assistant_publication_settled":
    case "tool_execution_started":
    case "tool_execution_progress":
    case "tool_execution_settled":
    case "message_committed":
    case "agent_status_composed":
    case "inbound_enqueued":
    case "inbound_drained":
    case "background_execution_updated":
    case "subagent_updated":
    case "capability_updated":
    case "session_model_changed":
    case "runtime_shutdown":
      return true;
    default:
      return false;
  }
}

/**
 * Classifies one decoded outbound record.
 *
 * A known notification carries a cursor and a known v1 event discriminator.
 * Malformed or future event-shaped records are handled separately by the
 * connection so they cannot fall through as responses.
 */
export function isProtocolEvent(
  record: unknown,
): record is RuntimeClientProtocolEvent {
  if (typeof record !== "object" || record === null) {
    return false;
  }

  const candidate = record as {
    cursor?: unknown;
    event?: unknown;
  };
  return (
    typeof candidate.cursor === "number" &&
    isKnownRuntimeClientEvent(candidate.event) &&
    (candidate.event.type !== "message_committed" &&
    candidate.event.type !== "inbound_enqueued"
      ? true
      : hasTranscriptCursorContract(
          candidate.event.message,
          candidate.event.transcript_cursor,
        ))
  );
}

/** Returns true for a record attempting to use the event notification shape. */
export function isEventLikeRecord(record: unknown): boolean {
  return (
    typeof record === "object" &&
    record !== null &&
    ("cursor" in record || "event" in record)
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
    case "resource_reload_busy":
      return `runtime resources are busy: ${error.reason}`;
    case "interaction_not_pending":
      return `interaction ${error.interaction_id} is no longer pending`;
    case "interaction_invalid_response":
      return `invalid interaction response: ${error.message}`;
    case "interaction_audit_failed":
      return `interaction ${error.interaction_id} was settled fail-closed: its durable audit could not be recorded`;
    case "approval_mode_inactive":
      return "the runtime is not activated for ApprovalMode changes";
    case "approval_mode_durability_failed":
      return `ApprovalMode change failed durability: ${error.message}`;
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
    case "session_failure":
      return `session operation failed: ${error.message}`;
    case "session_restart_required":
      return `the active Session runtime must be replaced: ${error.message}`;
    default:
      // A future runtime may add a category. Report it rather than crash.
      return `unrecognized protocol error: ${JSON.stringify(error)}`;
  }
}

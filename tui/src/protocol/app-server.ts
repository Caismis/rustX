/**
 * The App Server protocol as this client sees it.
 *
 * There is no wire transcription here. Every type below is either re-exported
 * from `protocol/app-server/v24.ts` — generated from the authoritative Rust DTOs
 * in `src/app_server/protocol.rs` — or **derived from one of those generated
 * types** with an indexed access. A derivation cannot drift: if the Rust DTO
 * changes shape, regeneration changes the type this file names, and every use
 * site fails to compile.
 *
 * ```text
 * src/app_server/protocol.rs      (Rust authority)
 *        | schemars
 * protocol/app-server/v24.schema.json
 *        | json-schema-to-typescript
 * protocol/app-server/v24.ts       (generated)
 *        | re-export + indexed access
 * this file                       (the only names the TUI spells)
 * ```
 *
 * The generator inlines many anonymous unions rather than naming them, and it
 * numbers structurally repeated definitions (`CapabilityView1`, `ToolCall1`).
 * Naming a numbered alias directly would be a guess about which occurrence the
 * projection receives, so the derivations below name each type by *the place it
 * actually appears on the wire* — `RuntimeClientSnapshot["capabilities"]` is by
 * construction the type of the snapshot's capability section.
 *
 * # Exact integer domains
 *
 * Runtime incarnations, cursors, revisions and sequences are `u64` on the wire
 * and arrive as canonical decimal **strings**, because binary64 cannot hold
 * them. They are never compared with `<`, which would order them
 * lexicographically; {@link compareExact} is the only ordering.
 */

import type {
  AttachmentTarget,
  InteractionResponse as InteractionResponseValue,
  QuestionSpecification,
  QuestionnaireAnswerEntry,
  ErrorData,
  Failure,
  MessageBlock as MessageBlockValue,
  MethodResult,
  Notification,
  Notification1,
  ProtocolMessage,
  Request1,
  Response,
  RpcError,
  RuntimeClientEvent,
  RuntimeClientSnapshot,
  SessionNode,
  SessionPersistentState,
  SessionSnapshot,
  SessionSummary,
  SessionUserMessageBoundary,
  Success,
} from "../../../protocol/app-server/v24.ts";

export type {
  ConfigurationApplication,
  AvailableConfiguration,
  AdmittedSettings,
  EffectiveConfiguration,
  AgentStatusGenerationMetadata,
  ApprovalMode,
  AttachmentTarget,
  AttachmentId,
  CapabilityInspection,
  CatalogModelView,
  ClientIdentity,
  ConversationId,
  EffectivePlugins,
  ErrorData,
  Failure,
  GoalControl,
  GoalMutation,
  GoalRef,
  GoalSnapshot,
  GoalView,
  InboundSequence,
  InitializeParams,
  InteractionRef,
  InteractionResponse,
  MessageBlock,
  MessageId,
  MethodResult,
  ModelCatalogView,
  ModelRef,
  Notification,
  Notification1,
  OptionSpecification,
  PresentationCapabilities,
  ProtocolMessage,
  QuestionSpecification,
  QuestionnaireAnswerEntry,
  QuestionnaireSpecification,
  ReasoningProfileId,
  Request,
  Request1,
  RequestId,
  Response,
  ReviewResponse,
  RpcError,
  RuntimeClientCursor,
  RuntimeClientEvent,
  RuntimeClientSnapshot,
  RuntimeClientTranscriptCursor,
  RuntimeDurabilityFailure,
  RuntimeIncarnationId,
  ServerCapabilities,
  SessionId,
  SessionNode,
  SessionNodeId,
  SessionPersistentState,
  SessionSnapshot,
  SessionSummary,
  SessionUserMessageBoundary,
  SettingsEvidence,
  SkillProvenance,
  SourceResolutionFailure,
  SubagentId,
  Success,
  SurfaceRevision,
  ToolCallId,
  ToolExecutionId,
  ToolId,
  ToolOrigin,
  UserContentBlock,
  UserInputBlock,
  WorkflowDependencyFailure,
  WorkflowInspection,
  WorkflowState,
} from "../../../protocol/app-server/v24.ts";

// ---------------------------------------------------------------------------
// Envelope helpers
//
// `Request1` and `Notification1` are the generated method unions. Addressing a
// method by name keeps every call site typed without a second method table.
// ---------------------------------------------------------------------------

/** Every method name the App Server admits. */
export type MethodName = Request1["method"];

/** The exact `params` object of one method. */
export type MethodParams<M extends MethodName> = Extract<
  Request1,
  { method: M }
>["params"];

/** Every result discriminator the App Server returns. */
export type ResultType = MethodResult["type"];

/** The exact result payload of one result discriminator. */
export type ResultOf<T extends ResultType> = Extract<MethodResult, { type: T }>;

/** Every notification method name. */
export type NotificationName = Notification1["method"];

/** The exact `params` object of one notification. */
export type NotificationParams<M extends NotificationName> = Extract<
  Notification1,
  { method: M }
>["params"];

/** The closed typed failure vocabulary of a domain error. */
export type ErrorKind = ErrorData["kind"];

// ---------------------------------------------------------------------------
// Identity domains
//
// Distinct names for distinct domains, exactly as the protocol requires: a
// Session ID, a Conversation ID, an incarnation, an attachment ID, a cursor and
// a JSON-RPC request ID are never interchangeable. The wire carries each as an
// opaque string; these aliases record which domain a value belongs to.
// ---------------------------------------------------------------------------

export type AgentId = string;
export type AttemptId = string;
export type TurnId = string;
export type InteractionId = string;
export type SkillId = string;
export type SkillVersionId = string;
export type McpServerId = string;
export type ContentBlockIndex = number;
/** Canonical binary64 text; never a JSON number. See `./number.ts`. */
export type FiniteNumberWire = string;

// ---------------------------------------------------------------------------
// Snapshot sections
//
// Each name below is the type of one authoritative snapshot field.
// ---------------------------------------------------------------------------

export type CapabilityView = RuntimeClientSnapshot["capabilities"];
export type RuntimeClientTool = NonNullable<CapabilityView["tools"]>[number];
export type RuntimeClientSkill = NonNullable<CapabilityView["skills"]>[number];
export type CapabilitySourceView = NonNullable<
  CapabilityView["sources"]
>[number];
export type CapabilitySourceStateView = CapabilitySourceView["state"];

export type RuntimeClientContextView = NonNullable<
  RuntimeClientSnapshot["context"]
>;
export type RuntimeClientResourcesView = NonNullable<
  RuntimeClientSnapshot["resources"]
>;
export type CapabilityInspectionView = RuntimeClientResourcesView["inspection"];
export type SkillDiagnostic = NonNullable<
  CapabilityInspectionView["skill_diagnostics"]
>[number];
export type AgentCapabilityInspection = NonNullable<
  CapabilityInspectionView["main"]
>;

export type RuntimeClientAttempt = NonNullable<RuntimeClientSnapshot["attempt"]>;
export type RuntimeClientAttemptPhase = RuntimeClientAttempt["phase"];
export type RuntimeClientOutcome = Extract<
  RuntimeClientAttemptPhase,
  { type: "settled" }
>["outcome"];
export type RuntimeClientAttemptFailure = Extract<
  RuntimeClientOutcome,
  { type: "failed" }
>["error"];
/** The closed vocabulary of runtime-owned failures. */
export type RuntimeError = Extract<
  RuntimeClientAttemptFailure,
  { type: "runtime" }
>["error"];
export type ModelUsage = NonNullable<RuntimeClientAttempt["last_usage"]>;
export type InFlightAssistantMessage = NonNullable<
  RuntimeClientAttempt["in_flight"]
>;
export type ForegroundToolExecution = NonNullable<
  RuntimeClientAttempt["foreground"]
>[number];
export type ForegroundToolState = ForegroundToolExecution["state"];

export type InboundDiagnostics = RuntimeClientSnapshot["inbound"];
export type RoutedInteraction = NonNullable<
  RuntimeClientSnapshot["pending_interactions"]
>[number];
export type InteractionRequest = RoutedInteraction["request"];
/** What a *live* interaction is asking for. */
export type InteractionKind = InteractionRequest["kind"];
export type InteractionSource = RoutedInteraction["source"];
export type InteractionRequester = Extract<
  InteractionKind,
  { type: "questionnaire" }
>["requester"];
/**
 * Caller correlation for one native invocation.
 *
 * Deliberately not a string: an agent tool call and a Workflow node are
 * different callers, and collapsing them would lose which one asked.
 */
export type ToolInvocationId = Extract<
  InteractionKind,
  { type: "approval" }
>["invocation_id"];
/** What an approval interaction was answered with. */
export type ApprovalDecision = Extract<
  InteractionResponseValue,
  { type: "approval" }
>["decision"];
/** What a Questionnaire interaction was answered with. */
export type QuestionnaireResponse = Extract<
  InteractionResponseValue,
  { type: "questionnaire" }
>["response"];
/** One typed answer to one Questionnaire question. */
export type QuestionnaireAnswer = QuestionnaireAnswerEntry["answer"];
/** The exact shape of a legal answer to one question. */
export type AnswerSpecification = QuestionSpecification["answer"];

export type RuntimeClientJob = NonNullable<
  RuntimeClientSnapshot["jobs"]
>[number];
export type BackgroundLifecycle = RuntimeClientJob["state"];

export type RuntimeClientAgent = NonNullable<
  RuntimeClientSnapshot["agents"]
>[number];
export type AgentState = RuntimeClientAgent["state"];
export type RuntimeClientAgentWorkspace = NonNullable<
  RuntimeClientAgent["workspace"]
>;
export type RuntimeClientAgentObservation = NonNullable<
  RuntimeClientAgent["observation"]
>;
export type RuntimeClientAgentActivity =
  RuntimeClientAgentObservation["activity"];

export type AgentStatusView = NonNullable<
  RuntimeClientSnapshot["statuses"]
>[number];
export type RuntimeClientStatusSection = NonNullable<
  AgentStatusView["sections"]
>[number];
export type AgentStatusOpportunityView = AgentStatusView["opportunities"];
export type RuntimeClientTodoStatusTask = NonNullable<
  Extract<RuntimeClientStatusSection, { type: "todo" }>["tasks"]
>[number];

export type TodoSnapshot = NonNullable<RuntimeClientSnapshot["todos"]>;
export type TodoTask = NonNullable<TodoSnapshot["tasks"]>[number];
export type TodoStatus = TodoTask["status"];

export type WorkflowSnapshot = RuntimeClientSnapshot["workflows"];
export type WorkflowRunView = NonNullable<WorkflowSnapshot["runs"]>[number];
export type WorkflowInstanceView = NonNullable<
  WorkflowRunView["instances"]
>[number];

export type SessionModelView = NonNullable<RuntimeClientSnapshot["model"]>;
export type SessionModelConfig = SessionModelView["configured"];
export type ModelInvocationView = SessionModelView["effective"];
export type AttemptModelView = NonNullable<RuntimeClientAttempt["model"]>;

export type RuntimeClientTranscriptPage = RuntimeClientSnapshot["transcript"];
export type RuntimeClientTranscriptEntry = NonNullable<
  RuntimeClientTranscriptPage["entries"]
>[number];
export type RuntimeClientTranscriptItem = RuntimeClientTranscriptEntry["item"];
export type PublicationAudit = Extract<
  RuntimeClientTranscriptItem,
  { type: "publication_audit" }
>["audit"];
export type InteractionSettlement = Extract<
  RuntimeClientTranscriptItem,
  { type: "interaction_settled" }
>["settlement"];
/**
 * What a *durably audited* interaction asked for.
 *
 * Deliberately a different type from {@link InteractionKind}: the durable audit
 * and the live request are separate Rust domains, and the live one carries
 * routing detail the audit does not.
 */
export type InteractionSubject = Extract<
  RuntimeClientTranscriptItem,
  { type: "interaction_requested" }
>["subject"];

/** The canonical user message, as the fork/branch boundary page carries it. */
export type UserMessageBlock = SessionUserMessageBoundary["message"];
/** One committed model generation. */
export type AssistantMessageBlock = Extract<MessageBlockValue, { role: "assistant" }>;
/** One ordered block of a committed assistant message. */
export type AssistantContentBlock = AssistantMessageBlock["content"][number];
/** One committed tool result. */
export type ToolMessageBlock = Extract<MessageBlockValue, { role: "tool" }>;


// ---------------------------------------------------------------------------
// Event sections
// ---------------------------------------------------------------------------

/** One event variant, addressed by its stable `type` discriminator. */
export type EventOf<T extends RuntimeClientEvent["type"]> = Extract<
  RuntimeClientEvent,
  { type: T }
>;

export type ToolExecutionResult = EventOf<"tool_execution_settled">["result"];
export type ToolProgress = NonNullable<
  EventOf<"tool_execution_progress">["progress"]
>;

// ---------------------------------------------------------------------------
// Method result sections
// ---------------------------------------------------------------------------

export type SessionDeleteResult = ResultOf<"deletion">["result"];
export type SessionDeletePreview = Extract<
  SessionDeleteResult,
  { status: "preview" }
>["preview"];
export type DeletionBlocker = Extract<
  SessionDeleteResult,
  { status: "blocked" }
>["reason"];
export type RuntimeClientAgentWorkspaceDisposalOutcome =
  ResultOf<"workspace_disposed">["outcome"];

// ---------------------------------------------------------------------------
// Exact integer domains
// ---------------------------------------------------------------------------

/**
 * Orders two canonical unsigned decimal strings numerically.
 *
 * `"9" < "10"` is false as text and true as a number, and every cursor,
 * revision and incarnation crosses 2^53, so `BigInt` is the comparison rather
 * than `Number`. Nothing here parses a bigint out of JSON: these values are
 * already text on the wire and stay text in this client.
 */
export function compareExact(left: string, right: string): number {
  const a = BigInt(left);
  const b = BigInt(right);
  return a < b ? -1 : a > b ? 1 : 0;
}

/** The canonical decimal zero of every exact `u64` domain. */
export const EXACT_ZERO = "0";

// ---------------------------------------------------------------------------
// Lifecycle classification
//
// Typed against the generated vocabularies, so a new lifecycle state is a
// compile error here rather than a state this client silently treats as live.
// ---------------------------------------------------------------------------

/**
 * The background states the registry will publish no further transition for.
 *
 * `timed_out` means a proven terminal settlement; `outcome_unknown` means the
 * execution crossed the external-effect frontier and the runtime could not
 * establish its outcome. Both are terminal, and neither is a failure.
 */
export const BACKGROUND_TERMINAL_STATES: ReadonlySet<BackgroundLifecycle> =
  new Set<BackgroundLifecycle>([
    "succeeded",
    "failed",
    "denied",
    "cancelled",
    "timed_out",
    "outcome_unknown",
  ]);

// ---------------------------------------------------------------------------
// Record classification
//
// These classifiers accept only DTOs already validated by decoder.ts. They
// narrow the generated union; they do not establish trust in arbitrary JSON.
// ---------------------------------------------------------------------------

/** Whether a validated record is a JSON-RPC response. */
export function isResponse(record: ProtocolMessage): record is Response {
  return "result" in record || "error" in record;
}

/** Whether a decoded record is a server notification. */
export function isNotification(record: ProtocolMessage): record is Notification {
  return !("id" in record) && "method" in record;
}

/** Whether a response carries a typed failure rather than a result. */
export function isFailure(response: Response): response is Failure {
  return "error" in response;
}

/** Whether a response carries a result. */
export function isSuccess(response: Response): response is Success {
  return "result" in response;
}

/** A bounded human-readable rendering of one typed protocol failure. */
export function describeRpcError(error: RpcError): string {
  const data = error.data;
  if (data === undefined || data === null) {
    return `${error.message} (code ${error.code})`;
  }
  switch (data.kind) {
    case "archive_preparation_failed":
      return error.message;
    case "request_capacity":
    case "residency_capacity":
    case "attachment_capacity":
      return `the App Server reached ${data.kind.replaceAll("_", " ")}`;
    case "server_draining":
      return "the App Server is shutting down and no longer accepts work";
    case "agent_stopping":
      return `Agent ${data.agent_id} is settling its activation; retry after it becomes inactive`;
    case "unknown_agent":
      return `Agent ${data.agent_id} does not belong to this parent`;
    case "agent_history_unavailable":
      return `Agent ${data.agent_id} history is unavailable`;
    case "unknown_subagent":
      return `Subagent ${data.subagent_id} does not belong to this parent`;
    case "subagent_history_unavailable":
      return `Subagent ${data.subagent_id} history is unavailable`;
    case "unknown_session":
      return `unknown session ${data.session_id}`;
    case "unknown_node":
      return `unknown node ${data.node_id} in session ${data.session_id}`;
    case "configuration_adoption":
      return `Configuration adoption: ${data.rejection.status === "failed" ? data.rejection.diagnostic : data.rejection.status.replaceAll("_", " ")}`;
    case "source_conflict":
      return `${data.scope} source changed (expected ${data.expected}, found ${data.actual})`;
    case "stale_settings":
      return `settings changed underneath this edit (expected revision ${data.expected}, found ${data.actual})`;
    case "interaction_not_pending":
      return "that interaction is no longer pending";
    case "interaction_audit_failed":
      return "the interaction response could not be recorded";
    case "committed_durability_uncertain":
      return "the change was committed but its durability is unknown";
    case "unsupported_version":
      return `the server speaks App Server protocol ${data.supported}, this client speaks ${data.requested}`;
    case "not_initialized":
      return "the connection has not completed initialize";
    case "already_initialized":
      return "the connection has already completed initialize";
    case "stale_attachment":
      return "this attachment has been replaced";
    case "stale_runtime":
      return "this runtime incarnation has been replaced";
    case "controller_in_use":
      return "another client already controls this Session";
    case "invalid_state":
      return `the server rejected the request: ${error.message}`;
    case "invalid_params":
      return `the request parameters were rejected: ${error.message}`;
    case "resync_required":
      return "the projection needs an authoritative repair";
    case "operation_failed":
      return `the operation failed: ${error.message}`;
    default: {
      const exhaustive: never = data;
      void exhaustive;
      return `${error.message} (code ${error.code})`;
    }
  }
}

/** Whether two attachment targets name the same attachment in every domain. */
export function sameTarget(
  left: AttachmentTarget,
  right: AttachmentTarget,
): boolean {
  return (
    left.session_id === right.session_id &&
    left.conversation_id === right.conversation_id &&
    left.runtime_incarnation === right.runtime_incarnation &&
    left.attachment_id === right.attachment_id
  );
}

/** Session identity alone, ignoring incarnation and attachment. */
export function sameSession(
  left: AttachmentTarget,
  right: AttachmentTarget,
): boolean {
  return left.session_id === right.session_id;
}

export type { SessionNode as SessionNodeView };
export type { SessionSnapshot as SessionView };
export type SessionSummaryView = SessionSummary;
export type { SessionUserMessageBoundary as SessionUserMessageBoundaryView };
export type { SessionPersistentState as SessionSettings };

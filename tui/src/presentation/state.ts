/**
 * The one TypeScript state model of the runtime projection.
 *
 * This state is **ephemeral, replaceable, and derived entirely from Runtime
 * Client facts**. It is not canonical conversation history, not execution
 * state, not mailbox state, not background ownership, not model or session
 * authority, not Agent Status, not capability state, and not durability. It
 * is a render cache.
 *
 * The governing invariant: given a fresh authoritative snapshot, the complete
 * meaningful UI state is reconstructable — no hidden local conversation log,
 * no client-side semantic history inference, and no Pi session object holding
 * anything the snapshot does not.
 *
 * Reasoning and refusal stay distinct presentation kinds all the way through.
 * Flattening either into generic assistant text here would make the client a
 * second interpreter of model output.
 */

import type {
  AgentStatusView,
  ApprovalMode,
  AttemptId,
  AttemptModelView,
  CapabilityView,
  ContentBlockIndex,
  ConversationId,
  ForegroundToolExecution,
  InboundDiagnostics,
  MessageBlock,
  MessageId,
  ModelUsage,
  RuntimeClientAttemptPhase,
  RuntimeClientBackgroundExecution,
  RuntimeClientContextView,
  RuntimeClientCursor,
  RuntimeClientResourcesView,
  RuntimeClientSubagent,
  RuntimeClientTranscriptCursor,
  RoutedInteraction,
  InteractionSettlement,
  InteractionSubject,
  PublicationAudit,
  SessionModelView,
  TodoSnapshot,
  ToolCallId,
  ToolId,
} from "../protocol/types.ts";

/** One committed canonical message, exactly as the runtime committed it. */
export interface TranscriptCommitted {
  kind: "committed";
  /** Stable identity for rendering. Never derived from list position. */
  key: string;
  messageId: MessageId;
  /** The committing attempt, absent for runtime-admitted commits. */
  attemptId?: AttemptId;
  /** The durable transcript position; never the Runtime Client event cursor. */
  cursor: RuntimeClientTranscriptCursor;
  message: MessageBlock;
}

/** One block of the assistant message currently streaming. */
export type StreamingBlock =
  | { kind: "text"; blockIndex: ContentBlockIndex; text: string }
  | { kind: "reasoning"; blockIndex: ContentBlockIndex; text: string }
  | { kind: "refusal"; blockIndex: ContentBlockIndex; text: string }
  | {
      kind: "tool_call";
      blockIndex: ContentBlockIndex;
      callId: ToolCallId;
      toolId: ToolId;
      name: string;
      /** Accumulated JSON argument fragments. Carried, never parsed. */
      argumentsText: string;
    };

/** The assistant message being assembled by the active attempt. */
export interface StreamingMessage {
  kind: "streaming";
  key: string;
  attemptId: AttemptId;
  messageId: MessageId;
  blocks: StreamingBlock[];
}

/** A noncanonical Assistant publication audit shown as historical output. */
export interface TranscriptPublicationAudit {
  kind: "publication_audit";
  key: string;
  cursor: RuntimeClientTranscriptCursor;
  audit: PublicationAudit;
}

/** A historical interaction request audit, never an actionable prompt. */
export interface TranscriptInteractionRequested {
  kind: "interaction_requested";
  key: string;
  cursor: RuntimeClientTranscriptCursor;
  eventId: string;
  timestamp: string;
  attemptId: string;
  turnId: string;
  interactionId: string;
  subject: InteractionSubject;
}

/** A historical interaction settlement audit, never a live waiter. */
export interface TranscriptInteractionSettled {
  kind: "interaction_settled";
  key: string;
  cursor: RuntimeClientTranscriptCursor;
  eventId: string;
  timestamp: string;
  attemptId: string;
  turnId: string;
  interactionId: string;
  settlement: InteractionSettlement;
}

/** One entry of the rendered transcript. */
export type TranscriptEntry =
  | TranscriptCommitted
  | StreamingMessage
  | TranscriptPublicationAudit
  | TranscriptInteractionRequested
  | TranscriptInteractionSettled;

/** The client-visible view of the current or latest attempt. */
export interface AttemptPresentation {
  attemptId: AttemptId;
  phase: RuntimeClientAttemptPhase;
  turn: number;
  lastUsage?: ModelUsage;
  /**
   * The immutable model this attempt froze at admission.
   *
   * Distinct from {@link PresentationState.sessionModel}: while this attempt
   * runs on A and the session moved to B, this stays A.
   */
  model: AttemptModelView;
  /** Foreground tool executions in call-assembly order. */
  foreground: ForegroundToolExecution[];
}

export interface PresentationState {
  conversationId: ConversationId;
  /** The cursor this state is consistent through. */
  cursor: RuntimeClientCursor;
  /** Loaded durable transcript plus at most one streaming assistant message. */
  transcript: TranscriptEntry[];
  /** Exclusive durable cursor for the next older page. */
  transcriptNextCursor?: RuntimeClientTranscriptCursor;
  attempt?: AttemptPresentation;
  inbound: InboundDiagnostics;
  /** Runtime-owned live interactions, reconstructed from snapshot/events. */
  pendingInteractions: RoutedInteraction[];
  background: RuntimeClientBackgroundExecution[];
  subagents: RuntimeClientSubagent[];
  status?: AgentStatusView;
  context: RuntimeClientContextView;
  capabilities: CapabilityView;
  /** The active runtime resource generation: context files, agent profile. */
  resources: RuntimeClientResourcesView;
  /** The session's *desired* model configuration. */
  sessionModel: SessionModelView;
  /** True once runtime drain begins; shutdown responses complete at quiescence. */
  runtimeShutdown: boolean;
  /**
   * The conversation's task list, as the runtime derived it from canonical
   * history. `undefined` only before the first snapshot arrives.
   */
  todos?: TodoSnapshot;
  /** Runtime-authoritative ApprovalMode control state. */
  effectiveApprovalMode: ApprovalMode;
  pendingApprovalMode?: ApprovalMode;
  approvalModeRevision: number;
}

/** Whether the attempt is doing work the UI should show as busy. */
export function isAttemptActive(state: PresentationState): boolean {
  const phase = state.attempt?.phase.type;
  return phase === "admitted" || phase === "running";
}

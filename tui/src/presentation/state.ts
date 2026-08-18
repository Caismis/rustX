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
  InteractionRequest,
  SessionModelView,
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

/** One entry of the rendered transcript. */
export type TranscriptEntry = TranscriptCommitted | StreamingMessage;

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

/** A transient client-side note. Never mistaken for a runtime fact. */
export interface ClientNotice {
  key: string;
  level: "info" | "error";
  text: string;
}

/**
 * One locally submitted inbound message awaiting its runtime identity.
 *
 * Optimistic feedback is explicitly modelled as transient client state so it
 * can never be confused with canonical history. It is reconciled away as soon
 * as the runtime publishes the authoritative `inbound_enqueued` fact, whose
 * message id, sequence, timestamp, and provenance are the real ones.
 */
export interface PendingSubmission {
  key: string;
  text: string;
}

export interface PresentationState {
  conversationId: ConversationId;
  /** The cursor this state is consistent through. */
  cursor: RuntimeClientCursor;
  /** Committed history plus at most one streaming assistant message. */
  transcript: TranscriptEntry[];
  attempt?: AttemptPresentation;
  inbound: InboundDiagnostics;
  /** Runtime-owned live interactions, reconstructed from snapshot/events. */
  pendingInteractions: InteractionRequest[];
  background: RuntimeClientBackgroundExecution[];
  status?: AgentStatusView;
  context: RuntimeClientContextView;
  capabilities: CapabilityView;
  /** The session's *desired* model configuration. */
  sessionModel: SessionModelView;
  /** True once runtime drain begins; shutdown responses complete at quiescence. */
  runtimeShutdown: boolean;
  pendingSubmissions: PendingSubmission[];
  notices: ClientNotice[];
}

/** Whether the attempt is doing work the UI should show as busy. */
export function isAttemptActive(state: PresentationState): boolean {
  const phase = state.attempt?.phase.type;
  return phase === "admitted" || phase === "running";
}

/**
 * The presentation reducer.
 *
 * Two pure entry points define the whole state model:
 *
 * ```text
 * replaceFromSnapshot(snapshot, cursor)   -> state      (authoritative repair)
 * reduce(state, protocolEvent)            -> nextState  (incremental fold)
 * ```
 *
 * Both are total functions over runtime facts. There is no third path that
 * invents state, and no event is ever synthesized locally to "fill a gap":
 * when the incremental stream can no longer be trusted, the caller replaces
 * from a fresh snapshot rather than replaying what the UI thinks happened.
 *
 * # Why this is not a second runtime state machine
 *
 * Every field is copied from a runtime-published value. The reducer decides
 * *where a fact goes in the render tree*, never *what the fact is*: it starts
 * no execution, admits nothing, drains nothing, settles nothing, and can
 * produce no state the runtime did not publish. Discard it at any moment and
 * one `snapshot_get` rebuilds it exactly.
 *
 * Identity is always runtime-provided — `attempt_id`, `message_id`,
 * `block_index`, `tool_call_id`, `execution_id`. Nothing is identified by its
 * position on screen.
 */

import type {
  AttemptModelView,
  InteractionRequest,
  RuntimeClientCursor,
  RuntimeClientProtocolEvent,
  RuntimeClientSnapshot,
  SessionModelView,
} from "../protocol/types.ts";
import type {
  AttemptPresentation,
  ClientNotice,
  PresentationState,
  StreamingBlock,
  StreamingMessage,
  TranscriptEntry,
} from "./state.ts";

/** The initial state of a client that has not attached yet. */
export function emptyPresentationState(
  sessionModel: SessionModelView,
): PresentationState {
  return {
    conversationId: "",
    cursor: 0,
    transcript: [],
    inbound: { pending: [], last_drain: undefined },
    pendingInteractions: [],
    background: [],
    subagents: [],
    context: { compaction_count: 0 },
    capabilities: { revision: 0, tools: [], skills: [] },
    sessionModel,
    runtimeShutdown: false,
    pendingSubmissions: [],
    notices: [],
  };
}

/**
 * Replaces the whole projection from an authoritative snapshot.
 *
 * This is the only repair path. It keeps exactly two things that the snapshot
 * genuinely does not describe — the client's own transient notices and its
 * not-yet-acknowledged submissions — and derives everything else.
 */
export function replaceFromSnapshot(
  snapshot: RuntimeClientSnapshot,
  cursor: RuntimeClientCursor,
  carry?: Partial<
    Pick<
      PresentationState,
      "notices" | "pendingSubmissions"
    >
  >,
): PresentationState {
  const transcript: TranscriptEntry[] = snapshot.messages.map(
    (message, index) => ({
      kind: "committed",
      key: `committed:${messageIdOf(message)}:${index}`,
      messageId: messageIdOf(message),
      message,
    }),
  );

  const attempt = snapshot.attempt;
  if (attempt?.in_flight !== undefined) {
    // A snapshot taken mid-stream carries every accumulated delta through its
    // cursor, so the streaming message is rebuilt exactly as it would have
    // been observed incrementally.
    transcript.push({
      kind: "streaming",
      key: `streaming:${attempt.attempt_id}:${attempt.in_flight.message_id}`,
      attemptId: attempt.attempt_id,
      messageId: attempt.in_flight.message_id,
      blocks: (attempt.in_flight.blocks ?? []).map(
        (block): StreamingBlock =>
          block.type === "tool_call"
            ? {
                kind: "tool_call",
                blockIndex: block.block_index,
                callId: block.call_id,
                toolId: block.tool_id,
                name: block.name,
                argumentsText: block.arguments,
              }
            : {
                kind: block.type,
                blockIndex: block.block_index,
                text: block.text,
              },
      ),
    });
  }

  return {
    conversationId: snapshot.conversation_id,
    cursor,
    transcript,
    attempt:
      attempt === undefined
        ? undefined
        : {
            attemptId: attempt.attempt_id,
            phase: attempt.phase,
            turn: attempt.turn,
            lastUsage: attempt.last_usage,
            model: attempt.model,
            foreground: [...(attempt.foreground ?? [])],
          },
    inbound: {
      pending: [...(snapshot.inbound.pending ?? [])],
      last_drain: snapshot.inbound.last_drain,
    },
    pendingInteractions: [...snapshot.pending_interactions],
    background: [...(snapshot.background ?? [])],
    subagents: [...(snapshot.subagents ?? [])],
    status: snapshot.status,
    context: snapshot.context,
    capabilities: snapshot.capabilities,
    sessionModel: snapshot.model,
    runtimeShutdown: snapshot.shutting_down,
    pendingSubmissions: carry?.pendingSubmissions ?? [],
    notices: carry?.notices ?? [],
  };
}

/**
 * Folds one protocol event into the next state.
 *
 * Pure: the input state is never mutated, so a caller may keep the previous
 * value (for a diff, a test assertion, or an undo of a render).
 */
export function reduce(
  state: PresentationState,
  protocolEvent: RuntimeClientProtocolEvent,
): PresentationState {
  const next = { ...state, cursor: protocolEvent.cursor };
  const event = protocolEvent.event;

  switch (event.type) {
    case "attempt_started":
      // The frozen model travels with the event, so the active attempt's
      // model is known without a snapshot round trip and without inference.
      next.attempt = startAttempt(event.attempt_id, event.model);
      next.status = undefined;
      return next;

    case "attempt_settled":
      if (state.attempt?.attemptId === event.attempt_id) {
        next.attempt = {
          ...state.attempt,
          phase: { type: "settled", outcome: event.outcome },
        };
      }
      // An unsettled streaming message belongs to an attempt that ended
      // without committing; drop the partial render, keep committed history.
      next.transcript = state.transcript.filter(
        (entry) =>
          entry.kind !== "streaming" || entry.attemptId !== event.attempt_id,
      );
      return next;

    case "attempt_turn_updated":
      if (state.attempt?.attemptId === event.attempt_id) {
        next.attempt = { ...state.attempt, turn: event.turn };
      }
      return next;

    case "attempt_usage_updated":
      if (state.attempt?.attemptId === event.attempt_id) {
        next.attempt = { ...state.attempt, lastUsage: event.usage };
      }
      return next;

    case "interaction_pending":
      next.pendingInteractions = upsertInteraction(
        state.pendingInteractions,
        event.interaction,
      );
      return next;

    case "interaction_settled":
      next.pendingInteractions = state.pendingInteractions.filter(
        (interaction) => interaction.id !== event.interaction_id,
      );
      return next;

    case "context_compacted":
      next.context = event.context;
      return next;

    case "assistant_message_started":
      next.transcript = [
        ...dropStreaming(state.transcript, event.attempt_id),
        {
          kind: "streaming",
          key: `streaming:${event.attempt_id}:${event.message_id}`,
          attemptId: event.attempt_id,
          messageId: event.message_id,
          blocks: [],
        },
      ];
      return next;

    case "assistant_text_delta":
      return appendText(next, state, event.message_id, event.block_index, "text", event.delta);

    case "assistant_reasoning_delta":
      return appendText(
        next,
        state,
        event.message_id,
        event.block_index,
        "reasoning",
        event.delta,
      );

    case "assistant_refusal_delta":
      return appendText(
        next,
        state,
        event.message_id,
        event.block_index,
        "refusal",
        event.delta,
      );

    case "tool_call_started":
      return withStreaming(next, state, event.message_id, (message) => ({
        ...message,
        blocks: [
          ...message.blocks,
          {
            kind: "tool_call",
            blockIndex: event.block_index,
            callId: event.call.id,
            toolId: event.call.tool_id,
            name: event.call.name,
            argumentsText: "",
          },
        ],
      }));

    case "tool_call_arguments_delta":
      return withStreaming(next, state, event.message_id, (message) => ({
        ...message,
        blocks: message.blocks.map((block) =>
          block.kind === "tool_call" && block.callId === event.call_id
            ? {
                ...block,
                argumentsText: block.argumentsText + event.arguments_delta,
              }
            : block,
        ),
      }));

    case "tool_call_assembled":
      return withStreaming(next, state, event.message_id, (message) => ({
        ...message,
        blocks: message.blocks.map((block) =>
          block.kind === "tool_call" && block.callId === event.call.id
            ? {
                ...block,
                name: event.call.name,
                argumentsText: JSON.stringify(event.call.arguments),
              }
            : block,
        ),
      }));

    case "tool_execution_started":
      return withForeground(next, state, event.attempt_id, (foreground) =>
        upsertForeground(foreground, event.tool_call_id, (existing) => ({
          call_id: event.tool_call_id,
          tool_id: event.tool_id,
          name: existing?.name ?? "",
          state: {
            type: "running",
            arguments: argumentsOf(existing),
          },
        })),
      );

    case "tool_execution_progress":
      if (event.execution_id !== undefined) {
        // A progress report carrying a detached execution identity belongs to
        // the conversation-owned background registry, not to this attempt's
        // foreground list.
        return next;
      }
      return withForeground(next, state, event.attempt_id, (foreground) =>
        upsertForeground(foreground, event.tool_call_id, (existing) => ({
          call_id: event.tool_call_id,
          tool_id: event.tool_id,
          name: existing?.name ?? "",
          state: {
            type: "running",
            arguments: argumentsOf(existing),
            progress: event.progress,
          },
        })),
      );

    case "tool_execution_settled":
      return withForeground(next, state, event.attempt_id, (foreground) =>
        upsertForeground(foreground, event.tool_call_id, (existing) => ({
          call_id: event.tool_call_id,
          tool_id: event.tool_id,
          name: existing?.name ?? "",
          state: {
            type: "settled",
            arguments: argumentsOf(existing),
            result: event.result,
          },
        })),
      );

    case "message_committed": {
      const messageId = messageIdOf(event.message);
      const transcript =
        event.message.role === "assistant" && event.attempt_id !== undefined
          ? dropStreaming(state.transcript, event.attempt_id)
          : state.transcript;
      next.transcript = [
        ...transcript,
        {
          kind: "committed",
          key: `committed:${messageId}:${transcript.length}`,
          messageId,
          attemptId: event.attempt_id,
          message: event.message,
        },
      ];
      return next;
    }

    case "agent_status_composed":
      next.status = event.status;
      return next;

    case "inbound_enqueued":
      next.inbound = {
        ...state.inbound,
        pending: [
          ...(state.inbound.pending ?? []),
          { sequence: event.sequence, message: event.message },
        ],
      };
      // The runtime's authoritative inbound fact supersedes the optimistic
      // local echo of the same text, if one is outstanding.
      next.pendingSubmissions = reconcileSubmission(
        state.pendingSubmissions,
        event.message,
      );
      return next;

    case "inbound_drained":
      next.inbound = {
        pending: (state.inbound.pending ?? []).filter(
          (item) => item.sequence > event.watermark,
        ),
        last_drain: { watermark: event.watermark, count: event.count },
      };
      return next;

    case "background_execution_updated":
      next.background = upsertBackground(state.background, event.execution);
      return next;

    case "subagent_updated":
      next.subagents = upsertSubagent(state.subagents, event.subagent);
      return next;

    case "capability_published":
      next.capabilities = event.capabilities;
      return next;

    case "session_model_changed":
      // The session's desired model only. A running attempt keeps the model
      // it froze; nothing about `state.attempt` changes here.
      next.sessionModel = event.model;
      return next;

    case "runtime_shutdown":
      next.runtimeShutdown = true;
      return next;

    default:
      // RuntimeClientConnection validates the v1 event vocabulary before an
      // event reaches this reducer. This branch is unreachable unless a
      // caller bypasses that boundary, and must never advance the cursor.
      const exhaustiveEvent: never = event;
      throw new Error(
        `unreachable Runtime Client Protocol v1 event: ${String(exhaustiveEvent)}`,
      );
  }
}

/** Adds a transient client notice. Never a runtime fact. */
export function withNotice(
  state: PresentationState,
  notice: ClientNotice,
): PresentationState {
  return { ...state, notices: [...state.notices, notice] };
}

/** Records an optimistic local echo of a submission not yet acknowledged. */
export function withPendingSubmission(
  state: PresentationState,
  key: string,
  text: string,
): PresentationState {
  return {
    ...state,
    pendingSubmissions: [...state.pendingSubmissions, { key, text }],
  };
}

/** Drops an optimistic echo whose submission the runtime rejected. */
export function withoutPendingSubmission(
  state: PresentationState,
  key: string,
): PresentationState {
  return {
    ...state,
    pendingSubmissions: state.pendingSubmissions.filter(
      (pending) => pending.key !== key,
    ),
  };
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

function startAttempt(
  attemptId: string,
  model: AttemptModelView,
): AttemptPresentation {
  return {
    attemptId,
    phase: { type: "running" },
    turn: 0,
    model,
    foreground: [],
  };
}

function messageIdOf(message: {
  id?: string;
  role?: string;
}): string {
  return message.id ?? "";
}

function dropStreaming(
  transcript: TranscriptEntry[],
  attemptId: string,
): TranscriptEntry[] {
  return transcript.filter(
    (entry) => entry.kind !== "streaming" || entry.attemptId !== attemptId,
  );
}

function withStreaming(
  next: PresentationState,
  previous: PresentationState,
  messageId: string,
  update: (message: StreamingMessage) => StreamingMessage,
): PresentationState {
  next.transcript = previous.transcript.map((entry) =>
    entry.kind === "streaming" && entry.messageId === messageId
      ? update(entry)
      : entry,
  );
  return next;
}

function appendText(
  next: PresentationState,
  previous: PresentationState,
  messageId: string,
  blockIndex: number,
  kind: "text" | "reasoning" | "refusal",
  delta: string,
): PresentationState {
  return withStreaming(next, previous, messageId, (message) => {
    const existing = message.blocks.find(
      (block) => block.blockIndex === blockIndex,
    );
    if (existing === undefined) {
      return {
        ...message,
        blocks: [...message.blocks, { kind, blockIndex, text: delta }],
      };
    }
    return {
      ...message,
      blocks: message.blocks.map((block) =>
        block.blockIndex === blockIndex && block.kind !== "tool_call"
          ? { ...block, text: block.text + delta }
          : block,
      ),
    };
  });
}

function withForeground(
  next: PresentationState,
  previous: PresentationState,
  attemptId: string,
  update: (
    foreground: AttemptPresentation["foreground"],
  ) => AttemptPresentation["foreground"],
): PresentationState {
  if (previous.attempt === undefined || previous.attempt.attemptId !== attemptId) {
    return next;
  }
  next.attempt = {
    ...previous.attempt,
    foreground: update(previous.attempt.foreground),
  };
  return next;
}

function argumentsOf(
  existing: AttemptPresentation["foreground"][number] | undefined,
): string {
  return existing === undefined ? "" : existing.state.arguments;
}

function upsertForeground(
  foreground: AttemptPresentation["foreground"],
  callId: string,
  build: (
    existing: AttemptPresentation["foreground"][number] | undefined,
  ) => AttemptPresentation["foreground"][number],
): AttemptPresentation["foreground"] {
  const index = foreground.findIndex((entry) => entry.call_id === callId);
  if (index === -1) {
    return [...foreground, build(undefined)];
  }
  const updated = [...foreground];
  updated[index] = build(foreground[index]);
  return updated;
}

function upsertBackground(
  background: PresentationState["background"],
  execution: PresentationState["background"][number],
): PresentationState["background"] {
  const index = background.findIndex(
    (entry) => entry.execution_id === execution.execution_id,
  );
  if (index === -1) {
    return [...background, execution];
  }
  const updated = [...background];
  updated[index] = execution;
  return updated;
}

function upsertSubagent(
  subagents: PresentationState["subagents"],
  subagent: PresentationState["subagents"][number],
): PresentationState["subagents"] {
  const index = subagents.findIndex(
    (entry) => entry.subagent_id === subagent.subagent_id,
  );
  if (index === -1) {
    return [...subagents, subagent];
  }
  const updated = [...subagents];
  updated[index] = subagent;
  return updated;
}

function upsertInteraction(
  interactions: InteractionRequest[],
  interaction: InteractionRequest,
): InteractionRequest[] {
  const index = interactions.findIndex((entry) => entry.id === interaction.id);
  if (index === -1) {
    return [...interactions, interaction].sort((left, right) =>
      left.id.localeCompare(right.id),
    );
  }
  const updated = [...interactions];
  updated[index] = interaction;
  return updated;
}

function reconcileSubmission(
  pending: PresentationState["pendingSubmissions"],
  message: { content?: Array<{ type: string; text?: string }> },
): PresentationState["pendingSubmissions"] {
  const text = (message.content ?? [])
    .filter((block) => block.type === "text")
    .map((block) => block.text ?? "")
    .join("");
  const index = pending.findIndex((entry) => entry.text === text);
  if (index === -1) {
    return pending;
  }
  return [...pending.slice(0, index), ...pending.slice(index + 1)];
}

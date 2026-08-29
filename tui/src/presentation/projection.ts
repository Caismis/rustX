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
  RuntimeClientTranscriptEntry,
  RuntimeClientTranscriptPage,
  SessionModelView,
  ToolExecutionResult,
} from "../protocol/types.ts";
import {
  isHiddenContextMessage,
  validateTranscriptCursorContract,
} from "../protocol/types.ts";
import { parseSnapshot, publishedTodos } from "./todos.ts";
import type {
  AttemptPresentation,
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
    cursor: 0 as RuntimeClientCursor,
    transcript: [],
    inbound: { pending: [], last_drain: undefined },
    pendingInteractions: [],
    background: [],
    subagents: [],
    context: { compaction_in_progress: false, compaction_count: 0 },
    capabilities: { revision: 0, tools: [], skills: [] },
    resources: { revision: 0, context_files: [], agent_profile: false },
    sessionModel,
    todos: undefined,
    runtimeShutdown: false,
    effectiveApprovalMode: "policy",
    pendingApprovalMode: undefined,
    approvalModeRevision: 0,
  };
}

/**
 * Replaces the whole projection from an authoritative snapshot.
 *
 * This is the only repair path. It derives the complete semantic projection
 * from the authoritative snapshot; there is no client-owned submission row
 * to preserve. TUI inspection and transient feedback surfaces live outside
 * this runtime-derived projection, so an authoritative repair cannot
 * reconstruct or carry them accidentally.
 */
export function replaceFromSnapshot(
  snapshot: RuntimeClientSnapshot,
  cursor: RuntimeClientCursor,
): PresentationState {
  const transcript: TranscriptEntry[] = orderTranscript(
    snapshot.transcript.entries.map(transcriptEntryFromWire),
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
    transcript: orderTranscript(transcript),
    transcriptNextCursor: snapshot.transcript.next_cursor,
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
    resources: snapshot.resources ?? {
      revision: 0,
      context_files: [],
      agent_profile: false,
    },
    // The runtime derives the list from the whole Ledger, so this is the
    // one repair path for it too: an attach, a resume, and a reload after
    // compaction all open on exactly the list canonical history holds,
    // however far back the last `todo` result now sits.
    todos: parseSnapshot(snapshot.todos) ?? { tasks: [], next_id: 1 },
    sessionModel: snapshot.model,
    runtimeShutdown: snapshot.shutting_down,
    effectiveApprovalMode: snapshot.effective_approval_mode,
    pendingApprovalMode: snapshot.pending_approval_mode,
    approvalModeRevision: snapshot.approval_mode_revision,
  };
}

/** Merges an older durable page ahead of the currently loaded transcript. */
export function mergeTranscriptPage(
  state: PresentationState,
  page: RuntimeClientTranscriptPage,
): PresentationState {
  const older = page.entries.map(transcriptEntryFromWire);
  const merged = deduplicateTranscript([...state.transcript, ...older]);
  return {
    ...state,
    transcript: orderTranscript(merged),
    transcriptNextCursor: page.next_cursor,
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
  const event = protocolEvent.event;
  const transcriptCursor =
    event.type === "message_committed" || event.type === "inbound_enqueued"
      ? validateTranscriptCursorContract(
          event.message,
          event.transcript_cursor,
        )
      : undefined;
  const next = { ...state, cursor: protocolEvent.cursor };

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
      next.context = { ...state.context, compaction_in_progress: false };
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

    case "interaction_audit_requested":
      next.transcript = appendTranscriptEntry(state.transcript, {
        kind: "interaction_requested",
        key: `interaction-requested:${event.audit.event_id}`,
        cursor: event.transcript_cursor,
        eventId: event.audit.event_id,
        timestamp: event.audit.timestamp,
        attemptId: event.audit.attempt_id,
        turnId: event.audit.turn_id,
        interactionId: event.audit.interaction_id,
        subject: event.audit.subject,
      });
      return next;

    case "interaction_audit_settled":
      next.transcript = appendTranscriptEntry(state.transcript, {
        kind: "interaction_settled",
        key: `interaction-settled:${event.audit.event_id}`,
        cursor: event.transcript_cursor,
        eventId: event.audit.event_id,
        timestamp: event.audit.timestamp,
        attemptId: event.audit.attempt_id,
        turnId: event.audit.turn_id,
        interactionId: event.audit.interaction_id,
        settlement: event.audit.settlement,
      });
      return next;

    case "approval_mode_changed":
      next.effectiveApprovalMode = event.effective_approval_mode;
      next.pendingApprovalMode = event.pending_approval_mode;
      next.approvalModeRevision = event.revision;
      return next;

    case "context_compaction_started":
      next.context = { ...state.context, compaction_in_progress: true };
      return next;

    case "context_compaction_failed":
      next.context = { ...state.context, compaction_in_progress: false };
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

    case "assistant_publication_settled":
      // The stream settled without ever becoming a canonical Assistant
      // message, so the in-flight card is dropped exactly as the Rust
      // projection drops it. The audit is committed-for-release output, not
      // canonical conversation history, so it enters the transcript only as
      // a typed non-canonical audit and its proposed tool calls never create
      // a foreground execution slot.
      next.transcript = appendTranscriptEntry(
        dropStreaming(state.transcript, event.attempt_id),
        {
          kind: "publication_audit",
          key: `publication:${event.audit.stream_id}`,
          cursor: event.transcript_cursor,
          audit: event.audit,
        },
      );
      return next;

    case "tool_call_started": {
      // The foreground slot is created here, exactly as the Rust projection
      // creates it, so an incremental client and a fresh snapshot describe
      // the same execution list. Deferring the slot to
      // `tool_execution_started` would lose the runtime-published name and
      // arguments, which that event does not repeat.
      withStreaming(next, state, event.message_id, (message) => ({
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
      return withForeground(next, state, event.attempt_id, (foreground) =>
        upsertForeground(foreground, event.call.id, (existing) => ({
          call_id: event.call.id,
          tool_id: event.call.tool_id,
          name: event.call.name,
          state: existing?.state ?? { type: "assembled", arguments: "" },
        })),
      );
    }

    case "tool_call_arguments_delta": {
      withStreaming(next, state, event.message_id, (message) => ({
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
      return withForeground(next, state, event.attempt_id, (foreground) =>
        updateForeground(foreground, event.call_id, (existing) =>
          withArguments(
            existing,
            argumentsOf(existing) + event.arguments_delta,
          ),
        ),
      );
    }

    case "tool_call_assembled": {
      const assembled = JSON.stringify(event.call.arguments);
      withStreaming(next, state, event.message_id, (message) => ({
        ...message,
        blocks: message.blocks.map((block) =>
          block.kind === "tool_call" && block.callId === event.call.id
            ? {
                ...block,
                name: event.call.name,
                argumentsText: assembled,
              }
            : block,
        ),
      }));
      return withForeground(next, state, event.attempt_id, (foreground) =>
        updateForeground(foreground, event.call.id, (existing) => ({
          ...withArguments(existing, assembled),
          name: event.call.name,
        })),
      );
    }

    case "tool_execution_started":
      return withForeground(next, state, event.attempt_id, (foreground) =>
        upsertForeground(foreground, event.tool_call_id, (existing) => ({
          call_id: event.tool_call_id,
          tool_id: event.tool_id,
          name: existing?.name ?? "",
          state:
            existing?.state.type === "settled"
              ? existing.state
              : {
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
          state:
            existing?.state.type === "settled"
              ? existing.state
              : {
                  type: "running",
                  arguments: argumentsOf(existing),
                  progress: event.progress,
                },
        })),
      );

    case "tool_execution_settled":
      return withForeground(next, state, event.attempt_id, (foreground) =>
        settleForeground(
          foreground,
          event.tool_call_id,
          event.result,
        ),
      );

    case "message_committed": {
      if (isHiddenContextMessage(event.message)) {
        return next;
      }
      if (transcriptCursor === undefined) {
        throw new Error(
          "visible message_committed event is missing its durable transcript cursor",
        );
      }
      const messageId = messageIdOf(event.message);
      const transcript =
        event.message.role === "assistant" && event.attempt_id !== undefined
          ? dropStreaming(state.transcript, event.attempt_id)
          : state.transcript;
      next.transcript = appendTranscriptEntry(transcript, {
        kind: "committed",
        key: `committed:${messageId}`,
        messageId,
        cursor: transcriptCursor,
        attemptId: event.attempt_id,
        message: event.message,
      });
      // A committed `todo` result *is* the list moving, so the panel follows
      // it live without waiting for the next snapshot — the same derivation
      // the runtime runs over the same fact.
      const todos = publishedTodos(event.message);
      if (todos !== undefined) {
        next.todos = todos;
      }
      // A canonical ToolMessage is also the authoritative repair path for a
      // foreground slot whose live execution settlement was not published
      // (for example BeforeStart cancellation). The Rust projection normally
      // emits one equivalent `tool_execution_settled` event before this
      // message, so this update is idempotent and also keeps the reducer safe
      // when the commit is the first fact it sees. The result is copied from
      // the typed message; phase is never inferred by the TUI.
      if (event.message.role === "tool" && event.attempt_id !== undefined) {
        const toolMessage = event.message;
        withForeground(next, state, event.attempt_id, (foreground) =>
          settleForeground(
            foreground,
            toolMessage.tool_call_id,
            toolMessage.result,
          ),
        );
      }
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
      // Durable acceptance is the display frontier. Context facts remain
      // model-visible runtime input but are hidden from ordinary chat.
      if (!isHiddenContextMessage(event.message)) {
        if (transcriptCursor === undefined) {
          throw new Error(
            "visible inbound_enqueued event is missing its durable transcript cursor",
          );
        }
        next.transcript = appendTranscriptEntry(state.transcript, {
          kind: "committed",
          key: `committed:${event.message.id}`,
          messageId: event.message.id,
          cursor: transcriptCursor,
          message: { role: "user", ...event.message },
        });
      }
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

    case "capability_updated":
      next.capabilities = event.capabilities;
      return next;

    case "resource_generation_updated":
      // A reload commits the resource generation and the capability
      // generation it was composed against as one fact, and publishes them
      // as one event at one cursor. Both halves are folded here, together:
      // there is no cut of this reducer at which the client holds the new
      // capability generation beside the resource generation the same
      // reload retired.
      next.capabilities = event.capabilities;
      next.resources = event.resources;
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
      // RuntimeClientConnection validates the Runtime Client event vocabulary before an
      // event reaches this reducer. This branch is unreachable unless a
      // caller bypasses that boundary, and must never advance the cursor.
      const exhaustiveEvent: never = event;
      throw new Error(
        `unreachable Runtime Client event: ${String(exhaustiveEvent)}`,
      );
  }
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

function transcriptEntryFromWire(
  entry: RuntimeClientTranscriptEntry,
): TranscriptEntry {
  switch (entry.item.type) {
    case "message": {
      const messageId = messageIdOf(entry.item.message);
      return {
        kind: "committed",
        key: `committed:${messageId}`,
        messageId,
        cursor: entry.cursor,
        message: entry.item.message,
      };
    }
    case "publication_audit":
      return {
        kind: "publication_audit",
        key: `publication:${entry.item.audit.stream_id}`,
        cursor: entry.cursor,
        audit: entry.item.audit,
      };
    case "interaction_requested":
      return {
        kind: "interaction_requested",
        key: `interaction-requested:${entry.item.event_id}`,
        cursor: entry.cursor,
        eventId: entry.item.event_id,
        timestamp: entry.item.timestamp,
        attemptId: entry.item.attempt_id,
        turnId: entry.item.turn_id,
        interactionId: entry.item.interaction_id,
        subject: entry.item.subject,
      };
    case "interaction_settled":
      return {
        kind: "interaction_settled",
        key: `interaction-settled:${entry.item.event_id}`,
        cursor: entry.cursor,
        eventId: entry.item.event_id,
        timestamp: entry.item.timestamp,
        attemptId: entry.item.attempt_id,
        turnId: entry.item.turn_id,
        interactionId: entry.item.interaction_id,
        settlement: entry.item.settlement,
      };
  }
}

function appendTranscriptEntry(
  transcript: TranscriptEntry[],
  entry: TranscriptEntry,
): TranscriptEntry[] {
  const existing = transcript.find((candidate) => candidate.key === entry.key);
  if (existing !== undefined) {
    if (
      existing.kind !== "streaming" &&
      entry.kind !== "streaming" &&
      existing.cursor !== entry.cursor
    ) {
      throw new Error(
        `transcript fact ${entry.key} changed durable cursor from ${existing.cursor} to ${entry.cursor}`,
      );
    }
    return transcript;
  }
  return orderTranscript([...transcript, entry]);
}

type DurableTranscriptEntry = Exclude<TranscriptEntry, StreamingMessage>;

function isDurableTranscriptEntry(
  entry: TranscriptEntry,
): entry is DurableTranscriptEntry {
  return entry.kind !== "streaming";
}

/** Orders every durable item by the cursor allocated by `transcript_order`. */
function orderTranscript(transcript: TranscriptEntry[]): TranscriptEntry[] {
  const durable = transcript
    .filter(isDurableTranscriptEntry)
    .sort((left, right) => left.cursor - right.cursor);
  const streaming = transcript.filter((entry) => entry.kind === "streaming");
  return [...durable, ...streaming];
}

/** Deduplicates the same durable fact without using identity to infer order. */
function deduplicateTranscript(transcript: TranscriptEntry[]): TranscriptEntry[] {
  const seen = new Map<string, TranscriptEntry>();
  for (const entry of transcript) {
    const existing = seen.get(entry.key);
    if (
      existing !== undefined &&
      isDurableTranscriptEntry(existing) &&
      isDurableTranscriptEntry(entry) &&
      existing.cursor !== entry.cursor
    ) {
      throw new Error(
        `transcript fact ${entry.key} has conflicting durable cursors`,
      );
    }
    if (existing === undefined) {
      seen.set(entry.key, entry);
    }
  }
  return [...seen.values()];
}

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

/** Replaces the arguments of one foreground slot, keeping its lifecycle. */
function withArguments(
  slot: AttemptPresentation["foreground"][number],
  args: string,
): AttemptPresentation["foreground"][number] {
  switch (slot.state.type) {
    case "assembled":
      return { ...slot, state: { type: "assembled", arguments: args } };
    case "running":
      return {
        ...slot,
        state: {
          type: "running",
          arguments: args,
          progress: slot.state.progress,
        },
      };
    default:
      return {
        ...slot,
        state: {
          type: "settled",
          arguments: args,
          result: slot.state.result,
        },
      };
  }
}

/**
 * Updates an existing foreground slot, or does nothing.
 *
 * Unlike {@link upsertForeground} this never creates a slot: an argument
 * fragment for a call the projection has not seen started describes nothing
 * the runtime published, so it is dropped rather than guessed into existence.
 */
function updateForeground(
  foreground: AttemptPresentation["foreground"],
  callId: string,
  update: (
    existing: AttemptPresentation["foreground"][number],
  ) => AttemptPresentation["foreground"][number],
): AttemptPresentation["foreground"] {
  const index = foreground.findIndex((entry) => entry.call_id === callId);
  if (index === -1) {
    return foreground;
  }
  const updated = [...foreground];
  updated[index] = update(foreground[index]!);
  return updated;
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

/**
 * Settles a foreground slot from an already-authoritative typed result.
 *
 * A canonical ToolMessage is the repair path for a matching slot belonging to
 * a call that never published a live execution settlement. Once a slot is
 * settled, later lifecycle facts are ignored so the client never emits or
 * displays a second terminal result. A commit never invents a slot: accepted
 * calls already assembled one, and a missing identity is not a projection
 * fact the message can reconstruct.
 */
function settleForeground(
  foreground: AttemptPresentation["foreground"],
  callId: string,
  result: ToolExecutionResult,
): AttemptPresentation["foreground"] {
  const existing = foreground.find((entry) => entry.call_id === callId);
  if (existing === undefined || existing.state.type === "settled") {
    return foreground;
  }
  return updateForeground(foreground, callId, (existing) => ({
    ...existing,
    state: {
      type: "settled",
      arguments: argumentsOf(existing),
      result,
    },
  }));
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

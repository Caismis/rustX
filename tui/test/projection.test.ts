/**
 * The presentation reducer over scripted runtime facts.
 *
 * Every case drives exact protocol events. The reducer is pure, so each
 * assertion is about a value, never about timing.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  mergeTranscriptPage,
  reduce,
  replaceFromSnapshot,
} from "../src/presentation/projection.ts";
import type { PresentationState, StreamingMessage } from "../src/presentation/state.ts";
import type {
  RuntimeClientEvent,
  RuntimeClientProtocolEvent,
} from "../src/protocol/types.ts";
import {
  assistantMessage,
  approvalInteraction,
  attemptModel,
  backgroundExecution,
  contextUserMessage,
  capabilities,
  runtimeInbound,
  sessionModel,
  questionnaireInteraction,
  runtimeCursor,
  snapshot,
  transcriptCursor,
  toolMessage,
  toolResult,
  userMessage,
} from "./support/fixtures.ts";

/** Folds a scripted event sequence, allocating cursors in order. */
function fold(
  state: PresentationState,
  events: RuntimeClientEvent[],
  startCursor = state.cursor,
): PresentationState {
  let current = state;
  let cursor = startCursor;
  for (const event of events) {
    cursor = runtimeCursor(cursor + 1);
    const protocolEvent: RuntimeClientProtocolEvent = { cursor, event };
    current = reduce(current, protocolEvent);
  }
  return current;
}

function streamingOf(state: PresentationState): StreamingMessage | undefined {
  return state.transcript.find(
    (entry): entry is StreamingMessage => entry.kind === "streaming",
  );
}

const initial = () => replaceFromSnapshot(snapshot(), runtimeCursor(0));

describe("presentation projection", () => {
  it("derives the initial state from an authoritative snapshot", () => {
    const state = replaceFromSnapshot(
      snapshot({
        conversation_id: "conv-1",
        messages: [userMessage("m1", "hello"), assistantMessage("m2", "hi")],
        capabilities: capabilities(4),
      }),
      runtimeCursor(9),
    );

    assert.equal(state.conversationId, "conv-1");
    assert.equal(state.cursor, 9);
    assert.equal(state.transcript.length, 2);
    assert.equal(state.capabilities.revision, 4);
    assert.equal(
      state.capabilities.skills?.[0]?.location,
      ".rustx/skills/review/SKILL.md",
    );
    assert.equal(state.sessionModel.configured.model, "alpha/model-a");
    assert.equal(state.attempt, undefined);
  });

  it("boots from the bounded transcript page, not the current Surface messages", () => {
    const state = replaceFromSnapshot(
      snapshot({
        messages: [assistantMessage("surface-only", "current Surface")],
        transcript: {
          entries: [
            {
              cursor: transcriptCursor(4),
              item: {
                type: "message",
                message: userMessage("historical", "retained history"),
              },
            },
          ],
          next_cursor: transcriptCursor(3),
        },
      }),
      runtimeCursor(12),
    );

    assert.deepEqual(
      state.transcript.map((entry) => entry.kind === "committed" && entry.messageId),
      ["historical"],
    );
    assert.equal(state.transcriptNextCursor, 3);
  });

  it("prepends an older page once, without moving the live event cursor", () => {
    const state = replaceFromSnapshot(
      snapshot({
        transcript: {
          entries: [
            {
              cursor: transcriptCursor(3),
              item: {
                type: "message",
                message: userMessage("middle", "middle"),
              },
            },
            {
              cursor: transcriptCursor(4),
              item: {
                type: "message",
                message: assistantMessage("newest", "newest"),
              },
            },
          ],
          next_cursor: transcriptCursor(2),
        },
      }),
      runtimeCursor(19),
    );
    const merged = mergeTranscriptPage(state, {
      entries: [
        {
          cursor: transcriptCursor(2),
          item: {
            type: "message",
            message: userMessage("oldest", "oldest"),
          },
        },
        {
          cursor: transcriptCursor(3),
          item: {
            type: "message",
            message: userMessage("middle", "duplicate"),
          },
        },
      ],
      next_cursor: undefined,
    });

    assert.deepEqual(
      merged.transcript.map((entry) => entry.kind === "committed" && entry.messageId),
      ["oldest", "middle", "newest"],
    );
    assert.equal(merged.cursor, 19);
    assert.equal(merged.transcriptNextCursor, undefined);
  });

  it("orders live and paged facts by durable cursor, not observation arrival", () => {
    let state = initial();
    state = reduce(state, {
      cursor: runtimeCursor(40),
      event: {
        type: "message_committed",
        message: assistantMessage("message-b", "B"),
        transcript_cursor: transcriptCursor(11),
      },
    });
    state = reduce(state, {
      cursor: runtimeCursor(41),
      event: {
        type: "inbound_enqueued",
        sequence: 1,
        message: userMessage("message-a", "A"),
        transcript_cursor: transcriptCursor(10),
      },
    });

    assert.deepEqual(
      state.transcript.map((entry) => entry.kind === "committed" && entry.messageId),
      ["message-a", "message-b"],
      "reverse observation order is repaired by the durable cursor",
    );
    assert.deepEqual(
      state.transcript.map((entry) => entry.kind === "committed" && entry.cursor),
      [10, 11],
    );
    assert.equal(state.cursor, 41, "live event cursor remains its own domain");

    const merged = mergeTranscriptPage(state, {
      entries: [
        {
          cursor: transcriptCursor(9),
          item: { type: "message", message: userMessage("message-old", "old") },
        },
        {
          cursor: transcriptCursor(11),
          item: { type: "message", message: assistantMessage("message-b", "duplicate") },
        },
      ],
      next_cursor: transcriptCursor(8),
    });
    assert.deepEqual(
      merged.transcript.map((entry) => entry.kind === "committed" && entry.messageId),
      ["message-old", "message-a", "message-b"],
    );
    assert.equal(merged.cursor, 41, "paging never advances the live cursor");

    const resynced = replaceFromSnapshot(
      snapshot({
        transcript: {
          entries: [
            {
              cursor: transcriptCursor(11),
              item: {
                type: "message",
                message: assistantMessage("message-b", "B"),
              },
            },
            {
              cursor: transcriptCursor(10),
              item: { type: "message", message: userMessage("message-a", "A") },
            },
            {
              cursor: transcriptCursor(9),
              item: { type: "message", message: userMessage("message-old", "old") },
            },
          ],
          next_cursor: transcriptCursor(8),
        },
      }),
      runtimeCursor(41),
    );
    assert.deepEqual(
      resynced.transcript.map((entry) => entry.kind === "committed" && entry.messageId),
      ["message-old", "message-a", "message-b"],
      "snapshot/resync keeps the durable relative order",
    );

    state = reduce(merged, {
      cursor: runtimeCursor(42),
      event: {
        type: "assistant_publication_settled",
        attempt_id: "attempt-1",
        transcript_cursor: transcriptCursor(12),
        audit: {
          stream_id: "stream-audit",
          attempt_id: "attempt-1",
          turn_id: "turn-1",
          request_id: "request-1",
          message_id: "message-provisional",
          kind: "incomplete",
          content: [
            {
              kind: "proposed_tool_call",
              block_index: 0,
              call_id: "call-proposed",
              tool_id: "tool-read",
              name: "Read",
              arguments: "{}",
              complete: true,
            },
          ],
          settled_at: "2026-08-24T12:00:00Z",
        },
      },
    });
    const publication = state.transcript.at(-1);
    assert.equal(publication?.kind, "publication_audit");
    assert.equal(
      publication?.kind === "publication_audit"
        ? publication.cursor
        : undefined,
      12,
    );
    assert.equal(state.attempt?.foreground.length ?? 0, 0);

    state = reduce(state, {
      cursor: runtimeCursor(43),
      event: {
        type: "interaction_audit_requested",
        transcript_cursor: transcriptCursor(13),
        audit: {
          event_id: "interaction-requested-event-1",
          timestamp: "2026-08-24T12:00:00Z",
          attempt_id: "attempt-1",
          turn_id: "turn-1",
          interaction_id: "interaction-1",
          subject: {
            type: "questionnaire",
            questionnaire: {
              questions: [
                {
                  question: "Which environment?",
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
        },
      },
    });
    const requested = state.transcript.at(-1);
    assert.equal(requested?.kind, "interaction_requested");
    assert.equal(
      requested?.kind === "interaction_requested"
        ? requested.cursor
        : undefined,
      13,
    );
    assert.equal(state.pendingInteractions.length, 0, "historical audit is not a waiter");
  });

  it("adds a user transcript row only when the durable acceptance event arrives", () => {
    const state = initial();
    assert.equal(state.transcript.length, 0);
    const accepted = reduce(state, {
      cursor: runtimeCursor(1),
      event: {
        type: "inbound_enqueued",
        sequence: 1,
        message: {
          id: "accepted-user",
          content: [{ type: "text", text: "accepted" }],
          source: "human",
          kind: "message",
        },
        transcript_cursor: transcriptCursor(1),
      },
    });
    assert.deepEqual(
      accepted.transcript.map((entry) => entry.kind === "committed" && entry.messageId),
      ["accepted-user"],
    );
  });

  it("keeps Agent Status context out of the normal transcript", () => {
    const state = reduce(initial(), {
      cursor: runtimeCursor(1),
      event: {
        type: "inbound_enqueued",
        sequence: 1,
        message: {
          id: "status",
          content: [{ type: "text", text: "internal" }],
          source: "runtime",
          kind: { context: "agent_status" },
        },
        transcript_cursor: undefined,
      },
    });
    assert.equal(state.transcript.length, 0);
    assert.equal(state.inbound.pending?.length, 1);
  });

  it("folds live approvals and repairs them from snapshot/resync state", () => {
    const interaction = approvalInteraction();
    const pending = fold(initial(), [
      { type: "interaction_pending", interaction },
    ]);

    assert.deepEqual(pending.pendingInteractions, [interaction]);

    const settled = fold(pending, [
      {
        type: "interaction_settled",
        interaction_id: interaction.id,
        outcome: {
          type: "responded",
          response: { type: "approval", decision: { type: "allow" } },
        },
      },
    ]);
    assert.deepEqual(settled.pendingInteractions, []);

    const repaired = replaceFromSnapshot(
      snapshot({ pending_interactions: [interaction] }),
      runtimeCursor(8),
    );
    assert.deepEqual(repaired.pendingInteractions, [interaction]);
    assert.equal(repaired.cursor, 8);
  });

  it("folds Questionnaires and authoritative ApprovalMode changes", () => {
    const question = questionnaireInteraction();
    const state = fold(initial(), [
      { type: "interaction_pending", interaction: question },
      {
        type: "approval_mode_changed",
        effective_approval_mode: "policy",
        pending_approval_mode: "full_access",
        revision: 1,
      },
    ]);

    assert.deepEqual(state.pendingInteractions, [question]);
    assert.equal(state.effectiveApprovalMode, "policy");
    assert.equal(state.pendingApprovalMode, "full_access");
    assert.equal(state.approvalModeRevision, 1);
  });

  it("folds committed compaction diagnostics and rebuilds them from a snapshot", () => {
    const state = fold(initial(), [
      {
        type: "context_compacted",
        attempt_id: "a1",
        context: {
          compaction_in_progress: false,
          compaction_count: 2,
          latest_compaction: {
            generation: 2,
            summary_message_id: "conv-1-summary-2",
            surface_revision: 7,
            tokens_before: { input_tokens: 4_700, source: "estimated" },
            estimated_tokens_after: 1_800,
          },
        },
      },
    ]);

    assert.equal(state.context.compaction_count, 2);
    assert.equal(state.context.latest_compaction?.generation, 2);
    assert.equal(
      replaceFromSnapshot(
        snapshot({ context: state.context }),
        state.cursor,
      ).context.latest_compaction?.tokens_before.source,
      "estimated",
    );
  });

  it("rebuilds a mid-stream assistant message from the snapshot alone", () => {
    const state = replaceFromSnapshot(
      snapshot({
        attempt: {
          attempt_id: "a1",
          phase: { type: "running" },
          turn: 1,
          model: attemptModel("alpha/model-a"),
          in_flight: {
            message_id: "m9",
            blocks: [
              { type: "reasoning", block_index: 0, text: "thinking" },
              { type: "text", block_index: 1, text: "partial" },
              {
                type: "tool_call",
                block_index: 2,
                call_id: "c1",
                tool_id: "tool-bash",
                name: "bash",
                arguments: '{"cmd":',
              },
            ],
          },
        },
      }),
      runtimeCursor(5),
    );

    const streaming = streamingOf(state);
    assert.ok(streaming);
    assert.deepEqual(
      streaming.blocks.map((block) => block.kind),
      ["reasoning", "text", "tool_call"],
    );
  });

  it("carries the frozen attempt model straight off attempt_started", () => {
    const state = fold(initial(), [
      { type: "attempt_started", attempt_id: "a1", model: attemptModel("alpha/model-a") },
    ]);

    assert.equal(state.attempt?.attemptId, "a1");
    assert.equal(state.attempt?.model.primary.model, "alpha/model-a");
    assert.equal(state.attempt?.phase.type, "running");
  });

  it("updates turn and normalized usage incrementally and reconstructs the same values", () => {
    const usage = {
      input_tokens: 10,
      output_tokens: 4,
      total_tokens: 14,
      details: { cached_input_tokens: 2 },
    };
    const incremental = fold(initial(), [
      { type: "attempt_started", attempt_id: "a1", model: attemptModel("alpha/model-a") },
      { type: "attempt_turn_updated", attempt_id: "a1", turn: 1 },
      { type: "attempt_turn_updated", attempt_id: "a1", turn: 2 },
      { type: "attempt_usage_updated", attempt_id: "a1", usage },
    ]);

    assert.equal(incremental.attempt?.turn, 2);
    assert.deepEqual(incremental.attempt?.lastUsage, usage);

    const repaired = replaceFromSnapshot(
      snapshot({
        attempt: {
          attempt_id: "a1",
          phase: { type: "running" },
          turn: 2,
          last_usage: usage,
          model: attemptModel("alpha/model-a"),
        },
      }),
      incremental.cursor,
    );
    assert.equal(repaired.attempt?.turn, incremental.attempt?.turn);
    assert.deepEqual(repaired.attempt?.lastUsage, incremental.attempt?.lastUsage);
  });

  it("accumulates streaming text, reasoning, and refusal as distinct kinds", () => {
    const state = fold(initial(), [
      { type: "attempt_started", attempt_id: "a1", model: attemptModel("alpha/model-a") },
      { type: "assistant_message_started", attempt_id: "a1", message_id: "m1" },
      {
        type: "assistant_reasoning_delta",
        attempt_id: "a1",
        message_id: "m1",
        block_index: 0,
        delta: "let me ",
      },
      {
        type: "assistant_reasoning_delta",
        attempt_id: "a1",
        message_id: "m1",
        block_index: 0,
        delta: "think",
      },
      {
        type: "assistant_text_delta",
        attempt_id: "a1",
        message_id: "m1",
        block_index: 1,
        delta: "Hello",
      },
      {
        type: "assistant_text_delta",
        attempt_id: "a1",
        message_id: "m1",
        block_index: 1,
        delta: " world",
      },
      {
        type: "assistant_refusal_delta",
        attempt_id: "a1",
        message_id: "m1",
        block_index: 2,
        delta: "I cannot",
      },
    ]);

    const streaming = streamingOf(state);
    assert.ok(streaming);
    assert.deepEqual(streaming.blocks, [
      { kind: "reasoning", blockIndex: 0, text: "let me think" },
      { kind: "text", blockIndex: 1, text: "Hello world" },
      { kind: "refusal", blockIndex: 2, text: "I cannot" },
    ]);
    // Reasoning and refusal are never flattened into assistant text.
    assert.equal(
      streaming.blocks.filter((block) => block.kind === "text").length,
      1,
    );
  });

  it("keeps block identity from block_index, not from arrival order", () => {
    const state = fold(initial(), [
      { type: "attempt_started", attempt_id: "a1", model: attemptModel("alpha/model-a") },
      { type: "assistant_message_started", attempt_id: "a1", message_id: "m1" },
      // Interleaved blocks: index 1 opens before index 0 finishes.
      { type: "assistant_text_delta", attempt_id: "a1", message_id: "m1", block_index: 0, delta: "A" },
      { type: "assistant_text_delta", attempt_id: "a1", message_id: "m1", block_index: 1, delta: "B" },
      { type: "assistant_text_delta", attempt_id: "a1", message_id: "m1", block_index: 0, delta: "A2" },
    ]);

    const streaming = streamingOf(state);
    assert.deepEqual(
      streaming?.blocks.map((block) =>
        block.kind === "tool_call" ? block.name : block.text,
      ),
      ["AA2", "B"],
    );
  });

  it("replaces the streaming message with the committed one", () => {
    const state = fold(initial(), [
      { type: "attempt_started", attempt_id: "a1", model: attemptModel("alpha/model-a") },
      { type: "assistant_message_started", attempt_id: "a1", message_id: "m1" },
      { type: "assistant_text_delta", attempt_id: "a1", message_id: "m1", block_index: 0, delta: "hi" },
      {
        type: "message_committed",
        attempt_id: "a1",
        message: assistantMessage("m1", "hi"),
        transcript_cursor: transcriptCursor(1),
      },
    ]);

    assert.equal(streamingOf(state), undefined);
    assert.equal(state.transcript.length, 1);
    assert.equal(state.transcript[0]?.kind, "committed");
  });

  it("renders committed human and runtime-originated inbound distinctly", () => {
    const state = fold(initial(), [
      {
        type: "message_committed",
        message: userMessage("m1", "from a human"),
        transcript_cursor: transcriptCursor(1),
      },
      {
        type: "message_committed",
        message: runtimeInbound("m2", "background work finished"),
        transcript_cursor: transcriptCursor(2),
      },
    ]);

    const sources = state.transcript.map((entry) =>
      entry.kind === "committed" && entry.message.role === "user"
        ? entry.message.source
        : undefined,
    );
    assert.deepEqual(sources, ["human", "runtime"]);
  });

  it("tracks foreground tool lifecycle by logical call identity", () => {
    const state = fold(initial(), [
      { type: "attempt_started", attempt_id: "a1", model: attemptModel("alpha/model-a") },
      {
        type: "tool_execution_started",
        attempt_id: "a1",
        tool_call_id: "c1",
        tool_id: "tool-bash",
      },
      {
        type: "tool_execution_progress",
        attempt_id: "a1",
        tool_call_id: "c1",
        tool_id: "tool-bash",
        progress: { message: "halfway", completed: 1, total: 2 },
      },
      {
        type: "tool_execution_settled",
        attempt_id: "a1",
        tool_call_id: "c1",
        tool_id: "tool-bash",
        result: toolResult(),
      },
    ]);

    assert.equal(state.attempt?.foreground.length, 1);
    const execution = state.attempt?.foreground[0];
    assert.equal(execution?.call_id, "c1");
    assert.equal(execution?.state.type, "settled");
  });

  it("keeps parallel foreground calls on their own identities", () => {
    const state = fold(initial(), [
      { type: "attempt_started", attempt_id: "a1", model: attemptModel("alpha/model-a") },
      { type: "tool_execution_started", attempt_id: "a1", tool_call_id: "c1", tool_id: "t1" },
      { type: "tool_execution_started", attempt_id: "a1", tool_call_id: "c2", tool_id: "t2" },
      // The second call settles first; the first must not be corrupted.
      {
        type: "tool_execution_settled",
        attempt_id: "a1",
        tool_call_id: "c2",
        tool_id: "t2",
        result: toolResult(),
      },
    ]);

    const byId = new Map(
      state.attempt?.foreground.map((entry) => [entry.call_id, entry.state.type]),
    );
    assert.deepEqual([...byId], [
      ["c1", "running"],
      ["c2", "settled"],
    ]);
  });

  it("routes detached progress to background, not to the foreground list", () => {
    const state = fold(initial(), [
      { type: "attempt_started", attempt_id: "a1", model: attemptModel("alpha/model-a") },
      {
        type: "tool_execution_progress",
        attempt_id: "a1",
        tool_call_id: "c1",
        tool_id: "tool-background",
        execution_id: "exec-1",
        progress: { message: "detached" },
      },
    ]);

    assert.deepEqual(state.attempt?.foreground, []);
  });

  it("keeps background executions alive independently of the transcript", () => {
    let state = fold(initial(), [
      { type: "attempt_started", attempt_id: "a1", model: attemptModel("alpha/model-a") },
      {
        type: "background_execution_updated",
        execution: backgroundExecution("exec-1", "running"),
      },
      {
        type: "attempt_settled",
        attempt_id: "a1",
        outcome: { type: "completed", finish_reason: { type: "stop" } },
      },
    ]);

    // The attempt settled; the conversation-owned execution did not.
    assert.equal(state.background.length, 1);
    assert.equal(state.background[0]?.state, "running");

    state = fold(state, [
      {
        type: "background_execution_updated",
        execution: backgroundExecution("exec-1", "succeeded", {
          result: toolResult(),
        }),
      },
      {
        type: "message_committed",
        message: runtimeInbound("m9", "background_task finished"),
        transcript_cursor: transcriptCursor(1),
      },
    ]);

    assert.equal(state.background.length, 1, "updates replace, never duplicate");
    assert.equal(state.background[0]?.state, "succeeded");
    assert.equal(state.transcript.length, 1);
  });

  it("folds inbound enqueue and finite drain from runtime facts", () => {
    let state = fold(initial(), [
      {
        type: "inbound_enqueued",
        sequence: 1,
        message: userMessage("m1", "first"),
        transcript_cursor: transcriptCursor(1),
      },
      {
        type: "inbound_enqueued",
        sequence: 2,
        message: userMessage("m2", "second"),
        transcript_cursor: transcriptCursor(2),
      },
    ]);
    assert.equal(state.inbound.pending?.length, 2);

    state = fold(state, [
      {
        type: "inbound_drained",
        watermark: 1,
        count: 1,
        message_ids: ["m1"],
      },
    ]);

    // The drain is finite: post-watermark arrivals wait for the next one.
    assert.deepEqual(
      state.inbound.pending?.map((item) => item.sequence),
      [2],
    );
    assert.deepEqual(state.inbound.last_drain, { watermark: 1, count: 1 });
  });

  it("folds Agent Status without composing one", () => {
    const status = {
      attempt_id: "a1",
      turn: 1,
      status_message_id: "status-1",
      opportunities: { fresh_inbound: { target_message_id: "m1" } },
      sections: [
        {
          type: "temporal" as const,
          current_time: "2026-08-14T00:00:00Z",
          inbound_message_time: "2026-08-14T00:00:00Z",
        },
      ],
      rendered: "## Status\ncurrent time: 2026-08-14",
    };
    const state = fold(initial(), [
      { type: "attempt_started", attempt_id: "a1", model: attemptModel("alpha/model-a") },
      {
        type: "agent_status_composed",
        attempt_id: "a1",
        turn: 1,
        status,
      },
    ]);

    // The rendered form comes from the runtime's own composition.
    assert.equal(state.status?.rendered, status.rendered);
    assert.deepEqual(state.status?.sections, status.sections);
  });

  it("folds a capability revision swap", () => {
    const state = fold(initial(), [
      { type: "capability_updated", capabilities: capabilities(7) },
    ]);
    assert.equal(state.capabilities.revision, 7);
    assert.equal(state.capabilities.tools?.length, 2);
  });

  it("marks runtime drain without inventing a settlement event", () => {
    const state = fold(initial(), [
      { type: "attempt_started", attempt_id: "a1", model: attemptModel("alpha/model-a") },
      { type: "runtime_shutdown" },
    ]);

    assert.equal(state.runtimeShutdown, true);
    // The projection event marks admission closure; the runtime owns the
    // later cancellation and terminal settlement facts.
    assert.equal(state.attempt?.phase.type, "running");
  });

  it("derives the shutdown marker directly from an authoritative snapshot", () => {
    const state = replaceFromSnapshot(
      snapshot({ shutting_down: true }),
      runtimeCursor(8),
    );
    assert.equal(state.runtimeShutdown, true);
  });

  it("records every attempt settlement kind faithfully", () => {
    const outcomes = [
      { type: "completed" as const, finish_reason: { type: "stop" as const } },
      { type: "cancelled" as const, reason: "user_requested" as const },
      { type: "timed_out" as const },
      { type: "limit_exceeded" as const, limit: "max_turns" as const },
      {
        type: "failed" as const,
        error: {
          type: "model" as const,
          kind: "rate_limit" as const,
          message: "slow down",
          retry_after_ms: 1_000,
        },
      },
    ];

    for (const outcome of outcomes) {
      const state = fold(initial(), [
        { type: "attempt_started", attempt_id: "a1", model: attemptModel("alpha/model-a") },
        { type: "attempt_settled", attempt_id: "a1", outcome },
      ]);
      assert.deepEqual(
        state.attempt?.phase,
        { type: "settled", outcome },
        `outcome ${outcome.type}`,
      );
    }
  });

  it("fails closed if a caller bypasses protocol event validation", () => {
    assert.throws(
      () =>
        reduce(initial(), {
          cursor: runtimeCursor(42),
          event: { type: "future_variant" } as unknown as RuntimeClientEvent,
        }),
      /unreachable Runtime Client Protocol v3 event/,
    );
  });

  it("fails closed for a visible Assistant commit without a transcript cursor", () => {
    const accepted = fold(initial(), [
      {
        type: "message_committed",
        message: assistantMessage("accepted", "kept"),
        transcript_cursor: transcriptCursor(1),
      },
    ]);
    const before = accepted.transcript.map((entry) => ({ ...entry }));

    assert.throws(
      () =>
        reduce(accepted, {
          cursor: runtimeCursor(42),
          event: {
            type: "message_committed",
            message: assistantMessage("missing-assistant-cursor", "bad"),
          },
        }),
      /visible transcript message is missing/,
    );
    assert.equal(accepted.cursor, 1, "the failed event never advances the cursor");
    assert.deepEqual(accepted.transcript, before, "the accepted state is unchanged");
  });

  it("fails closed for a visible Tool commit without a transcript cursor", () => {
    assert.throws(
      () =>
        reduce(initial(), {
          cursor: runtimeCursor(42),
          event: {
            type: "message_committed",
            message: toolMessage("missing-tool-cursor", "call-1", "tool-read"),
          },
        }),
      /visible transcript message is missing/,
    );
  });

  it("fails closed for an ordinary User commit without a transcript cursor", () => {
    assert.throws(
      () =>
        reduce(initial(), {
          cursor: runtimeCursor(42),
          event: {
            type: "message_committed",
            message: userMessage("missing-user-cursor", "bad"),
          },
        }),
      /visible transcript message is missing/,
    );
  });

  it("keeps a hidden Context commit out of the transcript without a cursor", () => {
    const state = reduce(initial(), {
      cursor: runtimeCursor(7),
      event: {
        type: "message_committed",
        message: contextUserMessage("hidden-context", "runtime status"),
      },
    });

    assert.equal(state.cursor, 7);
    assert.equal(state.transcript.length, 0);
  });

  it("rejects a hidden Context commit carrying a contradictory cursor", () => {
    const state = initial();
    assert.throws(
      () =>
        reduce(state, {
          cursor: runtimeCursor(42),
          event: {
            type: "message_committed",
            message: contextUserMessage("hidden-context", "runtime status"),
            transcript_cursor: transcriptCursor(8),
          },
        }),
      /hidden Context message must not carry/,
    );
    assert.equal(state.cursor, 0);
    assert.equal(state.transcript.length, 0);
  });

  it("fails closed for visible inbound acceptance without a transcript cursor", () => {
    assert.throws(
      () =>
        reduce(initial(), {
          cursor: runtimeCursor(42),
          event: {
            type: "inbound_enqueued",
            sequence: 1,
            message: userMessage("missing-inbound-cursor", "bad"),
          },
        }),
      /visible transcript message is missing/,
    );
  });

  it("rejects hidden Context inbound with a contradictory cursor", () => {
    const state = initial();
    assert.throws(
      () =>
        reduce(state, {
          cursor: runtimeCursor(42),
          event: {
            type: "inbound_enqueued",
            sequence: 1,
            message: contextUserMessage("hidden-inbound", "runtime status"),
            transcript_cursor: transcriptCursor(8),
          },
        }),
      /hidden Context message must not carry/,
    );
    assert.equal(state.cursor, 0);
    assert.equal(state.transcript.length, 0);
  });

  it("displays inbound input only after durable acceptance", () => {
    const state = initial();
    // No client-side semantic echo exists before the runtime fact arrives.
    assert.equal(state.transcript.length, 0);

    const accepted = fold(state, [
      {
        type: "inbound_enqueued",
        sequence: 1,
        message: userMessage("m1", "hello"),
        transcript_cursor: transcriptCursor(1),
      },
    ]);

    assert.equal(accepted.transcript.length, 1);
    assert.equal(accepted.transcript[0]?.kind, "committed");
    assert.equal(accepted.inbound.pending?.[0]?.message.id, "m1");
  });

  it("does not carry local semantic state through an authoritative repair", () => {
    const repaired = replaceFromSnapshot(
      snapshot({
        messages: [userMessage("m1", "queued")],
        model: sessionModel("beta/model-b"),
      }),
      runtimeCursor(100),
    );

    assert.equal(repaired.cursor, 100);
    assert.equal(repaired.transcript.length, 1);
    assert.equal(repaired.attempt, undefined, "the snapshot is authoritative");
    assert.equal("notices" in repaired, false);
  });

  it("is reconstructable from a snapshot alone", () => {
    // Drive a rich incremental sequence, then prove the same authoritative
    // snapshot yields the same meaningful state without any of that history.
    const streamed = fold(initial(), [
      { type: "attempt_started", attempt_id: "a1", model: attemptModel("alpha/model-a") },
      { type: "assistant_message_started", attempt_id: "a1", message_id: "m2" },
      { type: "assistant_text_delta", attempt_id: "a1", message_id: "m2", block_index: 0, delta: "hi" },
      {
        type: "message_committed",
        attempt_id: "a1",
        message: assistantMessage("m2", "hi"),
        transcript_cursor: transcriptCursor(1),
      },
      {
        type: "attempt_settled",
        attempt_id: "a1",
        outcome: { type: "completed", finish_reason: { type: "stop" } },
      },
    ]);

    const repaired = replaceFromSnapshot(
      snapshot({
        messages: [assistantMessage("m2", "hi")],
        attempt: {
          attempt_id: "a1",
          phase: {
            type: "settled",
            outcome: { type: "completed", finish_reason: { type: "stop" } },
          },
          turn: 1,
          model: attemptModel("alpha/model-a"),
        },
      }),
      streamed.cursor,
    );

    assert.equal(repaired.transcript.length, streamed.transcript.length);
    assert.deepEqual(repaired.attempt?.phase, streamed.attempt?.phase);
    assert.equal(repaired.attempt?.model.primary.model, "alpha/model-a");
  });
});

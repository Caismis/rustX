/**
 * The presentation reducer over scripted runtime facts.
 *
 * Every case drives exact protocol events. The reducer is pure, so each
 * assertion is about a value, never about timing.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  reduce,
  replaceFromSnapshot,
  withPendingSubmission,
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
  capabilities,
  runtimeInbound,
  sessionModel,
  questionInteraction,
  snapshot,
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
    cursor += 1;
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

const initial = () => replaceFromSnapshot(snapshot(), 0);

describe("presentation projection", () => {
  it("derives the initial state from an authoritative snapshot", () => {
    const state = replaceFromSnapshot(
      snapshot({
        conversation_id: "conv-1",
        messages: [userMessage("m1", "hello"), assistantMessage("m2", "hi")],
        capabilities: capabilities(4),
      }),
      9,
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
          type: "answered",
          response: { type: "approval", decision: { type: "allow" } },
        },
      },
    ]);
    assert.deepEqual(settled.pendingInteractions, []);

    const repaired = replaceFromSnapshot(
      snapshot({ pending_interactions: [interaction] }),
      8,
    );
    assert.deepEqual(repaired.pendingInteractions, [interaction]);
    assert.equal(repaired.cursor, 8);
  });

  it("folds Questions and authoritative ApprovalMode changes", () => {
    const question = questionInteraction();
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
      5,
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
      },
    ]);

    assert.equal(streamingOf(state), undefined);
    assert.equal(state.transcript.length, 1);
    assert.equal(state.transcript[0]?.kind, "committed");
  });

  it("renders committed human and runtime-originated inbound distinctly", () => {
    const state = fold(initial(), [
      { type: "message_committed", message: userMessage("m1", "from a human") },
      {
        type: "message_committed",
        message: runtimeInbound("m2", "background work finished"),
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
      },
    ]);

    assert.equal(state.background.length, 1, "updates replace, never duplicate");
    assert.equal(state.background[0]?.state, "succeeded");
    assert.equal(state.transcript.length, 1);
  });

  it("folds inbound enqueue and finite drain from runtime facts", () => {
    let state = fold(initial(), [
      { type: "inbound_enqueued", sequence: 1, message: userMessage("m1", "first") },
      { type: "inbound_enqueued", sequence: 2, message: userMessage("m2", "second") },
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
      target_message_id: "m1",
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
        target_message_id: "m1",
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
    const state = replaceFromSnapshot(snapshot({ shutting_down: true }), 8);
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
          cursor: 42,
          event: { type: "future_variant" } as unknown as RuntimeClientEvent,
        }),
      /unreachable Runtime Client Protocol v1 event/,
    );
  });

  it("keeps optimistic submissions as transient client state", () => {
    let state = withPendingSubmission(initial(), "local-1", "hello");
    assert.equal(state.pendingSubmissions.length, 1);
    // No fake canonical message was appended to authoritative history.
    assert.equal(state.transcript.length, 0);

    state = fold(state, [
      { type: "inbound_enqueued", sequence: 1, message: userMessage("m1", "hello") },
    ]);

    // The runtime's authoritative fact reconciles the local echo away.
    assert.deepEqual(state.pendingSubmissions, []);
    assert.equal(state.inbound.pending?.[0]?.message.id, "m1");
  });

  it("does not carry TUI feedback through an authoritative repair", () => {
    let state = withPendingSubmission(initial(), "local-1", "queued");
    state = fold(state, [
      { type: "attempt_started", attempt_id: "a1", model: attemptModel("beta/model-b") },
    ]);

    const repaired = replaceFromSnapshot(
      snapshot({
        messages: [userMessage("m1", "queued")],
        model: sessionModel("beta/model-b"),
      }),
      100,
      { pendingSubmissions: state.pendingSubmissions },
    );

    assert.equal(repaired.cursor, 100);
    assert.equal(repaired.transcript.length, 1);
    assert.equal(repaired.attempt, undefined, "the snapshot is authoritative");
    assert.equal(repaired.pendingSubmissions.length, 1);
    assert.equal("notices" in repaired, false);
  });

  it("is reconstructable from a snapshot alone", () => {
    // Drive a rich incremental sequence, then prove the same authoritative
    // snapshot yields the same meaningful state without any of that history.
    const streamed = fold(initial(), [
      { type: "attempt_started", attempt_id: "a1", model: attemptModel("alpha/model-a") },
      { type: "assistant_message_started", attempt_id: "a1", message_id: "m2" },
      { type: "assistant_text_delta", attempt_id: "a1", message_id: "m2", block_index: 0, delta: "hi" },
      { type: "message_committed", attempt_id: "a1", message: assistantMessage("m2", "hi") },
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

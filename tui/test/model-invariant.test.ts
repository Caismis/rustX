/**
 * The A -> B model invariant, proven deterministically.
 *
 * ```text
 * session model = A
 * attempt admitted; the attempt freezes A
 * model_set(B) succeeds while the attempt is active
 *
 *   presentation MUST show:
 *     desired session model = B
 *     active attempt model  = A
 *
 * after settlement, the next attempt admitted
 *   presentation MUST show:
 *     active attempt model  = B
 * ```
 *
 * The currently executing attempt must never visually mutate to B. This is
 * proven twice — once through the pure reducer, once end to end through the
 * transport and session owner against a scripted protocol peer — and neither
 * proof uses a timer.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { reduce } from "../src/presentation/projection.ts";
import { replaceFromSnapshot } from "../src/presentation/projection.ts";
import type { PresentationState } from "../src/presentation/state.ts";
import { RuntimeClientConnection } from "../src/runtime/connection.ts";
import { RuntimeClientSession } from "../src/runtime/session.ts";
import type {
  RuntimeClientEvent,
  RuntimeClientProtocolEvent,
} from "../src/protocol/types.ts";
import {
  attemptModel,
  inboundBlock,
  sessionModel,
  snapshot,
} from "./support/fixtures.ts";
import { ScriptedPeer, until } from "./support/scripted-peer.ts";

const MODEL_A = "alpha/model-a";
const MODEL_B = "beta/model-b";

function fold(
  state: PresentationState,
  events: RuntimeClientEvent[],
): PresentationState {
  let current = state;
  let cursor = current.cursor;
  for (const event of events) {
    cursor += 1;
    const protocolEvent: RuntimeClientProtocolEvent = { cursor, event };
    current = reduce(current, protocolEvent);
  }
  return current;
}

describe("session model A -> B invariant", () => {
  it("keeps the running attempt on A while the session moves to B", () => {
    let state = replaceFromSnapshot(
      snapshot({ model: sessionModel(MODEL_A) }),
      0,
    );

    // The attempt is admitted while the session model is A.
    state = fold(state, [
      { type: "attempt_started", attempt_id: "a1", model: attemptModel(MODEL_A) },
    ]);
    assert.equal(state.attempt?.model.primary.model, MODEL_A);
    assert.equal(state.sessionModel.configured.model, MODEL_A);

    // The session switches to B while that attempt is still running.
    state = fold(state, [
      { type: "session_model_changed", model: sessionModel(MODEL_B) },
    ]);

    assert.equal(
      state.sessionModel.configured.model,
      MODEL_B,
      "desired session model = B",
    );
    assert.equal(
      state.attempt?.model.primary.model,
      MODEL_A,
      "active attempt model stays A",
    );
    assert.equal(
      state.attempt?.phase.type,
      "running",
      "the running attempt is not restarted by a model change",
    );

    // The attempt settles; it still reports the model it ran with.
    state = fold(state, [
      {
        type: "attempt_settled",
        attempt_id: "a1",
        outcome: { type: "completed", finish_reason: { type: "stop" } },
      },
    ]);
    assert.equal(state.attempt?.model.primary.model, MODEL_A);
    assert.equal(state.sessionModel.configured.model, MODEL_B);

    // The next admission uses B.
    state = fold(state, [
      { type: "attempt_started", attempt_id: "a2", model: attemptModel(MODEL_B) },
    ]);
    assert.equal(state.attempt?.attemptId, "a2");
    assert.equal(
      state.attempt?.model.primary.model,
      MODEL_B,
      "the next attempt uses B",
    );
  });

  it("never mutates the active attempt model on any interleaved event", () => {
    let state = replaceFromSnapshot(
      snapshot({ model: sessionModel(MODEL_A) }),
      0,
    );
    state = fold(state, [
      { type: "attempt_started", attempt_id: "a1", model: attemptModel(MODEL_A) },
    ]);

    // Every kind of activity that can occur mid-attempt, with the session
    // switching to B in the middle of it.
    const interleaved: RuntimeClientEvent[] = [
      { type: "assistant_message_started", attempt_id: "a1", message_id: "m1" },
      {
        type: "assistant_text_delta",
        attempt_id: "a1",
        message_id: "m1",
        block_index: 0,
        delta: "still on A",
      },
      { type: "session_model_changed", model: sessionModel(MODEL_B) },
      {
        type: "tool_execution_started",
        attempt_id: "a1",
        tool_call_id: "c1",
        tool_id: "tool-bash",
      },
      { type: "capability_updated", capabilities: { revision: 9 } },
      {
        type: "inbound_enqueued",
        sequence: 1,
        message: inboundBlock("m2", "queued"),
      },
    ];

    for (const event of interleaved) {
      state = fold(state, [event]);
      assert.equal(
        state.attempt?.model.primary.model,
        MODEL_A,
        `the attempt model survived ${event.type}`,
      );
    }
    assert.equal(state.sessionModel.configured.model, MODEL_B);
  });

  it("proves the invariant end to end over the transport", async () => {
    const peer = new ScriptedPeer();
    const connection = new RuntimeClientConnection({
      input: peer.runtimeOutput,
      output: peer.clientOutput,
    });
    const session = new RuntimeClientSession({ connection });

    const attaching = session.attach();
    await peer.awaitRequests(1);
    peer.respond(1, {
      type: "initialized",
      attachment_id: "att-1",
      conversation_id: "conv-1",
      agent_id: "agent-1",
      snapshot: snapshot({ model: sessionModel(MODEL_A) }),
      cursor: 0,
    });
    await peer.awaitRequests(2); // subscribe_events
    peer.respond(2, { type: "subscribed", after_cursor: 0 });
    await attaching;

    assert.equal(session.state?.sessionModel.configured.model, MODEL_A);

    // The runtime admits an attempt on A. The start event is self-contained:
    // the client learns the frozen model without a second snapshot_get.
    peer.emit(1, {
      type: "attempt_started",
      attempt_id: "a1",
      model: attemptModel(MODEL_A),
    });
    await until(
      () => session.state?.attempt?.attemptId === "a1",
      "attempt observed",
    );
    assert.equal(session.state?.attempt?.model.primary.model, MODEL_A);

    // The client requests B while the attempt runs; the runtime accepts.
    const setting = session.modelSet({ model: MODEL_B });
    const requests = await peer.awaitRequests(3);
    const modelSet = requests[2];
    assert.equal(modelSet?.method, "model_set");
    peer.respond(3, { type: "model_set", model: sessionModel(MODEL_B) });
    await setting;

    // The authoritative change arrives on the same observation stream.
    peer.emit(2, {
      type: "session_model_changed",
      model: sessionModel(MODEL_B),
    });
    await until(
      () => session.state?.sessionModel.configured.model === MODEL_B,
      "session model change observed",
    );

    assert.equal(
      session.state?.attempt?.model.primary.model,
      MODEL_A,
      "the executing attempt did not visually mutate to B",
    );
    assert.equal(session.state?.attempt?.phase.type, "running");

    // Settle, then admit the next attempt.
    peer.emit(3, {
      type: "attempt_settled",
      attempt_id: "a1",
      outcome: { type: "completed", finish_reason: { type: "stop" } },
    });
    peer.emit(4, {
      type: "attempt_started",
      attempt_id: "a2",
      model: attemptModel(MODEL_B),
    });
    await until(
      () => session.state?.attempt?.attemptId === "a2",
      "next attempt observed",
    );

    assert.equal(
      session.state?.attempt?.model.primary.model,
      MODEL_B,
      "the next attempt uses B",
    );
    connection.close();
  });

  it("presents runtime-published reasoning support without inventing profiles", () => {
    // A reasoning-capable model with no declared profiles means: reasoning is
    // supported, no profile is selectable, and provider/runtime defaults
    // apply. The client must not synthesize off/low/medium/high.
    const alwaysOn = sessionModel("always/always-on", {
      reasoningEnabled: true,
      capabilities: {
        inputModalities: ["text"],
        outputModalities: ["text"],
        toolCalls: true,
        reasoning: true,
      },
      declaredCapabilities: {
        inputModalities: ["text"],
        outputModalities: ["text"],
        toolCalls: true,
        reasoning: true,
      },
    });
    const state = replaceFromSnapshot(snapshot({ model: alwaysOn }), 0);

    assert.equal(state.sessionModel.effective.capabilities.reasoning, true);
    assert.equal(state.sessionModel.effective.reasoningEnabled, true);
    assert.equal(
      state.sessionModel.effective.reasoningProfile,
      undefined,
      "no profile is invented for a model that declares none",
    );
  });

  it("shows effective capability, not the raw catalog claim", () => {
    // The catalog claims image input; the runtime cannot represent it yet.
    const narrowed = sessionModel(MODEL_A, {
      capabilities: {
        inputModalities: ["text"],
        outputModalities: ["text"],
        toolCalls: true,
        reasoning: false,
      },
      declaredCapabilities: {
        inputModalities: ["text", "image"],
        outputModalities: ["text"],
        toolCalls: true,
        reasoning: false,
      },
    });
    const state = replaceFromSnapshot(snapshot({ model: narrowed }), 0);

    assert.deepEqual(state.sessionModel.effective.capabilities.inputModalities, [
      "text",
    ]);
    assert.ok(
      state.sessionModel.effective.declaredCapabilities.inputModalities.includes(
        "image",
      ),
      "the declaration is still available to explain the difference",
    );
  });
});

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
 * typed App Server client and its Session owner against a scripted peer — and
 * neither proof uses a timer.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { reduce } from "../src/presentation/projection.ts";
import { replaceFromSnapshot } from "../src/presentation/projection.ts";
import type { PresentationState } from "../src/presentation/state.ts";
import {
  APP_SERVER_PROTOCOL_VERSION,
  AppServerClient,
} from "../src/app-server/client.ts";
import { AppServerSession } from "../src/app-server/session.ts";
import type {
  AttachmentTarget,
  RuntimeClientEvent,
} from "../src/protocol/app-server.ts";
import type { ProjectedEvent } from "../src/presentation/projection.ts";
import { runtimeCursor, transcriptCursor } from "./support/fixtures.ts";
import {
  attemptModel,
  inboundBlock,
  sessionModel,
  snapshot,
  nextCursor,
} from "./support/fixtures.ts";
import { FakeTransport, paramsOf, tick } from "./support/app-server-peer.ts";

const MODEL_A = "alpha/model-a";
const MODEL_B = "beta/model-b";

function fold(
  state: PresentationState,
  events: RuntimeClientEvent[],
): PresentationState {
  let current = state;
  let cursor = current.cursor;
  for (const event of events) {
    cursor = nextCursor(cursor);
    const protocolEvent: ProjectedEvent = { cursor, event };
    current = reduce(current, protocolEvent);
  }
  return current;
}

describe("session model A -> B invariant", () => {
  it("keeps the running attempt on A while the session moves to B", () => {
    let state = replaceFromSnapshot(
      snapshot({ model: sessionModel(MODEL_A) }),
      runtimeCursor(0),
    );

    // The attempt is admitted while the session model is A.
    state = fold(state, [
      { type: "attempt_started", execution_settings: null, attempt_id: "a1", model: attemptModel(MODEL_A) },
    ]);
    assert.equal(state.attempt?.model!.primary.model, MODEL_A);
    assert.equal(state.sessionModel!.configured.model, MODEL_A);

    // The session switches to B while that attempt is still running.
    state = fold(state, [
      { type: "session_model_changed", model: sessionModel(MODEL_B) },
    ]);

    assert.equal(
      state.sessionModel!.configured.model,
      MODEL_B,
      "desired session model = B",
    );
    assert.equal(
      state.attempt?.model!.primary.model,
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
    assert.equal(state.attempt?.model!.primary.model, MODEL_A);
    assert.equal(state.sessionModel!.configured.model, MODEL_B);

    // The next admission uses B.
    state = fold(state, [
      { type: "attempt_started", execution_settings: null, attempt_id: "a2", model: attemptModel(MODEL_B) },
    ]);
    assert.equal(state.attempt?.attemptId, "a2");
    assert.equal(
      state.attempt?.model!.primary.model,
      MODEL_B,
      "the next attempt uses B",
    );
  });

  it("never mutates the active attempt model on any interleaved event", () => {
    let state = replaceFromSnapshot(
      snapshot({ model: sessionModel(MODEL_A) }),
      runtimeCursor(0),
    );
    state = fold(state, [
      { type: "attempt_started", execution_settings: null, attempt_id: "a1", model: attemptModel(MODEL_A) },
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
      { type: "capability_updated", capabilities: { revision: "9" } },
      {
        type: "inbound_enqueued",
        sequence: "1",
        message: inboundBlock("m2", "queued"),
        transcript_cursor: transcriptCursor(1),
      },
    ];

    for (const event of interleaved) {
      state = fold(state, [event]);
      assert.equal(
        state.attempt?.model!.primary.model,
        MODEL_A,
        `the attempt model survived ${event.type}`,
      );
    }
    assert.equal(state.sessionModel!.configured.model, MODEL_B);
  });

  it("proves the invariant end to end through the App Server client", async () => {
    const transport = new FakeTransport();
    const connecting = AppServerClient.initialize({ transport });
    const initialize = (await transport.log.awaitMethod("initialize")).at(-1)!;
    transport.respond(initialize.id, {
      type: "initialized",
      protocol_version: APP_SERVER_PROTOCOL_VERSION,
      capabilities: {
        multi_session: true,
        single_writable_controller: true,
        headless_interactions: true,
        experimental_methods: [],
      },
    });
    const client = await connecting;

    const target: AttachmentTarget = {
      session_id: "ses_0a7e1c20-6bdf-7720-acad-22e2d65841be",
      conversation_id: "conv_36524fd8-f674-7fc2-b125-06d01fee0e18",
      runtime_incarnation: "1",
      attachment_id: "att-1",
    };
    const attaching = AppServerSession.attach(client, target.session_id);
    const attach = (await transport.log.awaitMethod("session/attach")).at(-1)!;
    // Attach is one cut: snapshot, cursor and subscription together.
    transport.respond(attach.id, {
      type: "attached",
      target,
      snapshot: snapshot({ model: sessionModel(MODEL_A) }),
      cursor: runtimeCursor(0),
    });
    const session = await attaching;
    const stop = client.onNotification((message) =>
      session.applyNotification(message),
    );

    assert.equal(session.state.sessionModel!.configured.model, MODEL_A);

    // The runtime admits an attempt on A. The start event is self-contained:
    // the client learns the frozen model without a second snapshot read.
    transport.emit(target, runtimeCursor(1), {
      type: "attempt_started",
      execution_settings: null,
      attempt_id: "a1",
      model: attemptModel(MODEL_A),
    });
    await tick();
    assert.equal(session.state.attempt?.attemptId, "a1");
    assert.equal(session.state.attempt?.model!.primary.model, MODEL_A);

    // The client requests B while the attempt runs; the runtime accepts.
    const setting = session.modelSet({ model: MODEL_B });
    const modelSet = (await transport.log.awaitMethod("session/setModel")).at(-1)!;
    assert.deepEqual(paramsOf(modelSet, "session/setModel").target, target);
    transport.respond(modelSet.id, {
      type: "model",
      model: sessionModel(MODEL_B),
    });
    await setting;

    // The authoritative change arrives on the same observation stream.
    transport.emit(target, runtimeCursor(2), {
      type: "session_model_changed",
      model: sessionModel(MODEL_B),
    });
    await tick();
    assert.equal(session.state.sessionModel!.configured.model, MODEL_B);

    assert.equal(
      session.state.attempt?.model!.primary.model,
      MODEL_A,
      "the executing attempt did not visually mutate to B",
    );
    assert.equal(session.state.attempt?.phase.type, "running");

    // Settle, then admit the next attempt.
    transport.emit(target, runtimeCursor(3), {
      type: "attempt_settled",
      attempt_id: "a1",
      outcome: { type: "completed", finish_reason: { type: "stop" } },
    });
    transport.emit(target, runtimeCursor(4), {
      type: "attempt_started",
      execution_settings: null,
      attempt_id: "a2",
      model: attemptModel(MODEL_B),
    });
    await tick();

    assert.equal(session.state.attempt?.attemptId, "a2");
    assert.equal(
      session.state.attempt?.model!.primary.model,
      MODEL_B,
      "the next attempt uses B",
    );
    stop();
    client.close();
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
    const state = replaceFromSnapshot(
      snapshot({ model: alwaysOn }),
      runtimeCursor(0),
    );

    assert.equal(state.sessionModel!.effective.capabilities.reasoning, true);
    assert.equal(state.sessionModel!.effective.reasoningEnabled, true);
    assert.equal(
      state.sessionModel!.effective.reasoningProfile,
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
    const state = replaceFromSnapshot(
      snapshot({ model: narrowed }),
      runtimeCursor(0),
    );

    assert.deepEqual(state.sessionModel!.effective.capabilities.inputModalities, [
      "text",
    ]);
    assert.ok(
      state.sessionModel!.effective.declaredCapabilities.inputModalities.includes(
        "image",
      ),
      "the declaration is still available to explain the difference",
    );
  });
});

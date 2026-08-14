/**
 * The attachment lifecycle: initialize, subscribe, resync repair, shutdown.
 *
 * Resync is proven to be *snapshot replacement*, never gap inference: after a
 * repair the state matches the fresh snapshot exactly, including facts the
 * client never observed incrementally and excluding ones it did.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { RuntimeClientConnection } from "../src/runtime/connection.ts";
import { RuntimeClientSession, isResyncRequired } from "../src/runtime/session.ts";
import { RuntimeRequestError } from "../src/runtime/connection.ts";
import {
  agentMessage,
  attemptModel,
  capabilities,
  sessionModel,
  snapshot,
  userMessage,
} from "./support/fixtures.ts";
import { ScriptedPeer, until } from "./support/scripted-peer.ts";

function connect(): {
  peer: ScriptedPeer;
  connection: RuntimeClientConnection;
  session: RuntimeClientSession;
} {
  const peer = new ScriptedPeer();
  const connection = new RuntimeClientConnection({
    input: peer.runtimeOutput,
    output: peer.clientOutput,
  });
  return { peer, connection, session: new RuntimeClientSession({ connection }) };
}

/** Completes the attach handshake with a given snapshot and cursor. */
async function attach(
  peer: ScriptedPeer,
  session: RuntimeClientSession,
  initial = snapshot(),
  cursor = 0,
): Promise<void> {
  const attaching = session.attach();
  await peer.awaitRequests(1);
  peer.respond(1, {
    type: "initialized",
    attachment_id: "att-1",
    conversation_id: initial.conversation_id,
    agent_id: "agent-1",
    snapshot: initial,
    cursor,
  });
  await peer.awaitRequests(2);
  peer.respond(2, { type: "subscribed", after_cursor: cursor });
  await attaching;
}

describe("RuntimeClientSession", () => {
  it("negotiates v1, installs the snapshot, and subscribes from its cursor", async () => {
    const { peer, session } = connect();
    await attach(peer, session, snapshot({ conversation_id: "conv-7" }), 12);

    const [initialize, subscribe] = peer.requests;
    assert.equal(initialize?.method, "initialize");
    assert.equal(
      initialize?.method === "initialize" ? initialize.protocol_version : null,
      1,
    );
    assert.equal(subscribe?.method, "subscribe_events");
    assert.equal(
      subscribe?.method === "subscribe_events" ? subscribe.after_cursor : null,
      12,
      "the subscription resumes at exactly the snapshot's cursor",
    );

    assert.equal(session.identity?.conversationId, "conv-7");
    assert.equal(session.state?.cursor, 12);
  });

  it("ignores an event at or before the installed cursor", async () => {
    const { peer, session } = connect();
    await attach(peer, session, snapshot(), 5);

    // A stale replay of an event the snapshot already describes.
    peer.emit(5, {
      type: "attempt_started",
      attempt_id: "stale",
      model: attemptModel("alpha/model-a"),
    });
    peer.emit(6, {
      type: "attempt_started",
      attempt_id: "fresh",
      model: attemptModel("alpha/model-a"),
    });

    await until(
      () => session.state?.attempt?.attemptId === "fresh",
      "fresh event applied",
    );
    assert.equal(session.state?.cursor, 6);
  });

  it("folds events published while the subscription is in flight", async () => {
    const { peer, session } = connect();
    const attaching = session.attach();
    await peer.awaitRequests(1);
    peer.respond(1, {
      type: "initialized",
      attachment_id: "att-1",
      conversation_id: "conv-1",
      agent_id: "agent-1",
      snapshot: snapshot(),
      cursor: 0,
    });
    await peer.awaitRequests(2);

    // The runtime publishes before answering subscribe_events.
    peer.emit(1, {
      type: "attempt_started",
      attempt_id: "a1",
      model: attemptModel("alpha/model-a"),
    });
    peer.respond(2, { type: "subscribed", after_cursor: 0 });
    await attaching;

    await until(
      () => session.state?.attempt?.attemptId === "a1",
      "the in-flight event was not dropped",
    );
  });

  it("repairs from the authoritative snapshot on resync_required", async () => {
    const { peer, session } = connect();
    await attach(peer, session, snapshot(), 0);

    // Observe a little incremental state, then lose trust in it.
    peer.emit(1, {
      type: "attempt_started",
      attempt_id: "a1",
      model: attemptModel("alpha/model-a"),
    });
    await until(() => session.state?.attempt?.attemptId === "a1", "attempt seen");

    // The repair: snapshot_get -> replace -> re-subscribe after the new cursor.
    const repairing = session.resync();
    const afterSnapshot = await peer.awaitRequests(3);
    assert.equal(afterSnapshot[2]?.method, "snapshot_get");

    peer.respond(3, {
      type: "snapshot",
      snapshot: snapshot({
        // Facts the client never observed incrementally.
        messages: [userMessage("m1", "missed"), agentMessage("m2", "also missed")],
        capabilities: capabilities(11),
        model: sessionModel("beta/model-b"),
        // The attempt the client thought was running is gone.
        attempt: undefined,
      }),
      cursor: 50,
    });
    const afterSubscribe = await peer.awaitRequests(4);
    assert.equal(afterSubscribe[3]?.method, "subscribe_events");
    assert.equal(
      afterSubscribe[3]?.method === "subscribe_events"
        ? afterSubscribe[3].after_cursor
        : null,
      50,
      "the subscription resumes after the *new* cursor",
    );
    peer.respond(4, { type: "subscribed", after_cursor: 50 });
    await repairing;

    assert.equal(session.resyncCount, 1);
    assert.equal(session.state?.cursor, 50);
    // The snapshot replaced the projection wholesale — no inference, no
    // replay of what the UI thought had happened.
    assert.equal(session.state?.transcript.length, 2);
    assert.equal(session.state?.capabilities.revision, 11);
    assert.equal(session.state?.sessionModel.configured.model, "beta/model-b");
    assert.equal(
      session.state?.attempt,
      undefined,
      "the stale attempt is gone because the snapshot says so",
    );
  });

  it("repairs when the subscription itself reports resync_required", async () => {
    const { peer, session } = connect();
    const attaching = session.attach();
    await peer.awaitRequests(1);
    peer.respond(1, {
      type: "initialized",
      attachment_id: "att-1",
      conversation_id: "conv-1",
      agent_id: "agent-1",
      snapshot: snapshot(),
      cursor: 3,
    });
    await peer.awaitRequests(2);

    // The cursor expired between the snapshot and the subscription.
    peer.respondError(2, {
      type: "resync_required",
      after_cursor: 3,
      earliest_serviceable: 40,
    });

    const afterSnapshot = await peer.awaitRequests(3);
    assert.equal(afterSnapshot[2]?.method, "snapshot_get");
    peer.respond(3, {
      type: "snapshot",
      snapshot: snapshot({ messages: [userMessage("m1", "recovered")] }),
      cursor: 41,
    });
    await peer.awaitRequests(4);
    peer.respond(4, { type: "subscribed", after_cursor: 41 });
    await attaching;

    assert.equal(session.resyncCount, 1);
    assert.equal(session.state?.cursor, 41);
    assert.equal(session.state?.transcript.length, 1);
  });

  it("carries an observed shutdown across an authoritative repair", async () => {
    const { peer, session } = connect();
    await attach(peer, session, snapshot(), 0);

    peer.emit(1, { type: "runtime_shutdown" });
    await until(
      () => session.state?.runtimeShutdown === true,
      "shutdown observed",
    );

    const repairing = session.resync();
    await peer.awaitRequests(3);
    peer.respond(3, { type: "snapshot", snapshot: snapshot(), cursor: 60 });
    await peer.awaitRequests(4);
    peer.respond(4, { type: "subscribed", after_cursor: 60 });
    await repairing;

    // Shutdown is absorbing and the snapshot has no field for it. Dropping it
    // would make the UI claim the runtime still admits inbound work.
    assert.equal(session.state?.runtimeShutdown, true);
  });

  it("classifies resync_required distinctly from other protocol errors", async () => {
    const { peer, session } = connect();
    await attach(peer, session);

    const failing = session.modelSet({ model: "nope/nope" });
    await peer.awaitRequests(3);
    peer.respondError(3, {
      type: "invalid_model_configuration",
      message: "unknown model reference",
    });

    await assert.rejects(failing, (error: unknown) => {
      assert.ok(error instanceof RuntimeRequestError);
      assert.equal(isResyncRequired(error), false);
      assert.match(error.message, /rejected/);
      return true;
    });
  });

  it("submits inbound and reports the runtime-assigned identity", async () => {
    const { peer, session } = connect();
    await attach(peer, session);

    const submitting = session.submitInbound([{ type: "text", text: "hello" }]);
    await peer.awaitRequests(3);
    peer.respond(3, {
      type: "inbound_accepted",
      message_id: "runtime-owned-id",
      inbound_sequence: 4,
    });

    // Identity, sequence, timestamp, and provenance are runtime-owned.
    assert.deepEqual(await submitting, {
      messageId: "runtime-owned-id",
      sequence: 4,
    });
  });

  it("treats background cancellation as acceptance, not settlement", async () => {
    const { peer, session } = connect();
    await attach(peer, session);

    const cancelling = session.cancelBackground("exec-1");
    await peer.awaitRequests(3);
    peer.respond(3, {
      type: "background_cancel_accepted",
      execution: {
        execution_id: "exec-1",
        tool_id: "tool-background",
        tool_name: "background_task",
        state: "cancelling",
      },
    });

    const accepted = await cancelling;
    assert.equal(
      accepted.state,
      "cancelling",
      "acceptance carries the registry state, never a terminal result",
    );

    // The terminal fact arrives later, and only from the runtime.
    peer.emit(1, {
      type: "background_execution_updated",
      execution: {
        execution_id: "exec-1",
        tool_id: "tool-background",
        tool_name: "background_task",
        state: "cancelled",
      },
    });
    await until(
      () => session.state?.background[0]?.state === "cancelled",
      "terminal background fact observed",
    );
  });

  it("accepts shutdown without closing the transport", async () => {
    const { peer, session } = connect();
    await attach(peer, session);

    const shuttingDown = session.shutdown();
    await peer.awaitRequests(3);
    peer.respond(3, { type: "shutdown_accepted" });
    await shuttingDown;

    // Shutdown is not transport closure: the session still serves reads.
    const reading = session.modelGet();
    await peer.awaitRequests(4);
    peer.respond(4, { type: "model", model: sessionModel("alpha/model-a") });
    assert.equal((await reading).configured.model, "alpha/model-a");
  });
});

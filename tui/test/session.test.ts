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
import { RuntimeClientAttachment, isResyncRequired } from "../src/runtime/attachment.ts";
import { RuntimeRequestError } from "../src/runtime/connection.ts";
import {
  assistantMessage,
  attemptModel,
  capabilities,
  sessionModel,
  sessionView,
  snapshot,
  runtimeCursor,
  transcriptCursor,
  userMessage,
} from "./support/fixtures.ts";
import { ScriptedPeer, until } from "./support/scripted-peer.ts";

function connect(): {
  peer: ScriptedPeer;
  connection: RuntimeClientConnection;
  session: RuntimeClientAttachment;
} {
  const peer = new ScriptedPeer();
  const connection = new RuntimeClientConnection({
    input: peer.runtimeOutput,
    output: peer.clientOutput,
  });
  return { peer, connection, session: new RuntimeClientAttachment({ connection }) };
}

/** Completes the attach handshake with a given snapshot and cursor. */
async function attach(
  peer: ScriptedPeer,
  session: RuntimeClientAttachment,
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
    cursor: runtimeCursor(cursor),
  });
  await peer.awaitRequests(2);
  peer.respond(2, { type: "subscribed", after_cursor: runtimeCursor(cursor) });
  await attaching;
}

describe("RuntimeClientAttachment", () => {
  it("negotiates v2, installs the snapshot, and subscribes from its cursor", async () => {
    const { peer, session } = connect();
    await attach(peer, session, snapshot({ conversation_id: "conv-7" }), 12);

    const [initialize, subscribe] = peer.requests;
    assert.equal(initialize?.method, "initialize");
    assert.equal(
      initialize?.method === "initialize" ? initialize.protocol_version : null,
      2,
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

  it("loads an older durable transcript page without changing the live cursor", async () => {
    const { peer, session } = connect();
    await attach(
      peer,
      session,
      snapshot({
        transcript: {
          entries: [
            {
              cursor: transcriptCursor(2),
              item: {
                type: "message",
                message: userMessage("newer", "newer"),
              },
            },
          ],
          next_cursor: transcriptCursor(1),
        },
      }),
      7,
    );

    const loading = session.loadOlderTranscript(8);
    await peer.awaitRequests(3);
    assert.deepEqual(peer.requests[2], {
      method: "transcript_page_get",
      before_cursor: 1,
      limit: 8,
      id: 3,
    });
    peer.respond(3, {
      type: "transcript_page",
      page: {
        entries: [
          {
            cursor: transcriptCursor(1),
            item: {
              type: "message",
              message: userMessage("older", "older"),
            },
          },
        ],
      },
    });
    assert.equal(await loading, true);
    assert.deepEqual(
      session.state?.transcript.map((entry) =>
        entry.kind === "committed" ? entry.messageId : entry.kind,
      ),
      ["older", "newer"],
    );
    assert.equal(session.state?.cursor, 7);
    assert.equal(session.state?.transcriptNextCursor, undefined);
  });

  it("notifies local-surface owners for initial attach and authoritative resync replacement", async () => {
    const { peer, session } = connect();
    let replacements = 0;
    const removeSnapshotListener = session.onSnapshot(() => {
      replacements += 1;
    });

    await attach(peer, session);
    assert.equal(replacements, 1, "initial attach installs one authoritative snapshot");

    const repairing = session.resync();
    await peer.awaitRequests(3);
    peer.respond(3, {
      type: "snapshot",
      snapshot: snapshot({ messages: [userMessage("m1", "repaired")] }),
      cursor: runtimeCursor(9),
    });
    await peer.awaitRequests(4);
    peer.respond(4, { type: "subscribed", after_cursor: runtimeCursor(9) });
    await repairing;

    assert.equal(replacements, 2, "resync installs a replacement snapshot");
    removeSnapshotListener();
  });

  it("answers a native interaction through the typed Runtime Client request", async () => {
    const { peer, session } = connect();
    await attach(peer, session);

    const responding = session.respondInteraction("attempt-1-interaction-1", {
      type: "approval",
      decision: { type: "allow" },
    });
    await peer.awaitRequests(3);

    const request = peer.requests[2];
    assert.equal(request?.method, "interaction_respond");
    assert.equal(
      request?.method === "interaction_respond"
        ? request.interaction_id
        : undefined,
      "attempt-1-interaction-1",
    );
    assert.deepEqual(
      request?.method === "interaction_respond" ? request.response : undefined,
      { type: "approval", decision: { type: "allow" } },
    );

    peer.respond(3, {
      type: "interaction_response_accepted",
      interaction_id: "attempt-1-interaction-1",
    });
    await responding;
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
      cursor: runtimeCursor(0),
    });
    await peer.awaitRequests(2);

    // The runtime publishes before answering subscribe_events.
    peer.emit(1, {
      type: "attempt_started",
      attempt_id: "a1",
      model: attemptModel("alpha/model-a"),
    });
    peer.respond(2, { type: "subscribed", after_cursor: runtimeCursor(0) });
    await attaching;

    await until(
      () => session.state?.attempt?.attemptId === "a1",
      "the in-flight event was not dropped",
    );
  });

  it("folds live turn and usage events, then reconstructs them from resync", async () => {
    const { peer, session } = connect();
    await attach(
      peer,
      session,
      snapshot({
        attempt: {
          attempt_id: "a1",
          phase: { type: "running" },
          turn: 0,
          model: attemptModel("alpha/model-a"),
        },
      }),
      0,
    );

    const usage = {
      input_tokens: 10,
      output_tokens: 4,
      total_tokens: 14,
      details: { cached_input_tokens: 2 },
    };
    peer.emit(1, { type: "attempt_turn_updated", attempt_id: "a1", turn: 1 });
    peer.emit(2, { type: "attempt_turn_updated", attempt_id: "a1", turn: 2 });
    peer.emit(3, { type: "attempt_usage_updated", attempt_id: "a1", usage });
    await until(
      () =>
        session.state?.attempt?.turn === 2 &&
        session.state.attempt.lastUsage?.total_tokens === 14,
      "live turn and usage facts",
    );
    assert.equal(
      peer.requests.some((request) => request.method === "snapshot_get"),
      false,
      "incremental facts do not require polling",
    );

    const repairing = session.resync();
    await peer.awaitRequests(3);
    peer.respond(3, {
      type: "snapshot",
      snapshot: snapshot({
        attempt: {
          attempt_id: "a1",
          phase: { type: "running" },
          turn: 2,
          last_usage: usage,
          model: attemptModel("alpha/model-a"),
        },
      }),
      cursor: runtimeCursor(30),
    });
    await peer.awaitRequests(4);
    peer.respond(4, { type: "subscribed", after_cursor: runtimeCursor(30) });
    await repairing;

    assert.equal(session.state?.attempt?.turn, 2);
    assert.deepEqual(session.state?.attempt?.lastUsage, usage);
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
        messages: [userMessage("m1", "missed"), assistantMessage("m2", "also missed")],
        capabilities: capabilities(11),
        model: sessionModel("beta/model-b"),
        // The attempt the client thought was running is gone.
        attempt: undefined,
      }),
      cursor: runtimeCursor(50),
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
    peer.respond(4, { type: "subscribed", after_cursor: runtimeCursor(50) });
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
      cursor: runtimeCursor(3),
    });
    await peer.awaitRequests(2);

    // The cursor expired between the snapshot and the subscription.
    peer.respondError(2, {
      type: "resync_required",
      after_cursor: runtimeCursor(3),
      earliest_serviceable: runtimeCursor(40),
    });

    const afterSnapshot = await peer.awaitRequests(3);
    assert.equal(afterSnapshot[2]?.method, "snapshot_get");
    peer.respond(3, {
      type: "snapshot",
      snapshot: snapshot({ messages: [userMessage("m1", "recovered")] }),
      cursor: runtimeCursor(41),
    });
    await peer.awaitRequests(4);
    peer.respond(4, { type: "subscribed", after_cursor: runtimeCursor(41) });
    await attaching;

    assert.equal(session.resyncCount, 1);
    assert.equal(session.state?.cursor, 41);
    assert.equal(session.state?.transcript.length, 1);
  });

  it("derives shutdown from the replacement snapshot, not prior client state", async () => {
    const { peer, session } = connect();
    await attach(peer, session, snapshot(), 0);

    peer.emit(1, { type: "runtime_shutdown" });
    await until(
      () => session.state?.runtimeShutdown === true,
      "shutdown observed",
    );

    const repairing = session.resync();
    await peer.awaitRequests(3);
    peer.respond(3, {
      type: "snapshot",
      snapshot: snapshot({ shutting_down: false }),
      cursor: runtimeCursor(60),
    });
    await peer.awaitRequests(4);
    peer.respond(4, { type: "subscribed", after_cursor: runtimeCursor(60) });
    await repairing;

    assert.equal(
      session.state?.runtimeShutdown,
      false,
      "authoritative replacement owns the shutdown value",
    );
  });

  it("waits for the exact attempt settlement and handles an already-settled attempt", async () => {
    const outcome = {
      type: "completed" as const,
      finish_reason: { type: "stop" as const },
    };
    const { peer, session } = connect();
    await attach(
      peer,
      session,
      snapshot({
        attempt: {
          attempt_id: "a1",
          phase: { type: "settled", outcome },
          turn: 1,
          model: attemptModel("alpha/model-a"),
        },
      }),
      5,
    );

    assert.deepEqual(await session.waitForAttemptSettlement("a1"), outcome);
  });

  it("resolves the settlement waiter from the matching event", async () => {
    const { peer, session } = connect();
    await attach(
      peer,
      session,
      snapshot({
        attempt: {
          attempt_id: "a1",
          phase: { type: "running" },
          turn: 1,
          model: attemptModel("alpha/model-a"),
        },
      }),
      5,
    );

    const waiting = session.waitForAttemptSettlement("a1");
    const outcome = {
      type: "cancelled" as const,
      reason: "user_requested" as const,
    };
    peer.emit(6, { type: "attempt_settled", attempt_id: "a1", outcome });
    assert.deepEqual(await waiting, outcome);
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

  it("preserves a committed transition draft in the typed attachment result", async () => {
    const { peer, session } = connect();
    await attach(peer, session);

    const switching = session.forkSession(7, "user-exact-7f3b");
    await peer.awaitRequests(3);
    peer.respond(3, {
      type: "session_committed_restart_required",
      session: sessionView({
        id: "session-2",
        active_node: "node-2",
        active_conversation_id: "conv-2",
      }),
      editor_content: [{ type: "text", text: "fork-draft-exact-7f3b" }],
      diagnostic: "catalog visibility committed; durability uncertain",
    });

    assert.deepEqual(await switching, {
      session: sessionView({
        id: "session-2",
        active_node: "node-2",
        active_conversation_id: "conv-2",
      }),
      editorContent: [{ type: "text", text: "fork-draft-exact-7f3b" }],
      restartRequired: true,
      restartDiagnostic: "catalog visibility committed; durability uncertain",
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
    peer.respond(3, { type: "shutdown_completed" });
    await shuttingDown;

    // Shutdown is not transport closure: the session still serves reads.
    const reading = session.modelGet();
    await peer.awaitRequests(4);
    peer.respond(4, { type: "model", model: sessionModel("alpha/model-a") });
    assert.equal((await reading).configured.model, "alpha/model-a");
  });
});

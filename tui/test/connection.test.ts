/**
 * The RPC contract of the one transport owner.
 *
 * Correlation, interleaving, reordering, terminal settlement, and the
 * post-terminal rule are proven with scripted records. No test uses a timer to
 * establish an ordering.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  ConnectionClosedError,
  RuntimeClientConnection,
  RuntimeRequestError,
} from "../src/runtime/connection.ts";
import { reduce, replaceFromSnapshot } from "../src/presentation/projection.ts";
import type { RuntimeClientProtocolEvent } from "../src/protocol/types.ts";
import { ScriptedPeer, until } from "./support/scripted-peer.ts";
import {
  assistantMessage,
  contextUserMessage,
  runtimeCursor,
  snapshot,
  toolMessage,
  userMessage,
} from "./support/fixtures.ts";

function connect(): { peer: ScriptedPeer; connection: RuntimeClientConnection } {
  const peer = new ScriptedPeer();
  const connection = new RuntimeClientConnection({
    input: peer.runtimeOutput,
    output: peer.clientOutput,
  });
  return { peer, connection };
}

async function assertMalformedTranscriptEventCloses(
  event: unknown,
): Promise<void> {
  const { peer, connection } = connect();
  const delivered: RuntimeClientProtocolEvent[] = [];
  let projection = replaceFromSnapshot(snapshot(), runtimeCursor(0));
  connection.onEvent((next) => {
    delivered.push(next);
    projection = reduce(projection, next);
  });

  const pending = connection.request({ method: "snapshot_get" });
  await peer.awaitRequests(1);
  peer.writeRecord({ cursor: 17, event });

  await assert.rejects(pending, (error: unknown) => {
    assert.ok(error instanceof ConnectionClosedError);
    assert.equal(error.reason, "protocol_error");
    return true;
  });
  assert.deepEqual(delivered, []);
  assert.equal(projection.cursor, 0, "rejected facts do not advance presentation state");
  assert.ok(connection.closed instanceof ConnectionClosedError);
}

describe("RuntimeClientConnection", () => {
  it("allocates request ids itself and correlates the response", async () => {
    const { peer, connection } = connect();
    const pending = connection.request({ method: "snapshot_get" });
    const [request] = await peer.awaitRequests(1);

    assert.equal(request?.method, "snapshot_get");
    assert.equal(request?.id, 1, "the connection is the sole id allocator");

    peer.respond(1, { type: "detached" });
    assert.deepEqual(await pending, { type: "detached" });
    assert.equal(connection.pendingCount, 0);
  });

  it("allocates distinct ids for pipelined requests", async () => {
    const { peer, connection } = connect();
    const first = connection.request({ method: "snapshot_get" });
    const second = connection.request({ method: "model_get" });
    const third = connection.request({ method: "capability_get" });
    const requests = await peer.awaitRequests(3);

    assert.deepEqual(
      requests.map((request) => request.id),
      [1, 2, 3],
    );
    assert.equal(connection.pendingCount, 3);

    peer.respond(1, { type: "detached" });
    peer.respond(2, { type: "detached" });
    peer.respond(3, { type: "detached" });
    await Promise.all([first, second, third]);
  });

  it("correlates responses that arrive out of request order", async () => {
    const { peer, connection } = connect();
    const first = connection.request({ method: "snapshot_get" });
    const second = connection.request({ method: "capability_get" });
    await peer.awaitRequests(2);

    // Answer the second request first.
    peer.respond(2, { type: "subscribed", after_cursor: runtimeCursor(7) });
    peer.respond(1, { type: "detached" });

    assert.deepEqual(await second, { type: "subscribed", after_cursor: 7 });
    assert.deepEqual(await first, { type: "detached" });
  });

  it("delivers events interleaved with responses without confusing them", async () => {
    const { peer, connection } = connect();
    const events: RuntimeClientProtocolEvent[] = [];
    connection.onEvent((event) => events.push(event));

    const pending = connection.request({ method: "snapshot_get" });
    await peer.awaitRequests(1);

    peer.emit(1, { type: "runtime_shutdown" });
    peer.respond(1, { type: "detached" });
    peer.emit(2, { type: "runtime_shutdown" });

    await pending;
    await until(() => events.length === 2, "both events delivered");
    assert.deepEqual(events[0]?.event, { type: "runtime_shutdown" });
    assert.deepEqual(
      events.map((event) => event.cursor),
      [1, 2],
    );
  });

  it("closes on an unknown v2 event without advancing the projection", async () => {
    const { peer, connection } = connect();
    const events: RuntimeClientProtocolEvent[] = [];
    let projection = replaceFromSnapshot(snapshot(), runtimeCursor(0));
    connection.onEvent((event) => {
      events.push(event);
      projection = reduce(projection, event);
    });

    const rejectionCounts = { pending: 0 };
    const pending = connection
      .request({ method: "snapshot_get" })
      .catch((error: unknown) => {
        rejectionCounts.pending += 1;
        throw error;
      });
    await peer.awaitRequests(1);

    peer.writeRecord({
      cursor: 17,
      event: { type: "future_semantic_transition" },
    });

    await assert.rejects(pending, (error: unknown) => {
      assert.ok(error instanceof ConnectionClosedError);
      assert.equal(error.reason, "protocol_error");
      return true;
    });
    assert.equal(rejectionCounts.pending, 1);
    assert.ok(connection.closed instanceof ConnectionClosedError);
    assert.equal(connection.closed.reason, "protocol_error");
    assert.deepEqual(events, []);
    assert.equal(projection.cursor, 0);

    await assert.rejects(
      connection.request({ method: "model_get" }),
      (error: unknown) => {
        assert.ok(error instanceof ConnectionClosedError);
        assert.equal(error.reason, "protocol_error");
        return true;
      },
    );
    assert.equal(peer.requests.length, 1);
  });

  for (const [label, event] of [
    [
      "Assistant message_committed",
      {
        type: "message_committed",
        message: assistantMessage("missing-assistant-cursor", "bad"),
      },
    ],
    [
      "Tool message_committed",
      {
        type: "message_committed",
        message: toolMessage("missing-tool-cursor", "call-1", "tool-read"),
      },
    ],
    [
      "ordinary User message_committed",
      {
        type: "message_committed",
        message: userMessage("missing-user-cursor", "bad"),
      },
    ],
    [
      "visible inbound_enqueued",
      {
        type: "inbound_enqueued",
        sequence: 1,
        message: userMessage("missing-inbound-cursor", "bad"),
      },
    ],
    [
      "hidden Context with a cursor",
      {
        type: "message_committed",
        message: contextUserMessage("hidden-context", "runtime status"),
        transcript_cursor: 8,
      },
    ],
    ] as const) {
    it(`closes on ${label} before the malformed fact reaches the reducer`, async () => {
      await assertMalformedTranscriptEventCloses(event);
    });
  }

  it("rejects a request with its typed protocol error and stays usable", async () => {
    const { peer, connection } = connect();
    const failing = connection.request({ method: "cancel_current_attempt" });
    await peer.awaitRequests(1);
    peer.respondError(1, { type: "no_current_attempt" });

    await assert.rejects(failing, (error: unknown) => {
      assert.ok(error instanceof RuntimeRequestError);
      assert.equal(error.error.type, "no_current_attempt");
      return true;
    });

    // A semantic error is an answer from a healthy runtime, not a transport
    // failure: the next request still works.
    assert.equal(connection.closed, undefined);
    const next = connection.request({ method: "snapshot_get" });
    await peer.awaitRequests(2);
    peer.respond(2, { type: "detached" });
    await next;
  });

  it("treats an unknown response id as terminal instead of guessing", async () => {
    const { peer, connection } = connect();
    const pending = connection.request({ method: "snapshot_get" });
    await peer.awaitRequests(1);

    peer.respond(99, { type: "detached" });

    await assert.rejects(pending, (error: unknown) => {
      assert.ok(error instanceof ConnectionClosedError);
      assert.equal(error.reason, "protocol_error");
      return true;
    });
  });

  it("treats a duplicate response as terminal", async () => {
    const { peer, connection } = connect();
    const pending = connection.request({ method: "snapshot_get" });
    await peer.awaitRequests(1);

    peer.respond(1, { type: "detached" });
    await pending;

    const second = connection.request({ method: "model_get" });
    await peer.awaitRequests(2);
    // Re-answering the already-settled id 1 is a correlation violation.
    peer.respond(1, { type: "detached" });

    await assert.rejects(second, (error: unknown) => {
      assert.ok(error instanceof ConnectionClosedError);
      assert.equal(error.reason, "protocol_error");
      return true;
    });
  });

  it("rejects every pending request exactly once on EOF", async () => {
    const { peer, connection } = connect();
    const first = connection.request({ method: "snapshot_get" });
    const second = connection.request({ method: "model_get" });
    const third = connection.request({ method: "capability_get" });
    await peer.awaitRequests(3);

    peer.endOutput();

    for (const pending of [first, second, third]) {
      await assert.rejects(pending, (error: unknown) => {
        assert.ok(error instanceof ConnectionClosedError);
        assert.equal(error.reason, "input_eof");
        return true;
      });
    }
    assert.equal(connection.pendingCount, 0);
  });

  it("rejects pending requests when the stream ends mid-record", async () => {
    const { peer, connection } = connect();
    const pending = connection.request({ method: "snapshot_get" });
    await peer.awaitRequests(1);

    peer.writeRaw('{"id":1,"result"');
    peer.endOutput();

    await assert.rejects(pending, (error: unknown) => {
      assert.ok(error instanceof ConnectionClosedError);
      assert.equal(error.reason, "framing_error");
      return true;
    });
  });

  it("rejects pending requests on a malformed protocol record", async () => {
    const { peer, connection } = connect();
    const pending = connection.request({ method: "snapshot_get" });
    await peer.awaitRequests(1);

    peer.writeRaw("this is not a protocol record\n");

    await assert.rejects(pending, (error: unknown) => {
      assert.ok(error instanceof ConnectionClosedError);
      assert.equal(error.reason, "framing_error");
      return true;
    });
  });

  it("rejects pending requests when the process exits", async () => {
    const { peer, connection } = connect();
    const pending = connection.request({ method: "snapshot_get" });
    await peer.awaitRequests(1);

    connection.reportProcessExit(101, null);

    await assert.rejects(pending, (error: unknown) => {
      assert.ok(error instanceof ConnectionClosedError);
      assert.equal(error.reason, "process_exit");
      return true;
    });
  });

  it("fails a request issued after termination immediately", async () => {
    const { peer, connection } = connect();
    peer.endOutput();
    await until(() => connection.closed !== undefined, "connection closed");

    await assert.rejects(
      connection.request({ method: "snapshot_get" }),
      (error: unknown) => {
        assert.ok(error instanceof ConnectionClosedError);
        return true;
      },
    );
    // Nothing was written for the post-terminal request.
    assert.equal(peer.requests.length, 0);
  });

  it("notifies close listeners exactly once, including late subscribers", async () => {
    const { peer, connection } = connect();
    const observed: string[] = [];
    connection.onClose((error) => observed.push(`early:${error.reason}`));

    peer.endOutput();
    await until(() => observed.length === 1, "close observed");

    connection.onClose((error) => observed.push(`late:${error.reason}`));
    assert.deepEqual(observed, ["early:input_eof", "late:input_eof"]);
  });

  it("writes records in issue order", async () => {
    const { peer, connection } = connect();
    const methods = [
      "snapshot_get",
      "model_get",
      "capability_get",
      "model_catalog_get",
    ] as const;
    for (const method of methods) {
      void connection.request({ method }).catch(() => {});
    }
    const requests = await peer.awaitRequests(methods.length);
    assert.deepEqual(
      requests.map((request) => request.method),
      [...methods],
    );
  });

  it("carries whole-state model configuration through to the wire", async () => {
    const { peer, connection } = connect();
    void connection
      .request({
        method: "model_set",
        config: { model: "alpha/model-a", reasoningProfile: "on" },
      })
      .catch(() => {});
    const [request] = await peer.awaitRequests(1);

    assert.equal(request?.method, "model_set");
    assert.deepEqual(request?.method === "model_set" ? request.config : null, {
      model: "alpha/model-a",
      reasoningProfile: "on",
    });
  });
});

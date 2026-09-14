/**
 * The one typed App Server client, proved at its own boundary.
 *
 * These tests use {@link FakeTransport}, which has no bytes at all. That is
 * deliberate: every contract below is a *client* contract — negotiation,
 * correlation, routing, fencing, settlement, no-replay — and running them over
 * a real pipe would let a framing bug masquerade as a semantics bug, or a
 * semantics bug hide behind framing that happened to work.
 *
 * Nothing here sleeps. Every ordering is established by a data barrier: a test
 * writes a message and the client's own promise settles, or it awaits the
 * request log reaching an exact count.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  APP_SERVER_PROTOCOL_VERSION,
  AppServerClient,
  AppServerRequestError,
  SIDE_EFFECTING_METHODS,
  UncertainOutcomeError,
  isResyncRequired,
  isStaleAttachment,
  isUncertainOutcome,
} from "../src/app-server/client.ts";
import { TransportClosedError } from "../src/app-server/transport.ts";
import { AppServerSession } from "../src/app-server/session.ts";
import type {
  AttachmentTarget,
  MethodResult,
  RpcError,
} from "../src/protocol/app-server.ts";
import {
  FakeTransport,
  notification,
  paramsOf,
  tick,
} from "./support/app-server-peer.ts";
import { runtimeCursor, snapshot } from "./support/fixtures.ts";

const CAPABILITIES = {
  multi_session: true,
  single_writable_controller: true,
  headless_interactions: true,
  experimental_methods: [],
};

function target(overrides: Partial<AttachmentTarget> = {}): AttachmentTarget {
  return {
    session_id: "session-a",
    conversation_id: "conv-a",
    runtime_incarnation: "1",
    attachment_id: "attach-1",
    ...overrides,
  };
}

/** Answers the pending `initialize` and returns the live client. */
async function initialized(): Promise<{
  transport: FakeTransport;
  client: AppServerClient;
}> {
  const transport = new FakeTransport();
  const pending = AppServerClient.initialize({ transport });
  const [request] = await transport.log.awaitMethod("initialize");
  transport.respond(request!.id, {
    type: "initialized",
    protocol_version: APP_SERVER_PROTOCOL_VERSION,
    capabilities: CAPABILITIES,
  });
  return { transport, client: await pending };
}

/**
 * Attaches one Session.
 *
 * The count is captured before the request is issued so a second attach on the
 * same connection answers its *own* request rather than re-answering the first
 * one — the same correlation discipline the client itself keeps.
 */
async function attached(
  client: AppServerClient,
  transport: FakeTransport,
  attachmentTarget = target(),
): Promise<AppServerSession> {
  const attaches = transport.log.count("session/attach") + 1;
  const pending = AppServerSession.attach(client, attachmentTarget.session_id);
  const attach = (await transport.log.awaitMethod("session/attach", attaches)).at(-1)!;
  transport.respond(attach.id, {
    type: "attached",
    target: attachmentTarget,
    snapshot: snapshot(),
    cursor: runtimeCursor(5),
  });
  return pending;
}

describe("initialization", () => {
  it("negotiates the protocol version once and records server capabilities", async () => {
    const { client, transport } = await initialized();
    const params = paramsOf(transport.log.matching("initialize")[0]!, "initialize");
    assert.equal(params.protocol_version, APP_SERVER_PROTOCOL_VERSION);
    assert.equal(params.client.name, "rustx-tui");
    assert.deepEqual(client.capabilities, CAPABILITIES);
    assert.equal(transport.log.count("initialize"), 1);
  });

  it("fails explicitly on a version the server does not support", async () => {
    const transport = new FakeTransport();
    const pending = AppServerClient.initialize({ transport });
    const [request] = await transport.log.awaitMethod("initialize");
    const error: RpcError = {
      code: -32000,
      message: "unsupported protocol version",
      data: { kind: "unsupported_version", supported: 2, requested: 1 },
    };
    transport.respondError(request!.id, error);

    const failure = await pending.then(
      () => undefined,
      (cause: unknown) => cause,
    );
    assert.ok(failure instanceof AppServerRequestError);
    assert.equal(failure.kind, "unsupported_version");
    // There is no downgrade: the client does not retry with another version.
    assert.equal(transport.log.count("initialize"), 1);
  });

  it("refuses a negotiated version other than the one it asked for", async () => {
    const transport = new FakeTransport();
    const pending = AppServerClient.initialize({ transport });
    const [request] = await transport.log.awaitMethod("initialize");
    transport.respond(request!.id, {
      type: "initialized",
      protocol_version: 99,
      capabilities: CAPABILITIES,
    });
    await assert.rejects(pending, /negotiated protocol 99/);
  });
});

describe("request correlation", () => {
  it("correlates pipelined requests by id even when responses finish out of order", async () => {
    const { client, transport } = await initialized();
    const first = client.call("server/info", {}, "server_info");
    const second = client.call(
      "session/list",
      { query: null, offset: 0, limit: 2 },
      "sessions",
    );
    const requests = await transport.log.awaitRequests(3);
    const info = requests[1]!;
    const list = requests[2]!;
    assert.notEqual(info.id, list.id);

    // Answer the *second* request first. Correlation is by id and nothing
    // else, so neither promise may take the other's result.
    transport.respond(list.id, { type: "sessions", sessions: [], next_offset: null });
    transport.respond(info.id, { type: "server_info", capabilities: CAPABILITIES });

    assert.equal((await second).type, "sessions");
    assert.equal((await first).type, "server_info");
  });

  it("allocates a fresh id per request and never reuses one", async () => {
    const { client, transport } = await initialized();
    void client.call("server/info", {}, "server_info");
    void client.call("server/info", {}, "server_info");
    const requests = await transport.log.awaitRequests(3);
    const ids = requests.map((request) => request.id);
    assert.equal(new Set(ids).size, ids.length, "ids are never reused");
  });

  it("rejects a typed domain failure without ending the connection", async () => {
    const { client, transport } = await initialized();
    const pending = client.call("server/info", {}, "server_info");
    const [request] = await transport.log.awaitMethod("server/info");
    transport.respondError(request!.id, {
      code: -32000,
      message: "controller in use",
      data: { kind: "controller_in_use" },
    });
    await assert.rejects(pending, AppServerRequestError);
    // A protocol error is a well-formed answer from a healthy server.
    assert.equal(client.closed, undefined);
  });

  it("ends the connection when the server answers an id nobody asked for", async () => {
    const { client, transport } = await initialized();
    transport.respond(4242, { type: "detached" });
    await tick();
    assert.ok(client.closed);
    assert.equal(client.closed?.reason, "protocol_error");
  });
});

describe("terminal settlement", () => {
  it("settles every outstanding request exactly once", async () => {
    const { client, transport } = await initialized();
    const settled: string[] = [];
    const reads = [
      client.call("server/info", {}, "server_info"),
      client.call("session/list", { query: null, offset: 0, limit: 1 }, "sessions"),
    ];
    for (const [index, read] of reads.entries()) {
      void read.then(
        () => settled.push(`resolved-${index}`),
        () => settled.push(`rejected-${index}`),
      );
    }
    await transport.log.awaitRequests(3);
    assert.equal(client.pendingCount, 2);

    transport.fail("input_eof", "the peer closed its output");
    // A second terminal event must not settle anything twice.
    transport.fail("process_exit", "and then the process exited");
    await tick();

    assert.deepEqual(settled.sort(), ["rejected-0", "rejected-1"]);
    assert.equal(client.pendingCount, 0);
  });

  it("fails a request issued after termination without sending anything", async () => {
    const { client, transport } = await initialized();
    const before = transport.log.requests.length;
    transport.fail("input_eof");
    await assert.rejects(
      client.call("server/info", {}, "server_info"),
      TransportClosedError,
    );
    assert.equal(transport.log.requests.length, before, "nothing was written");
  });

  it("reports a lost read as a connection failure, not an unknown outcome", async () => {
    const { client, transport } = await initialized();
    const read = client.call("session/list", { query: null, offset: 0, limit: 1 }, "sessions");
    await transport.log.awaitMethod("session/list");
    transport.fail("socket_error");
    const failure = await read.then(() => undefined, (cause: unknown) => cause);
    // Re-reading after reconnecting is always safe: nothing happened.
    assert.ok(failure instanceof TransportClosedError);
    assert.ok(!isUncertainOutcome(failure));
  });
});

describe("uncertain mutations", () => {
  it("never replays a side-effecting request whose response was lost", async () => {
    const { client, transport } = await initialized();
    const submitted = client.call(
      "turn/start",
      { target: target(), content: [{ type: "text", text: "ship it" }] },
      "inbound_accepted",
    );
    await transport.log.awaitMethod("turn/start");
    transport.fail("input_eof", "the connection dropped mid-turn");

    const failure = await submitted.then(
      () => undefined,
      (cause: unknown) => cause,
    );
    assert.ok(failure instanceof UncertainOutcomeError);
    assert.equal(failure.method, "turn/start");
    assert.match(failure.message, /whether the server accepted it is unknown/);
    // The whole point: exactly one `turn/start` was ever written.
    await tick();
    assert.equal(transport.log.count("turn/start"), 1);
  });

  it("covers every mutating method in the vocabulary", () => {
    // A method that can change server state and is missing here would settle
    // as a plain failure, and a caller could reasonably resend it.
    for (const method of [
      "turn/start",
      "turn/steer",
      "turn/cancel",
      "interaction/respond",
      "interaction/cancel",
      "session/create",
      "session/delete",
      "session/name",
      "session/fork",
      "session/branch",
      "session/unload",
      "settings/replace",
      "settings/setModel",
      "settings/setApprovalMode",
    ] as const) {
      assert.ok(
        SIDE_EFFECTING_METHODS.has(method),
        `${method} must settle as an unknown outcome`,
      );
    }
    // Pure reads are deliberately absent: reissuing one changes nothing.
    for (const method of ["session/list", "session/read", "session/snapshot"] as const) {
      assert.ok(!SIDE_EFFECTING_METHODS.has(method));
    }
  });
});

describe("Session routing and stale fencing", () => {
  it("routes each Session's events to its own projection", async () => {
    const { client, transport } = await initialized();
    const a = await attached(client, transport, target());
    const b = await attached(client, transport, {
      session_id: "session-b",
      conversation_id: "conv-b",
      runtime_incarnation: "2",
      attachment_id: "attach-2",
    });

    const stopA = client.onNotification((message) => a.applyNotification(message));
    const stopB = client.onNotification((message) => b.applyNotification(message));

    transport.emit(a.target, runtimeCursor(6), { type: "runtime_shutdown" });
    await tick();

    assert.equal(a.state.runtimeShutdown, true);
    assert.equal(
      b.state.runtimeShutdown,
      false,
      "Session B's projection is untouched by Session A's event",
    );
    stopA();
    stopB();
  });

  it("declines an event addressed to a superseded incarnation", async () => {
    const { client, transport } = await initialized();
    const session = await attached(client, transport, target());

    const stale = { ...session.target, runtime_incarnation: "0" };
    const accepted = session.applyNotification(
      notification("session/event", {
        target: stale,
        cursor: runtimeCursor(6),
        event: { type: "runtime_shutdown" },
      }),
    );

    assert.equal(accepted, false, "a stale incarnation is not this attachment");
    assert.equal(session.state.runtimeShutdown, false);
  });

  it("declines an event addressed to a superseded attachment of the same runtime", async () => {
    const { client, transport } = await initialized();
    const session = await attached(client, transport, target());
    const previous = { ...session.target, attachment_id: "attach-0" };

    assert.equal(
      session.applyNotification(
        notification("session/event", {
          target: previous,
          cursor: runtimeCursor(6),
          event: { type: "runtime_shutdown" },
        }),
      ),
      false,
    );
    assert.equal(session.state.runtimeShutdown, false);
  });

  it("ignores an observation at or before the installed cursor", async () => {
    const { client, transport } = await initialized();
    const session = await attached(client, transport, target());
    // The snapshot was taken at cursor 5, so 5 is already described by it.
    session.applyNotification(
      notification("session/event", {
        target: session.target,
        cursor: runtimeCursor(5),
        event: { type: "runtime_shutdown" },
      }),
    );
    assert.equal(session.state.runtimeShutdown, false);
    assert.equal(session.state.cursor, "5");
  });

  it("orders cursors numerically, not as text", async () => {
    const { client, transport } = await initialized();
    const session = await attached(client, transport, target());
    // "10" < "5" lexicographically and 10 > 5 numerically. A client comparing
    // text would silently drop every event past cursor 9.
    session.applyNotification(
      notification("session/event", {
        target: session.target,
        cursor: runtimeCursor(10),
        event: { type: "runtime_shutdown" },
      }),
    );
    assert.equal(session.state.runtimeShutdown, true);
    assert.equal(session.state.cursor, "10");
  });

  it("surfaces stale attachment and stale runtime as their own failures", () => {
    const staleAttachment = new AppServerRequestError("turn/start", {
      code: -32000,
      message: "stale",
      data: { kind: "stale_attachment" },
    });
    const staleRuntime = new AppServerRequestError("turn/start", {
      code: -32000,
      message: "stale",
      data: { kind: "stale_runtime" },
    });
    assert.ok(isStaleAttachment(staleAttachment));
    assert.ok(isStaleAttachment(staleRuntime));
    assert.ok(!isStaleAttachment(new Error("something else")));
  });
});

describe("authoritative repair", () => {
  it("replaces the projection from a fresh snapshot on resyncRequired", async () => {
    const { client, transport } = await initialized();
    const session = await attached(client, transport, target());
    const stop = client.onNotification((message) =>
      session.applyNotification(message),
    );

    let replaced = 0;
    session.onSnapshot(() => {
      replaced += 1;
    });

    transport.notify(
      notification("session/resyncRequired", {
        target: session.target,
        after_cursor: runtimeCursor(5),
        earliest_serviceable: runtimeCursor(40),
      }),
    );

    const snapshotRequest = (await transport.log.awaitMethod("session/snapshot")).at(-1)!;
    transport.respond(snapshotRequest.id, {
      type: "snapshot",
      snapshot: snapshot({ shutting_down: true }),
      cursor: runtimeCursor(41),
    });
    const resubscribe = (await transport.log.awaitMethod("session/subscribe")).at(-1)!;
    transport.respond(resubscribe.id, {
      type: "subscribed",
      after_cursor: runtimeCursor(41),
    });
    await tick();

    // The gap is never interpolated: the client asks for authoritative state
    // and installs exactly that.
    assert.equal(session.state.cursor, "41");
    assert.equal(session.state.runtimeShutdown, true);
    assert.equal(session.resyncCount, 1);
    assert.equal(replaced, 1);
    stop();
  });

  it("subscribes only when repairing, never after attach", async () => {
    const { client, transport } = await initialized();
    const session = await attached(client, transport, target());

    // Attach already registered this attachment's subscription at the snapshot
    // cursor. Re-registering here would replace a registration the client
    // already holds, and reopen the very gap that single cut closes.
    assert.equal(transport.log.count("session/subscribe"), 0);

    const repair = session.resync();
    const snapshotRequest = (await transport.log.awaitMethod("session/snapshot")).at(-1)!;
    transport.respond(snapshotRequest.id, {
      type: "snapshot",
      snapshot: snapshot(),
      cursor: runtimeCursor(60),
    });
    const resubscribe = (await transport.log.awaitMethod("session/subscribe")).at(-1)!;
    // The new registration starts exactly where the new snapshot ends.
    assert.equal(
      paramsOf(resubscribe, "session/subscribe").after_cursor,
      runtimeCursor(60),
    );
    transport.respond(resubscribe.id, {
      type: "subscribed",
      after_cursor: runtimeCursor(60),
    });
    await repair;
    assert.equal(session.state.cursor, "60");
  });

  it("repairs again when the repair's own cursor has already fallen out of the ring", async () => {
    const { client, transport } = await initialized();
    const session = await attached(client, transport, target());
    const repair = session.resync();

    const first = (await transport.log.awaitMethod("session/snapshot")).at(-1)!;
    transport.respond(first.id, {
      type: "snapshot",
      snapshot: snapshot(),
      cursor: runtimeCursor(60),
    });
    const subscribe = (await transport.log.awaitMethod("session/subscribe")).at(-1)!;
    transport.respondError(subscribe.id, {
      code: -32000,
      message: "resync required",
      data: { kind: "resync_required" },
    });
    // The ring moved again between the snapshot and the subscription. The
    // answer is another authoritative read, never an interpolated gap.
    const second = (await transport.log.awaitMethod("session/snapshot", 2)).at(-1)!;
    transport.respond(second.id, {
      type: "snapshot",
      snapshot: snapshot(),
      cursor: runtimeCursor(90),
    });
    const resubscribe = (await transport.log.awaitMethod("session/subscribe", 2)).at(-1)!;
    transport.respond(resubscribe.id, {
      type: "subscribed",
      after_cursor: runtimeCursor(90),
    });
    await repair;

    assert.equal(session.state.cursor, "90");
    assert.ok(
      isResyncRequired(
        new AppServerRequestError("session/subscribe", {
          code: -32000,
          message: "resync required",
          data: { kind: "resync_required" },
        }),
      ),
    );
  });

  it("cannot install a snapshot that lost its attachment mid-flight", async () => {
    const { client, transport } = await initialized();
    const session = await attached(client, transport, target());
    const repair = session.resync();
    const snapshotRequest = (await transport.log.awaitMethod("session/snapshot")).at(-1)!;

    // The attachment is released while the authoritative read is in flight.
    const detach = session.detach();
    const detachRequest = (await transport.log.awaitMethod("session/detach")).at(-1)!;
    transport.respond(detachRequest.id, { type: "detached" });
    await detach;

    transport.respond(snapshotRequest.id, {
      type: "snapshot",
      snapshot: snapshot({ shutting_down: true }),
      cursor: runtimeCursor(99),
    });
    await repair;

    // The late response belongs to an ownership that has ended. Installing it
    // would overwrite current truth with stale truth.
    assert.equal(session.state.cursor, "5");
    assert.equal(session.state.runtimeShutdown, false);
    assert.equal(session.resyncCount, 0);
  });
});

describe("attachment release", () => {
  it("detach removes the client relationship and nothing else", async () => {
    const { client, transport } = await initialized();
    const session = await attached(client, transport, target());
    const detach = session.detach();
    const [request] = await transport.log.awaitMethod("session/detach");
    transport.respond(request!.id, { type: "detached" });
    await detach;

    // Detach is the only thing that was sent: no cancellation, no unload, no
    // shutdown, no interaction settlement.
    const methods = transport.log.requests.map((message) => message.method);
    assert.ok(!methods.includes("turn/cancel"));
    assert.ok(!methods.includes("session/unload"));
    assert.ok(!methods.includes("interaction/cancel"));
    assert.equal(session.released, true);
  });

  it("reports session/closed without fabricating a runtime outcome", async () => {
    const { client, transport } = await initialized();
    const session = await attached(client, transport, target());
    let closed = 0;
    session.onClosed(() => {
      closed += 1;
    });

    session.applyNotification(
      notification("session/closed", { target: session.target }),
    );

    assert.equal(closed, 1);
    assert.equal(session.serverClosed, true);
    // Residency ended. That is not an attempt settling, an interaction being
    // answered, or a tool completing.
    assert.equal(session.state.attempt, undefined);
    assert.equal(session.state.runtimeShutdown, false);
  });
});

describe("typed results", () => {
  it("refuses a result whose discriminator is not the method's", async () => {
    const { client, transport } = await initialized();
    const pending = client.call("server/info", {}, "server_info");
    const [request] = await transport.log.awaitMethod("server/info");
    transport.respond(request!.id, { type: "detached" } as MethodResult);
    await assert.rejects(pending, /returned detached instead of server_info/);
  });
});

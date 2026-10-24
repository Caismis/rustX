import { METHOD_POLICY, requestEffect } from "../../protocol/app-server/method-policy.ts";
import { jobReceipt, domainReceipt } from './support/fixtures.ts';
import { checkpoint } from "./support/fixtures.ts";
import { fixtures as uploadFixtures } from "../../protocol/app-server/fixtures.ts";
const uploadPolicy = uploadFixtures.find(row => "result" in row && row.result?.type === "initialized")!;
const nativeUploadPolicy = "result" in uploadPolicy && uploadPolicy.result?.type === "initialized" ? uploadPolicy.result.capabilities.upload_policy : (() => { throw new Error("missing policy") })();
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
  UncertainOutcomeError,
  isDeliveryCancelled,
  isResyncRequired,
  isStaleAttachment,
  isUncertainOutcome,
} from "../src/app-server/client.ts";
import { TransportClosedError } from "../src/app-server/transport.ts";
import { AppServerSession } from "../src/app-server/session.ts";
import type {
  AttachmentTarget,
  MethodResult,
  MethodName,
  RpcError,
} from "../src/protocol/app-server.ts";
import {
  FakeTransport,
  notification,
  paramsOf,
  tick,
} from "./support/app-server-peer.ts";
import { backgroundExecution, subagent, runtimeCursor, snapshot } from "./support/fixtures.ts";

const CAPABILITIES = {
  upload_policy: nativeUploadPolicy, multi_session: true,
  single_writable_controller: true,
  headless_interactions: true, delivery_access: false,
  experimental_methods: [],
};

function target(overrides: Partial<AttachmentTarget> = {}): AttachmentTarget {
  return {
    session_id: "ses_fa57a52d-bf08-7902-9852-9730a3e99db6",
    conversation_id: "conv_bf9033a7-86e2-71aa-8314-b791ebfdbfec",
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
    type: "initialized", authority_id: 'fixture-app-server-authority',
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
  transport.respond(attach.id, {type:'attached', target: attachmentTarget, configuration_revision:'0', attachment_kind:'controller', observation_source:'live'});
  const read=(await transport.log.awaitMethod('session/checkpoint',attaches)).at(-1)!;
  transport.respond(read.id,{type:'checkpoint',checkpoint:checkpoint({...snapshot(),conversation_id:attachmentTarget.conversation_id},runtimeCursor(5))});
  const subscribe=(await transport.log.awaitMethod('session/subscribe',attaches)).at(-1)!;
  transport.respond(subscribe.id,{type:'subscribed',target:attachmentTarget,registration_id:`attached-${attaches}`,after_cursor:runtimeCursor(5)});
  const configuration=(await transport.log.awaitMethod('session/configuration',attaches)).at(-1)!;
  transport.respond(configuration.id,{type:'session_configuration',projection:(null)});
  return pending;
}

describe("initialization", () => {
  it("negotiates the protocol version once and records server capabilities", async () => {
    const { client, transport } = await initialized();
    const params = paramsOf(transport.log.matching("initialize")[0]!, "initialize");
    assert.equal(APP_SERVER_PROTOCOL_VERSION, 57);
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
      data: { kind: "unsupported_version", supported: 6, requested: 7 },
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
    assert.ok(transport.closed, "failed initialization releases its physical transport");
  });

  it("refuses a v53 server before using the v57 Model capabilities contract", async () => {
    const transport = new FakeTransport();
    const pending = AppServerClient.initialize({ transport });
    const [request] = await transport.log.awaitMethod("initialize");
    transport.respond(request!.id, {
      type: "initialized", authority_id: 'fixture-app-server-authority',
      protocol_version: 53,
      capabilities: CAPABILITIES,
    });
    await assert.rejects(pending, /negotiated protocol 53, this client speaks 57/);
    assert.equal(transport.log.count("initialize"), 1);
    assert.ok(transport.closed, "invalid initialization does not retain a listener slot");
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

  it("classifies a lost exact Session summary as a retryable read, never uncertain mutation", async () => {
    const { client, transport } = await initialized();
    const read = client.call("session/summary", { session_id: "ses_00000000-0000-7000-8000-000000000001" }, "session_summary");
    await transport.log.awaitMethod("session/summary");
    transport.fail("socket_error");
    const failure = await read.then(() => undefined, (cause: unknown) => cause);
    assert.ok(failure instanceof TransportClosedError);
    assert.ok(!isUncertainOutcome(failure));
    assert.equal(transport.log.count("session/summary"), 1);
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
});

describe("Session routing and stale fencing", () => {
  it("routes each Session's events to its own projection", async () => {
    const { client, transport } = await initialized();
    const a = await attached(client, transport, target());
    const b = await attached(client, transport, {
      session_id: "ses_e8de016f-bd70-782f-ad23-25e81df82550",
      conversation_id: "conv_449370cc-308b-7409-94bd-ff539142e4fb",
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
    for (let cursor = 6; cursor <= 10; cursor++) session.applyNotification(
      notification("session/event", { target: session.target, cursor: runtimeCursor(cursor), event: { type: "runtime_shutdown" } }),
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

    const snapshotRequest = (await transport.log.awaitMethod("session/checkpoint", 2)).at(-1)!;
    transport.respond(snapshotRequest.id, {type:'checkpoint', checkpoint:checkpoint(snapshot({ conversation_id: session.target.conversation_id, shutting_down: true }),runtimeCursor(41))});
    const resubscribe = (await transport.log.awaitMethod("session/subscribe", 2)).at(-1)!;
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

  it("does not repeat the initial checkpoint subscription until repairing", async () => {
    const { client, transport } = await initialized();
    const session = await attached(client, transport, target());

    // The attach workflow already captured its independent checkpoint and
    // subscribed at that exact cut. Repeating Subscribe here would replace
    // the registration the client already holds.
    assert.equal(transport.log.count("session/subscribe"), 1);

    const repair = session.resync();
    const snapshotRequest = (await transport.log.awaitMethod("session/checkpoint", 2)).at(-1)!;
    transport.respond(snapshotRequest.id, {type:'checkpoint', checkpoint:checkpoint(snapshot({ conversation_id: session.target.conversation_id }),runtimeCursor(60))});
    const resubscribe = (await transport.log.awaitMethod("session/subscribe", 2)).at(-1)!;
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

    const first = (await transport.log.awaitMethod("session/checkpoint", 2)).at(-1)!;
    transport.respond(first.id, {type:'checkpoint', checkpoint:checkpoint(snapshot({ conversation_id: session.target.conversation_id }),runtimeCursor(60))});
    const subscribe = (await transport.log.awaitMethod("session/subscribe", 2)).at(-1)!;
    transport.respondError(subscribe.id, {
      code: -32000,
      message: "resync required",
      data: { kind: "resync_required" },
    });
    // The ring moved again between the snapshot and the subscription. The
    // answer is another authoritative read, never an interpolated gap.
    const second = (await transport.log.awaitMethod("session/checkpoint", 3)).at(-1)!;
    transport.respond(second.id, {type:'checkpoint', checkpoint:checkpoint(snapshot({ conversation_id: session.target.conversation_id }),runtimeCursor(90))});
    const resubscribe = (await transport.log.awaitMethod("session/subscribe", 3)).at(-1)!;
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
    const snapshotRequest = (await transport.log.awaitMethod("session/checkpoint", 2)).at(-1)!;

    // The attachment is released while the authoritative read is in flight.
    const detach = session.detach();
    const detachRequest = (await transport.log.awaitMethod("session/detach")).at(-1)!;
    transport.respond(detachRequest.id, { type: "detached" });
    await detach;

    transport.respond(snapshotRequest.id, {type:'checkpoint', checkpoint:checkpoint(snapshot({ conversation_id: session.target.conversation_id, shutting_down: true }),runtimeCursor(99))});
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
    assert.ok(!methods.includes("session/switchNode"));
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

it("overlapping repair requests share one authoritative acquisition", async () => {
  const { client, transport } = await initialized();
  const session = await attached(client, transport);
  const older = session.resync();
  const newer = session.resync();
  const request = (await transport.log.awaitMethod("session/checkpoint", 2)).at(-1)!;
  assert.equal(transport.log.count("session/checkpoint"), 2);
  transport.respond(request!.id, {type:'checkpoint', checkpoint:checkpoint(snapshot({ conversation_id: session.target.conversation_id }),"20")});
  const subscribe = (await transport.log.awaitMethod("session/subscribe", 2)).at(-1)!;
  transport.respond(subscribe!.id, { type: "subscribed", after_cursor: "20" });
  await Promise.all([older, newer]);
  assert.equal(session.state.cursor, "20");
  assert.equal(session.state.runtimeShutdown, false);
  assert.equal(transport.log.count("session/subscribe"), 2);
});

it("server closure fences pending snapshots and all later observations", async () => {
  const { client, transport } = await initialized();
  const session = await attached(client, transport);
  const pending = session.resync();
  const request = (await transport.log.awaitMethod("session/checkpoint", 2)).at(-1)!;
  session.applyNotification({ jsonrpc: "2.0", method: "session/closed", params: { target: session.target } });
  transport.respond(request.id, {type:'checkpoint', checkpoint:checkpoint(snapshot({ conversation_id: session.target.conversation_id, shutting_down: true }),"99")});
  await pending;
  session.applyNotification(notification("session/event", { target: session.target, cursor: "100", event: { type: "runtime_shutdown" } }));
  assert.equal(session.state.cursor, "5");
  assert.equal(session.state.runtimeShutdown, false);
});

describe("generated-contract ingress", () => {
  const event = () => notification("session/event", {
    target: target(), cursor: runtimeCursor(6), event: { type: "runtime_shutdown" },
  });
  const malformed: [string, unknown][] = [
    ["unknown notification", { jsonrpc: "2.0", method: "session/unknown", params: {} }],
    ["obsolete Session-only retirement", { jsonrpc: "2.0", method: "session/ownershipRetired", params: { session_id: "ses_00000000-0000-7000-8000-000000000001" } }],
    ["inexact ownership watermark", { jsonrpc: "2.0", method: "session/ownershipRetired", params: { session_id: "ses_00000000-0000-7000-8000-000000000001", retired_through: 1 } }],
    ["missing params", { jsonrpc: "2.0", method: "session/event" }],
    ["malformed target", { ...event(), params: { ...event().params, target: { session_id: "a" } } }],
    ["nested exact integer", { ...event(), params: { ...event().params, target: { ...target(), runtime_incarnation: "01" } } }],
    ["nested event", { ...event(), params: { ...event().params, event: { type: "runtime_shutdown", unexpected: true } } }],
    ["wrong jsonrpc", { ...event(), jsonrpc: "1.0" }],
    ["missing jsonrpc", { method: "session/closed", params: { target: target() } }],
    ["invalid id", { jsonrpc: "2.0", id: true, result: { type: "detached" } }],
    ["fractional id", { jsonrpc: "2.0", id: 1.5, result: { type: "detached" } }],
    ["unsafe id", { jsonrpc: "2.0", id: Number.MAX_SAFE_INTEGER + 1, result: { type: "detached" } }],
    ["exclusive response fields", { jsonrpc: "2.0", id: 2, result: { type: "detached" }, error: { code: -1, message: "failure" } }],
    ["missing response body", { jsonrpc: "2.0", id: 2 }],
    ["malformed correlated mutation result", { jsonrpc: "2.0", id: 3, result: { type: "cancellation_accepted" } }],
    ["wrong correlated mutation result tag", { jsonrpc: "2.0", id: 3, result: { type: "detached" } }],
    ["invalid result DTO", { jsonrpc: "2.0", id: 2, result: { type: "sessions", sessions: "invalid" } }],
    ["nested result bounds", { jsonrpc: "2.0", id: 2, result: {type:'checkpoint', checkpoint:checkpoint(snapshot({ context: { compaction_in_progress: false, compaction_count: -1 } }),runtimeCursor(6))} }],
    ["no nested boolean coercion", { jsonrpc: "2.0", id: 2, result: { type: "server_info", capabilities: { ...CAPABILITIES, upload_policy: nativeUploadPolicy, multi_session: "true" } } }],
    ["invalid RPC error", { jsonrpc: "2.0", id: 2, error: { code: "bad", message: null } }],
    ["invalid nested RPC error", { jsonrpc: "2.0", id: 2, error: { code: -1, message: "failure", data: { kind: "stale_settings", expected: "not-a-revision", actual: "1" } } }],
    ["unexpected request", { jsonrpc: "2.0", id: 2, method: "server/info", params: {} }],
    ["null", null], ["array", []], ["primitive", 1],
  ];
  for (const [name, record] of malformed) {
    it(`rejects ${name} without delivery or listener exceptions and settles pending requests once`, async () => {
      const { client, transport } = await initialized();
      let deliveries = 0;
      let closes = 0;
      let reads = 0;
      let mutations = 0;
      client.onNotification(() => { deliveries += 1; });
      client.onClose(() => { closes += 1; assert.equal(client.closed?.reason, "protocol_error"); });
      const read = client.call("session/list", { query: null, offset: 0, limit: 1 }, "sessions")
        .then(() => assert.fail("invalid response accepted"), (error: unknown) => { reads += 1; return error; });
      const mutation = client.call("turn/cancel", { target: target() }, "cancellation_accepted")
        .then(() => assert.fail("mutation falsely completed"), (error: unknown) => { mutations += 1; return error; });
      await transport.log.awaitRequests(3);
      assert.doesNotThrow(() => transport.deliver(record));
      assert.equal(client.closed?.reason, "protocol_error");
      assert.equal(client.pendingCount, 0);
      assert.equal(transport.disposed, true);
      transport.deliver(event());
      transport.fail("socket_error");
      const [readError, mutationError] = await Promise.all([read, mutation]);
      assert.equal(readError, client.closed);
      assert.ok(mutationError instanceof UncertainOutcomeError);
      assert.equal(mutationError.transportFailure, client.closed);
      assert.deepEqual([deliveries, closes, reads, mutations], [0, 1, 1, 1]);
      assert.equal(transport.log.count("turn/cancel"), 1);
    });
  }

  it("delivers a validated notification unchanged to semantic listeners", async () => {
    const { client, transport } = await initialized();
    const message = event();
    let observed: unknown;
    client.onNotification((value) => { observed = value; });
    transport.deliver(message);
    assert.equal(observed, message);
    assert.equal(client.closed, undefined);
  });

  it("does not relabel a valid semantic listener bug as wire corruption", async () => {
    const { client, transport } = await initialized();
    const bug = new Error("semantic bug");
    client.onNotification(() => { throw bug; });
    assert.throws(() => transport.deliver(event()), (error) => error === bug);
    assert.equal(client.closed, undefined);
  });

  it("losing initialize fails the connection without an uncertain product mutation", async () => {
    const transport = new FakeTransport();
    const pending = AppServerClient.initialize({ transport });
    await transport.log.awaitMethod("initialize");
    transport.fail("input_eof");
    await assert.rejects(pending, TransportClosedError);
    assert.equal(transport.log.count("initialize"), 1);
    assert.equal(METHOD_POLICY.initialize.effect, "connection_local");
    assert.equal(METHOD_POLICY["session/summary"].effect, "read");
  });
});

// Compile-time regression: extending the vocabulary cannot inherit a default.
// @ts-expect-error A future method requires its own response-loss decision.
const futurePolicy: Record<MethodName | "future/mutation", (typeof METHOD_POLICY)[MethodName]> = METHOD_POLICY;
void futurePolicy;

describe("bounded request ownership", () => {
  it("T20 observation and ordinary lanes remain independent with two reserved control slots", async t => {
    const {client,transport}=await initialized();t.after(()=>client.close());
    const observations=Array.from({length:8},(_,i)=>client.call('agent/observe',{target:target(),agent_id:`child-${i}`,after:null},'agent_observe'));
    const waits=Array.from({length:6},(_,i)=>i%2===0?client.call('agent/wait',{target:target(),agent_id:`agent-${i}`},'agent_wait'):client.call('job/wait',{target:target(),job_id:`job-${i}`},'job_outcome'));
    const held=Promise.allSettled([...observations,...waits]);
    await transport.log.awaitRequests(15);
    await assert.rejects(client.call('agent/observe',{target:target(),agent_id:'overflow',after:null},'agent_observe'),/observation capacity/);
    await assert.rejects(client.call('agent/list',{target:target()},'domain_page'),/rpc capacity/);
    const controls=Array.from({length:2},()=>client.call('turn/cancel',{target:target()},'cancellation_accepted'));
    const controlSettled=Promise.allSettled(controls);await transport.log.awaitRequests(17);
    assert.equal(client.pendingCount,16);assert.equal(client.closed,undefined);
    await assert.rejects(client.call('turn/cancel',{target:target()},'cancellation_accepted'),/control capacity/);
    const [cancel]=await transport.log.awaitMethod('turn/cancel');transport.respond(cancel!.id,{type:'cancellation_accepted',attempt_id:'attempt-a'});
    await controls[0];assert.equal(client.pendingCount,15);
    const next=client.call('turn/cancel',{target:target()},'cancellation_accepted');const nextSettled=next.catch(error=>error);
    await transport.log.awaitMethod('turn/cancel',3);assert.equal(client.pendingCount,16);
    transport.fail('input_eof');const results=await held;await controlSettled;
    assert.equal(results.filter(result=>result.status==='rejected').length,14);
    assert.ok((results[8] as PromiseRejectedResult).reason instanceof TransportClosedError);
    assert.equal(isUncertainOutcome((results[8] as PromiseRejectedResult).reason),false,
      "lost Agent wait is observation loss, not an unknown execution mutation");
    assert.ok((results[9] as PromiseRejectedResult).reason instanceof TransportClosedError);
    assert.ok(await nextSettled instanceof UncertainOutcomeError);
    assert.equal(transport.log.count('agent/observe'),8);assert.equal(transport.log.count('agent/wait')+transport.log.count('job/wait'),6,'no replay or domain cancellation');
  });
  it("T20 classified ordinary responses release exactly one capacity slot", async t => {
    const {client,transport}=await initialized();t.after(()=>client.close());
    const waits=Array.from({length:6},(_,i)=>client.call('job/wait',{target:target(),job_id:`job-${i}`},'job_outcome'));
    const settled=Promise.allSettled(waits);await transport.log.awaitMethod('job/wait',6);
    await assert.rejects(client.call('job/wait',{target:target(),job_id:'overflow'},'job_outcome'),/rpc capacity/);
    const [first]=await transport.log.awaitMethod('job/wait');transport.respond(first!.id,jobReceipt(backgroundExecution('exec_c8536561-1a50-7edc-a396-b3a459465efb','succeeded')));await waits[0];
    const next=client.call('job/wait',{target:target(),job_id:'next'},'job_outcome');const nextSettled=next.catch(error=>error);
    await transport.log.awaitMethod('job/wait',7);assert.equal(client.pendingCount,6);
    transport.fail('input_eof');const outcomes=await settled;
    assert.equal(outcomes[0]!.status,'fulfilled');assert.equal(outcomes.filter(result=>result.status==='rejected').length,5);
    assert.ok(await nextSettled instanceof TransportClosedError);assert.equal(transport.log.count('job/wait'),7);
  });
});

describe("request-scoped delivery cancellation", () => {
  const read = { target: target(), locator: {type:'by_message_id' as const,message_id:"tool-msg"}, delivery_index: 0 };
  const cancelled: RpcError = {
    code: -32000, message: "Delivery request was cancelled before publication",
    data: { kind: "delivery_cancelled" },
  };

  it("cancels exactly its own request and settles it once with the server's terminal outcome", async t => {
    const { client, transport } = await initialized(); t.after(() => client.close());
    const owner = new AbortController();
    const sibling = new AbortController();
    const outcome = client.callDelivery("delivery/read", read, "session_file_bytes", owner.signal).catch((error: unknown) => error);
    const other = client.callDelivery("delivery/read", read, "session_file_bytes", sibling.signal);
    const [first, second] = await transport.log.awaitMethod("delivery/read", 2);
    owner.abort();
    owner.abort();
    const [cancel] = await transport.log.awaitMethod("delivery/cancel");
    assert.deepEqual(paramsOf(cancel!, "delivery/cancel"), { request_id: first!.id }, "names the exact in-flight id");
    assert.equal(transport.log.count("delivery/cancel"), 1, "one cancel per owner");
    assert.equal(client.pendingCount, 3, "the cancelled request keeps its correlation until answered");
    transport.respond(cancel!.id, { type: "delivery_cancel", accepted: true });
    transport.respondError(first!.id, cancelled);
    const error = await outcome;
    assert.ok(isDeliveryCancelled(error));
    transport.respond(second!.id, { type: "session_file_bytes", file: REPORT_FILE, data: "QQ==" });
    assert.equal((await other).data, "QQ==", "an unrelated request is untouched");
    assert.equal(client.pendingCount, 0);
    assert.equal(client.closed, undefined, "the late answer is a known id, never a protocol failure");
  });

  it("keeps a result whose publication won the race, and sends nothing when already aborted", async t => {
    const { client, transport } = await initialized(); t.after(() => client.close());
    const owner = new AbortController();
    const outcome = client.callDelivery("delivery/locate", read, "session_file_location", owner.signal);
    const [request] = await transport.log.awaitMethod("delivery/locate");
    owner.abort();
    const [cancel] = await transport.log.awaitMethod("delivery/cancel");
    transport.respond(request!.id, { type: "session_file_location", file: REPORT_FILE, path: "/w/r.md", device: "1", inode: "2" });
    transport.respond(cancel!.id, { type: "delivery_cancel", accepted: false });
    assert.equal((await outcome).path, "/w/r.md", "a committed response is not represented as unpublished");
    const aborted = new AbortController();
    aborted.abort();
    await assert.rejects(client.callDelivery("delivery/read", read, "session_file_bytes", aborted.signal));
    assert.equal(transport.log.count("delivery/read"), 0, "nothing sent before admission");
    assert.equal(client.pendingCount, 0);
    assert.equal(client.closed, undefined);
  });

  /** Every request settled so far has reached the log; a data barrier, not a delay. */
  const drained = () => new Promise<void>((resolve) => setImmediate(resolve));

  it("reaches the server through its reserved slot when every other lane is full, never as a seventeenth request", async t => {
    const { client, transport } = await initialized(); t.after(() => client.close());
    const pending: Promise<unknown>[] = [];
    // All8observe+6ordinary+2control lanes saturate the16-request envelope.
    for(let i=0;i<8;i++)pending.push(client.call('agent/observe',{target:target(),agent_id:`child-${i}`,after:null},'agent_observe'));
    for(let i=0;i<3;i++)pending.push(client.call('job/wait',{target:target(),job_id:`wait-${i}`},'job_outcome'));
    for(let i=0;i<2;i++)pending.push(client.call('turn/cancel',{target:target()},'cancellation_accepted'));
    const owners=[new AbortController(),new AbortController(),new AbortController()];
    const outcomes=owners.map(owner=>client.callDelivery('delivery/read',read,'session_file_bytes',owner.signal).catch((error:unknown)=>error));
    const settled=Promise.allSettled(pending),reads=await transport.log.awaitMethod('delivery/read',3);
    await transport.log.awaitRequests(17);assert.equal(client.pendingCount,16);
    await assert.rejects(client.call('agent/list',{target:target()},'domain_page'),/rpc capacity/);
    await assert.rejects(client.call('turn/cancel',{target:target()},'cancellation_accepted'),/control capacity/);

    // Three reads cancelled together: one cancel in flight, the others wait
    // for the slot, so the server never sees a seventeenth request.
    for (const owner of owners) owner.abort();
    await drained();assert.equal(transport.log.count('delivery/cancel'),0,'cancel waits without becoming a seventeenth request');
    const [control]=await transport.log.awaitMethod('turn/cancel');transport.respond(control!.id,{type:'cancellation_accepted',attempt_id:'attempt-a'});
    const [first] = await transport.log.awaitMethod("delivery/cancel");
    await drained();
    assert.equal(transport.log.count("delivery/cancel"), 1);
    assert.equal(client.pendingCount, 16);
    assert.deepEqual(paramsOf(first!, "delivery/cancel"), { request_id: reads[0]!.id });

    // The second read's publication wins before its cancel is sent: that
    // cancel is never sent, and the result stands.
    transport.respond(reads[1]!.id, { type: "session_file_bytes", file: REPORT_FILE, data: "QQ==" });
    transport.respond(first!.id, { type: "delivery_cancel", accepted: true });
    const [, third] = await transport.log.awaitMethod("delivery/cancel", 2);
    await drained();
    assert.deepEqual(paramsOf(third!, "delivery/cancel"), { request_id: reads[2]!.id }, "abort order, settled requests skipped");
    assert.equal(transport.log.count("delivery/cancel"), 2);
    transport.respond(third!.id, { type: "delivery_cancel", accepted: true });
    transport.respondError(reads[0]!.id, cancelled);
    transport.respondError(reads[2]!.id, cancelled);
    const [one, two, three] = await Promise.all(outcomes);
    assert.ok(isDeliveryCancelled(one) && isDeliveryCancelled(three));
    assert.equal((two as { data: string }).data, "QQ==");
    await drained();
    assert.equal(transport.log.count("delivery/cancel"), 2, "nothing resent");
    assert.equal(client.pendingCount, 12, "every delivery and cancel slot recovered; unrelated work untouched");
    assert.equal(client.closed, undefined);
    transport.fail("input_eof");
    await settled;
  });

  it("ends the connection explicitly when the server refuses a cancellation it owes", async t => {
    const { client, transport } = await initialized(); t.after(() => client.close());
    const owner = new AbortController();
    const outcome = client.callDelivery("delivery/read", read, "session_file_bytes", owner.signal).catch((error: unknown) => error);
    await transport.log.awaitMethod("delivery/read");
    owner.abort();
    const [cancel] = await transport.log.awaitMethod("delivery/cancel");
    transport.respondError(cancel!.id, { code: -32602, message: "Invalid params", data: { kind: "invalid_params" } });
    const error = await outcome;
    assert.ok(error instanceof TransportClosedError, String(error));
    assert.match(error.message, /refused delivery\/cancel for request/);
    assert.equal(client.closed, error, "a lost cancellation is never silent");
    assert.equal(client.pendingCount, 0);
  });
});

const REPORT_FILE = {
  scope: { conversation_id: "conv_bf9033a7-86e2-71aa-8314-b791ebfdbfec", device: "1", inode: "2" },
  path: "out/r.md", name: "r.md", description: null, mime_type: "text/markdown",
};

it('T11 selected pending reads fence exact sparse native row identity before issuing a request',async t=>{
 const{client,transport}=await initialized();t.after(()=>client.close());
 const session=await attached(client,transport,target());
 const{approvalInteraction,nativeInteraction}=await import('./support/fixtures.ts');
 const routed=approvalInteraction('sparse-row');routed.interaction.conversation_id=session.target.conversation_id;routed.request.conversation_id=session.target.conversation_id;
 const header={...nativeInteraction(routed),source_offset:66,body:null,body_complete:false};
 const cut={...session.checkpoint!.read_cut,revision:'6'};
 session.applyNotification(notification('session/event',{target:session.target,cursor:'6',event:{type:'projection_transition',events:[{type:'interaction_pending',interaction:header}],metadata:{read_cut:cut,collections:[],content:[]}}}));
 const before=transport.log.count('conversation/checkpointValueRead');
 await assert.rejects(session.interactionValue(header.interaction,0,undefined,0),/view no longer owns/);
 assert.equal(transport.log.count('conversation/checkpointValueRead'),before);assert.equal(session.state.cursor,'6');
 const read=session.interactionValue(header.interaction,66,undefined,0,['request','kind','arguments']);
 const request=(await transport.log.awaitMethod('conversation/checkpointValueRead',before+1)).at(-1)!;
 assert.equal(paramsOf(request,'conversation/checkpointValueRead').item_offset,66);
 transport.respond(request.id,{type:'checkpoint_value_chunk',chunk:{representation:'canonical_json_v1',cut,domain:'pending_interactions',item_offset:66,path:['request','kind','arguments'],offset:0,bytes_base64:'ew==',readable_end:2,next_offset:1}});
 assert.equal((await read).bytes_base64,'ew==');assert.equal(session.state.cursor,'6');
});

it('T13 a lost read-only Goal Show stays an observation failure while a Goal mutation outcome remains unknown',async()=>{
 const {client,transport}=await initialized();
 const show=client.call('goal/control',{target:target(),control:{action:'show'}},'domain_page');
 const create=client.call('goal/control',{target:target(),control:{action:'create',objective:'Review the source',budget:1}},'goal_mutation');
 const showOutcome=show.catch(error=>error),createOutcome=create.catch(error=>error);
 await transport.log.awaitMethod('goal/control',2);transport.fail('input_eof','Lost terminal records');
 const readError=await showOutcome,mutationError=await createOutcome;
 assert.ok(readError instanceof TransportClosedError);assert.equal(isUncertainOutcome(readError),false);
 assert.ok(mutationError instanceof UncertainOutcomeError);assert.equal(transport.log.count('goal/control'),2);
});


it('T10 T13 a lost Agent wait does not invent an unknown execution mutation or replay the wait',async()=>{
 const {client,transport}=await initialized();
 const waiting=client.call('agent/wait',{target:target(),agent_id:'child'},'agent_wait');
 const outcome=waiting.catch(error=>error);
 await transport.log.awaitMethod('agent/wait');
 transport.fail('input_eof','Lost wait response');
 const error=await outcome;
 assert.ok(error instanceof TransportClosedError);
 assert.equal(isUncertainOutcome(error),false);
 assert.equal(transport.log.count('agent/wait'),1);
 assert.equal(transport.log.count('agent/interrupt'),0);
});


it('T18 T20 missing server authority is rejected and releases its transport',async()=>{
 const transport=new FakeTransport();const initializing=AppServerClient.initialize({transport});
 const [request]=await transport.log.awaitMethod('initialize');
 transport.respond(request!.id,{type:'initialized',authority_id:'',protocol_version:APP_SERVER_PROTOCOL_VERSION,capabilities:CAPABILITIES});
 await assert.rejects(initializing,/process authority identity/);
 assert.ok(transport.closed);assert.equal(transport.log.count('initialize'),1);
});


it('T11 T15 capability reads retain deferred native page evidence without whole-view materialization', async t => {
  const { client, transport } = await initialized();
  t.after(() => client.close());
  const session = await attached(client, transport, target());
  const loading = session.capabilitiesPage();
  const [request] = await transport.log.awaitMethod('resources/read');
  const receipt = domainReceipt('capabilities', [null]);
  receipt.page.items[0]!.deferred = true;
  receipt.page.items[0]!.body_complete = false;
  receipt.page.collections = [{ domain: 'capabilities.available_tools', revision: '0', total: '1000', loaded: '0', next_offset: '0' }];
  transport.respond(request!.id, receipt);
  const page = await loading;
  assert.deepEqual(page, receipt.page);
  assert.equal(page.items[0]!.deferred, true);
  assert.equal(page.collections![0]!.total, '1000');
  assert.equal(transport.log.count('conversation/checkpointValueRead'), 0);
  assert.equal(transport.log.count('conversation/checkpointPage'), 0);
});

it('T13 a lost native read-only MCP probe preserves connection-failure classification and does not replay',async()=>{
 const {client,transport}=await initialized();
 assert.equal(METHOD_POLICY['mcp/probe'].effect,'read');
 const request=client.call('mcp/probe',{target:{kind:'user'},id:'exa',expected_revision:'mcp-1'},'mcp_probe');
 const failure=request.catch((error:unknown)=>error);
 await transport.log.awaitMethod('mcp/probe');transport.fail('socket_error');
 const error=await failure;assert.ok(error instanceof TransportClosedError);assert.ok(!isUncertainOutcome(error));assert.equal(transport.log.count('mcp/probe'),1);
});

it("derives parameter-sensitive Goal reads and lanes from the one shared method policy", () => {
  const goal = (action: "show" | "mutate") => ({ method: "goal/control", params: { target: target(), control: { action } } }) as unknown as import("../../protocol/app-server/v57.ts").Request1;
  assert.equal(requestEffect(goal("show")), "read");
  assert.equal(requestEffect(goal("mutate")), "side_effecting");
  for (const [method, policy] of Object.entries(METHOD_POLICY)) {
    assert.ok(["read", "side_effecting", "connection_local"].includes(policy.effect), method);
    assert.ok(policy.lane === undefined || policy.lane === "observation" || policy.lane === "control", method);
  }
  assert.equal(METHOD_POLICY["delivery/cancel"].lane, "control");
  assert.equal(METHOD_POLICY["agent/observe"].lane, "observation");
});

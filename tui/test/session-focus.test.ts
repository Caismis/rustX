/**
 * Switching the visible Session is focus, and only focus.
 *
 * ```text
 * Session A starts long-running work
 *         |
 * TUI changes focus to Session B          <- no process replacement
 *         |                                  no cancellation
 * Session B is usable                       no unload, no detach of A
 *         |
 * Session A keeps executing in the same App Server process
 *         |
 * TUI switches back to A
 *         |
 * an authoritative snapshot repairs the visible projection
 * ```
 *
 * This is the invariant #290 exists for, so it is proved deterministically: the
 * assertions are over the exact request log, not over timing. Nothing sleeps.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  APP_SERVER_PROTOCOL_VERSION,
  AppServerClient,
} from "../src/app-server/client.ts";
import type { AppServerChild, ChildExit } from "../src/app-server/child-process.ts";
import { AppServerHost } from "../src/app-server/host.ts";
import type { AppServerSession } from "../src/app-server/session.ts";
import type { AttachmentTarget } from "../src/protocol/app-server.ts";
import { FakeTransport, paramsOf, tick } from "./support/app-server-peer.ts";
import { attemptView, runtimeCursor, snapshot } from "./support/fixtures.ts";

const CAPABILITIES = {
  multi_session: true,
  single_writable_controller: true,
  headless_interactions: true,
  experimental_methods: [],
};

function targetFor(session: string, index: number): AttachmentTarget {
  return {
    session_id: session,
    conversation_id: `conv-${session}`,
    runtime_incarnation: String(index),
    attachment_id: `attach-${index}`,
  };
}

/** A connected external host over a scripted transport. */
async function host(): Promise<{ transport: FakeTransport; host: AppServerHost }> {
  const transport = new FakeTransport();
  const pending = AppServerClient.initialize({ transport });
  const [request] = await transport.log.awaitMethod("initialize");
  transport.respond(request!.id, {
    type: "initialized",
    protocol_version: APP_SERVER_PROTOCOL_VERSION,
    capabilities: CAPABILITIES,
  });
  const client = await pending;
  return { transport, host: new AppServerHost({ client, ownership: "external" }) };
}

/** Drives one `host.attach` to completion against the scripted transport. */
async function focus(
  connected: { transport: FakeTransport; host: AppServerHost },
  sessionId: string,
  index: number,
  options: { running?: boolean; cursor?: number } = {},
): Promise<AppServerSession> {
  const { transport } = connected;
  const attaches = transport.log.count("session/attach") + 1;
  const pending = connected.host.attach(sessionId);
  const attach = (await transport.log.awaitMethod("session/attach", attaches)).at(-1)!;
  transport.respond(attach.id, {
    type: "attached",
    target: targetFor(sessionId, index),
    snapshot: snapshot(
      options.running === true
        ? { attempt: attemptView({ phase: { type: "running" } }) }
        : {},
    ),
    cursor: runtimeCursor(options.cursor ?? 1),
  });
  return pending;
}

describe("multi-Session focus on one connection", () => {
  it("keeps Session A attached and running while Session B is visible", async () => {
    const connected = await host();
    const a = await focus(connected, "session-a", 1, { running: true });
    const b = await focus(connected, "session-b", 2);

    // Both attachments exist at once. One App Server, two live Sessions.
    assert.equal(connected.host.attached.length, 2);
    assert.equal(connected.host.attachment("session-a"), a);
    assert.equal(connected.host.attachment("session-b"), b);

    // Session A is still the runtime's business, and it is still running.
    assert.equal(a.state.attempt?.phase.type, "running");
    assert.equal(a.released, false);
    assert.equal(a.serverClosed, false);
  });

  it("sends no cancellation, unload, detach or shutdown when focus changes", async () => {
    const connected = await host();
    await focus(connected, "session-a", 1, { running: true });
    await focus(connected, "session-b", 2);

    const methods = connected.transport.log.requests.map((m) => m.method);
    for (const forbidden of [
      "turn/cancel",
      "session/unload",
      "session/detach",
      "interaction/cancel",
      "interaction/respond",
    ] as const) {
      assert.ok(
        !methods.includes(forbidden),
        `focus change must never send ${forbidden}`,
      );
    }
    // What it *does* send is exactly two attaches — one cut each, snapshot
    // and subscription together.
    assert.equal(connected.transport.log.count("session/attach"), 2);
    assert.equal(connected.transport.log.count("session/subscribe"), 0);
  });

  it("keeps Session A's events flowing while Session B holds the screen", async () => {
    const connected = await host();
    const a = await focus(connected, "session-a", 1, { running: true });
    const b = await focus(connected, "session-b", 2);
    const stopA = connected.host.client.onNotification((m) => a.applyNotification(m));
    const stopB = connected.host.client.onNotification((m) => b.applyNotification(m));

    // A publishes while it is off screen. The projection of the Session that
    // is not visible is still maintained, because it is still attached.
    connected.transport.emit(a.target, runtimeCursor(2), {
      type: "runtime_shutdown",
    });
    await tick();

    assert.equal(a.state.runtimeShutdown, true);
    assert.equal(b.state.runtimeShutdown, false);
    stopA();
    stopB();
  });

  it("reuses the existing attachment when focus returns to a Session", async () => {
    const connected = await host();
    const a = await focus(connected, "session-a", 1, { running: true });
    await focus(connected, "session-b", 2);

    const returned = await connected.host.attach("session-a");

    assert.equal(returned, a, "the same attachment, not a second one");
    assert.equal(
      connected.transport.log.count("session/attach"),
      2,
      "returning to a Session does not re-attach it",
    );
  });

  it("repairs the returning projection from authoritative state", async () => {
    const connected = await host();
    const a = await focus(connected, "session-a", 1, { running: true });
    await focus(connected, "session-b", 2);

    // While A was off screen the server moved on. The client does not
    // reconstruct that from what it last believed; it asks.
    const repair = a.resync();
    const snapshotRequest = (
      await connected.transport.log.awaitMethod("session/snapshot")
    ).at(-1)!;
    connected.transport.respond(snapshotRequest.id, {
      type: "snapshot",
      snapshot: snapshot({
        attempt: attemptView({
          phase: {
            type: "settled",
            outcome: { type: "completed", finish_reason: { type: "stop" } },
          },
        }),
      }),
      cursor: runtimeCursor(77),
    });
    const resubscribe = (
      await connected.transport.log.awaitMethod("session/subscribe")
    ).at(-1)!;
    connected.transport.respond(resubscribe.id, {
      type: "subscribed",
      after_cursor: runtimeCursor(77),
    });
    await repair;

    assert.equal(a.state.cursor, "77");
    assert.equal(a.state.attempt?.phase.type, "settled");
    assert.equal(a.resyncCount, 1);
  });

  it("drops the route when the server retires an attachment, without inventing an outcome", async () => {
    const connected = await host();
    const a = await focus(connected, "session-a", 1, { running: true });
    const stop = connected.host.client.onNotification((m) => a.applyNotification(m));

    connected.transport.notify({
      jsonrpc: "2.0",
      method: "session/closed",
      params: { target: a.target },
    });
    await tick();

    assert.equal(connected.host.attachment("session-a"), undefined);
    // The runtime was unloaded. The attempt it was running is not thereby
    // settled, and this client never says it was.
    assert.equal(a.state.attempt?.phase.type, "running");
    stop();
  });
});

describe("explicit release is a different operation from focus", () => {
  it("detach sends exactly one detach and forgets the route", async () => {
    const connected = await host();
    await focus(connected, "session-a", 1);
    const pending = connected.host.detach("session-a");
    const detach = (await connected.transport.log.awaitMethod("session/detach")).at(-1)!;
    connected.transport.respond(detach.id, { type: "detached" });
    await pending;

    assert.equal(connected.transport.log.count("session/detach"), 1);
    assert.equal(connected.host.attachment("session-a"), undefined);
    const methods = connected.transport.log.requests.map((m) => m.method);
    assert.ok(!methods.includes("turn/cancel"));
    assert.ok(!methods.includes("session/unload"));
  });

  it("unload is an explicit product action, never a side effect of navigation", async () => {
    const connected = await host();
    const a = await focus(connected, "session-a", 1);
    const pending = a.unload();
    const unload = (await connected.transport.log.awaitMethod("session/unload")).at(-1)!;
    // The request carries the full four-domain target, so the server can
    // refuse it for a superseded incarnation.
    assert.deepEqual(paramsOf(unload, "session/unload").target, a.target);
    connected.transport.respond(unload.id, { type: "unloaded" });
    await pending;
  });
});

describe("process ownership", () => {
  it("an external host disconnects on exit and stops nothing", async () => {
    const connected = await host();
    await focus(connected, "session-a", 1, { running: true });

    const exit = await connected.host.shutdown();

    assert.equal(exit, undefined, "there is no child process to wait for");
    assert.ok(connected.host.client.closed, "only this client's socket ended");
    // Exiting a remote TUI must not ask the server to stop anything.
    const methods = connected.transport.log.requests.map((m) => m.method);
    assert.ok(!methods.includes("session/unload"));
    assert.ok(!methods.includes("turn/cancel"));
    assert.ok(!methods.includes("session/detach"));
  });

  it("describes ownership from who spawned the process, not from the endpoint", async () => {
    const connected = await host();
    assert.equal(connected.host.ownership, "external");
    assert.match(connected.host.describe(), /external App Server/);
    // Ownership is structural: an external host has no child to report on.
    assert.equal(connected.host.childExit, undefined);
    assert.deepEqual(connected.host.stderrTail(), {
      text: "",
      truncatedBytes: 0,
    });
  });

  it("refuses a composition that claims ownership without a process", () => {
    assert.throws(
      () =>
        new AppServerHost({
          client: undefined as never,
          ownership: "owned_child",
        }),
      /owned App Server host has a child process/,
    );
  });
});


it("concurrent owned-host shutdown callers await the same child settlement", async () => {
  const connected = await host();
  const calls: string[] = [];
  let reap!: (exit: ChildExit) => void;
  const reaped = new Promise<ChildExit>(resolve => { reap = resolve; });
  const child = {
    requestShutdown: () => calls.push("signal"),
    closeStdin: () => calls.push("eof"),
    wait: () => reaped,
  } as unknown as AppServerChild;
  const owned = new AppServerHost({ client: connected.host.client, ownership: "owned_child", child });
  const first = owned.shutdown();
  assert.equal(owned.shutdown(), first);
  await Promise.resolve();
  assert.deepEqual(calls, ["signal", "eof"]);
  assert.ok(!connected.host.client.closed);
  reap({ code: 0, signal: null });
  assert.deepEqual(await first, { code: 0, signal: null });
  assert.ok(connected.host.client.closed);
  assert.equal(owned.shutdown(), first);
});

/**
 * One connected App Server client, host and attached Session, over a scripted
 * transport.
 *
 * The suites that exercise commands, presentation and focus all need the same
 * thing: a live client whose every request a test can inspect and answer by
 * hand. Building it once keeps those suites arguing about semantics instead of
 * about handshake bookkeeping.
 *
 * Nothing here sleeps. Every wait is a data barrier on the request log.
 */

import {
  APP_SERVER_PROTOCOL_VERSION,
  AppServerClient,
} from "../../src/app-server/client.ts";
import { AppServerHost } from "../../src/app-server/host.ts";
import type { AppServerSession } from "../../src/app-server/session.ts";
import { CommandDispatcher } from "../../src/commands/dispatcher.ts";
import type { DebugDiagnostics } from "../../src/commands/dispatcher.ts";
import type {
  AttachmentTarget,
  RuntimeClientSnapshot,
  SessionSettings,
} from "../../src/protocol/app-server.ts";
import { FakeTransport } from "./app-server-peer.ts";
import { runtimeCursor, snapshot as defaultSnapshot } from "./fixtures.ts";

export const SERVER_CAPABILITIES = {
  multi_session: true,
  single_writable_controller: true,
  headless_interactions: true,
  experimental_methods: [],
};

export const SESSION_SETTINGS: SessionSettings = { cwd: "/work/project" };

export const NO_DIAGNOSTICS: () => DebugDiagnostics = () => ({
  connection: "scripted",
  ownership: "external",
  connectionState: "connected",
  attachedSessions: 1,
  childStatus: "not applicable (external App Server)",
  stderrTail: "",
  stderrTruncatedBytes: 0,
  pendingRequests: 0,
  resyncCount: 0,
});

export interface Harness {
  transport: FakeTransport;
  client: AppServerClient;
  host: AppServerHost;
  session: AppServerSession;
  dispatcher: CommandDispatcher;
  target: AttachmentTarget;
  /** Routes notifications into the attached Session, as the host does. */
  stopRouting: () => void;
}

/** Connects, attaches one Session, and binds a dispatcher to it. */
export async function harness(
  initial: RuntimeClientSnapshot = defaultSnapshot(),
  sessionId = "session-1",
): Promise<Harness> {
  const transport = new FakeTransport();
  const connecting = AppServerClient.initialize({ transport });
  const initialize = (await transport.log.awaitMethod("initialize")).at(-1)!;
  transport.respond(initialize.id, {
    type: "initialized",
    protocol_version: APP_SERVER_PROTOCOL_VERSION,
    capabilities: SERVER_CAPABILITIES,
  });
  const client = await connecting;
  const host = new AppServerHost({ client, ownership: "external" });

  const target: AttachmentTarget = {
    session_id: sessionId,
    conversation_id: initial.conversation_id,
    runtime_incarnation: "1",
    attachment_id: "att-1",
  };
  const attaching = host.attach(sessionId);
  const attach = (await transport.log.awaitMethod("session/attach")).at(-1)!;
  // Attach is one cut: snapshot, cursor and subscription together. There is no
  // separate subscribe to answer here.
  transport.respond(attach.id, {
    type: "attached",
    target,
    snapshot: initial,
    cursor: runtimeCursor(0),
  });
  const session = await attaching;

  const stopRouting = client.onNotification((message) =>
    session.applyNotification(message),
  );
  const dispatcher = new CommandDispatcher({
    host,
    session,
    sessionSettings: SESSION_SETTINGS,
    diagnostics: NO_DIAGNOSTICS,
  });
  return { transport, client, host, session, dispatcher, target, stopRouting };
}

/** The next request for one method, awaited by count rather than by time. */
export async function nextRequest(
  harnessed: Pick<Harness, "transport">,
  method: string,
  seen = harnessed.transport.transportCount(method),
) {
  return (await harnessed.transport.log.awaitMethod(method, seen + 1)).at(-1)!;
}

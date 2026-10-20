import { fixtures as uploadFixtures } from "../../../protocol/app-server/fixtures.ts";
const uploadPolicy = uploadFixtures.find(row => "result" in row && row.result?.type === "initialized")!;
const nativeUploadPolicy = "result" in uploadPolicy && uploadPolicy.result?.type === "initialized" ? uploadPolicy.result.capabilities.upload_policy : (() => { throw new Error("missing policy") })();
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
import type { DebugDiagnostics } from "../../src/commands/presentation.ts";
import type {
  AttachmentTarget,
  RuntimeClientSnapshot,
  SessionSettings,
} from "../../src/protocol/app-server.ts";
import { FakeTransport } from "./app-server-peer.ts";
import { checkpoint, runtimeCursor, snapshot as defaultSnapshot } from "./fixtures.ts";

export const SERVER_CAPABILITIES = {
  upload_policy: nativeUploadPolicy, multi_session: true,
  single_writable_controller: true,
  headless_interactions: true, delivery_access: false,
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
  sessionId = "ses_84097828-fc31-78c8-9292-10df48901a85",
  previousHost?: AppServerHost,
): Promise<Harness> {
  const transport = new FakeTransport();
  const connecting = AppServerClient.initialize({ transport });
  const initialize = (await transport.log.awaitMethod("initialize")).at(-1)!;
  transport.respond(initialize.id, {
    type: "initialized", authority_id: 'fixture-app-server-authority',
    protocol_version: APP_SERVER_PROTOCOL_VERSION,
    capabilities: SERVER_CAPABILITIES,
  });
  const client = await connecting;
  client.uploadCarrier = async () => {};
  const host = new AppServerHost({ client, ownership: "external" });
  if (previousHost) host.inheritModelSelectionUncertainty(previousHost);

  const target: AttachmentTarget = {
    session_id: sessionId,
    conversation_id: initial.conversation_id,
    runtime_incarnation: "1",
    attachment_id: "att-1",
  };
  const attaching = host.attach(sessionId);
  const attach = (await transport.log.awaitMethod("session/attach")).at(-1)!;
  // Identity admission precedes the independently bounded cut and registration.
  transport.respond(attach.id, {type:'attached', target, configuration_revision:'0', attachment_kind:'controller', observation_source:'live'});
  const read=(await transport.log.awaitMethod('session/checkpoint')).at(-1)!;
  transport.respond(read.id,{type:'checkpoint',checkpoint:checkpoint(initial,runtimeCursor(0))});
  const subscribe=(await transport.log.awaitMethod('session/subscribe')).at(-1)!;
  transport.respond(subscribe.id,{type:'subscribed',target,registration_id:'harness-registration-1',after_cursor:runtimeCursor(0)});
  const configuration=(await transport.log.awaitMethod('session/configuration')).at(-1)!;
  transport.respond(configuration.id,{type:'session_configuration',projection:(null)});
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
  seen = 0,
) {
  for (;;) {
    const pending = harnessed.transport.log.matching(method).slice(seen)
      .find(request => !harnessed.transport.answered.has(request.id));
    if (pending) return pending;
    await harnessed.transport.log.awaitMethod(method, harnessed.transport.transportCount(method) + 1);
  }
}

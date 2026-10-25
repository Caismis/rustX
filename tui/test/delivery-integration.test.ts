/**
 * Committed `present` deliveries through a real App Server child and socket.
 *
 * ```text
 * local   AppServerHost.spawnLocal -> owned stdio child (--stdio-delivery-access)
 *         read/save original bytes, locate + identity-verified local open
 * remote  AppServerHost.connectRemote -> WebSocket
 *         without the delivery credential: metadata only, reads fail closed
 *         with it: client-local save; never a local open of a server path
 * ```
 *
 * Every action after the turn issues zero provider requests: the emulator
 * scenario has exactly the two model steps of the turn itself.
 */

import assert from "node:assert/strict";
import { spawn, spawnSync, type ChildProcessWithoutNullStreams } from "node:child_process";
import { chmodSync, existsSync, mkdirSync, readFileSync, readdirSync, realpathSync, rmSync, statSync, writeFileSync } from "node:fs";
import { createInterface } from "node:readline";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { after, before, describe, it } from "node:test";

import { contentRefKey } from "../../protocol/app-server/content-identity.ts";
import { parseObservedValue } from "../../protocol/app-server/domain-observation.ts";
import type { ToolExecutionResult } from "../src/protocol/app-server.ts";
import type { CorrelatedTool } from "../src/presentation/tools.ts";
import { CommandDispatcher } from "../src/commands/dispatcher.ts";
import { AppServerHost } from "../src/app-server/host.ts";
import { AppServerRequestError, isDeliveryCancelled } from "../src/app-server/client.ts";
import type { AppServerSession } from "../src/app-server/session.ts";
import { TransportClosedError } from "../src/app-server/transport.ts";
import {
  DeliveryActionError,
  deliveryDestination,
  openDelivery,
  saveDelivery,
} from "../src/app-server/delivery-files.ts";
import { correlateTools } from "../src/presentation/tools.ts";
import { renderToolCard } from "../src/ui/components/tool-card.ts";
import { plainText } from "../src/ui/theme.ts";
import { DEFAULT_PREVIEW_CHARS, DEFAULT_PREVIEW_LINES } from "../src/ui/preferences.ts";
import { ProviderEmulator } from "./support/provider-emulator.ts";
import { TempFixture } from "./support/temp-fixture.ts";
import { until } from "./support/app-server-peer.ts";

const BINARY =
  process.env.RUSTX_BINARY ??
  fileURLToPath(new URL("../../target/debug/rustx", import.meta.url));
const SKIP = existsSync(BINARY)
  ? (await ProviderEmulator.available())
    ? undefined
    : "uv is not installed; the shared provider emulator cannot run"
  : `the rustx binary is not built at ${BINARY}; run \`cargo build --bins\``;
if (SKIP !== undefined && process.env.RUSTX_REQUIRE_PROVIDER_EMULATOR) throw new Error(SKIP);

const CREDENTIAL_VARIABLE = "RUSTX_TUI_DELIVERY_KEY";
const TRANSPORT_TOKEN = "tui-delivery-transport-token-000000000000000";
const DELIVERY_TOKEN = "tui-delivery-access-token-00000000000000000000";
const REPORT = "报告 final file.md";
const REPORT_BYTES = Buffer.from("# 报告\r\n\r\nOriginal CRLF bytes\r\n\u0000tail", "utf8");
const DATA_BYTES = Buffer.from("a,b\r\n1,2\r\n");

class Fixture {
  retainFailure = false;
  readonly temp: TempFixture;
  readonly env: NodeJS.ProcessEnv;

  constructor(providerUrl: string) {
    this.temp = TempFixture.create("rustx-tui-delivery-");
    const home = this.temp.path("home");
    mkdirSync(join(home, "rustx"), { recursive: true });
    this.env = { ...process.env, HOME: home, [CREDENTIAL_VARIABLE]: "delivery-secret" };
    delete this.env.XDG_CONFIG_HOME;
    delete this.env.XDG_STATE_HOME;
    const initialized = spawnSync(BINARY, [
      "init", "--template", "openai-chat", "--provider", "fixture", "--model-id", "integration-model",
      "--endpoint", providerUrl, "--credential-env", CREDENTIAL_VARIABLE, "--context-window", "128000",
      "--max-output", "4096", "--tool-calls", "true", "--reasoning", "false",
      "--compat", 'chat_reasoning_replay = "omit"',
    ], { env: this.env, encoding: "utf8" });
    assert.equal(initialized.status, 0, initialized.stderr);
    writeFileSync(this.temp.path("token"), `${TRANSPORT_TOKEN}\n`);
    writeFileSync(this.temp.path("delivery"), `${DELIVERY_TOKEN}\n`);
    chmodSync(this.temp.path("delivery"), 0o600);
  }

  workspace(name: string): string {
    const workspace = this.temp.path(name);
    mkdirSync(workspace, { recursive: true });
    writeFileSync(join(workspace, "rustx.toml"), '[agent.tools]\nbuiltin = ["read", "write", "present"]\n');
    writeFileSync(join(workspace, REPORT), REPORT_BYTES);
    writeFileSync(join(workspace, "data set.csv"), DATA_BYTES);
    return workspace;
  }

  cleanup(): void {
    if (this.retainFailure) {
      console.error(`Retained owned delivery failure fixture: ${this.temp.root}`);
    } else this.temp.cleanup();
  }

  get runtimeRoot(): string {
    return this.temp.path("runtime");
  }
}

function cutDiagnostic(cut: import('../../protocol/app-server/v57.ts').CheckpointReadCut | undefined) {
  if (!cut) return undefined;
  const source = cut.source_cut;
  return { conversation_id: cut.conversation_id, activation_id: cut.activation_id,
    revision: cut.revision, through: cut.through,
    source: source ? { conversation_id: source.read.conversation_id, journal: source.read.journal,
      ledger: source.ledger_through, transcript: source.transcript_total, surface: source.surface_revision,
      pending: source.read.pending_through } : null };
}

function eventDiagnostic(event: import('../../protocol/app-server/v57.ts').RuntimeClientEvent) {
  if (event.type === 'checkpoint_advanced') return { type: event.type, cursor: event.checkpoint.cursor,
    conversation_id: event.checkpoint.snapshot.conversation_id, cut: cutDiagnostic(event.checkpoint.read_cut),
    attempt: event.checkpoint.snapshot.attempt?.phase.type };
  if (event.type === 'projection_transition') return { type: event.type, cut: cutDiagnostic(event.metadata.read_cut),
    facts: event.events.slice(0, 32).map(fact => fact.type) };
  return { type: event.type };
}

async function deliver(host: AppServerHost, cwd: string, fixture: Fixture, provider: ProviderEmulator): Promise<AppServerSession> {
  const created = await host.createSession({ cwd });
  await host.renameSession(created.session.id, "TUI delivery fixture");
  const session = await host.attach(created.session.id, created.session.active_node);
  const records: object[] = [];
  const stop = host.client.onNotification(notification => {
    if (notification.method !== 'session/event' && notification.method !== 'session/resyncRequired') return;
    const target = notification.params.target;
    if (target.session_id !== session.target.session_id || target.conversation_id !== session.target.conversation_id
      || target.runtime_incarnation !== session.target.runtime_incarnation || target.attachment_id !== session.target.attachment_id) return;
    if (records.length === 32) records.shift();
    records.push(notification.method === 'session/event'
      ? { cursor: notification.params.cursor, ...eventDiagnostic(notification.params.event) }
      : { type: notification.method, after: notification.params.after_cursor, reason: notification.params.reason });
  });
  try {
    await session.submitInbound([{ type: "text", text: "tui present: deliver the report" }]);
    await until(() => session.state.attempt?.phase.type === "settled", "the present turn settles");
    return session;
  } catch (error) {
    // Failure-only evidence; do not repair, restamp a cut, or change execution.
    fixture.retainFailure = true;
    let budget = 16 * 1024;
    const print = (label: string, value: unknown) => {
      const text = `${label}: ${JSON.stringify(value)}`;
      const bytes = Buffer.byteLength(text) + 1;
      if (bytes <= budget) { budget -= bytes; console.error(text); }
    };
    print('Delivery settlement owner', { target: session.target, fixture: fixture.temp.root,
      database: join(fixture.runtimeRoot, 'sessions', session.target.session_id, 'conversations', session.target.conversation_id, 'conversation.sqlite'),
      applied: session.state.cursor, observation: session.observationPhase, error: session.observationError,
      attempt: { id: session.state.attempt?.attemptId, phase: session.state.attempt?.phase.type },
      cut: cutDiagnostic(session.checkpoint?.read_cut), childExit: host.childExit ? { code: host.childExit.code, signal: host.childExit.signal } : null });
    const requests = await provider.requests().catch(() => undefined);
    print('Delivery provider metadata', { count: requests?.length,
      requests: requests?.slice(0, 16).map(request => ({ index: typeof request.index === 'number' ? request.index : null,
        model: typeof request.model === 'string' ? request.model.slice(0, 256) : null })) });
    // At most1,024 UTF16 units is at most4KiB of UTF8 diagnostic text.
    print('Delivery child stderr', host.stderrTail().text.slice(-1024));
    for (const record of records) print('Delivery original native record', record);
    throw error;
  } finally { stop(); }
}

/** Exercise the actual /content reader for a selected deferred Tool result. */
async function loadCommittedMetadata(host: AppServerHost, session: AppServerSession, cwd: string, tool: CorrelatedTool): Promise<CorrelatedTool> {
  if (!tool.committedResultDeferred) {
    assert.ok(tool.committedResult, "the finite native C result is already loaded");
    return tool;
  }
  const reference = tool.resultMetadataRef;
  assert.ok(reference, "the native C receipt advertises its original metadata source");
  assert.equal(reference.representation, "canonical_json_v1");
  assert.ok(reference.readable_end <= 65_536, "this selected two-file metadata fits one bounded range");
  const identity = contentRefKey(reference);
  const index = session.deferredContent.findIndex(binding => contentRefKey(binding.content_ref) === identity);
  assert.ok(index >= 0, "the actual product content picker exposes this original result");
  const dispatcher = new CommandDispatcher({host,session,sessionSettings:{cwd},diagnostics:()=>{throw new Error("not requested");}});
  const outcome = await dispatcher.submit(`/content ${index + 1} 0`);
  assert.equal(outcome.kind, "inspect");
  if (outcome.kind !== "inspect") throw new Error("the selected content command must inspect original bytes");
  const match = outcome.body.match(/^0–(\d+) \/ (\d+) bytes\n([^\n]+)\nEnd of content\.$/);
  assert.ok(match, "the product viewer returned one complete original canonical range");
  assert.equal(Number(match[1]), reference.readable_end);
  assert.equal(Number(match[2]), reference.readable_end);
  const metadata = parseObservedValue(match[3]!) as {role:string;tool_call_id:string;tool_id:string;result:Omit<ToolExecutionResult,"duration_ms"> & {duration_ms:number}};
  assert.equal(metadata.role, "tool");
  assert.equal(metadata.tool_call_id, tool.callId);
  assert.equal(metadata.tool_id, tool.toolId);
  assert.ok(Number.isSafeInteger(metadata.result.duration_ms) && metadata.result.duration_ms >= 0);
  // This schema-owned native clock field is exact in the canonical bytes;
  // only its typed presentation uses decimal text. Opaque fields stay intact.
  return {...tool,committedResult:{...metadata.result,duration_ms:String(metadata.result.duration_ms)},committedResultDeferred:false};
}

/** The same real carrier exposes a finite Tool head and original frozen Definition.
 * Neither explicit inspection nor a selected field read may ask the model again. */
async function inspectPresentDefinition(host:AppServerHost,session:AppServerSession,cwd:string,provider:ProviderEmulator) {
  const before=(await provider.requests()).length;
  const {page}=await host.client.call('session/trace',{target:session.target,limit:32,records:[]},'trace');
  const record=page.records.find(record=>record.kind==='tool'&&record.tool?.call_id==='call-tui-present');
  assert.ok(record,'the actual native present execution has its original Trace record');
  const dispatcher=new CommandDispatcher({host,session,sessionSettings:{cwd},diagnostics:()=>{throw new Error('not requested');}});
  const head=await dispatcher.submit(`/observation trace ${record.id}`);
  assert.equal(head.kind,'inspect');if(head.kind==='inspect')assert.match(head.body,/Tool definition: available/);
  const fields=session.deferredTraceFields,index=fields.findIndex(field=>field.record===record.id&&field.source.field==='definition');
  assert.ok(index>=0,'the terminal content picker exposes the native Definition selector');
  const selected=fields[index]!;
  assert.equal(selected.source.type,'value');
  if(selected.source.type!=='value')throw new Error('Definition must use its advertised original indexed field.');
  assert.deepEqual(selected.source.path,[]);
  assert.ok(BigInt(selected.source.encoded_bytes)<=65_536n,'this native present Definition fits one original byte window');
  const ordinal=session.deferredContent.length+session.deferredJournalFields.length+session.deferredAttemptIdentities.length+index+1;
  const result=await dispatcher.submit(`/content ${ordinal} 0`);
  assert.equal(result.kind,'inspect');if(result.kind!=='inspect')throw new Error('Original Definition read must return selected bytes.');
  const matched=/^0–(\d+) \/ (\d+) bytes\n([\s\S]*)\nEnd of original field\.$/.exec(result.body);
  assert.ok(matched,'the original selected Definition completed without whole-detail hydration');
  assert.equal(matched[1],selected.source.encoded_bytes);assert.equal(matched[2],selected.source.encoded_bytes);
  assert.equal(Buffer.byteLength(matched[3]!),Number(selected.source.encoded_bytes));
  const original=parseObservedValue(matched[3]!);
  assert.ok(original&&typeof original==='object'&&'name' in original&&'tool_id' in original&&'input_schema' in original);
  assert.equal(original.name,'present');assert.equal(original.tool_id,'tool-present');
  const schema=original.input_schema;
  assert.ok(schema&&typeof schema==='object'&&'truncated' in schema&&'value' in schema);assert.equal(schema.truncated,false);
  const value=schema.value;assert.ok(value&&typeof value==='object'&&'properties' in value);
  const properties=value.properties;assert.ok(properties&&typeof properties==='object'&&'files' in properties);
  assert.equal((await provider.requests()).length,before,'head and Definition observation issue no provider requests');
}

function rejectedWith(reason: string) {
  return (error: unknown) => error instanceof AppServerRequestError
    && error.error.data?.kind === "session_file_read" && error.error.data.reason === reason;
}

describe("committed deliveries through an owned stdio child", { skip: SKIP }, () => {
  let provider: ProviderEmulator;
  let fixture: Fixture;
  before(async () => {
    provider = await ProviderEmulator.start("tui_present_delivery");
    fixture = new Fixture(provider.url("/v1"));
  });
  after(async () => {
    fixture?.cleanup();
    await provider?.finish();
  });

  it("lists, renders, saves byte-exactly and opens only a verified shared leaf", { timeout: 120_000 }, async () => {
    const host = await AppServerHost.spawnLocal({
      binary: BINARY,
      env: fixture.env,
      launch: { runtimeRoot: fixture.runtimeRoot },
    });
    try {
      const workspace = fixture.workspace("local");
      const session = await deliver(host, workspace, fixture, provider);
      await inspectPresentDefinition(host,session,workspace,provider);
      assert.equal(session.deliveryAccess, true, "the owned stdio child delegates delivery access");
      const requests = (await provider.requests()).length;

      // The card and /files consume the same committed typed facts.
      const correlation = correlateTools(session.state);
      const present = [...correlation.byCallId.values()].find((tool) => tool.toolId === "tool-present");
      assert.ok(present?.resultCommitted, `the presented card reads the committed canonical result: ${JSON.stringify({
        calls:[...correlation.byCallId.values()].map(tool=>({callId:tool.callId,toolId:tool.toolId,resultCommitted:tool.resultCommitted,metadata:tool.resultMetadataRef!=null})),
        foreground:session.state.attempt?.foreground.map(tool=>({call:tool.call_id,tool:tool.tool_id,metadata:tool.result_metadata_ref!=null,body:tool.committed_result!=null})),
        descriptors:session.state.messageDescriptors.map(message=>({role:message.role,tools:message.tool_calls.map(tool=>({call:tool.call_id,tool:tool.tool_id,metadata:tool.result_metadata_ref!=null,body:tool.committed_result!=null}))})),
        deferred:session.state.deferredTranscriptEntries.length,
      })}`);
      const loadedPresent = await loadCommittedMetadata(host, session, workspace, present);
      const card = plainText(renderToolCard(loadedPresent, {
        expanded: true,
        budget: { maxLines: DEFAULT_PREVIEW_LINES, maxChars: DEFAULT_PREVIEW_CHARS },
      }));
      assert.match(card, /Delivered 2 files/);
      assert.ok(card.indexOf(REPORT) < card.indexOf("data set.csv"), "canonical order");
      const page = await session.deliveryPage();
      assert.deepEqual(page.records.map((record) => [record.file!.name, record.index, record.file!.description ?? null]), [
        [REPORT, 0, "Final report"],
        ["data set.csv", 1, null],
      ]);
      const [report, data] = page.records;
      assert.ok(report && data);

      // Save: original bytes, Unicode/spaces name, client-local destination.
      const saved = await saveDelivery(
        () => session.readDelivery(report),
        deliveryDestination(`saved/${report.file!.name}`, fixture.temp.path(".")),
      ).catch((error: unknown) => error);
      assert.ok(saved instanceof Error, "a missing destination directory fails before writing");
      mkdirSync(fixture.temp.path("saved"));
      const destination = deliveryDestination(`saved/${report.file!.name}`, fixture.temp.path("."));
      assert.deepEqual(await saveDelivery(() => session.readDelivery(report), destination), { path: destination });
      assert.deepEqual(readFileSync(destination), REPORT_BYTES);
      assert.equal(statSync(destination).mode & 0o777, 0o600, "private to this user, whatever the umask");
      await assert.rejects(saveDelivery(() => session.readDelivery(data), destination), /already exists/);
      assert.deepEqual(readFileSync(destination), REPORT_BYTES, "existing data is never truncated");

      // Mutable source: reopening observes the current file.
      writeFileSync(join(workspace, "data set.csv"), "replaced\n");
      const replaced = fixture.temp.path("saved/data set.csv");
      await saveDelivery(() => session.readDelivery(data), replaced);
      assert.equal(readFileSync(replaced, "utf8"), "replaced\n");

      // Oversized sources remain declared, fail explicitly, and leave no file.
      writeFileSync(join(workspace, "data set.csv"), Buffer.alloc(512 * 1024 + 1, 0x78));
      const oversized = fixture.temp.path("saved/oversized.csv");
      await assert.rejects(saveDelivery(() => session.readDelivery(data), oversized), rejectedWith("too_large"));
      assert.equal(existsSync(oversized), false);
      rmSync(join(workspace, "data set.csv"));
      await assert.rejects(session.readDelivery(data), rejectedWith("missing"));

      // Cancellation after the read and before the write leaves nothing.
      const abort = new AbortController();
      const cancelled = fixture.temp.path("saved/cancelled.md");
      await assert.rejects(saveDelivery(async () => {
        const body = await session.readDelivery(report);
        abort.abort();
        return body;
      }, cancelled, abort.signal), { name: "AbortError" });
      assert.equal(existsSync(cancelled), false);
      assert.deepEqual(readdirSync(fixture.temp.path("saved")).sort(), ["data set.csv", REPORT].sort(), "no staging left behind");

      // Request-scoped cancellation over the real protocol: the native
      // request answers exactly once (bytes only if its publication won),
      // and the connection stays healthy for the next read.
      const owner = new AbortController();
      const racing = session.readDelivery(report, owner.signal).then(
        () => "published",
        (error: unknown) => isDeliveryCancelled(error) ? "cancelled" : error,
      );
      owner.abort();
      assert.ok(["published", "cancelled"].includes(await racing as string));
      assert.deepEqual(Buffer.from((await session.readDelivery(report)).data, "base64"), REPORT_BYTES);

      // Open: native location, verified against this client's own leaf.
      const launches: string[][] = [];
      const opened = await openDelivery(() => session.locateDelivery(report), {
        sharedHost: true,
        opener: "fake-opener",
        launch: async (command, path) => { launches.push([command, path]); return 0; },
      });
      assert.equal(opened.path, realpathSync(join(workspace, REPORT)));
      assert.deepEqual(launches, [["fake-opener", opened.path]]);
      await assert.rejects(openDelivery(() => session.locateDelivery(report), {
        sharedHost: true,
        opener: "fake-opener",
        launch: async () => 3,
      }), /did not accept the open request/);
      await assert.rejects(openDelivery(async () => ({ ...await session.locateDelivery(report), inode: "1" }), {
        sharedHost: true,
        opener: "fake-opener",
        launch: async () => assert.fail("a mismatched leaf is never launched"),
      }), DeliveryActionError);

      assert.equal((await provider.requests()).length, requests, "file actions issue no model requests");
    } finally {
      await host.shutdown();
    }
  });
});

class ExternalAppServer {
  readonly #child: ChildProcessWithoutNullStreams;
  readonly endpoint: string;
  private constructor(child: ChildProcessWithoutNullStreams, endpoint: string) {
    this.#child = child;
    this.endpoint = endpoint;
  }
  static async start(fixture: Fixture): Promise<ExternalAppServer> {
    const child = spawn(BINARY, [
      "app-server", "--runtime-root", fixture.runtimeRoot, "--listen", "ws://127.0.0.1:0",
      "--token-file", fixture.temp.path("token"),
      "--delivery-access-token-file", fixture.temp.path("delivery"),
    ], { env: fixture.env, stdio: ["pipe", "pipe", "pipe"] }) as ChildProcessWithoutNullStreams;
    const lines = createInterface({ input: child.stderr });
    const endpoint = await new Promise<string>((resolve, reject) => {
      child.once("exit", (code) => reject(new Error(`the App Server exited before binding (code ${code})`)));
      lines.on("line", (line) => {
        const bound = line.match(/rustx app-server listening (ws:\/\/\S+)/);
        if (bound !== null) resolve(bound[1]!);
      });
    });
    return new ExternalAppServer(child, endpoint);
  }
  async stop(): Promise<void> {
    if (this.#child.exitCode !== null) return;
    this.#child.kill("SIGTERM");
    await new Promise<void>((resolve) => this.#child.once("exit", () => resolve()));
  }
}

describe("committed deliveries through a remote WebSocket App Server", { skip: SKIP }, () => {
  let provider: ProviderEmulator;
  let fixture: Fixture;
  let server: ExternalAppServer;
  before(async () => {
    provider = await ProviderEmulator.start("tui_present_delivery");
    fixture = new Fixture(provider.url("/v1"));
    server = await ExternalAppServer.start(fixture);
  });
  after(async () => {
    await server?.stop();
    fixture?.cleanup();
    await provider?.finish();
  });

  it("shows metadata without the delivery credential and saves client-side with it", { timeout: 120_000 }, async () => {
    const ordinary = await AppServerHost.connectRemote({ endpoint: server.endpoint, token: TRANSPORT_TOKEN });
    let sessionId: string;
    try {
      const workspace=fixture.workspace("remote");
      const session = await deliver(ordinary, workspace, fixture, provider);
      await inspectPresentDefinition(ordinary,session,workspace,provider);
      sessionId = session.sessionId;
      assert.equal(session.deliveryAccess, false);
      const [report] = (await session.deliveryPage()).records;
      assert.equal(report?.file?.name, REPORT, "metadata is visible to an ordinary client");
      await assert.rejects(session.readDelivery(report), rejectedWith("unauthorized"));
      await assert.rejects(session.locateDelivery(report), rejectedWith("unauthorized"));
      await session.detach();
    } finally {
      await ordinary.shutdown();
    }
    await assert.rejects(AppServerHost.connectRemote({
      endpoint: server.endpoint,
      token: TRANSPORT_TOKEN,
      deliveryAccessToken: "wrong-delivery-token-00000000000000000000000",
    }), (error: unknown) => error instanceof TransportClosedError && error.reason === "handshake_failed");
    const requests = (await provider.requests()).length;
    // A reconnecting authorized client resumes the same committed history.
    const trusted = await AppServerHost.connectRemote({
      endpoint: server.endpoint,
      token: TRANSPORT_TOKEN,
      deliveryAccessToken: DELIVERY_TOKEN,
    });
    try {
      const session = await trusted.attach(sessionId!);
      assert.equal(session.deliveryAccess, true);
      const [report] = (await session.deliveryPage()).records;
      assert.ok(report);
      const destination = fixture.temp.path("remote-copy.md");
      assert.deepEqual(await saveDelivery(() => session.readDelivery(report), destination), { path: destination });
      assert.deepEqual(readFileSync(destination), REPORT_BYTES);
      assert.equal(statSync(destination).mode & 0o777, 0o600, "private to this user, whatever the umask");
      assert.deepEqual(readdirSync(fixture.temp.path(".")).filter((name) => name.startsWith(".rustx-save-")), [], "no staging left behind");
      // A remote server path is never interpreted as a local one.
      await assert.rejects(openDelivery(() => assert.fail("no location is requested remotely"), {
        sharedHost: trusted.ownership === "owned_child",
        opener: "fake-opener",
        launch: async () => assert.fail("never launched"),
      }), /not on this machine/);
      assert.equal((await provider.requests()).length, requests, "file actions issue no model requests");
    } finally {
      await trusted.shutdown();
    }
  });
});

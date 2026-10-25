import {SubagentTranscript} from '../src/app-server/subagent-transcript.ts';
import { materializeSource } from '../../protocol/app-server/source-observation.ts';
/**
 * The real boundary: a real `rustx app-server`, real pipes, a real socket.
 *
 * ```text
 * local self-hosted                        existing / remote
 *   AppServerHost.spawnLocal                 AppServerHost.connectRemote
 *   -> AppServerChild (real OS process)      -> WebSocketTransport (real socket)
 *   -> StdioTransport (real JSONL)           -> the #36 admission contract
 *          \                                        /
 *           \____ AppServerClient (one protocol) __/
 * ```
 *
 * A scripted peer alone would not prove this client speaks the bytes rustX
 * actually writes, or that the process and socket boundaries behave. The model
 * provider is the shared external emulator (Issue #47), so the server exercises
 * its own adapter, credential resolution and streaming path with no network and
 * no credential in CI — and the TUI owns no provider protocol of its own.
 *
 * Readiness is protocol synchronization throughout: the client writes a request
 * and awaits its correlated response, or awaits a provider gate. Nothing sleeps,
 * and no ordering is established by a timer.
 *
 * The suite skips itself with a clear reason when the binary has not been built
 * or the provider emulator's toolchain is missing, so a partial checkout still
 * runs the rest of the tests.
 */

import assert from "node:assert/strict";
import { spawn, spawnSync, type ChildProcessWithoutNullStreams } from "node:child_process";
import { existsSync, mkdirSync, writeFileSync, renameSync, readFileSync, realpathSync } from "node:fs";
import { createInterface } from "node:readline";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { after, before, describe, it } from "node:test";

import { CommandDispatcher } from "../src/commands/dispatcher.ts";
import { renderModel } from "../src/commands/presentation.ts";
import { parseArguments } from "../src/cli.ts";
import { prepareStartup } from "../src/startup.ts";

import { AppServerHost } from "../src/app-server/host.ts";
import { AppServerClient, UncertainOutcomeError } from "../src/app-server/client.ts";
import { AppServerChild } from "../src/app-server/child-process.ts";
import { StdioTransport } from "../src/app-server/stdio-transport.ts";
import { WebSocketTransport } from "../src/app-server/websocket-transport.ts";
import { TransportClosedError } from "../src/app-server/transport.ts";
import type { AppServerSession } from "../src/app-server/session.ts";
import type { SessionSettings, SourceTarget } from "../src/protocol/app-server.ts";
import { SourceRepairStatus } from "../src/ui/components/source-repair-status.ts";
import { ProviderEmulator } from "./support/provider-emulator.ts";
import { TempFixture } from "./support/temp-fixture.ts";
import { until } from "./support/app-server-peer.ts";
import { mcpStartupGate, requireMcpStartupSupervisor, startupToolSchema, startupWait } from './support/mcp-startup-gate.ts';

/** The cargo target directory, overridable for a non-default layout. */
const BINARY =
  process.env.RUSTX_BINARY ??
  fileURLToPath(new URL("../../target/debug/rustx", import.meta.url));

const SKIP = existsSync(BINARY)
  ? (await ProviderEmulator.available())
    ? undefined
    : "uv is not installed; the shared provider emulator cannot run"
  : `the rustx binary is not built at ${BINARY}; run \`cargo build --bins\``;

if (SKIP !== undefined && process.env.RUSTX_REQUIRE_PROVIDER_EMULATOR) throw new Error(SKIP);

const CREDENTIAL_VARIABLE = "RUSTX_TUI_INTEGRATION_KEY";
const CREDENTIAL_VALUE = "integration-secret";
/** 43 base64url characters, exactly as the server's credential bound requires. */
const TRANSPORT_TOKEN = "tui-integration-token-0000000000000000000000";

/**
 * One user environment for a standalone App Server.
 *
 * The App Server owns process-level user configuration; a Session owns its own
 * cwd and project configuration. This fixture builds both, keeps them separate,
 * and never lets the process's launch directory stand in for a Session's cwd.
 */
class ServerFixture {
  readonly fixture: TempFixture;
  readonly home: string;
  readonly env: NodeJS.ProcessEnv;

  private constructor(fixture: TempFixture, home: string, env: NodeJS.ProcessEnv) {
    this.fixture = fixture;
    this.home = home;
    this.env = env;
  }

  /** Authors the canonical user documents with the real `rustx init`. */
  static create(prefix: string, providerUrl: string): ServerFixture {
    const fixture = TempFixture.create(prefix);
    const home = fixture.path("home");
    mkdirSync(join(home, "rustx"), { recursive: true });
    const env: NodeJS.ProcessEnv = {
      ...process.env,
      HOME: home,
      [CREDENTIAL_VARIABLE]: CREDENTIAL_VALUE,
    };
    delete env.XDG_CONFIG_HOME;
    delete env.XDG_STATE_HOME;

    const initialized = spawnSync(
      BINARY,
      [
        "init",
        "--template", "openai-chat",
        "--provider", "fixture",
        "--model-id", "integration-model",
        "--endpoint", providerUrl,
        "--credential-env", CREDENTIAL_VARIABLE,
        "--context-window", "128000",
        "--max-output", "4096",
        "--tool-calls", "true",
        "--reasoning", "false",
        "--compat", 'chat_reasoning_replay = "omit"',
      ],
      { env, encoding: "utf8" },
    );
    assert.equal(initialized.status, 0, initialized.stderr);
    writeFileSync(fixture.path("token"), `${TRANSPORT_TOKEN}\n`);
    return new ServerFixture(fixture, home, env);
  }

  /** A Workspace with an explicit Root profile, ready to be a Session's cwd. */
  workspace(name: string): string {
    const workspace = this.fixture.path(name);
    mkdirSync(workspace, { recursive: true });
    writeFileSync(join(workspace, "rustx.toml"), '[agent.tools]\nbuiltin = ["read", "write", "edit", "glob", "grep", "bash", "job_list", "job_status", "job_wait", "job_cancel"]\n[agent.plugins.todo]\nenabled = true\n[agent.plugins.agent_status]\nenabled = true\n');
    return workspace;
  }

  settings(name: string): SessionSettings {
    return { cwd: this.workspace(name) };
  }

  get runtimeRoot(): string {
    return this.fixture.path("runtime");
  }

  cleanup(): void {
    this.fixture.cleanup();
  }
}

/** One externally managed App Server, listening on an ephemeral loopback port. */
class ExternalAppServer {
  readonly #child: ChildProcessWithoutNullStreams;
  readonly endpoint: string;
  #exited = false;
  #diagnostics = "";

  private constructor(child: ChildProcessWithoutNullStreams, endpoint: string) {
    this.#child = child;
    this.endpoint = endpoint;
    child.stderr.on("data", (chunk: Buffer) => { this.#diagnostics = (this.#diagnostics + chunk.toString("utf8")).slice(-8192); });
    child.on("exit", () => {
      this.#exited = true;
    });
  }

  /** Starts the server and waits for the address it actually bound. */
  static async start(server: ServerFixture): Promise<ExternalAppServer> {
    const child = spawn(
      BINARY,
      [
        "app-server",
        "--runtime-root", server.runtimeRoot,
        "--listen", "ws://127.0.0.1:0",
        "--token-file", server.fixture.path("token"),
      ],
      { env: server.env, stdio: ["pipe", "pipe", "pipe"] },
    ) as ChildProcessWithoutNullStreams;

    // The server advertises its bound address on stderr only after bootstrap
    // succeeds, so this is a readiness barrier rather than a delay.
    const lines = createInterface({ input: child.stderr });
    const endpoint = await new Promise<string>((resolve, reject) => {
      child.once("exit", (code) =>
        reject(new Error(`the App Server exited before binding (code ${code})`)),
      );
      lines.on("line", (line) => {
        const bound = line.match(/rustx app-server listening (ws:\/\/\S+)/);
        if (bound !== null) resolve(bound[1]!);
      });
    });
    return new ExternalAppServer(child, endpoint);
  }

  get diagnostics(): string { return this.#diagnostics; }

  get running(): boolean {
    return !this.#exited;
  }

  async stop(): Promise<void> {
    if (this.#exited) return;
    this.#child.kill("SIGTERM");
    await new Promise<void>((resolve) => this.#child.once("exit", () => resolve()));
  }
}

/** Creates and attaches one Session, returning its live attachment. */
async function openSession(
  host: AppServerHost,
  settings: SessionSettings,
): Promise<AppServerSession> {
  const created = await host.createSession(settings);
  await host.renameSession(created.session.id, "TUI integration fixture");
  return host.attach(created.session.id, created.session.active_node);
}

it('early TUI startup waits for live MCP discovery before its first provider request', { skip: SKIP, timeout: 120_000 }, async t => {
  requireMcpStartupSupervisor(BINARY);
  const provider = await ProviderEmulator.start('mcp_startup');
  let fixture: ServerFixture | undefined;
  let gate: Awaited<ReturnType<typeof mcpStartupGate>> | undefined;
  let host: AppServerHost | undefined;
  let passed = false;
  let settled: Promise<void> | undefined;
  const cleanup = () => settled ??= (async () => {
    // Close the peer's held recv before joining native shutdown. The hook owns
    // partial setup too, including cancellation before the test body unwinds.
    try { await gate?.close(); }
    finally {
      try { await host?.shutdown(); }
      finally {
        try { fixture?.cleanup(); }
        finally { if (passed) await provider.finish(); else await provider.stop(); }
      }
    }
  })();
  t.after(cleanup);
  try {
    fixture = ServerFixture.create('rustx-tui-mcp-startup-', provider.url());
    const cwd = fixture.workspace('selected-source');
    gate = await mcpStartupGate(cwd, t.signal);
    const activeHost = await AppServerHost.spawnLocal({ binary: BINARY, launch: { runtimeRoot: fixture.runtimeRoot }, env: fixture.env, signal: t.signal });
    host = activeHost;
    const focus = await startupWait(() => prepareStartup(activeHost, parseArguments(['--binary', BINARY, '--workspace', cwd, '--name', 'MCP startup'])), t.signal);
    assert.ok('connecting' in focus && focus.connecting, 'the editable TUI can open before attachment');
    const connecting = focus.connecting;
    const nodeId = connecting.nodeId;
    assert.ok(nodeId, 'creation supplies the exact first-submission node');
    assert.equal(gate.calls, 0, 'Session creation does not prepare an MCP source');
    let attached = false;
    const attaching = activeHost.attach(connecting.sessionId, nodeId).then(session => {
      attached = true;
      return session;
    });
    await gate.waitForDiscovery(attaching);
    assert.equal(attached, false, 'attachment waits for the selected live catalog');
    assert.deepEqual(await startupWait(() => provider.requests(), t.signal), [], 'no provider request may cross the held startup gate');
    // The actual first Send reaches native independently of the held attach.
    await startupWait(() => activeHost.submissionIntent(connecting.sessionId, nodeId), t.signal);
    gate.release();
    const session = await startupWait(() => attaching, t.signal);
    await startupWait(() => session.submitInbound([{ type: 'text', text: 'mcp startup from the TUI' }]), t.signal);
    await startupWait(() => provider.awaitGate('startup-request'), t.signal);
    const requests = await startupWait(() => provider.requests(), t.signal);
    assert.equal(requests.length, 1);
    const body = requests[0]!.body as { tools: { function: { name: string; parameters: unknown } }[] };
    assert.deepEqual(body.tools.map(tool => tool.function.name), ['startup_echo']);
    assert.deepEqual(body.tools[0]!.function.parameters, startupToolSchema);
    assert.equal(gate.calls, 1, 'first admission does not rediscover the prepared source');
    await startupWait(() => provider.releaseGate('startup-request'), t.signal);
    await startupWait(() => until(() => session.state.attempt?.phase.type === 'settled', 'first MCP-enabled attempt settles'), t.signal);
    passed = true;
  } catch (error) {
    t.diagnostic(`${error instanceof Error ? error.stack : String(error)}\n${host?.stderrTail().text ?? ''}\n${provider.diagnostics}`);
    throw error;
  } finally {
    await cleanup();
  }
});

it("same-model TUI command persists inherited intent then delegates explicit NoOp to the native owner", { skip: SKIP, timeout: 120_000 }, async () => {
  const provider = await ProviderEmulator.start("tui_multi_session");
  const fixture = ServerFixture.create("rustx-tui-explicit-model-", provider.url());
  let host = await AppServerHost.spawnLocal({ binary: BINARY, launch: { runtimeRoot: fixture.runtimeRoot }, env: fixture.env });
  try {
    const settings = fixture.settings("inherited-model");
    const created = await host.createSession(settings);
    await host.renameSession(created.session.id, "TUI inherited model fixture");
    await host.shutdown();
    // Public creation materializes its selected default. Seed a legitimate
    // inherited Session only while the native owner is stopped; the next
    // owner then loads it normally, as in the native inherited-selection test.
    const catalogPath = join(fixture.runtimeRoot, "sessions/catalog.json");
    const catalog = JSON.parse(readFileSync(catalogPath, "utf8")) as {
      sessions: Record<string, { state: SessionSettings }>;
    };
    assert.ok(catalog.sessions[created.session.id]!.state.model);
    delete catalog.sessions[created.session.id]!.state.model;
    writeFileSync(catalogPath, JSON.stringify(catalog));
    host = await AppServerHost.spawnLocal({ binary: BINARY, launch: { runtimeRoot: fixture.runtimeRoot }, env: fixture.env });
    const session = await host.attach(created.session.id, created.session.active_node);
    const before = await host.readSettings(session.sessionId);
    assert.equal(before.settings.model, undefined);
    const inherited = await session.modelGet();
    const dispatcher = new CommandDispatcher({ host, session, sessionSettings: settings, diagnostics: () => { throw new Error("not requested"); } });
    const selected = await dispatcher.submit(`/model ${inherited.configured.model}`);
    assert.equal(selected.kind, "transient");
    if (selected.kind === "transient") assert.equal(selected.level, "info", selected.text);
    const explicit = await host.readSettings(session.sessionId);
    assert.deepEqual(explicit.settings.model, inherited.configured);
    assert.equal(BigInt(explicit.revision), BigInt(before.revision) + 1n);

    const repeated = await dispatcher.submit(`/model ${inherited.configured.model}`);
    assert.equal(repeated.kind, "transient");
    if (repeated.kind === "transient") assert.equal(repeated.level, "info", repeated.text);
    assert.deepEqual(await host.readSettings(session.sessionId), explicit);
    assert.deepEqual((await session.modelGet()).configured, inherited.configured);
    assert.deepEqual(await provider.requests(), [], "selection never invokes inference");
  } finally {
    await host.shutdown();
    fixture.cleanup();
    await provider.stop();
  }
});

it("live profile selection crosses the real TUI/App Server boundary while the original request is gated", { skip: SKIP, timeout: 120_000 }, async () => {
  const provider = await ProviderEmulator.start("tui_multi_session");
  const fixture = ServerFixture.create("rustx-tui-live-profile-", provider.url());
  const configPath = join(fixture.home, "rustx/rustx.toml");
  const config = readFileSync(configPath, "utf8").replace('[models."fixture/integration-model"]', '[models."fixture/integration-model"]\ndefault_profile = "high"').replace('reasoning = false', 'reasoning = true');
  writeFileSync(configPath, config + '\n[models."fixture/integration-model".profiles.high]\nreasoning_enabled = true\nrequest_params = \'{"reasoning_effort":"high"}\'\n[models."fixture/integration-model".profiles.low]\nreasoning_enabled = false\nmax_output_tokens = 1024\n');
  const host = await AppServerHost.spawnLocal({ binary: BINARY, launch: { runtimeRoot: fixture.runtimeRoot }, env: fixture.env });
  let passed = false;
  try {
    const settings = fixture.settings("live-profile");
    settings.model = { classificationModel: { mode: "session" }, btwModel: { mode: "session" }, model: "fixture/integration-model", profile: "high", requestParams: { temperature: 0.2 }, maxOutputTokens: 777, summaryModel: { mode: "explicit", model: "fixture/integration-model", profile: "low", request_params: {}, max_output_tokens: 333 }, titleModel: { mode: "explicit", model: "fixture/integration-model", profile: "low", request_params: { title_tag: "preserved" }, max_output_tokens: 222 } };
    const session = await openSession(host, settings);
    await session.submitInbound([{ type: "text", text: "tui multi-session: session A long task" }]);
    await provider.awaitGate("session-a-holding");
    await until(() => session.state.attempt?.phase.type === "running", "original attempt is observed");
    const frozen = structuredClone(session.state.attempt!.model);
    const dispatcher = new CommandDispatcher({ host, session, sessionSettings: settings, diagnostics: () => { throw new Error("not requested"); } });
    const beforeSameModel = await host.readSettings(session.sessionId);
    const sameModel = await dispatcher.submit("/model fixture/integration-model");
    assert.equal(sameModel.kind, "transient");
    if (sameModel.kind === "transient") assert.equal(sameModel.level, "info", sameModel.text);
    assert.deepEqual(await host.readSettings(session.sessionId), beforeSameModel);
    assert.deepEqual((await session.modelGet()).configured, settings.model);
    assert.deepEqual(session.state.attempt!.model, frozen);
    const selected = await dispatcher.submit("/model profile set low");
    assert.equal(selected.kind, "transient");
    if (selected.kind === "transient") assert.equal(selected.level, "info", selected.text);
    const committed = await session.modelGet();
    assert.deepEqual(committed.configured, { ...settings.model, profile: "low" });
    assert.equal(committed.effective?.reasoningEnabled, false);
    assert.equal(session.state.attempt?.phase.type, "running");
    assert.deepEqual(session.state.attempt?.model, frozen);
    assert.match(renderModel(session.state), /profile: high/);
    assert.equal((await host.readSettings(session.sessionId)).settings.model?.profile, "low");
    await provider.releaseGate("session-a-holding");
    await until(() => session.state.attempt?.phase.type === "settled", "original attempt settles");
    assert.deepEqual(session.state.attempt?.model, frozen);
    await session.submitInbound([{ type: "text", text: "tui multi-session: session B quick task" }]);
    await until(() => session.state.attempt?.model?.primary.profile === "low" && session.state.attempt.phase.type === "settled", "new independent attempt captures low");
    const requests = (await provider.requests()).map(request => request.body as Record<string, unknown>);
    assert.equal(requests.length, 2);
    assert.equal(requests[0]!.reasoning_effort, "high");
    assert.equal(requests[1]!.reasoning_effort, undefined);
    for (const request of requests) {
      assert.equal(request.temperature, 0.2);
      assert.equal(request.max_tokens ?? request.max_completion_tokens, 777);
    }
    await host.detach(session.sessionId);
    const reattached = await host.attach(session.sessionId);
    assert.equal(reattached.state.sessionModel?.configured.profile, "low");
    assert.equal(reattached.state.attempt?.model?.primary.profile, "low");
    passed = true;
  } finally {
    await host.shutdown();
    fixture.cleanup();
    if (passed) await provider.finish(); else await provider.stop();
  }
});

describe("local self-hosted mode: one owned App Server child over stdio", { skip: SKIP }, () => {
  let provider: ProviderEmulator;
  let server: ServerFixture;

  before(async () => {
    provider = await ProviderEmulator.start("tui_multi_session");
    server = ServerFixture.create("rustx-tui-stdio-", provider.url("/v1"));
  });
  after(async () => {
    server?.cleanup();
    await provider?.finish();
  });

  it("C01 C02 C12 C20 real TUI Settings author without Sessions or runtime allocation", { timeout: 90_000 }, async () => {
    const isolated = ServerFixture.create("rustx-settings-zero-", provider.url("/v1"));
    const host = await AppServerHost.spawnLocal({ binary: BINARY, launch: { runtimeRoot: isolated.runtimeRoot }, env: isolated.env });
    try {
      const dispatcher = new CommandDispatcher({ host, session: undefined, diagnostics: () => { throw new Error("not requested"); }, sessionSettings: isolated.settings("authoring") });
      assert.equal((await host.client.call("session/list", { offset: 0, limit: 32 }, "sessions")).sessions.length, 0);
      const read = await dispatcher.submit("/settings");
      assert.equal(read.kind, "inspect");
      const workspace = isolated.workspace("explicit-workspace");
      const saved = await dispatcher.submit('/settings workspace "' + workspace + '" approval full_access');
      assert.equal(saved.kind, "inspect");
      const source = await host.client.call("configuration/sourcesRead", { target: { kind: "workspace", directory: workspace } }, "source_settings");
      const nativeSource = await materializeSource(source.projection, async(reference,offset)=>(await host.client.call('configuration/sourceValueRead',{target:{kind:'workspace',directory:workspace},reference,offset,max_bytes:65536},'source_value_chunk')).chunk,()=>true);
      assert.equal(nativeSource.workspace?.authored?.approval_mode, "full_access");
      assert.equal((await host.client.call("session/list", { offset: 0, limit: 32 }, "sessions")).sessions.length, 0);
      assert.equal(host.attached.length, 0);
      for (const command of ["/permissions", "/configuration"]) assert.equal((await dispatcher.submit(command)).kind, "transient");
    } finally { await host.shutdown(); isolated.cleanup(); }
  });

  it("keeps one child across every Session, and never replaces it", { timeout: 90_000 }, async () => {
    const host = await AppServerHost.spawnLocal({
      binary: BINARY,
      launch: { runtimeRoot: server.runtimeRoot },
      env: server.env,
    });
    try {
      // The handshake completed over real JSONL against the real binary.
      assert.equal(host.ownership, "owned_child");
      assert.ok(host.client.capabilities?.multi_session);
      assert.equal(host.childExit, undefined);
      assert.match(host.describe(), /owned App Server child \(pid \d+\)/);

      const a = await openSession(host, server.settings("ses_fa57a52d-bf08-7902-9852-9730a3e99db6"));
      const b = await openSession(host, server.settings("ses_e8de016f-bd70-782f-ad23-25e81df82550"));

      // Two Sessions, two distinct runtimes, one process. Nothing about the
      // second attachment replaced or restarted anything.
      assert.notEqual(a.sessionId, b.sessionId);
      assert.notEqual(a.target.runtime_incarnation, b.target.runtime_incarnation);
      assert.equal(host.attached.length, 2);
      assert.equal(host.childExit, undefined);

      // Returning to A reuses its exact attachment: same four-domain target.
      const returned = await host.attach(a.sessionId);
      assert.equal(returned, a);
      assert.deepEqual(returned.target, a.target);

      const exit = await host.shutdown();
      assert.equal(exit?.code, 0, "the owned child completed explicit owner drain");
    } finally {
      await host.shutdown();
    }
  });

  it("runs Session A while Session B is visible, and repairs A from authoritative state", { timeout: 120_000 }, async (t) => {
    const host = await AppServerHost.spawnLocal({
      binary: BINARY,
      launch: { runtimeRoot: server.runtimeRoot },
      env: server.env,
    });
    try {
      const processIdentity = host.describe();
      const sent: string[] = [];
      const call = host.client.call.bind(host.client);
      t.mock.method(host.client, "call", (...args: Parameters<typeof call>) => {
        sent.push(args[0]); return call(...args);
      });
      const a = await openSession(host, server.settings("multi-a"));
      const b = await openSession(host, server.settings("multi-b"));

      // A starts work and the provider holds its response open.
      await a.submitInbound([
        { type: "text", text: "tui multi-session: session A long task" },
      ]);
      await provider.awaitGate("session-a-holding");

      // Focus moves to B. Nothing was sent that could stop A.
      const before = host.attached.length;
      await host.attach(b.sessionId);
      assert.equal(host.attached.length, before);

      // B is fully usable while A is still executing in the same process.
      await b.submitInbound([
        { type: "text", text: "tui multi-session: session B quick task" },
      ]);
      await until(
        () => b.state.attempt?.phase.type === "settled",
        "Session B settles while Session A is still running",
      );
      assert.equal(
        a.state.attempt?.phase.type,
        "running",
        "Session A kept executing while Session B held the screen",
      );

      // A finishes, and focus returns to it. The projection is repaired from
      // authoritative state rather than from what the client last believed.
      await provider.releaseGate("session-a-holding");
      await until(
        () => a.state.attempt?.phase.type === "settled",
        "Session A settles in the same App Server process",
      );
      const returned = await host.attach(a.sessionId);
      await returned.resync();
      assert.equal(returned.state.attempt?.phase.type, "settled");
      assert.ok(returned.resyncCount >= 1);
      assert.equal(host.childExit, undefined, "one child served both Sessions");
      assert.equal(host.describe(), processIdentity, "focus preserves the exact child PID");
      assert.equal((await provider.requests()).length, 2);
      assert.equal(sent.some(method => ["turn/cancel", "session/unload"].includes(method)), false);
      assert.equal(JSON.stringify(a.state.transcript).includes("B answered while A was still working"), false);
      assert.equal(JSON.stringify(b.state.transcript).includes("A is working"), false);
      const page = await a.boundaries();
      assert.equal(page.boundaries.length, 1, "the committed user boundary is available for tree navigation");
      const original = await host.readSession(a.sessionId);
      const branched = await host.branchSession(a.sessionId, original.active_node,
        page.surfaceRevision, page.boundaries[0]!.locator);
      await assert.rejects(host.attach(a.sessionId, branched.session.active_node), /branch switching/);
      const branch = await host.openNode(a.sessionId, branched.session.active_node);
      assert.notEqual(branch.target.conversation_id, a.target.conversation_id);
      assert.equal(branch.nodeId, branched.session.active_node);
      const back = await host.openNode(a.sessionId, original.active_node);
      assert.equal(back.target.conversation_id, a.target.conversation_id);
      assert.notEqual(back.target.runtime_incarnation, a.target.runtime_incarnation,
        "confirmed node changes use server-owned unload/recovery");
      assert.equal(b.released, false, "another Session's attachment is untouched");
    } catch (error) {
      t.diagnostic(`${JSON.stringify(error)}\n${host.stderrTail().text}`);
      throw error;
    } finally {
      await host.shutdown();
    }
  });

  it("keeps child diagnostics out of the protocol stream", { timeout: 90_000 }, async () => {
    // The child writes its bound-address diagnostic and any warning to stderr.
    // stderr is read into a bounded tail by the process owner and is never
    // handed to the protocol decoder, so no amount of logging can corrupt a
    // JSONL record — which a completed handshake proves.
    const host = await AppServerHost.spawnLocal({
      binary: BINARY,
      launch: { runtimeRoot: server.runtimeRoot },
      env: server.env,
      stderrTailBytes: 2048,
    });
    try {
      const session = await openSession(host, server.settings("stderr-session"));
      assert.equal(session.state.conversationId, session.target.conversation_id);
      const tail = host.stderrTail();
      assert.ok(tail.text.length <= 2048, "the diagnostic tail stays bounded");
      assert.equal(host.client.closed, undefined, "stdout stayed well-framed");
    } finally {
      await host.shutdown();
    }
  });

  it("reports unexpected child death as a process failure, never as a runtime outcome", { timeout: 90_000 }, async () => {
    // The child is driven directly here: the subject is what the client does
    // when a process it owns disappears mid-request.
    const child = AppServerChild.spawn({
      binary: BINARY,
      launch: { runtimeRoot: server.runtimeRoot },
      env: server.env,
    });
    const transport = new StdioTransport({
      input: child.stdout,
      output: child.stdin,
      label: "stdio integration",
    });
    void child.wait().then((exit) => {
      transport.reportProcessExit(exit.code, exit.signal, exit.spawnError);
    });
    const client = await AppServerClient.initialize({ transport });

    const created = await client.call(
      "session/create",
      { settings: server.settings("death-session") },
      "session_transition",
    );
    const pending = client.call(
      "session/attach",
      { session_id: created.session.id, node_id: null },
      "attached",
    );
    // The process dies with a request in flight.
    process.kill(child.pid!, "SIGKILL");

    const failure = await pending.then(
      () => undefined,
      (error: unknown) => error,
    );
    // `session/attach` can change server state, so a lost response is an
    // unknown outcome — not a failure, and never something to resend.
    assert.ok(failure instanceof UncertainOutcomeError, String(failure));
    assert.ok(failure.transportFailure instanceof TransportClosedError);
    // SIGKILL can be observed first by the pending pipe write, stdout EOF,
    // or the process-exit callback. These are all transport facts; callback
    // ordering does not change the mutation's uncertain outcome.
    if (failure.transportFailure.reason === "write_error") {
      const cause = failure.transportFailure.cause;
      assert.ok(cause instanceof Error && "code" in cause);
      assert.equal(cause.code, "EPIPE");
    } else {
      assert.ok(
        failure.transportFailure.reason === "process_exit" ||
        failure.transportFailure.reason === "input_eof",
        failure.transportFailure.reason,
      );
    }
    // Nothing here claims a turn settled, an interaction was answered, or a
    // tool completed. The client only lost its ability to observe.
    assert.ok(client.closed);
    assert.deepEqual(await child.wait(), { code: null, signal: "SIGKILL" });
  });
});

describe("existing/remote mode: WebSocket to an externally managed App Server", { skip: SKIP }, () => {
  let provider: ProviderEmulator;
  let server: ServerFixture;
  let external: ExternalAppServer;

  before(async () => {
    provider = await ProviderEmulator.start("tui_multi_session");
    server = ServerFixture.create("rustx-tui-ws-", provider.url("/v1"));
    external = await ExternalAppServer.start(server);
  });
  after(async () => {
    await external?.stop();
    server?.cleanup();
    await provider?.finish();
  });

  it("resume bootstrap can browse while another real connection controls A, then attach only a chosen Session", { timeout: 90_000 }, async (t) => {
    const owner = await AppServerHost.connectRemote({ endpoint: external.endpoint, token: TRANSPORT_TOKEN });
    const browser = await AppServerHost.connectRemote({ endpoint: external.endpoint, token: TRANSPORT_TOKEN });
    try {
      const a = await openSession(owner, server.settings("resume-A"));
      const settings = server.settings("resume-B");
      const b = await owner.createSession(settings);
      const attachments: string[] = [];
      const attach = browser.attach.bind(browser);
      t.mock.method(browser, "attach", (id: string, node?: string) => { attachments.push(id); return attach(id, node); });
      const parsed = parseArguments(["--connect", external.endpoint, "--token-file", server.fixture.path("token"), "--workspace", settings.cwd, "--resume"]);
      const focus = await prepareStartup(browser, parsed);
      assert.equal(focus.session, undefined);
      assert.deepEqual(attachments, []);
      assert.equal(browser.attached.length, 0);
      assert.ok(focus.resumePage?.sessions.some((s) => s.id === a.sessionId));
      assert.ok(focus.resumePage?.sessions.some((s) => s.id === b.session.id));
      await assert.rejects(browser.attach(a.sessionId), (error: unknown) =>
        error instanceof Error && "kind" in error && error.kind === "controller_in_use");
      const selected = await browser.attach(b.session.id);
      assert.equal(selected.sessionId, b.session.id);
      assert.deepEqual(attachments, [a.sessionId, b.session.id]);
      assert.deepEqual(browser.attached.map((s) => s.sessionId), [b.session.id]);
      assert.equal(owner.attachment(a.sessionId), a);
      assert.equal(a.released, false);
      await a.resync();
      assert.equal(owner.client.closed, undefined);
    } finally {
      await browser.shutdown();
      await owner.shutdown();
    }
  });

  it("admits a credentialed client and refuses one without the token", { timeout: 90_000 }, async () => {
    const host = await AppServerHost.connectRemote({
      endpoint: external.endpoint,
      token: TRANSPORT_TOKEN,
    });
    try {
      assert.equal(host.ownership, "external");
      assert.ok(host.client.capabilities?.multi_session);
      assert.match(host.describe(), /external App Server at ws:\/\//);
      // Ownership is structural, not inferred: a loopback endpoint is still
      // somebody else's process.
      assert.equal(host.childExit, undefined);
      assert.deepEqual(host.stderrTail(), { text: "", truncatedBytes: 0 });
    } finally {
      await host.shutdown();
    }

    await assert.rejects(
      AppServerHost.connectRemote({
        endpoint: external.endpoint,
        token: "wrong-token-000000000000000000000000000000",
      }),
      (error: unknown) =>
        error instanceof TransportClosedError && error.reason === "handshake_failed",
    );
    assert.ok(external.running, "a refused client never stops the listener");
  });

  it("CONVERGE-TUI a stale warning retires when native re-admits its forgotten Workspace", { timeout: 120_000 }, async () => {
    const malformed = (revision: number) => `[agent\nlegacy = ${revision}\n`;
    const directory = (name: string) => {
      const path = server.fixture.path(name);
      mkdirSync(path, { recursive: true });
      return { kind: "workspace", directory: realpathSync(path) } as const;
    };
    const w0 = directory("converge-w0");
    const w0File = join(w0.directory, "rustx.toml");
    const others = Array.from({ length: 64 }, (_, index) => directory(`converge-w${index + 1}`));
    const key = (target: SourceTarget) => target.kind === "user" ? "user" : `workspace:${target.directory}`;
    const held = (host: AppServerHost) => host.sourceRepairs().find(observation => key(observation.target) === key(w0));
    const requests = (await provider.requests()).length;
    const a = await AppServerHost.connectRemote({ endpoint: external.endpoint, token: TRANSPORT_TOKEN });
    const b = await AppServerHost.connectRemote({ endpoint: external.endpoint, token: TRANSPORT_TOKEN });
    // The status line, composed exactly as the TUI composes it.
    const status = new SourceRepairStatus();
    status.adopt(a.authorityId);
    for (const observation of a.sourceRepairs()) status.observe(a.authorityId, observation);
    a.onSourceRepair(observation => status.observe(a.authorityId, observation));
    const warned = () => status.pendingSources().some(source => key(source) === key(w0));
    try {
      // 1. A observes W0's unresolved receipt: its failed Session admits W0.
      writeFileSync(w0File, malformed(1));
      await assert.rejects(a.createSession({ cwd: w0.directory }));
      await until(() => held(a)?.receipt?.status === "unresolved", "A observes W0 unresolved");
      assert.ok(warned());
      const first = held(a)!;
      // 2. A addresses 63 more Workspaces: native interest is full.
      for (const target of others.slice(0, 63)) await a.readSourceRepairs(target);
      // 3. W0's unresolved document changes, observed through B: a newer
      //    unresolved level, which A — still interested — observes most
      //    recently.
      writeFileSync(w0File, malformed(2));
      await assert.rejects(b.createSession({ cwd: w0.directory }));
      await until(() => held(a) !== undefined && BigInt(held(a)!.sequence) > BigInt(first.sequence), "A observes W0's newer level");
      assert.equal(held(a)!.receipt?.status, "unresolved");
      // 4. A 65th Workspace: native forgets W0, the least recently addressed;
      //    A's caches forget the least recently observed, and keep W0.
      await a.readSourceRepairs(others[63]!);
      assert.equal(held(a)?.receipt?.status, "unresolved");
      assert.ok(warned());
      // 5. B resolves W0 through the manual repair; A is no longer interested.
      await b.readSourceRepairs(w0);
      const source = await b.client.call('configuration/sourcesRead', { target: w0 }, 'source_settings');
      const read = await materializeSource(source.projection, async (reference, offset) =>
        (await b.client.call('configuration/sourceValueRead', { target: w0, reference, offset, max_bytes: 65_536 }, 'source_value_chunk')).chunk, () => !b.client.closed);
      await b.client.call("configuration/sourceWrite", {
        target: w0,
        expected_revision: read.workspace!.revision,
        mutation: { kind: "repair_config", document: '[environment]\nRESIDENCY_SESSION = "converged"\n' },
      }, "sources_written");
      const healthy = readFileSync(w0File, "utf8");
      // 6. A re-admits W0 by opening a Session there — no read of W0 — and the
      //    native admission level withdraws the stale warning.
      const session = await openSession(a, { cwd: w0.directory });
      await until(() => held(a)?.receipt === undefined, "native announces W0's empty level");
      assert.equal(warned(), false);
      assert.equal(status.visible, status.pendingSources().length > 0);
      assert.ok(!status.pendingSources().some(source => key(source) === key(w0)));
      // 7. Reads and a reconnect to the same authority never restore it.
      await a.readSourceRepairs(w0);
      assert.equal(held(a)?.receipt, undefined);
      const reconnected = await AppServerHost.connectRemote({ endpoint: external.endpoint, token: TRANSPORT_TOKEN });
      try {
        assert.equal(status.adopt(reconnected.authorityId), true, "the same authority");
        reconnected.onSourceRepair(observation => status.observe(reconnected.authorityId, observation));
        await reconnected.readSourceRepairs(w0);
        assert.equal(reconnected.sourceRepairs().find(observation => key(observation.target) === key(w0))?.receipt, undefined);
        assert.equal(warned(), false);
      } finally {
        await reconnected.shutdown();
      }
      // Observation alone: W0's bytes are B's write, the Session is idle, and
      // no model was asked anything.
      assert.equal(readFileSync(w0File, "utf8"), healthy);
      assert.equal(session.state.attempt ?? null, null);
      assert.equal((await provider.requests()).length, requests);
    } finally {
      await a.shutdown();
      await b.shutdown();
    }
  });

  it("LEDGER-EVICT-08 a warning whose receipt native's ledger forgets retires while A keeps observing W0", { timeout: 120_000 }, async (t) => {
    const malformed = (revision: number) => `[agent\nlegacy = ${revision}\n`;
    const directory = (name: string) => {
      const path = server.fixture.path(name);
      mkdirSync(path, { recursive: true });
      return { kind: "workspace", directory: realpathSync(path) } as const;
    };
    const w0 = directory("evict-w0");
    const w0File = join(w0.directory, "rustx.toml");
    const others = Array.from({ length: 64 }, (_, index) => directory(`evict-w${index + 1}`));
    for (const target of others) writeFileSync(join(target.directory, "rustx.toml"), malformed(1));
    const key = (target: SourceTarget) => target.kind === "user" ? "user" : `workspace:${target.directory}`;
    const held = (host: AppServerHost) => host.sourceRepairs().find(observation => key(observation.target) === key(w0));
    const requests = (await provider.requests()).length;
    const a = await AppServerHost.connectRemote({ endpoint: external.endpoint, token: TRANSPORT_TOKEN });
    const b = await AppServerHost.connectRemote({ endpoint: external.endpoint, token: TRANSPORT_TOKEN });
    const status = new SourceRepairStatus();
    status.adopt(a.authorityId);
    for (const observation of a.sourceRepairs()) status.observe(a.authorityId, observation);
    a.onSourceRepair(observation => status.observe(a.authorityId, observation));
    const warned = () => status.pendingSources().some(source => key(source) === key(w0));
    try {
      // 1. A's failed Session admits W0, and A observes its unresolved R0.
      writeFileSync(w0File, malformed(1));
      await assert.rejects(a.createSession({ cwd: w0.directory }));
      await until(() => held(a)?.receipt?.status === "unresolved", "A observes W0 unresolved");
      assert.ok(warned());
      const r0 = held(a)!;
      // From here A sends nothing: no read, no Session, so no re-admission.
      const sent: string[] = [];
      const call = a.client.call.bind(a.client);
      t.mock.method(a.client, "call", (...args: Parameters<typeof call>) => {
        sent.push(args[0]);
        return call(...args);
      });
      // 2. B reconciles 64 malformed Workspaces A never addressed; each is a
      //    newer ledger entry, so native's finite ledger forgets W0.
      for (const target of others) await b.client.call("configuration/reconcile", { target }, "configuration_receipt");
      // 3. Native still observes W0 for A and delivers its absence.
      await until(() => held(a)?.receipt === undefined, "native delivers W0's absence");
      assert.ok(BigInt(held(a)!.sequence) > BigInt(r0.sequence));
      assert.equal(warned(), false);
      assert.deepEqual(sent, []);
      // Absence is not health: W0's bytes are unchanged and its read still
      // reports the malformed document.
      assert.equal(readFileSync(w0File, "utf8"), malformed(1));
      await b.readSourceRepairs(w0);
      const source = await b.client.call('configuration/sourcesRead', { target: w0 }, 'source_settings');
      const read = await materializeSource(source.projection, async (reference, offset) =>
        (await b.client.call('configuration/sourceValueRead', { target: w0, reference, offset, max_bytes: 65_536 }, 'source_value_chunk')).chunk, () => !b.client.closed);
      assert.equal(read.workspace_repair ?? null, null);
      assert.ok(read.workspace?.diagnostic);
      // 4. R0 delivered again late is older and restores nothing.
      assert.equal(status.observe(a.authorityId, r0), false);
      assert.equal(warned(), false);
      // 5. A still observes W0: a new episode reaches it with no request.
      writeFileSync(w0File, malformed(2));
      await assert.rejects(b.createSession({ cwd: w0.directory }));
      await until(() => held(a)?.receipt?.status === "unresolved", "A observes W0's new episode");
      assert.notEqual(held(a)!.receipt!.id, r0.receipt!.id);
      assert.ok(warned());
      assert.deepEqual(sent, []);
      // R0's announcement identity survived its retirement: R0 current again
      // at a newer sequence retires R1 from the line and is not announced.
      assert.equal(status.observe(a.authorityId, { ...r0, sequence: String(BigInt(held(a)!.sequence) + 1n) }), true);
      assert.equal(warned(), false);
      assert.equal((await provider.requests()).length, requests);
    } finally {
      await a.shutdown();
      await b.shutdown();
    }
  });

  it("T10 T17 keeps execution alive and resumes real WebSocket replay from locally Applied", { timeout: 120_000 }, async (t) => {
    const first = await AppServerHost.connectRemote({
      endpoint: external.endpoint,
      token: TRANSPORT_TOKEN,
    });
    const settings = server.settings("remote-session");
    const session = await openSession(first, settings);
    const sessionId = session.sessionId;
    const incarnation = session.target.runtime_incarnation;

    const running = new Promise<void>(resolve => {
      const stop = session.onState(state => {
        if (state.attempt?.phase.type === 'running') { stop(); resolve(); }
      });
    });
    await session.submitInbound([{ type: "text", text: "tui multi-session: session A long task" }]);
    try {
      // Admission accepts input; the provider arrival deadline starts after native execution starts.
      await running;
      await provider.awaitGate("session-a-holding");
    } catch (error) {
      const published = session.checkpoint, source = published?.read_cut.source_cut;
      t.diagnostic(JSON.stringify({
        stage: 'accepted inbound awaiting original provider gate',
        observation: session.observationPhase, controlsReady: session.controlReady,
        applied: session.state.cursor, observationError: session.observationError?.slice(0, 512),
        attempt: session.state.attempt && {
          id: session.state.attempt.attemptId?.slice(0, 256), phase: session.state.attempt.phase.type,
        },
        runtimeShutdown: session.state.runtimeShutdown, durabilityFailure: session.state.durabilityFailure && {
          operation: session.state.durabilityFailure.operation.slice(0, 256), diagnostic: session.state.durabilityFailure.diagnostic.slice(0, 512),
        },
        checkpoint: published && { cursor: published.cursor, revision: published.read_cut.revision, through: published.read_cut.through,
          source: source && { journal: source.read.journal, transcript: source.read.transcript,
            ledger: source.ledger_through, mutation: source.read.mutation_revision } },
        nativeStderr: external.diagnostics.slice(-4096), providerStderr: provider.diagnostics.slice(-2048),
      }));
      throw error;
    }

    const applied=session.state.cursor, attemptId=session.state.attempt?.attemptId;
    assert.ok(BigInt(applied)>0n);
    // Exiting a remote TUI is a disconnect and nothing more.
    await first.shutdown();
    assert.ok(external.running, "TUI exit never stops an external App Server");

    // Reconnect to the same Runtime using its matching locally Applied seed.
    // The fresh registration must replay from the exact cursor already folded.
    const second = await AppServerHost.connectRemote({
      endpoint: external.endpoint,
      token: TRANSPORT_TOKEN,
    });
    try {
      const methods:Array<{method:string;params:unknown}>=[];
      const call=second.client.call.bind(second.client);
      t.mock.method(second.client, 'call', (...args: Parameters<typeof call>) => {
        methods.push({ method: args[0], params: args[1] });
        return call(...args);
      });
      const reattached = await second.attach(sessionId,session.nodeId,session);
      assert.equal(methods.filter(value=>value.method==='session/checkpoint').length,0,'matching Runtime resumes replay without silently replacing Applied');
      const follow=methods.find(value=>value.method==='session/subscribe');
      assert.equal((follow?.params as {after_cursor:string}).after_cursor,applied);
      assert.equal(reattached.state.attempt?.attemptId,attemptId,'disconnect did not restart the attempt');
      assert.equal(reattached.sessionId, sessionId);
      assert.notEqual(
        reattached.target.attachment_id,
        session.target.attachment_id,
        "reconnecting always receives a new attachment identity",
      );
      assert.equal(
        reattached.target.runtime_incarnation,
        incarnation,
        "the Session kept running in the same server incarnation",
      );
      // The Session's accepted work survived the disconnect: the projection
      // this client reads is the server's, not the previous client's.
      assert.equal(reattached.state.conversationId, session.state.conversationId);
      assert.equal(reattached.state.attempt?.phase.type, "running");
      const b = await openSession(second, server.settings("remote-b"));
      await b.submitInbound([{ type: "text", text: "tui multi-session: session B quick task" }]);
      await until(() => b.state.attempt?.phase.type === "settled", "B completes while disconnected A remains held");
      await provider.releaseGate("session-a-holding");
      await until(() => reattached.state.attempt?.phase.type === "settled", "accepted remote work finishes after reconnect");
      assert.equal((await provider.requests()).length, 2, "one admission per Session, no replay");
    } finally {
      await second.shutdown();
    }
    assert.ok(external.running);
  });

  it("never replays an uncertain mutation when the socket dies", { timeout: 90_000 }, async () => {
    const transport = await WebSocketTransport.connect({ endpoint: external.endpoint, token: TRANSPORT_TOKEN });
    let loseResponse = false;
    let sentMutations = 0;
    const client = await AppServerClient.initialize({ transport: {
      get closed() { return transport.closed; },
      describe: () => transport.describe(),
      close: () => transport.close(),
      onClose: (listener) => transport.onClose(listener),
      send: async (message) => {
        if ((message as { method?: string }).method === "session/name") sentMutations++;
        await transport.send(message);
      },
      onMessage: (listener) => transport.onMessage((message) => {
        const result = typeof message === "object" && message !== null && "result" in message
          ? message.result : undefined;
        if (loseResponse && typeof result === "object" && result !== null && "type" in result && result.type === "session") {
          transport.close(); // The server committed the rename; drop its response.
          return;
        }
        listener(message);
      }),
    } });
    const host = new AppServerHost({ client, ownership: "external" });
    const session = await openSession(host, server.settings("replay-session"));
    sentMutations = 0; // Count only the mutation whose response is deliberately lost.
    loseResponse = true;
    await assert.rejects(host.renameSession(session.sessionId, "accepted-once"), UncertainOutcomeError);
    const recovered = await AppServerHost.connectRemote({ endpoint: external.endpoint, token: TRANSPORT_TOKEN });
    try {
      assert.equal((await recovered.readSession(session.sessionId)).name, "accepted-once");
      assert.equal(sentMutations, 1);
      await recovered.attach(session.sessionId);
    } finally {
      await recovered.shutdown();
      await host.shutdown();
    }
    assert.ok(external.running);
  });
});

describe("cross-transport parity", { skip: SKIP }, () => {
  let provider: ProviderEmulator;
  let server: ServerFixture;

  before(async () => {
    provider = await ProviderEmulator.start("tui_integration");
    server = ServerFixture.create("rustx-tui-parity-", provider.url("/v1"));
  });
  after(async () => {
    server?.cleanup();
    await provider?.stop();
  });

  it("invokes the same operations and reaches the same projection over stdio and WebSocket", { timeout: 120_000 }, async () => {
    /** One representative scenario, expressed once, run over each transport. */
    async function scenario(host: AppServerHost, name: string) {
      const session = await openSession(host, server.settings(name));
      const renamed = await host.renameSession(session.sessionId, "parity");
      const listed = await host.listSessions();
      const capabilities = await session.capabilitiesPage();
      const capabilityRevision = session.state.capabilities.revision;
      assert.equal(capabilities.read_cut.domain_revisions.capabilities,
        session.checkpoint?.read_cut.domain_revisions.capabilities,
        'capability page belongs to the exact loaded native header generation');
      const requestedCut = session.checkpoint?.read_cut;
      let available: Awaited<ReturnType<AppServerSession["observationPage"]>>;
      try {
        available = await session.observationPage('capabilities.available_tools');
      } catch (error) {
        throw new Error(`Capability collection cut evidence: ${JSON.stringify({
          domain:'capabilities.available_tools', capabilityCut:capabilities.read_cut,
          requestedCut, currentCut:session.checkpoint?.read_cut, applied:session.state.cursor,
        })}`, {cause:error});
      }
      assert.ok(available.items.length <= 32);
      if (available.next_offset != null) {
        const next = await session.observationPage(available.domain, Number(available.next_offset), available);
        assert.ok(next.items.length <= 32);
        assert.ok(next.next_offset == null || BigInt(next.next_offset) > BigInt(available.next_offset));
      }
      const boundaries = await session.boundaries();
      await session.resync();
      return {
        // Everything compared here is server-authoritative and
        // transport-independent by construction.
        named: renamed.name,
        listedSelf: listed.sessions.some((row) => row.id === session.sessionId),
        conversation: session.state.conversationId === session.target.conversation_id,
        capabilityRevisionIsExact: /^(0|[1-9][0-9]*)$/.test(capabilityRevision),
        capabilityReadCutIsExact: /^(0|[1-9][0-9]*)$/.test(capabilities.read_cut.domain_revisions.capabilities!),
        capabilityPageBounded: capabilities.items.length <= 32,
        availableToolPageBounded: available.items.length <= 32,
        surfaceRevisionIsExact: /^(0|[1-9][0-9]*)$/.test(boundaries.surfaceRevision),
        boundaries: boundaries.boundaries.length,
        repaired: session.resyncCount,
        attempt: session.state.attempt?.phase.type,
      };
    }

    const local = await AppServerHost.spawnLocal({
      binary: BINARY,
      launch: { runtimeRoot: server.runtimeRoot },
      env: server.env,
    });
    let overStdio;
    try {
      overStdio = await scenario(local, "parity-stdio");
      assert.equal(local.ownership, "owned_child");
    } finally {
      await local.shutdown();
    }

    const external = await ExternalAppServer.start(server);
    const remote = await AppServerHost.connectRemote({
      endpoint: external.endpoint,
      token: TRANSPORT_TOKEN,
    });
    let overWebSocket;
    try {
      overWebSocket = await scenario(remote, "parity-ws");
      assert.equal(remote.ownership, "external");
    } finally {
      await remote.shutdown();
      // The one legitimate difference: who owns the process, and therefore
      // what exiting means. Everything above this line is identical.
      assert.ok(external.running, "exiting a remote TUI stops nothing");
      await external.stop();
    }

    assert.deepEqual(
      overWebSocket,
      overStdio,
      "the same operations reach the same projection on both transports",
    );
  });
});

describe("bounded product lifecycle", { skip: SKIP }, () => {
  it("reclaims live registries and owned children across repeated stdio and WebSocket lifetimes", { timeout: 120_000 }, async () => {
    for (const remote of [false, true]) {
      const provider = await ProviderEmulator.start("app_server_lifecycle");
      const server = ServerFixture.create("rustx-lifecycle-", provider.url("/v1"));
      const external = remote ? await ExternalAppServer.start(server) : undefined;
      let passed = false;
      try {
        for (let cycle = 0; cycle < 3; cycle++) {
          const host = external
            ? await AppServerHost.connectRemote({ endpoint: external.endpoint, token: TRANSPORT_TOKEN })
            : await AppServerHost.spawnLocal({ binary: BINARY, launch: { runtimeRoot: server.runtimeRoot }, env: server.env });
          try {
            let baseline = (await host.client.call("server/diagnostics", {}, "diagnostics")).snapshot;
            // On the same external process, wait for actual prior socket reaping.
            // No elapsed duration establishes this resource claim.
            while (remote && baseline.transport.websocket_connections !== 1) {
              baseline = (await host.client.call("server/diagnostics", {}, "diagnostics")).snapshot;
            }
            assert.equal(baseline.loaded, 0);
            const session = await openSession(host, server.settings(`cycle-${cycle}`));
            await session.submitInbound([{ type: "text", text: "lifecycle small turn" }]);
            await until(() => session.state.attempt?.phase.type === "settled", "small turn settles");
            const snapshot = await host.client.call("session/checkpoint", { target: session.target }, "checkpoint");
            assert.deepEqual(snapshot.checkpoint.snapshot.pending_interactions, []);
            assert.deepEqual(snapshot.checkpoint.snapshot.jobs, []);
            assert.deepEqual(snapshot.checkpoint.snapshot.agents, []);
            assert.deepEqual(snapshot.checkpoint.snapshot.workflows.runs, []);
            await host.detach(session.sessionId);
            const attached = await host.attach(session.sessionId);
            const preview = await host.previewSessionDeletion(attached.sessionId);
            assert.equal(preview.status, "preview");
            if (preview.status !== "preview") throw new Error("expected preview");
            const deleted = await host.deleteSession(attached.sessionId, preview.preview.target_revision);
            assert.equal(deleted.status, "deleted");
            await until(() => attached.serverClosed, "deleted attachment closes");
            const end = (await host.client.call("server/diagnostics", {}, "diagnostics")).snapshot;
            for (const key of ["loaded", "loading", "unloading", "active_roots", "external_attachments"] as const) assert.equal(end[key], baseline[key], key);
            assert.equal((await provider.requests()).length, cycle + 1);
            const exit = await host.shutdown();
            if (external) assert.equal(external.running, true);
            else assert.equal(exit?.code, 0, "owned process is reaped every cycle");
          } finally { await host.shutdown(); }
        }
        passed = true;
      } finally {
        await external?.stop();
        if (passed) await provider.finish(); else await provider.stop();
        server.cleanup();
      }
    }
  });
});

for (const carrier of ["stdio", "websocket"] as const) it(`native child transcript ${carrier}: exact ownership, running/waiting/terminal, tools, metadata and reconnect`, { skip: SKIP, timeout: 120_000 }, async t => {
  const provider = await ProviderEmulator.start("tui_subagent_inspection");
  const server = ServerFixture.create("rustx-child-transcript-", provider.url());
  const workspace = server.workspace("parent");
  writeFileSync(join(workspace, "rustx.toml"), '[agent]\nagents = ["researcher"]\n[agent.tools]\nbuiltin = ["ask_user", "job_list", "job_status", "job_wait", "job_cancel"]\n');
  mkdirSync(join(workspace, ".agents/agents"), { recursive: true });
  writeFileSync(join(workspace, ".agents/agents/researcher.toml"), 'description = "Research history"\ninstructions = "Inspect canonical child history"\n[tools]\nbuiltin = ["ask_user"]\n');
  const external = carrier === "websocket" ? await ExternalAppServer.start(server) : undefined;
  let host = external ? await AppServerHost.connectRemote({ endpoint: external.endpoint, token: TRANSPORT_TOKEN }) : await AppServerHost.spawnLocal({ binary: BINARY, launch: { runtimeRoot: server.runtimeRoot }, env: server.env });
  const sent: string[] = [];
  const call = host.client.call.bind(host.client);
  t.mock.method(host.client, "call", (...args: Parameters<typeof call>) => { sent.push(args[0]); return call(...args); });
  try {
    let session = await openSession(host, { cwd: workspace });
    const stateWhen = (predicate: () => boolean) => new Promise<void>(resolve => {
      if (predicate()) { resolve(); return; }
      const unsubscribe = session.onState(() => { if (predicate()) { unsubscribe(); resolve(); } });
    });
    await session.submitInbound([{ type: "text", text: "Delegate a canonical history inspection" }]);
    await Promise.all([provider.awaitGate("inspection-request-0"), provider.awaitGate("inspection-request-1")]);
    await session.resync();
    const child = session.state.agents[0]!;
    assert.ok(child); assert.equal(child.state, "active");
    const running = await session.agentTranscriptPage(child.agent_id);
    assert.ok(running); assert.ok(JSON.stringify(running).includes("Inspect canonical child history"));
    assert.equal(host.attached.length, 1);
    await assert.rejects(session.agentTranscriptPage(child.child_conversation_id), error => error instanceof Error && "kind" in error && error.kind === "unknown_agent");
    const assertInvalidTranscriptLimits = async (subagentId: string) => {
      for (const limit of [0, 257]) {
        await assert.rejects(
          host.client.call("agent/transcript", { target: session.target, agent_id: subagentId, at: { type: "latest" }, limit }, "transcript_window"),
          error => error instanceof Error && "kind" in error && error.kind === "invalid_params",
        );
      }
    };
    await assertInvalidTranscriptLimits(child.agent_id);
    await assertInvalidTranscriptLimits("unknown-child");
    const unrelated = await openSession(host, server.settings("unrelated"));
    await assert.rejects(unrelated.agentTranscriptPage(child.agent_id), error => error instanceof Error && "kind" in error && error.kind === "unknown_agent");
    await unrelated.detach();
    await provider.releaseGate("inspection-request-0"); await provider.releaseGate("inspection-request-1");
    await stateWhen(() => session.state.pendingInteractions.length === 2);
    const routed = session.state.pendingInteractions.find(item => item.interaction.conversation_id === child.child_conversation_id)!;
    const primary = session.state.pendingInteractions.find(item => item.interaction.conversation_id === session.target.conversation_id)!;
    assert.ok(routed); assert.ok(primary); assert.equal(routed.source.type, "subagent");
    const waiting = await session.agentTranscriptPage(child.agent_id);
    assert.ok(waiting); assert.equal(session.state.pendingInteractions.length, 2, "reading creates no second waiter or answer");
    const answer = { type: "questionnaire", response: { type: "submitted", value: { answers: [{ question_index: 0, answer: { type: "option", value: { option_index: 0 } } }] } } } as const;
    // Only the root attachment answers the exact child-routed InteractionRef.
    await session.respondInteraction(routed.interaction, { ...answer, response: { type: "submitted", value: { answers: [...answer.response.value.answers] } } });
    await provider.awaitGate("child-completion");
    const withTool = await session.agentTranscriptPage(child.agent_id);
    assert.ok(withTool);
    const messages = withTool.entries!.flatMap(entry => entry.item.type === "message" ? [entry.item.message] : []);
    const result = messages.find(message => message.role === "tool");
    assert.ok(result && result.role === "tool");
    const proposal = messages.find(message => message.role === "assistant" && message.id === result.occurrence.assistant_message_id);
    assert.ok(proposal && proposal.role === "assistant");
    const block = proposal.content[result.occurrence.block_index];
    assert.ok(block?.type === "tool_call"); assert.equal(block.id, result.tool_call_id);
    assert.equal(withTool.entries!.filter(entry => entry.completed_response).length, 0);
    // Explicit original Child inspection uses this same parent carrier while
    // both model owners remain at their existing execution gates.
    const beforeInspection=(await provider.requests()).length;
    const childTrace=await host.client.call('agent/trace',{target:session.target,agent_id:child.agent_id,limit:32,records:[]},'trace');
    const toolRecord=childTrace.page.records.find(record=>record.kind==='tool'&&record.tool?.call_id===result.tool_call_id);
    assert.ok(toolRecord,'the real Child Tool retains its native Trace record');
    const inspection=new SubagentTranscript(session,child.agent_id);
    try {
      await inspection.inspectTrace(toolRecord.id);assert.equal(inspection.error,undefined);
      const selected=inspection.inspection;assert.ok(selected?.head&&selected.page);
      assert.equal(selected.page.read_cut.conversation_id,child.child_conversation_id);
      assert.equal(selected.page.read_cut.activation_id,child.activation_id);
      assert.equal(selected.head.content.type,'tool');
      if(selected.head.content.type==='tool')assert.equal(selected.head.content.definition.type,'available');
      const index=inspection.traceFields.findIndex(field=>field.field==='definition');assert.ok(index>=0);
      await inspection.loadContent(inspection.deferred.length+inspection.deferredJournal.length+inspection.deferredAttemptIdentities.length+index);
      assert.equal(inspection.error,undefined);assert.ok(inspection.content);assert.equal(inspection.content.next,undefined);
      const original:unknown=JSON.parse(inspection.content.text);
      assert.ok(original&&typeof original==='object'&&'name' in original&&'input_schema' in original);
      assert.equal(original.name,'ask_user');
      const schema=original.input_schema;assert.ok(schema&&typeof schema==='object'&&'truncated' in schema&&'value' in schema);assert.equal(schema.truncated,false);
      const value=schema.value;assert.ok(value&&typeof value==='object'&&'properties' in value);
      const resultField=inspection.traceFields.findIndex(field=>field.field==='result'&&field.type==='content'&&field.binding.content_ref.source.type==='canonical_message_block'&&field.binding.content_ref.source.component==='message_metadata');assert.ok(resultField>=0,'the genuine Tool C advertises original result metadata');
      const toolBase=inspection.deferred.length+inspection.deferredJournal.length+inspection.deferredAttemptIdentities.length;
      await inspection.loadContent(toolBase+resultField);assert.equal(inspection.error,undefined);assert.ok(inspection.content);assert.equal(inspection.content.next,undefined);
      const resultMetadata:unknown=JSON.parse(inspection.content.text);
      assert.ok(resultMetadata&&typeof resultMetadata==='object'&&'result' in resultMetadata&&'tool_call_id' in resultMetadata);assert.equal(resultMetadata.tool_call_id,result.tool_call_id);
      const status=resultMetadata.result;assert.ok(status&&typeof status==='object'&&'status' in status);assert.deepEqual(status.status,{type:'success'});assert.equal('content' in status,false,'result metadata retains its own finite contract');
      const resultBlocks=inspection.inspectionCollections.findIndex(frontier=>frontier.domain.endsWith('.blocks'));assert.ok(resultBlocks>=0,'the Tool head advertises its original canonical result blocks');
      for(let index=0;index<=resultBlocks;index++)await inspection.selectTraceCollection();
      assert.equal(inspection.error,undefined);const resultPage=inspection.inspection?.collection;assert.ok(resultPage);
      const resultPayload=resultPage.items.flatMap(item=>item.content).find(binding=>binding.content_ref.source.type==='canonical_message_block'&&binding.content_ref.source.component==='payload');assert.ok(resultPayload);
      const resultPayloadIndex=inspection.collectionSources.findIndex(source=>source.type==='content'&&JSON.stringify(source.binding)===JSON.stringify(resultPayload));assert.ok(resultPayloadIndex>=0);
      await inspection.loadContent(toolBase+inspection.traceFields.length+resultPayloadIndex);assert.equal(inspection.error,undefined);assert.ok(inspection.content);assert.equal(inspection.content.next,undefined);
      assert.deepEqual(JSON.parse(inspection.content.text),{cancelled:false,answers:[{question_index:0,question:'Inspection checkpoint?',header:'Inspect',kind:'option',answer:'Continue'}]});
      const toolMessage=inspection.inspectionMessage;assert.equal(toolMessage,undefined,'a Tool result does not manufacture a Message inspection head');
      assert.equal(session.state.agents.find(agent=>agent.agent_id===child.agent_id)?.state,'active','Tool C result access never claims Child settlement');
      const assistantRecord=childTrace.page.records.find(record=>record.kind==='assistant'&&record.message_id===proposal.id);
      assert.ok(assistantRecord,'the original Child Assistant has its canonical Trace record');
      await inspection.inspectTrace(assistantRecord.id);assert.equal(inspection.error,undefined);
      const message=inspection.inspectionMessage;assert.ok(message);
      const blocks=message.collections.findIndex(frontier=>frontier.domain.endsWith('.blocks'));assert.ok(blocks>=0);
      for(let index=0;index<=blocks;index++)await inspection.selectTraceCollection();
      assert.equal(inspection.error,undefined);const collection=inspection.inspection?.collection;assert.ok(collection);
      const block=collection.items.find(item=>item.identity==='tool_call');assert.ok(block,'the original Tool call remains selectable through canonical blocks');
      const sourceIndex=inspection.collectionSources.findIndex(source=>source.type==='inline'&&source.item.offset===block.offset);assert.ok(sourceIndex>=0);
      const base=inspection.deferred.length+inspection.deferredJournal.length+inspection.deferredAttemptIdentities.length+inspection.traceFields.length;
      await inspection.loadContent(base+sourceIndex);
      assert.equal(inspection.error,undefined);assert.ok(inspection.content);assert.equal(inspection.content.next,undefined);
      const originalBlock:unknown=JSON.parse(inspection.content.text);
      assert.ok(originalBlock&&typeof originalBlock==='object'&&'type' in originalBlock&&'id' in originalBlock);assert.equal(originalBlock.type,'tool_call');assert.equal(originalBlock.id,result.tool_call_id);
      const payload=block.content.find(binding=>binding.content_ref.source.type==='canonical_message_block'&&binding.content_ref.source.component==='payload');assert.ok(payload,'the actual native block advertises its original argument bytes');
      const payloadIndex=inspection.collectionSources.findIndex(source=>source.type==='content'&&JSON.stringify(source.binding)===JSON.stringify(payload));assert.ok(payloadIndex>=0);
      await inspection.loadContent(base+payloadIndex);assert.equal(inspection.error,undefined);assert.ok(inspection.content);assert.equal(inspection.content.next,undefined);
      const argumentsValue:unknown=JSON.parse(inspection.content.text);
      // Compare against the already selected canonical Tool occurrence, rather
      // than manufacturing a whole-block ValueRead source from its page offset.
      const originalCall=proposal.content[result.occurrence.block_index];assert.ok(originalCall?.type==='tool_call');assert.deepEqual(argumentsValue,originalCall.arguments);
      assert.equal((await provider.requests()).length,beforeInspection,'Child selected reads never request execution');
    } finally {inspection.dispose();}
    await provider.releaseGate("child-completion");
    await stateWhen(() => session.state.agents[0]?.state === "inactive");
    const terminal = await session.agentTranscriptPage(child.agent_id);
    assert.ok(terminal);
    const closing = terminal.entries!.find(entry => entry.completed_response !== undefined)!;
    assert.ok(closing); assert.equal(closing.completed_response!.origin.conversation_id, child.child_conversation_id);
    assert.equal(closing.item.type, "message");
    if (closing.item.type === "message") assert.equal(closing.completed_response!.closing_message_id, closing.item.message.id);
    assert.equal(closing.completed_response!.usage?.total_tokens, "132");
    // Explicit one-entry paging proves exact child cursor order over the transport.
    const newest = await host.client.call("agent/transcript", { target: session.target, agent_id: child.agent_id, at: { type: "latest" }, limit: 1 }, "transcript_window");
    assert.equal(newest.window.page.entries!.length, 1); assert.ok(newest.window.page.next_cursor);
    const older = await host.client.call("agent/transcript", { target: session.target, agent_id: child.agent_id, at: { type: "older", before: newest.window.page.next_cursor!, cut: newest.window.cut }, limit: 1 }, "transcript_window");
    assert.ok(BigInt(older.window.page.entries![0]!.cursor) < BigInt(newest.window.page.entries![0]!.cursor));
    await session.respondInteraction(primary.interaction, { ...answer, response: { type: "submitted", value: { answers: [...answer.response.value.answers] } } });
    await stateWhen(() => session.state.attempt?.phase.type === "settled");
    const id = session.sessionId; const node = session.nodeId;
    await session.detach();
    if (external) {
      await host.shutdown();
      host = await AppServerHost.connectRemote({ endpoint: external.endpoint, token: TRANSPORT_TOKEN });
      const replacementCall = host.client.call.bind(host.client);
      t.mock.method(host.client, "call", (...args: Parameters<typeof replacementCall>) => { sent.push(args[0]); return replacementCall(...args); });
    }
    session = await host.attach(id, node);
    const restored = await session.agentTranscriptPage(child.agent_id);
    assert.deepEqual(restored, terminal, "reattach reconstructs from native child authority");
    const database = join(server.runtimeRoot, "sessions", session.sessionId, "conversations", child.child_conversation_id, "conversation.sqlite");
    renameSync(database, `${database}.held`);
    try {
      await assertInvalidTranscriptLimits(child.agent_id);
      await assertInvalidTranscriptLimits("unknown-child");
      await assert.rejects(session.agentTranscriptPage(child.agent_id), error => error instanceof Error && "kind" in error && error.kind === "agent_history_unavailable");
      assert.equal(existsSync(database), false, "inspection never creates an empty history");
    } finally { renameSync(`${database}.held`, database); }
    assert.equal(sent.filter(method => method === "turn/start").length, 1);
    assert.equal(sent.filter(method => method === "interaction/respond").length, 2);
    for (const method of ["turn/steer", "agent/interrupt"]) assert.equal(sent.includes(method), false);
  } catch (error) {
    t.diagnostic(`${error instanceof Error ? error.stack : String(error)}\n${JSON.stringify(error)}\n${host.stderrTail().text}\n${external?.diagnostics ?? ""}`);
    throw error;
  } finally {
    await host.shutdown(); await external?.stop(); server.cleanup(); await provider.finish();
  }
});


for (const carrier of ["stdio", "websocket"] as const) it(`native Agent interrupt ${carrier}: captured settlement, same identity resume, inactive no-op`, { skip: SKIP, timeout: 120_000 }, async (t) => {
  const provider = await ProviderEmulator.start("tui_agent_interrupt");
  const server = ServerFixture.create("rustx-agent-interrupt-", provider.url());
  const workspace = server.workspace("parent");
  writeFileSync(join(workspace, "rustx.toml"), '[agent]\nagents = ["reviewer"]\n');
  mkdirSync(join(workspace, ".agents/agents"), { recursive: true });
  writeFileSync(join(workspace, ".agents/agents/reviewer.toml"), 'description = "Review"\ninstructions = "Inspect the workspace"\n[tools]\nbuiltin = ["read"]\n');
  const external = carrier === "websocket" ? await ExternalAppServer.start(server) : undefined;
  const host = external ? await AppServerHost.connectRemote({ endpoint: external.endpoint, token: TRANSPORT_TOKEN }) : await AppServerHost.spawnLocal({ binary: BINARY, launch: { runtimeRoot: server.runtimeRoot }, env: server.env });
  try {
    const session = await openSession(host, { cwd: workspace });
    await session.submitInbound([{ type: "text", text: "TUI_AGENT_PARENT: delegate a review" }]);
    await Promise.all([provider.awaitGate("interrupt-initial-0"), provider.awaitGate("interrupt-initial-1")]);
    await session.resync();
    const first = session.state.agents[0]!;
    assert.equal(first.state, "active");
    const interrupted = await session.interruptAgent(first.agent_id);
    assert.equal(interrupted.type, "agent_interruption_accepted");
    assert.equal(interrupted.activation_id, first.activation_id);
    assert.equal('outcome' in interrupted, false, "intent does not manufacture physical completion");
    // Interruption cancels exactly the captured activation. agent/wait waits
    // for the current activation, and an Agent already inactive answers at
    // once with none: a wait admitted after the cancelled activation settled
    // reports no activation, and that activation's own record (asserted
    // below) carries the outcome. A wait that captured it reports it.
    const cancelled = await session.waitAgent(first.agent_id);
    if (cancelled.activation_id !== null) {
      assert.equal(cancelled.activation_id, first.activation_id);
      assert.equal(cancelled.outcome, "cancelled");
    } else {
      assert.equal(cancelled.outcome, null);
    }
    assert.equal(interrupted.agent_id, first.agent_id);
    await Promise.all([provider.releaseGate("interrupt-initial-0"), provider.releaseGate("interrupt-initial-1")]);
    await session.resync();
    // The execution receipt and native display publication have separate
    // owners. An exact checkpoint may precede the registry's inactive fact;
    // wait for its contiguous native event instead of assuming a newer cut.
    if (!session.state.agents.some(agent => agent.agent_id === first.agent_id
      && agent.activation_id === first.activation_id && agent.state === "inactive")) {
      await new Promise<void>(resolve => {
        const unsubscribe = session.onState(() => {
          if (session.state.agents.some(agent => agent.agent_id === first.agent_id
            && agent.activation_id === first.activation_id && agent.state === "inactive")) {
            unsubscribe(); resolve();
          }
        });
      });
    }
    const settled = session.state.agents.find(agent => agent.agent_id === first.agent_id);
    assert.equal(settled?.state, "inactive");
    assert.equal(settled?.activation_id, first.activation_id);
    assert.equal(settled?.activation_state, "cancelled");
    await provider.awaitGate("interrupt-parent-notice");
    await provider.releaseGate("interrupt-parent-notice");
    if (!JSON.stringify(session.state.transcript).includes("Parent observed interruption.")) {
      await new Promise<void>(resolve => {
        const unsubscribe = session.onState(() => {
          if (JSON.stringify(session.state.transcript).includes("Parent observed interruption.")) { unsubscribe(); resolve(); }
        });
      });
    }
    const resumed = await session.sendMessage(first.agent_id, "TUI_AGENT_RESUME: continue the review");
    assert.equal(resumed.resumed, true);
    assert.notEqual(resumed.activation_id, first.activation_id);
    await provider.awaitGate("interrupt-resumed");
    await session.resync();
    assert.equal(session.state.agents[0]?.agent_id, first.agent_id);
    assert.equal(session.state.agents[0]?.child_conversation_id, first.child_conversation_id);
    const waiting = session.waitAgent(first.agent_id);
    await provider.releaseGate("interrupt-resumed");
    const completed = await waiting;
    assert.equal(completed.activation_id, resumed.activation_id);
    assert.equal(completed.outcome, "succeeded");
    const inactive = await session.interruptAgent(first.agent_id);
    assert.equal(inactive.activation_id, null);
    assert.equal("outcome" in inactive, false);
    assert.equal(inactive.agent_id, first.agent_id);
    // Await the canonical parent report through its native stream, not a timer.
    if (!JSON.stringify(session.state.transcript).includes("Parent received resumed report.")) {
      await new Promise<void>(resolve => {
        const unsubscribe = session.onState(() => {
          if (JSON.stringify(session.state.transcript).includes("Parent received resumed report.")) { unsubscribe(); resolve(); }
        });
      });
    }
  } catch (error) {
    t.diagnostic(`${error instanceof Error ? error.stack : String(error)}\n${JSON.stringify(error)}\n${host.stderrTail().text}\n${external?.diagnostics ?? ""}`);
    throw error;
  } finally {
    await host.shutdown(); await external?.stop(); server.cleanup(); await provider.finish();
  }
});

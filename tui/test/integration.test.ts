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
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { createInterface } from "node:readline";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { after, before, describe, it } from "node:test";

import { parseArguments } from "../src/cli.ts";
import { prepareStartup } from "../src/startup.ts";

import { AppServerHost } from "../src/app-server/host.ts";
import { AppServerClient, UncertainOutcomeError } from "../src/app-server/client.ts";
import { AppServerChild } from "../src/app-server/child-process.ts";
import { StdioTransport } from "../src/app-server/stdio-transport.ts";
import { WebSocketTransport } from "../src/app-server/websocket-transport.ts";
import { TransportClosedError } from "../src/app-server/transport.ts";
import type { AppServerSession } from "../src/app-server/session.ts";
import type { SessionSettings } from "../src/protocol/app-server.ts";
import { ProviderEmulator } from "./support/provider-emulator.ts";
import { TempFixture } from "./support/temp-fixture.ts";
import { until } from "./support/app-server-peer.ts";

/** The cargo target directory, overridable for a non-default layout. */
const BINARY =
  process.env.RUSTX_BINARY ??
  fileURLToPath(new URL("../../target/debug/rustx", import.meta.url));

const SKIP = existsSync(BINARY)
  ? (await ProviderEmulator.available())
    ? undefined
    : "uv is not installed; the shared provider emulator cannot run"
  : `the rustx binary is not built at ${BINARY}; run \`cargo build --bin rustx\``;

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
    writeFileSync(join(workspace, "rustx.toml"), '[agent.tools]\nbuiltin = ["read", "write", "edit", "glob", "grep", "bash", "execution"]\n[agent.plugins.todo]\nenabled = true\n[agent.plugins.agent_status]\nenabled = true\n');
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

  private constructor(child: ChildProcessWithoutNullStreams, endpoint: string) {
    this.#child = child;
    this.endpoint = endpoint;
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
  return host.attach(created.session.id, created.session.active_node);
}

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
      const stop = host.client.onNotification((message) => {
        a.applyNotification(message);
        b.applyNotification(message);
      });

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
        page.surfaceRevision, page.boundaries[0]!.message.id);
      await assert.rejects(host.attach(a.sessionId, branched.session.active_node), /explicit unload confirmation/);
      const branch = await host.openNode(a.sessionId, branched.session.active_node);
      assert.notEqual(branch.target.conversation_id, a.target.conversation_id);
      assert.equal(branch.nodeId, branched.session.active_node);
      const back = await host.openNode(a.sessionId, original.active_node);
      assert.equal(back.target.conversation_id, a.target.conversation_id);
      assert.notEqual(back.target.runtime_incarnation, a.target.runtime_incarnation,
        "confirmed node changes use server-owned unload/recovery");
      assert.equal(b.released, false, "another Session's attachment is untouched");
      stop();
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

  it("keeps the server and its accepted work alive across disconnect and reconnect", { timeout: 120_000 }, async () => {
    const first = await AppServerHost.connectRemote({
      endpoint: external.endpoint,
      token: TRANSPORT_TOKEN,
    });
    const settings = server.settings("remote-session");
    const session = await openSession(first, settings);
    const sessionId = session.sessionId;
    const incarnation = session.target.runtime_incarnation;
    const stop = first.client.onNotification((m) => session.applyNotification(m));

    await session.submitInbound([{ type: "text", text: "tui multi-session: session A long task" }]);
    await provider.awaitGate("session-a-holding");

    // Exiting a remote TUI is a disconnect and nothing more.
    stop();
    await first.shutdown();
    assert.ok(external.running, "TUI exit never stops an external App Server");

    // Reconnect: a new transport, a new initialize, a new attachment, and the
    // authoritative state — not a reconstruction from what the old client held.
    const second = await AppServerHost.connectRemote({
      endpoint: external.endpoint,
      token: TRANSPORT_TOKEN,
    });
    try {
      const reattached = await second.attach(sessionId);
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
      const capabilities = await session.capabilities();
      const boundaries = await session.boundaries();
      await session.resync();
      return {
        // Everything compared here is server-authoritative and
        // transport-independent by construction.
        named: renamed.name,
        listedSelf: listed.sessions.some((row) => row.id === session.sessionId),
        conversation: session.state.conversationId === session.target.conversation_id,
        capabilityRevisionIsExact: /^(0|[1-9][0-9]*)$/.test(capabilities.revision),
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
            const snapshot = await host.client.call("session/snapshot", { target: session.target }, "snapshot");
            assert.deepEqual(snapshot.snapshot.pending_interactions, []);
            assert.deepEqual(snapshot.snapshot.background, []);
            assert.deepEqual(snapshot.snapshot.subagents, []);
            assert.deepEqual(snapshot.snapshot.workflows.runs, []);
            await host.detach(session.sessionId);
            const attached = await host.attach(session.sessionId);
            await attached.unload();
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

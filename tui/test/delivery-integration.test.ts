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
import { chmodSync, existsSync, mkdirSync, readFileSync, readdirSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { createInterface } from "node:readline";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { after, before, describe, it } from "node:test";

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

  get runtimeRoot(): string {
    return this.temp.path("runtime");
  }
}

async function deliver(host: AppServerHost, cwd: string): Promise<AppServerSession> {
  const created = await host.createSession({ cwd });
  const session = await host.attach(created.session.id, created.session.active_node);
  await session.submitInbound([{ type: "text", text: "tui present: deliver the report" }]);
  await until(() => session.state.attempt?.phase.type === "settled", "the present turn settles");
  return session;
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
    fixture?.temp.cleanup();
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
      const session = await deliver(host, workspace);
      assert.equal(session.deliveryAccess, true, "the owned stdio child delegates delivery access");
      const requests = (await provider.requests()).length;

      // The card and /files consume the same committed typed facts.
      const correlation = correlateTools(session.state);
      const present = [...correlation.byCallId.values()].find((tool) => tool.toolId === "tool-present");
      assert.ok(present?.resultCommitted, "the presented card reads the committed canonical result");
      const card = plainText(renderToolCard(present, {
        expanded: true,
        budget: { maxLines: DEFAULT_PREVIEW_LINES, maxChars: DEFAULT_PREVIEW_CHARS },
      }));
      assert.match(card, /Delivered 2 files/);
      assert.ok(card.indexOf(REPORT) < card.indexOf("data set.csv"), "canonical order");
      const page = await session.deliveryPage();
      assert.deepEqual(page.records.map((record) => [record.file.name, record.index, record.file.description ?? null]), [
        [REPORT, 0, "Final report"],
        ["data set.csv", 1, null],
      ]);
      const [report, data] = page.records;
      assert.ok(report && data);

      // Save: original bytes, Unicode/spaces name, client-local destination.
      const saved = await saveDelivery(
        () => session.readDelivery(report),
        deliveryDestination(`saved/${report.file.name}`, fixture.temp.path(".")),
      ).catch((error: unknown) => error);
      assert.ok(saved instanceof Error, "a missing destination directory fails before writing");
      mkdirSync(fixture.temp.path("saved"));
      const destination = deliveryDestination(`saved/${report.file.name}`, fixture.temp.path("."));
      assert.deepEqual(await saveDelivery(() => session.readDelivery(report), destination), { path: destination });
      assert.deepEqual(readFileSync(destination), REPORT_BYTES);
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
    fixture?.temp.cleanup();
    await provider?.finish();
  });

  it("shows metadata without the delivery credential and saves client-side with it", { timeout: 120_000 }, async () => {
    const ordinary = await AppServerHost.connectRemote({ endpoint: server.endpoint, token: TRANSPORT_TOKEN });
    let sessionId: string;
    try {
      const session = await deliver(ordinary, fixture.workspace("remote"));
      sessionId = session.sessionId;
      assert.equal(session.deliveryAccess, false);
      const [report] = (await session.deliveryPage()).records;
      assert.equal(report?.file.name, REPORT, "metadata is visible to an ordinary client");
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
      await saveDelivery(() => session.readDelivery(report), destination);
      assert.deepEqual(readFileSync(destination), REPORT_BYTES);
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

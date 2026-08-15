/**
 * The real rustX child: TypeScript client -> actual `rustx` binary.
 *
 * ```text
 * RuntimeClientSession
 *   -> RuntimeClientConnection      (real JSONL framing)
 *   -> ChildRuntimeProcess          (real OS process)
 *   -> rustx --models ... --session ... --workspace ... --runtime-root ...
 *   -> real Runtime Client Protocol v1
 *   -> real local runtime composition (#42)
 * ```
 *
 * A fake protocol process alone would not prove this client speaks the bytes
 * rustX actually writes. The model provider is the shared external emulator
 * (issue #47), so the runtime exercises its own adapter, its own credential
 * resolution, and its own streaming path with no network and no credential in
 * CI — and the TUI owns no provider protocol of its own.
 *
 * Readiness is protocol synchronization throughout: the client writes a
 * request and awaits its correlated response, or awaits an event. Nothing
 * sleeps, and no ordering is established by a timer.
 *
 * The suite skips itself with a clear reason when the binary has not been
 * built or the provider emulator's toolchain is missing, so a partial
 * checkout still runs the rest of the tests.
 */

import assert from "node:assert/strict";
import { existsSync, mkdtempSync, writeFileSync, mkdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { after, before, describe, it } from "node:test";

import { ChildRuntimeProcess } from "../src/runtime/child-process.ts";
import { RuntimeClientConnection } from "../src/runtime/connection.ts";
import { RuntimeClientSession } from "../src/runtime/session.ts";
import { CommandDispatcher } from "../src/commands/dispatcher.ts";
import { ProviderEmulator } from "./support/provider-emulator.ts";
import { until } from "./support/scripted-peer.ts";

/** The cargo target directory, overridable for a non-default layout. */
const BINARY =
  process.env.RUSTX_BINARY ??
  fileURLToPath(new URL("../../target/debug/rustx", import.meta.url));

const SKIP = existsSync(BINARY)
  ? (await ProviderEmulator.available())
    ? undefined
    : "uv is not installed; the shared provider emulator cannot run"
  : `the rustx binary is not built at ${BINARY}; run \`cargo build --bin rustx\``;

const CREDENTIAL_VARIABLE = "RUSTX_TUI_INTEGRATION_KEY";
const CREDENTIAL_VALUE = "integration-secret";

function modelsJson(baseUrl: string): string {
  return JSON.stringify({
    providers: {
      fixture: {
        baseUrl,
        // The runtime resolves this from its environment. The client never
        // reads, defaults, or forwards a credential value.
        apiKey: `$${CREDENTIAL_VARIABLE}`,
        models: [
          {
            id: "integration-model",
            protocol: "openai_chat_completions",
            contextWindow: 128_000,
            maxOutputTokens: 512,
            capabilities: {
              inputModalities: ["text"],
              outputModalities: ["text"],
              toolCalls: true,
              reasoning: false,
            },
            compat: { chatReasoningReplay: "omit" },
            requestParams: { temperature: 0.25 },
          },
          {
            id: "second-model",
            protocol: "openai_chat_completions",
            contextWindow: 32_000,
            maxOutputTokens: 256,
            capabilities: {
              inputModalities: ["text"],
              outputModalities: ["text"],
              toolCalls: true,
              reasoning: false,
            },
            compat: { chatReasoningReplay: "omit" },
          },
        ],
      },
    },
  });
}

const SESSION_JSON = JSON.stringify({
  conversationId: "conv-tui-integration",
  agentId: "agent-tui-integration",
  model: { model: "fixture/integration-model" },
  context: { reserveTokens: 1024, keepRecentTokens: 8192 },
});

interface Harness {
  child: ChildRuntimeProcess;
  connection: RuntimeClientConnection;
  session: RuntimeClientSession;
  provider: ProviderEmulator;
}

describe("real rustx child integration", { skip: SKIP }, () => {
  let harness: Harness | undefined;

  before(async () => {
    const provider = await ProviderEmulator.start("tui_integration");
    const root = mkdtempSync(join(tmpdir(), "rustx-tui-"));
    const workspace = join(root, "workspace");
    mkdirSync(workspace, { recursive: true });
    writeFileSync(join(root, "models.json"), modelsJson(provider.url("/v1")));
    writeFileSync(join(root, "session.json"), SESSION_JSON);

    const child = ChildRuntimeProcess.spawn({
      binary: BINARY,
      paths: {
        models: join(root, "models.json"),
        session: join(root, "session.json"),
        workspace,
        runtimeRoot: join(root, "private"),
      },
      // The child performs its own credential resolution from this
      // environment.
      env: { ...process.env, [CREDENTIAL_VARIABLE]: CREDENTIAL_VALUE },
    });

    const connection = new RuntimeClientConnection({
      input: child.stdout,
      output: child.stdin,
    });
    void child
      .wait()
      .then((exit) =>
        connection.reportProcessExit(exit.code, exit.signal, exit.spawnError),
      );

    const session = new RuntimeClientSession({ connection });
    harness = { child, connection, session, provider };
  });

  after(async () => {
    if (harness === undefined) {
      return;
    }
    harness.child.closeStdin();
    await harness.child.waitOrTerminate(10_000);
    // The scenario is asserted on the provider side too: every declared
    // step consumed, in order, with no unexpected request.
    await harness.provider.finish();
  });

  it("completes the whole lifecycle against the real binary", async () => {
    assert.ok(harness);
    const { child, connection, session, provider } = harness;

    // --- spawn + initialize ------------------------------------------------
    const identity = await session.attach();
    assert.equal(identity.conversationId, "conv-tui-integration");
    assert.equal(identity.agentId, "agent-tui-integration");

    const initial = session.state;
    assert.ok(initial);
    assert.equal(
      initial.sessionModel.configured.model,
      "fixture/integration-model",
    );
    assert.equal(initial.sessionModel.effective.contextWindow, 128_000);

    // --- model / capability inspection through the protocol only -----------
    const catalog = await session.modelCatalog();
    const references = (catalog.models ?? []).map((model) => model.model);
    assert.deepEqual(references, [
      "fixture/integration-model",
      "fixture/second-model",
    ]);
    // The catalog exposes the credential *source*, never a value.
    const entry = (catalog.models ?? [])[0];
    assert.deepEqual(entry?.credentialSource, {
      type: "environment",
      variable: CREDENTIAL_VARIABLE,
    });
    assert.ok(!JSON.stringify(catalog).includes(CREDENTIAL_VALUE));

    const capabilities = await session.capabilityGet();
    const toolNames = (capabilities.tools ?? []).map((tool) => tool.name);
    for (const expected of ["bash", "read", "write", "background_task"]) {
      assert.ok(toolNames.includes(expected), `${expected} in ${toolNames}`);
    }

    // --- submit inbound ----------------------------------------------------
    const accepted = await session.submitInbound([
      { type: "text", text: "hello from the tui" },
    ]);
    // Identity and sequence are runtime-assigned; the client invented neither.
    assert.ok(accepted.messageId.length > 0);
    assert.equal(accepted.sequence, 1);

    // --- streaming and commit, observed through the subscription -----------
    await until(
      () =>
        (session.state?.transcript ?? []).some(
          (entry) =>
            entry.kind === "committed" && entry.message.role === "agent",
        ),
      "the assistant message committed",
    );

    const committed = session.state?.transcript.find(
      (entry) => entry.kind === "committed" && entry.message.role === "agent",
    );
    assert.ok(committed?.kind === "committed");
    assert.ok(committed.message.role === "agent");
    const text = committed.message.content
      .filter((block) => block.type === "text")
      .map((block) => (block.type === "text" ? block.text : ""))
      .join("");
    assert.equal(text, "Hello world");

    // --- the attempt really ran, with the model it froze -------------------
    await until(
      () => session.state?.attempt?.phase.type === "settled",
      "the attempt settled",
    );
    const attempt = session.state?.attempt;
    assert.equal(attempt?.model.primary.model, "fixture/integration-model");
    assert.deepEqual(attempt?.phase, {
      type: "settled",
      outcome: { type: "completed", finish_reason: { type: "stop" } },
    });

    // Exactly one provider request, carrying the catalog's own request
    // parameters, which the runtime — not this client — assembled. The
    // emulator asserted the same fields on arrival; this reads back the
    // record it kept.
    const requests = await provider.requests();
    assert.equal(requests.length, 1);
    const body = requests[0]?.body as Record<string, unknown>;
    assert.equal(body.model, "integration-model");
    assert.equal(body.temperature, 0.25);
    assert.deepEqual(requests[0]?.credentialHeaders, ["authorization"]);
    assert.ok(!JSON.stringify(body).includes(CREDENTIAL_VALUE));

    // --- the A -> B invariant against the real runtime ---------------------
    const updated = await session.modelSet({ model: "fixture/second-model" });
    assert.equal(updated.configured.model, "fixture/second-model");
    await until(
      () => session.state?.sessionModel.configured.model === "fixture/second-model",
      "the session model change was published on the stream",
    );
    assert.equal(
      session.state?.attempt?.model.primary.model,
      "fixture/integration-model",
      "the settled attempt still reports the model it ran with",
    );

    // --- inspection commands over the real projection ----------------------
    const dispatcher = new CommandDispatcher({
      session,
      diagnostics: () => ({
        connectionState: "connected",
        childStatus: "running",
        stderrTail: child.stderrTail().text,
        stderrTruncatedBytes: child.stderrTail().truncatedBytes,
        pendingRequests: connection.pendingCount,
        resyncCount: session.resyncCount,
      }),
    });
    for (const command of ["/model", "/tools", "/skills", "/status", "/debug"]) {
      const outcome = await dispatcher.submit(command);
      assert.equal(outcome.kind, "message", command);
      if (outcome.kind === "message") {
        assert.equal(outcome.level, "info", `${command}: ${outcome.text}`);
        assert.ok(
          !outcome.text.includes(CREDENTIAL_VALUE),
          `${command} must never render a credential`,
        );
      }
    }

    // --- resync repairs from the real authoritative snapshot ---------------
    await session.resync();
    assert.equal(session.resyncCount, 1);
    assert.ok(
      (session.state?.transcript ?? []).some(
        (entry) => entry.kind === "committed" && entry.message.role === "agent",
      ),
      "the repaired projection carries the committed history",
    );

    // --- shutdown is not transport closure ---------------------------------
    await session.shutdown();
    const stillReadable = await session.modelGet();
    assert.equal(stillReadable.configured.model, "fixture/second-model");

    // --- stdin EOF, clean exit --------------------------------------------
    child.closeStdin();
    const exit = await child.waitOrTerminate(15_000);
    assert.equal(exit.code, 0, "a clean transport EOF exits successfully");
    assert.equal(exit.signal, null, "no fallback termination was needed");

    // After the process is gone, requests fail immediately rather than hang.
    await assert.rejects(session.modelGet());
  });
});

/**
 * The real rustX child: TypeScript client -> actual `rustx` binary.
 *
 * ```text
 * RuntimeClientAttachment
 *   -> RuntimeClientConnection      (real JSONL framing)
 *   -> ChildRuntimeProcess          (real OS process)
 *   -> rustx --models ... --config ... --workspace ... --runtime-root ...
 *   -> real Runtime Client Protocol v7
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
import { existsSync, writeFileSync, mkdirSync } from "node:fs";
import { join, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { after, before, describe, it } from "node:test";

import { ChildRuntimeProcess } from "../src/runtime/child-process.ts";
import { RuntimeClientConnection } from "../src/runtime/connection.ts";
import { RuntimeClientAttachment } from "../src/runtime/attachment.ts";
import { CommandDispatcher } from "../src/commands/dispatcher.ts";
import { QuestionnaireOverlay } from "../src/ui/components/questionnaire.ts";
import type { RuntimeClientProtocolEvent } from "../src/protocol/types.ts";
import { ProviderEmulator } from "./support/provider-emulator.ts";
import { TempFixture } from "./support/temp-fixture.ts";
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

const PROJECT_INSTRUCTIONS = "# Project\n\nthe workspace instruction file\n";
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

const RUNTIME_CONFIG_JSON = JSON.stringify({
  schemaVersion: 3,
  agentId: "agent-tui-integration",
  model: { model: "fixture/integration-model" },
  context: { reserveTokens: 1024, keepRecentTokens: 8192 },
});

const BEFORE_START_RUNTIME_CONFIG_JSON = JSON.stringify({
  schemaVersion: 3,
  agentId: "agent-tui-before-start",
  model: { model: "fixture/integration-model" },
  context: { reserveTokens: 1024, keepRecentTokens: 8192 },
  // Requiring approval gives the test a deterministic pre-tool boundary. The
  // client cancels while the runtime is waiting there, before Bash can start.
  nativeTools: { bash: { approval: "always" } },
  defaultTools: ["bash"],
});

interface Harness {
  child: ChildRuntimeProcess;
  connection: RuntimeClientConnection;
  session: RuntimeClientAttachment;
  provider: ProviderEmulator;
}

describe("real rustx child integration", { skip: SKIP }, () => {
  let harness: Harness | undefined;
  let fixture: TempFixture | undefined;

  before(async () => {
    const provider = await ProviderEmulator.start("tui_integration");
    fixture = TempFixture.create("rustx-tui-");
    const workspace = fixture.path("workspace");
    mkdirSync(workspace, { recursive: true });
    // A real project instruction file, so the resource projection is proven
    // against what the runtime actually loaded rather than a fixture.
    writeFileSync(join(workspace, "AGENTS.md"), PROJECT_INSTRUCTIONS);
    writeFileSync(fixture.path("models.jsonc"), modelsJson(provider.url("/v1")));
    writeFileSync(fixture.path("rustx.jsonc"), RUNTIME_CONFIG_JSON);

    const child = ChildRuntimeProcess.spawn({
      binary: BINARY,
      paths: {
        models: fixture.path("models.jsonc"),
        config: fixture.path("rustx.jsonc"),
        workspace,
        runtimeRoot: fixture.path("private"),
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

    const session = new RuntimeClientAttachment({ connection });
    harness = { child, connection, session, provider };
  });

  after(async () => {
    if (harness !== undefined) {
      harness.child.closeStdin();
      await harness.child.waitOrTerminate(10_000);
      // The scenario is asserted on the provider side too: every declared
      // step consumed, in order, with no unexpected request.
      await harness.provider.finish();
    }
    // The owned root is removed after the child process is gone — on pass
    // AND failure, because the `after` hook always runs.
    fixture?.cleanup();
  });

  it("completes the whole lifecycle against the real binary", async () => {
    assert.ok(harness);
    const { child, connection, session, provider } = harness;

    // --- spawn + initialize ------------------------------------------------
    const identity = await session.attach();
    assert.equal(identity.conversationId, "conversation-1");
    assert.equal(identity.agentId, "agent-tui-integration");

    const initial = session.state;
    assert.ok(initial);
    assert.equal(
      initial.sessionModel.configured.model,
      "fixture/integration-model",
    );
    assert.equal(initial.sessionModel.effective.contextWindow, 128_000);

    // The runtime names the project instruction files it loaded, by path and
    // by exact byte length. The client never reads the file to find out.
    const contextFiles = initial.resources.context_files ?? [];
    const instructions = contextFiles.find((file) =>
      file.path.endsWith(`${sep}AGENTS.md`),
    );
    assert.ok(
      instructions,
      `AGENTS.md in the published generation: ${JSON.stringify(initial.resources)}`,
    );
    assert.equal(
      instructions.bytes,
      Buffer.byteLength(PROJECT_INSTRUCTIONS, "utf8"),
    );

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
            entry.kind === "committed" && entry.message.role === "assistant",
        ),
      "the assistant message committed",
    );

    const committed = session.state?.transcript.find(
      (entry) => entry.kind === "committed" && entry.message.role === "assistant",
    );
    assert.ok(committed?.kind === "committed");
    assert.ok(committed.message.role === "assistant");
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
    for (const command of [
      "/model show",
      "/tools",
      "/skills",
      "/todos",
      "/status",
      "/debug",
    ]) {
      const outcome = await dispatcher.submit(command);
      assert.equal(outcome.kind, "inspect", command);
      if (outcome.kind === "inspect") {
        assert.ok(outcome.body.length > 0, `${command} must have inspection content`);
        assert.ok(
          !outcome.body.includes(CREDENTIAL_VALUE),
          `${command} must never render a credential`,
        );
      }
    }

    // `/model` with no argument opens the selector over the real catalog the
    // runtime published; it opens no inspection and sends no model_set.
    const chooser = await dispatcher.submit("/model");
    assert.equal(chooser.kind, "choose_model");
    if (chooser.kind === "choose_model") {
      assert.ok(
        chooser.models.length > 0,
        "the runtime catalog reached the selector",
      );
    }

    // --- resync repairs from the real authoritative snapshot ---------------
    await session.resync();
    assert.equal(session.resyncCount, 1);
    assert.ok(
      (session.state?.transcript ?? []).some(
        (entry) => entry.kind === "committed" && entry.message.role === "assistant",
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

// ---------------------------------------------------------------------------
// BeforeStart cancellation through the real Runtime Client/TUI boundary
// ---------------------------------------------------------------------------

describe("real rustx BeforeStart cancellation projection", { skip: SKIP }, () => {
  let harness: Harness | undefined;
  let fixture: TempFixture | undefined;
  const wireEvents: RuntimeClientProtocolEvent[] = [];
  let removeEventListener: (() => void) | undefined;

  before(async () => {
    const provider = await ProviderEmulator.start("tui_before_start_cancellation");
    fixture = TempFixture.create("rustx-tui-before-start-");
    const workspace = fixture.path("workspace");
    mkdirSync(workspace, { recursive: true });
    writeFileSync(fixture.path("models.json"), modelsJson(provider.url("/v1")));
    writeFileSync(
      fixture.path("rustx.json"),
      BEFORE_START_RUNTIME_CONFIG_JSON,
    );

    const child = ChildRuntimeProcess.spawn({
      binary: BINARY,
      paths: {
        models: fixture.path("models.json"),
        config: fixture.path("rustx.json"),
        workspace,
        runtimeRoot: fixture.path("private"),
      },
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
    removeEventListener = connection.onEvent((event) => wireEvents.push(event));
    const session = new RuntimeClientAttachment({ connection });
    harness = { child, connection, session, provider };
  });

  after(async () => {
    removeEventListener?.();
    if (harness !== undefined) {
      harness.child.closeStdin();
      await harness.child.waitOrTerminate(10_000);
      await harness.provider.finish();
    }
    fixture?.cleanup();
  });

  it("keeps incremental and fresh snapshot foreground state identical", async () => {
    assert.ok(harness);
    const { session } = harness;
    await session.attach();
    await session.submitInbound([
      { type: "text", text: "cancel before executor start" },
    ]);

    await until(
      () =>
        (session.state?.pendingInteractions ?? []).length === 1 &&
        session.state?.attempt !== undefined,
      "the real runtime to reach the pre-tool approval boundary",
    );
    const admitted = session.state;
    assert.ok(admitted?.attempt);
    const attemptId = admitted.attempt.attemptId;
    assert.equal(admitted.pendingInteractions.length, 1);

    await session.cancelCurrentAttempt();
    const outcome = await session.waitForAttemptSettlement(attemptId);
    assert.deepEqual(outcome, {
      type: "cancelled",
      reason: "user_requested",
    });

    await until(
      () =>
        session.state?.transcript.some(
          (entry) =>
            entry.kind === "committed" &&
            entry.message.role === "tool" &&
            entry.message.tool_call_id === "call-tui-before-start",
        ) === true,
      "the canonical BeforeStart ToolMessage to commit",
    );

    const incremental = session.state;
    assert.ok(incremental?.attempt);
    const incrementalForeground = incremental.attempt.foreground.find(
      (entry) => entry.call_id === "call-tui-before-start",
    );
    assert.ok(incrementalForeground);
    assert.deepEqual(incrementalForeground.state.type, "settled");
    if (incrementalForeground.state.type !== "settled") {
      throw new Error("the incremental foreground slot did not settle");
    }
    assert.deepEqual(incrementalForeground.state.result.status, {
      type: "cancelled",
      reason: "user_requested",
      phase: "before_start",
    });

    const incrementalToolMessage = incremental.transcript.find(
      (entry) =>
        entry.kind === "committed" &&
        entry.message.role === "tool" &&
        entry.message.tool_call_id === "call-tui-before-start",
    );
    assert.ok(incrementalToolMessage?.kind === "committed");
    assert.equal(incrementalToolMessage.message.role, "tool");
    assert.deepEqual(
      incrementalToolMessage.message.result,
      incrementalForeground.state.result,
    );

    const executionStarted = wireEvents.filter(
      (event) => event.event.type === "tool_execution_started",
    );
    const executionSettled = wireEvents.filter(
      (event) => event.event.type === "tool_execution_settled",
    );
    assert.equal(
      executionStarted.length,
      0,
      "BeforeStart does not fabricate ToolExecutionStarted",
    );
    assert.equal(
      executionSettled.length,
      1,
      "the canonical commit exposes exactly one client settlement",
    );
    assert.equal(
      wireEvents.filter(
        (event) =>
          event.event.type === "message_committed" &&
          event.event.message.role === "tool",
      ).length,
      1,
      "the canonical ToolMessage is committed exactly once",
    );

    // This is the real Runtime Client snapshot_get response, consumed over
    // the same JSONL connection. No TypeScript-side snapshot is constructed.
    await session.resync();
    const fresh = session.state;
    assert.ok(fresh?.attempt);
    const freshForeground = fresh.attempt.foreground.find(
      (entry) => entry.call_id === "call-tui-before-start",
    );
    assert.ok(freshForeground);
    assert.deepEqual(freshForeground, incrementalForeground);
    assert.deepEqual(fresh.attempt.phase, incremental.attempt.phase);
    const freshToolMessage = fresh.transcript.find(
      (entry) =>
        entry.kind === "committed" &&
        entry.message.role === "tool" &&
        entry.message.tool_call_id === "call-tui-before-start",
    );
    assert.ok(freshToolMessage?.kind === "committed");
    assert.equal(freshToolMessage.message.role, "tool");
    assert.deepEqual(freshToolMessage.message, incrementalToolMessage.message);
  });
});

// ---------------------------------------------------------------------------
// Structured ask_user through the real provider/runtime/client/TUI path
// ---------------------------------------------------------------------------

describe("real rustx structured ask_user questionnaire", { skip: SKIP }, () => {
  let harness: Harness | undefined;
  let fixture: TempFixture | undefined;

  before(async () => {
    const provider = await ProviderEmulator.start("tui_ask_user_questionnaire");
    fixture = TempFixture.create("rustx-tui-questionnaire-");
    const workspace = fixture.path("workspace");
    mkdirSync(workspace, { recursive: true });
    writeFileSync(fixture.path("models.json"), modelsJson(provider.url("/v1")));
    writeFileSync(fixture.path("rustx.json"), RUNTIME_CONFIG_JSON);

    const child = ChildRuntimeProcess.spawn({
      binary: BINARY,
      paths: {
        models: fixture.path("models.json"),
        config: fixture.path("rustx.json"),
        workspace,
        runtimeRoot: fixture.path("private"),
      },
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
    const session = new RuntimeClientAttachment({ connection });
    harness = { child, connection, session, provider };
  });

  after(async () => {
    if (harness !== undefined) {
      harness.child.closeStdin();
      await harness.child.waitOrTerminate(10_000);
      await harness.provider.finish();
    }
    fixture?.cleanup();
  });

  it("publishes one questionnaire, resyncs it, and continues after submission", async () => {
    assert.ok(harness);
    const { child, session } = harness;
    await session.attach();

    await session.submitInbound([
      { type: "text", text: "choose the visual direction" },
    ]);
    await until(
      () =>
        (session.state?.pendingInteractions ?? []).some(
          (interaction) => interaction.kind.type === "questionnaire",
        ),
      "the structured questionnaire to become pending",
    );

    const pending = session.state?.pendingInteractions.find(
      (interaction) => interaction.kind.type === "questionnaire",
    );
    assert.ok(pending);
    assert.equal(pending.kind.type, "questionnaire");
    if (pending.kind.type !== "questionnaire") throw new Error("not a questionnaire");
    assert.equal(pending.kind.questionnaire.questions.length, 2);
    assert.equal(pending.kind.questionnaire.questions[1]?.multi_select, true);
    assert.equal(
      pending.kind.questionnaire.questions[0]?.options[0]?.label,
      "Swiss / Klein blue",
    );

    // The authoritative snapshot reconstructs the same request facts; no
    // client-side draft or echoed prose is needed to restore the overlay.
    await session.resync();
    const reconstructed = session.state?.pendingInteractions.find(
      (interaction) => interaction.id === pending.id,
    );
    assert.ok(reconstructed);
    assert.deepEqual(reconstructed.kind, pending.kind);
    if (reconstructed.kind.type !== "questionnaire") {
      throw new Error("resynchronized interaction is not a questionnaire");
    }

    let submitted:
      | import("../src/protocol/types.ts").QuestionnaireResponse
      | undefined;
    const overlay = new QuestionnaireOverlay({
      interactionId: reconstructed.id,
      questionnaire: reconstructed.kind.questionnaire,
      onSubmit: (response) => {
        submitted = response;
      },
      onDecline: () => assert.fail("the questionnaire should be submitted"),
      onInterrupt: () => assert.fail("the questionnaire should not cancel the attempt"),
    });
    assert.match(overlay.render(56).join("\n"), /Swiss preview/);
    assert.match(overlay.render(120).join("\n"), /Swiss preview/);

    // Select the first authored option, leave the multi-select question
    // unanswered, then submit the partial answer from the review tab.
    overlay.handleInput("\r");
    overlay.handleInput("\t");
    overlay.handleInput("\t");
    overlay.handleInput("\r");
    assert.deepEqual(submitted, {
      type: "submitted",
      value: {
        answers: [
          {
            question_index: 0,
            answer: {
              type: "single_option",
              value: { label: "Swiss / Klein blue" },
            },
          },
        ],
      },
    });

    await session.respondInteraction(reconstructed.id, {
      type: "questionnaire",
      response: submitted!,
    });
    await until(
      () =>
        !(session.state?.pendingInteractions ?? []).some(
          (interaction) => interaction.id === reconstructed.id,
        ),
      "the questionnaire to settle",
    );
    await until(
      () =>
        (session.state?.transcript ?? []).some(
          (entry) =>
            entry.kind === "committed" &&
            entry.message.role === "assistant" &&
            entry.message.content.some(
              (block) =>
                block.type === "text" && block.text === "Questionnaire continued",
            ),
        ),
      "the model continuation after the questionnaire",
    );

    // The first provider call is the structured tool call and the second is
    // the model's next turn with rustX's canonical structured result.
    const requests = await harness.provider.requests();
    assert.equal(requests.length, 2);
    child.closeStdin();
  });
});

// ---------------------------------------------------------------------------
// Repeated compaction over the real stdio transport (Issue #27)
// ---------------------------------------------------------------------------

const COMPACTION_RUNTIME_CONFIG_JSON = JSON.stringify({
  schemaVersion: 3,
  agentId: "agent-tui-compaction",
  model: { model: "fixture/integration-model" },
  // Both compaction budgets carry the reserve, so the shared limit is
  // 56000 - 1536 - 1024 = 53440: above the selected span's estimate (~51k)
  // and below the whole turn-two request estimate (~56k). See the same
  // derivation in tests/issue47_conformance.rs.
  context: { reserveTokens: 1_536, keepRecentTokens: 256 },
});

const TUI_TURN_ONE = "tui compaction: turn one";
const TUI_TURN_TWO = "tui compaction: turn two";
const TUI_TURN_THREE = "tui compaction: turn three";
const FILLER_ONE_MARKER = "tui-compaction-filler-one-marker-39c1";
const FILLER_TWO_MARKER = "tui-compaction-filler-two-marker-84e2";
const SUMMARY_ONE_TEXT =
  "tui summary one: the assistant produced filler report one.";
const SUMMARY_TWO_TEXT =
  "tui summary two: the assistant produced filler report two.";

/**
 * Awaits one exact condition with a wall-clock deadlock bound.
 *
 * The compaction turns stream ~200 KB through two hops (emulator -> child ->
 * client), which the event-loop-only `until` exhausts before; the condition
 * itself stays the exact ordering signal and the deadline only fails a
 * genuine stall.
 */
async function awaitCondition(
  condition: () => boolean,
  what: string,
): Promise<void> {
  const deadline = Date.now() + 30_000;
  while (!condition()) {
    if (Date.now() > deadline) {
      throw new Error(`${what} never became true`);
    }
    await new Promise<void>((resolve) => setImmediate(resolve));
  }
}

/**
 * The small-window catalog of the compaction leg: a 56k-token window with an
 * 8k reserve crosses the soft input limit on the emulator's scripted ~200 KB
 * fillers by construction — the same sizing the Rust conformance suite uses.
 */
function compactionModelsJson(baseUrl: string): string {
  return JSON.stringify({
    providers: {
      fixture: {
        baseUrl,
        apiKey: `$${CREDENTIAL_VARIABLE}`,
        models: [
          {
            id: "integration-model",
            protocol: "openai_chat_completions",
            contextWindow: 56_000,
            maxOutputTokens: 1024,
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

describe("real rustx child repeated compaction", { skip: SKIP }, () => {
  let harness: Harness | undefined;
  let fixture: TempFixture | undefined;

  before(async () => {
    const provider = await ProviderEmulator.start("tui_compaction");
    fixture = TempFixture.create("rustx-tui-compaction-");
    const workspace = fixture.path("workspace");
    mkdirSync(workspace, { recursive: true });
    writeFileSync(
      fixture.path("models.jsonc"),
      compactionModelsJson(provider.url("/v1")),
    );
    writeFileSync(
      fixture.path("rustx.jsonc"),
      COMPACTION_RUNTIME_CONFIG_JSON,
    );

    const child = ChildRuntimeProcess.spawn({
      binary: BINARY,
      paths: {
        models: fixture.path("models.jsonc"),
        config: fixture.path("rustx.jsonc"),
        workspace,
        runtimeRoot: fixture.path("private"),
      },
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

    const session = new RuntimeClientAttachment({ connection });
    harness = { child, connection, session, provider };
  });

  after(async () => {
    if (harness !== undefined) {
      harness.child.closeStdin();
      await harness.child.waitOrTerminate(10_000);
      await harness.provider.finish();
    }
    fixture?.cleanup();
  });

  it("commits two compactions across three turns, observed over real stdio", async () => {
    assert.ok(harness);
    const { session, provider } = harness;
    // The settled-phase signal alone is ambiguous across turns (a settled
    // attempt keeps reporting "settled" until the next attempt starts), so
    // per-turn settlement is observed through the committed assistant count.
    const committedAssistants = () =>
      (session.state?.transcript ?? []).filter(
        (entry) =>
          entry.kind === "committed" && entry.message.role === "assistant",
      ).length;

    await session.attach();
    assert.equal(session.state?.context.compaction_count, 0);

    await session.submitInbound([{ type: "text", text: TUI_TURN_ONE }]);
    await awaitCondition(() => committedAssistants() === 1, "turn one committed");
    assert.equal(
      session.state?.context.compaction_count,
      0,
      "the filling turn never compacts",
    );

    await session.submitInbound([{ type: "text", text: TUI_TURN_TWO }]);
    await awaitCondition(
      () => (session.state?.context.compaction_count ?? 0) >= 1,
      "compaction #1 committed",
    );
    await awaitCondition(() => committedAssistants() === 2, "turn two committed");
    assert.equal(session.state?.context.compaction_count, 1);
    assert.equal(session.state?.context.latest_compaction?.generation, 1);

    await session.submitInbound([{ type: "text", text: TUI_TURN_THREE }]);
    await awaitCondition(
      () => (session.state?.context.compaction_count ?? 0) >= 2,
      "compaction #2 committed",
    );
    await awaitCondition(() => committedAssistants() === 3, "turn three committed");
    assert.equal(session.state?.context.compaction_count, 2);
    assert.equal(session.state?.context.latest_compaction?.generation, 2);

    // The recorded wire: five requests; retired bytes never returned to the
    // provider, and the second summary's span is the already-compacted
    // surface.
    const requests = await provider.requests();
    assert.equal(requests.length, 5);
    const bodies = requests.map((request) => JSON.stringify(request.body));
    assert.ok(bodies[0]?.includes(TUI_TURN_ONE));
    assert.ok(
      bodies[1]?.includes(FILLER_ONE_MARKER) &&
        !bodies[1]?.includes(FILLER_TWO_MARKER),
    );
    assert.ok(
      bodies[2]?.includes(SUMMARY_ONE_TEXT) &&
        bodies[2]?.includes(TUI_TURN_TWO) &&
        !bodies[2]?.includes(FILLER_ONE_MARKER),
      "the first rewritten surface reached the provider",
    );
    assert.ok(
      bodies[3]?.includes(SUMMARY_ONE_TEXT) &&
        bodies[3]?.includes(FILLER_TWO_MARKER) &&
        !bodies[3]?.includes(FILLER_ONE_MARKER),
      "the second compaction's span is the already-compacted surface",
    );
    assert.ok(
      bodies[4]?.includes(SUMMARY_TWO_TEXT) &&
        bodies[4]?.includes(TUI_TURN_THREE) &&
        !bodies[4]?.includes(FILLER_ONE_MARKER) &&
        !bodies[4]?.includes(FILLER_TWO_MARKER) &&
        !bodies[4]?.includes(SUMMARY_ONE_TEXT),
      "the second rewritten surface carries exactly the second summary",
    );

    // The projection carries the continuous truth: both canonical summaries
    // and both retired fillers remain visible as committed history.
    const transcript = JSON.stringify(session.state?.transcript ?? []);
    assert.ok(transcript.includes(SUMMARY_ONE_TEXT));
    assert.ok(transcript.includes(SUMMARY_TWO_TEXT));
    assert.ok(
      transcript.includes(FILLER_ONE_MARKER),
      "the retired filler stays a committed transcript fact",
    );
    assert.ok(transcript.includes(FILLER_TWO_MARKER));
  });
});

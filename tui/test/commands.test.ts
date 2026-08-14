/**
 * The command layer, tested without a terminal.
 *
 * Every command either renders projection state or invokes exactly one
 * canonical Runtime Client operation. These cases assert both halves: what
 * the user sees, and which protocol request (if any) reached the wire.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { SlashCommandAutocompleteProvider, commandPrefix } from "../src/commands/autocomplete.ts";
import { CommandDispatcher } from "../src/commands/dispatcher.ts";
import { COMMANDS, parseCommandLine } from "../src/commands/registry.ts";
import { RuntimeClientConnection } from "../src/runtime/connection.ts";
import { RuntimeClientSession } from "../src/runtime/session.ts";
import { ArgumentError, parseArguments } from "../src/cli.ts";
import {
  attemptModel,
  capabilities,
  sessionModel,
  snapshot,
} from "./support/fixtures.ts";
import { ScriptedPeer } from "./support/scripted-peer.ts";

const NO_DIAGNOSTICS = () => ({
  connectionState: "connected",
  childStatus: "running (pid 1)",
  stderrTail: "",
  stderrTruncatedBytes: 0,
  pendingRequests: 0,
  resyncCount: 0,
});

async function harness(initial = snapshot()) {
  const peer = new ScriptedPeer();
  const connection = new RuntimeClientConnection({
    input: peer.runtimeOutput,
    output: peer.clientOutput,
  });
  const session = new RuntimeClientSession({ connection });
  const attaching = session.attach();
  await peer.awaitRequests(1);
  peer.respond(1, {
    type: "initialized",
    attachment_id: "att-1",
    conversation_id: initial.conversation_id,
    agent_id: "agent-1",
    snapshot: initial,
    cursor: 0,
  });
  await peer.awaitRequests(2);
  peer.respond(2, { type: "subscribed", after_cursor: 0 });
  await attaching;

  const dispatcher = new CommandDispatcher({
    session,
    diagnostics: NO_DIAGNOSTICS,
  });
  return { peer, connection, session, dispatcher };
}

describe("command registry", () => {
  it("declares exactly the bounded command surface", () => {
    assert.deepEqual(
      COMMANDS.map((command) => command.name),
      [
        "/help",
        "/model",
        "/tools",
        "/skills",
        "/status",
        "/debug",
        "/cancel",
        "/quit",
      ],
    );
  });

  it("declares no shell, file, or Skill-execution escape", () => {
    // These would bypass rustX semantics entirely. There is no `!bash`, no
    // `@file` attachment, and no client-side Skill invocation.
    const names = COMMANDS.map((command) => command.name).join(" ");
    for (const forbidden of ["!", "@", "/bash", "/sh", "/run", "/read", "/edit"]) {
      assert.ok(!names.includes(forbidden), `${forbidden} must not exist`);
    }
  });

  it("splits a command line into a name and its argument", () => {
    assert.deepEqual(parseCommandLine("/model"), {
      name: "/model",
      argument: "",
    });
    assert.deepEqual(parseCommandLine("  /model alpha/model-a  "), {
      name: "/model",
      argument: "alpha/model-a",
    });
    assert.equal(parseCommandLine("just a message"), undefined);
    assert.equal(parseCommandLine("what about a / mid-sentence"), undefined);
  });
});

describe("slash-command autocomplete", () => {
  const provider = new SlashCommandAutocompleteProvider();

  it("completes only rustX commands", async () => {
    const suggestions = await provider.getSuggestions(["/mo"], 0, 3, {
      signal: new AbortController().signal,
    });
    assert.ok(suggestions);
    assert.deepEqual(
      suggestions.items.map((item) => item.value),
      ["/model"],
    );
  });

  it("offers the whole table for a bare slash", async () => {
    const suggestions = await provider.getSuggestions(["/"], 0, 1, {
      signal: new AbortController().signal,
    });
    assert.equal(suggestions?.items.length, COMMANDS.length);
  });

  it("never triggers file completion", () => {
    // Pi's CombinedAutocompleteProvider walks the filesystem and may invoke
    // `fd`. This client has no workspace reader by design.
    assert.equal(provider.shouldTriggerFileCompletion(), false);
  });

  it("does not treat prose or an argument position as a command", () => {
    assert.equal(commandPrefix(["hello /model"], 0, 12), undefined);
    assert.equal(commandPrefix(["/model alpha/"], 0, 13), undefined);
    assert.equal(commandPrefix(["line one", "/model"], 1, 6), undefined);
    assert.equal(commandPrefix(["/mod"], 0, 4), "/mod");
  });

  it("applies a completion by replacing the command token", () => {
    const applied = provider.applyCompletion(
      ["/mo extra"],
      0,
      3,
      { value: "/model", label: "/model" },
      "/mo",
    );
    assert.equal(applied.lines[0], "/model  extra");
    assert.equal(applied.cursorCol, "/model ".length);
  });
});

describe("CommandDispatcher", () => {
  it("submits plain text as one inbound message", async () => {
    const { peer, dispatcher } = await harness();
    const submitting = dispatcher.submit("hello runtime");
    const requests = await peer.awaitRequests(3);

    const submit = requests[2];
    assert.equal(submit?.method, "submit_inbound");
    assert.deepEqual(submit?.method === "submit_inbound" ? submit.content : null, [
      { type: "text", text: "hello runtime" },
    ]);

    peer.respond(3, {
      type: "inbound_accepted",
      message_id: "m1",
      inbound_sequence: 1,
    });
    assert.deepEqual(await submitting, { kind: "none" });
  });

  it("renders /help from the command table", async () => {
    const { dispatcher } = await harness();
    const outcome = await dispatcher.submit("/help");
    assert.equal(outcome.kind, "message");
    if (outcome.kind !== "message") {
      return;
    }
    for (const command of COMMANDS) {
      assert.ok(outcome.text.includes(command.name), command.name);
    }
  });

  it("renders /model from the runtime-owned session state", async () => {
    const { dispatcher } = await harness(
      snapshot({ model: sessionModel("alpha/model-a") }),
    );
    const outcome = await dispatcher.submit("/model");
    assert.equal(outcome.kind, "message");
    if (outcome.kind !== "message") {
      return;
    }
    assert.match(outcome.text, /alpha\/model-a/);
    assert.match(outcome.text, /context window: 128000/);
    // No provider endpoint or credential can appear: neither is in the view.
    assert.ok(!/apiKey|api_key|baseUrl/i.test(outcome.text));
  });

  it("shows both models when the session moved past the running attempt", async () => {
    const { dispatcher, session } = await harness(
      snapshot({
        model: sessionModel("beta/model-b"),
        attempt: {
          attempt_id: "a1",
          phase: { type: "running" },
          turn: 1,
          model: attemptModel("alpha/model-a"),
        },
      }),
    );
    assert.equal(session.state?.attempt?.model.primary.model, "alpha/model-a");

    const outcome = await dispatcher.submit("/model");
    assert.equal(outcome.kind, "message");
    if (outcome.kind !== "message") {
      return;
    }
    assert.match(outcome.text, /Active attempt model \(frozen at admission\)/);
    assert.match(outcome.text, /this attempt keeps the model it froze/);
  });

  it("changes the model only through model_catalog_get + model_set", async () => {
    const { peer, dispatcher } = await harness();
    const changing = dispatcher.submit("/model beta/model-b");

    const afterCatalog = await peer.awaitRequests(3);
    assert.equal(
      afterCatalog[2]?.method,
      "model_catalog_get",
      "the client reads the runtime catalog, never models.json",
    );
    peer.respond(3, {
      type: "model_catalog",
      catalog: {
        models: [
          {
            model: "beta/model-b",
            protocol: "openai_chat_completions",
            contextWindow: 32_000,
            maxOutputTokens: 2_048,
            declaredCapabilities: {
              inputModalities: ["text"],
              outputModalities: ["text"],
              toolCalls: true,
              reasoning: false,
            },
            effectiveCapabilities: {
              inputModalities: ["text"],
              outputModalities: ["text"],
              toolCalls: true,
              reasoning: false,
            },
            credentialSource: { environment: { variable: "RUSTX_KEY" } },
          },
        ],
      },
    });

    const afterSet = await peer.awaitRequests(4);
    const modelSet = afterSet[3];
    assert.equal(modelSet?.method, "model_set");
    // A whole-state replacement, never a patch.
    assert.deepEqual(modelSet?.method === "model_set" ? modelSet.config : null, {
      model: "beta/model-b",
    });
    peer.respond(4, { type: "model_set", model: sessionModel("beta/model-b") });

    const outcome = await changing;
    assert.equal(outcome.kind, "message");
    if (outcome.kind === "message") {
      assert.match(outcome.text, /session model is now beta\/model-b/);
    }
  });

  it("rejects a model the runtime catalog does not offer", async () => {
    const { peer, dispatcher } = await harness();
    const changing = dispatcher.submit("/model made/up");
    await peer.awaitRequests(3);
    peer.respond(3, { type: "model_catalog", catalog: { models: [] } });

    const outcome = await changing;
    assert.equal(outcome.kind, "message");
    if (outcome.kind === "message") {
      assert.equal(outcome.level, "error");
      assert.match(outcome.text, /not in the runtime's catalog/);
    }
    // No model_set was attempted for an unknown reference.
    assert.equal(peer.requests.length, 3);
  });

  it("renders /tools generically from the capability projection", async () => {
    const { dispatcher } = await harness(
      snapshot({ capabilities: capabilities(5) }),
    );
    const outcome = await dispatcher.submit("/tools");
    assert.equal(outcome.kind, "message");
    if (outcome.kind !== "message") {
      return;
    }
    assert.match(outcome.text, /capability revision 5/);
    assert.match(outcome.text, /`bash`/);
    assert.match(outcome.text, /mcp:corpus/);
    // Policies come from the runtime; nothing is inferred from the name.
    assert.match(outcome.text, /execution: model_selectable/);
  });

  it("renders /skills from the runtime's Skill projection", async () => {
    const { dispatcher } = await harness(
      snapshot({ capabilities: capabilities(2) }),
    );
    const outcome = await dispatcher.submit("/skills");
    assert.equal(outcome.kind, "message");
    if (outcome.kind === "message") {
      assert.match(outcome.text, /`review` \(skill-review@1\)/);
    }
  });

  it("renders /status from the runtime's own Agent Status composition", async () => {
    const rendered = "## Agent Status\n- current time: 2026-08-14T00:00:00Z";
    const { dispatcher } = await harness(
      snapshot({
        status: {
          attempt_id: "a1",
          turn: 2,
          target_message_id: "m1",
          sections: [],
          rendered,
        },
        inbound: {
          pending: [
            {
              sequence: 3,
              message: {
                id: "m2",
                content: [{ type: "text", text: "queued" }],
                source: "human",
                kind: "message",
              },
            },
          ],
          last_drain: { watermark: 2, count: 2 },
        },
      }),
    );

    const outcome = await dispatcher.submit("/status");
    assert.equal(outcome.kind, "message");
    if (outcome.kind !== "message") {
      return;
    }
    // The rendering is the runtime's, verbatim.
    assert.ok(outcome.text.includes(rendered));
    assert.match(outcome.text, /inbound pending: 1/);
    assert.match(outcome.text, /last drain: watermark 2, 2 item\(s\)/);
  });

  it("renders bounded /debug diagnostics without any credential", async () => {
    const { session } = await harness();
    const dispatcher = new CommandDispatcher({
      session,
      diagnostics: () => ({
        attachmentId: "att-1",
        conversationId: "conv-test",
        agentId: "agent-1",
        cursor: 0,
        connectionState: "connected",
        childStatus: "running (pid 42)",
        stderrTail: "rustx: warning: something bounded",
        stderrTruncatedBytes: 1_024,
        pendingRequests: 0,
        resyncCount: 2,
      }),
    });

    const outcome = await dispatcher.submit("/debug");
    assert.equal(outcome.kind, "message");
    if (outcome.kind !== "message") {
      return;
    }
    assert.match(outcome.text, /attachment: `att-1`/);
    assert.match(outcome.text, /authoritative repairs \(resync\): 2/);
    assert.match(outcome.text, /1024 dropped/);
    assert.ok(!/sk-|api[_-]?key|secret/i.test(outcome.text));
  });

  it("treats attempt cancellation as acceptance", async () => {
    const { peer, dispatcher } = await harness();
    const cancelling = dispatcher.submit("/cancel");
    await peer.awaitRequests(3);
    assert.equal(peer.requests[2]?.method, "cancel_current_attempt");
    peer.respond(3, {
      type: "attempt_cancellation_accepted",
      attempt_id: "a1",
    });

    const outcome = await cancelling;
    assert.equal(outcome.kind, "message");
    if (outcome.kind === "message") {
      assert.match(outcome.text, /acceptance/);
      assert.match(outcome.text, /runtime owns the terminal settlement/);
    }
  });

  it("cancels one background execution by its runtime identity", async () => {
    const { peer, dispatcher } = await harness();
    const cancelling = dispatcher.submit("/cancel exec-7");
    await peer.awaitRequests(3);
    const request = peer.requests[2];
    assert.equal(request?.method, "background_cancel");
    assert.equal(
      request?.method === "background_cancel" ? request.execution_id : null,
      "exec-7",
    );
    peer.respond(3, {
      type: "background_cancel_accepted",
      execution: {
        execution_id: "exec-7",
        tool_id: "tool-background",
        tool_name: "background_task",
        state: "cancelling",
      },
    });

    const outcome = await cancelling;
    if (outcome.kind === "message") {
      assert.match(outcome.text, /acceptance, not settlement/);
    }
  });

  it("surfaces a typed protocol error as an error message", async () => {
    const { peer, dispatcher } = await harness();
    const cancelling = dispatcher.submit("/cancel");
    await peer.awaitRequests(3);
    peer.respondError(3, { type: "no_current_attempt" });

    const outcome = await cancelling;
    assert.equal(outcome.kind, "message");
    if (outcome.kind === "message") {
      assert.equal(outcome.level, "error");
      assert.match(outcome.text, /no attempt is currently cancellable/);
    }
  });

  it("reports /quit as a quit intent rather than acting itself", async () => {
    const { dispatcher } = await harness();
    assert.deepEqual(await dispatcher.submit("/quit"), { kind: "quit" });
  });

  it("rejects an unknown command without reaching the wire", async () => {
    const { peer, dispatcher } = await harness();
    const outcome = await dispatcher.submit("/definitely-not-a-command");
    assert.equal(outcome.kind, "message");
    if (outcome.kind === "message") {
      assert.equal(outcome.level, "error");
    }
    assert.equal(peer.requests.length, 2, "no request was issued");
  });
});

describe("CLI arguments", () => {
  const complete = [
    "--binary",
    "/usr/bin/rustx",
    "--models",
    "/m.json",
    "--session",
    "/s.json",
    "--workspace",
    "/ws",
    "--runtime-root",
    "/private",
  ];

  it("parses the complete argument set", () => {
    const parsed = parseArguments(complete);
    assert.equal(parsed.binary, "/usr/bin/rustx");
    assert.deepEqual(parsed.paths, {
      models: "/m.json",
      session: "/s.json",
      workspace: "/ws",
      runtimeRoot: "/private",
    });
  });

  it("fails explicitly on malformed arguments", () => {
    const cases: string[][] = [
      [],
      ["--models"],
      ["--future", "x"],
      [...complete, "--models", "/again.json"],
    ];
    for (const argv of cases) {
      assert.throws(
        () => parseArguments(argv),
        ArgumentError,
        JSON.stringify(argv),
      );
    }
  });
});

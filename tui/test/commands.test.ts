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
import { CommandDispatcher, renderTools } from "../src/commands/dispatcher.ts";
import { COMMANDS, parseCommandLine } from "../src/commands/registry.ts";
import { emptyPresentationState } from "../src/presentation/projection.ts";
import { RuntimeClientConnection } from "../src/runtime/connection.ts";
import { RuntimeClientAttachment } from "../src/runtime/attachment.ts";
import { TransientFeedbackSurface } from "../src/ui/components/transient-feedback.ts";
import { ArgumentError, parseArguments } from "../src/cli.ts";
import {
  attemptModel,
  approvalInteraction,
  catalogModel,
  capabilities,
  questionInteraction,
  sessionModel,
  sessionView,
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

function deferred<T>(): {
  promise: Promise<T>;
  resolve: (value: T) => void;
} {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((promiseResolve) => {
    resolve = promiseResolve;
  });
  return { promise, resolve };
}

async function harness(initial = snapshot()) {
  const peer = new ScriptedPeer();
  const connection = new RuntimeClientConnection({
    input: peer.runtimeOutput,
    output: peer.clientOutput,
  });
  const session = new RuntimeClientAttachment({ connection });
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
        "/new",
        "/resume",
        "/session",
        "/name",
        "/clone",
        "/fork",
        "/tree",
        "/tools",
        "/skills",
        "/status",
        "/debug",
        "/reasoning",
        "/expand",
        "/cancel",
        "/approve",
        "/answer",
        "/approval",
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

  it("routes ordinary text to a focused Question instead of submit_inbound", async () => {
    const { peer, dispatcher } = await harness(
      snapshot({ pending_interactions: [questionInteraction()] }),
    );
    const responding = dispatcher.submit("production");
    await peer.awaitRequests(3);

    assert.equal(peer.requests[2]?.method, "interaction_respond");
    assert.deepEqual(
      peer.requests[2]?.method === "interaction_respond"
        ? peer.requests[2].response
        : null,
      {
        type: "question",
        answer: { type: "choice", value: "production" },
      },
    );
    assert.equal(
      peer.requests.some((request) => request.method === "submit_inbound"),
      false,
    );
    peer.respond(3, {
      type: "interaction_response_accepted",
      interaction_id: "attempt-1-interaction-question-1",
    });
    assert.equal((await responding).kind, "transient");
  });

  it("routes focused free text and focused approval through typed responses", async () => {
    const openQuestion = questionInteraction("attempt-1-interaction-question-open");
    assert.equal(openQuestion.kind.type, "question");
    if (openQuestion.kind.type !== "question") return;
    const { peer, dispatcher } = await harness(
      snapshot({
        pending_interactions: [
          {
            ...openQuestion,
            kind: {
              type: "question",
              prompt: "What name should I use?",
              allow_free_text: true,
            },
          },
        ],
      }),
    );
    const responding = dispatcher.submit("a private environment");
    await peer.awaitRequests(3);
    assert.deepEqual(
      peer.requests[2]?.method === "interaction_respond"
        ? peer.requests[2].response
        : null,
      {
        type: "question",
        answer: { type: "free_text", value: "a private environment" },
      },
    );
    peer.respond(3, {
      type: "interaction_response_accepted",
      interaction_id: openQuestion.id,
    });
    assert.equal((await responding).kind, "transient");

    const approval = approvalInteraction("attempt-1-interaction-approval-1");
    const second = await harness(snapshot({ pending_interactions: [approval] }));
    const approving = second.dispatcher.submit("deny because it is unsafe");
    await second.peer.awaitRequests(3);
    assert.deepEqual(
      second.peer.requests[2]?.method === "interaction_respond"
        ? second.peer.requests[2].response
        : null,
      {
        type: "approval",
        decision: { type: "deny", reason: "because it is unsafe" },
      },
    );
    second.peer.respond(3, {
      type: "interaction_response_accepted",
      interaction_id: approval.id,
    });
    assert.equal((await approving).kind, "transient");
  });

  it("focuses the lexicographically smallest interaction when several are pending", async () => {
    const first = questionInteraction("attempt-1-interaction-z");
    const second = questionInteraction("attempt-1-interaction-a");
    const { peer, dispatcher } = await harness(
      snapshot({ pending_interactions: [first, second] }),
    );
    const responding = dispatcher.submit("production");
    await peer.awaitRequests(3);
    assert.equal(
      peer.requests[2]?.method === "interaction_respond"
        ? peer.requests[2].interaction_id
        : undefined,
      second.id,
    );
    peer.respond(3, {
      type: "interaction_response_accepted",
      interaction_id: second.id,
    });
    await responding;
  });

  it("does not turn an invalid focused choice into an inbound message", async () => {
    const { peer, dispatcher } = await harness(
      snapshot({ pending_interactions: [questionInteraction()] }),
    );
    const outcome = await dispatcher.submit("custom environment");
    assert.equal(outcome.kind, "transient");
    if (outcome.kind === "transient") {
      assert.equal(outcome.level, "error");
      assert.match(outcome.text, /focused question requires one of/);
    }
    assert.equal(peer.requests.length, 2);
  });

  it("renders /help from the command table", async () => {
    const { dispatcher } = await harness();
    const outcome = await dispatcher.submit("/help");
    assert.equal(outcome.kind, "inspect");
    if (outcome.kind !== "inspect") {
      return;
    }
    assert.equal(outcome.title, "Help");
    for (const command of COMMANDS) {
      assert.ok(outcome.body.includes(command.name), command.name);
    }
  });

  it("renders /model show from the runtime-owned session state", async () => {
    const { dispatcher } = await harness(
      snapshot({ model: sessionModel("alpha/model-a") }),
    );
    const outcome = await dispatcher.submit("/model show");
    assert.equal(outcome.kind, "inspect");
    if (outcome.kind !== "inspect") {
      return;
    }
    assert.equal(outcome.title, "Model");
    assert.match(outcome.body, /alpha\/model-a/);
    assert.match(outcome.body, /context window: 128000/);
    // No provider endpoint or credential can appear: neither is in the view.
    assert.ok(!/apiKey|api_key|baseUrl/i.test(outcome.body));
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

    const outcome = await dispatcher.submit("/model show");
    assert.equal(outcome.kind, "inspect");
    if (outcome.kind !== "inspect") {
      return;
    }
    assert.match(outcome.body, /Active attempt model \(frozen at admission\)/);
    assert.match(outcome.body, /this attempt keeps the model it froze/);
  });

  it("changes the model only through model_catalog_get + model_set", async () => {
    const summaryPolicy = {
      mode: "explicit" as const,
      model: "summary/model-s",
      reasoning_profile: "compact",
      request_params: { summary_tag: "keep" },
      max_output_tokens: 300,
    };
    const { peer, dispatcher } = await harness(
      snapshot({
        model: {
          ...sessionModel("alpha/model-a"),
          configured: {
            model: "alpha/model-a",
            reasoningProfile: "on",
            requestParams: { temperature: 0.2 },
            maxOutputTokens: 777,
            summaryModel: summaryPolicy,
          },
        },
        attempt: {
          attempt_id: "attempt-a",
          phase: { type: "running" },
          turn: 1,
          model: attemptModel("alpha/model-a"),
        },
      }),
    );
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
            defaultReasoningProfile: "off",
            credentialSource: { type: "environment", variable: "RUSTX_KEY" },
          },
        ],
      },
    });

    const afterSet = await peer.awaitRequests(4);
    const modelSet = afterSet[3];
    assert.equal(modelSet?.method, "model_set");
    // A deliberate whole-state replacement: primary overrides reset, while
    // the independent summary policy survives exactly.
    assert.deepEqual(modelSet?.method === "model_set" ? modelSet.config : null, {
      model: "beta/model-b",
      reasoningProfile: "off",
      requestParams: {},
      summaryModel: summaryPolicy,
    });
    peer.respond(4, { type: "model_set", model: sessionModel("beta/model-b") });

    const outcome = await changing;
    assert.equal(outcome.kind, "transient");
    if (outcome.kind === "transient") {
      assert.match(outcome.text, /session model -> beta\/model-b/);
      assert.match(outcome.text, /current attempt remains alpha\/model-a/);
      assert.match(outcome.text, /change applies to next attempt/);
      const surface = new TransientFeedbackSurface();
      surface.replace(outcome);
      const rendered = surface.render(80).join("\n");
      assert.match(rendered, /beta\/model-b/);
      assert.match(rendered, /alpha\/model-a/);
      assert.match(rendered, /next attempt/);
      assert.ok(surface.render(80).length <= 3);
    }
  });

  it("keeps a two-phase model command on its admitted attachment", async () => {
    const catalogStarted = deferred<undefined>();
    const catalogResponse = deferred<{
      models: ReturnType<typeof catalogModel>[];
    }>();
    let aCatalog = 0;
    let aModelSet = 0;
    let bModelSet = 0;
    const sessionA = {
      state: emptyPresentationState(sessionModel("alpha/model-a")),
      modelCatalog: async () => {
        aCatalog += 1;
        catalogStarted.resolve(undefined);
        return catalogResponse.promise;
      },
      modelSet: async () => {
        aModelSet += 1;
        return sessionModel("beta/model-b");
      },
    } as unknown as RuntimeClientAttachment;
    const sessionB = {
      state: emptyPresentationState(sessionModel("alpha/model-a")),
      modelSet: async () => {
        bModelSet += 1;
        return sessionModel("beta/model-b");
      },
    } as unknown as RuntimeClientAttachment;
    const dispatcher = new CommandDispatcher({
      session: sessionA,
      diagnostics: NO_DIAGNOSTICS,
    });

    const changing = dispatcher.submit("/model beta/model-b");
    await catalogStarted.promise;

    // Rebinding changes admission for future invocations while the admitted
    // two-phase command is still waiting on A's catalog response.
    dispatcher.setSession(sessionB);
    catalogResponse.resolve({ models: [catalogModel("beta/model-b")] });

    const outcome = await changing;
    assert.equal(outcome.kind, "transient");
    assert.equal(aCatalog, 1);
    assert.equal(aModelSet, 1, "the admitted command completes on A");
    assert.equal(bModelSet, 0, "the admitted command must never retarget B");
  });

  it("rejects a model the runtime catalog does not offer", async () => {
    const { peer, dispatcher } = await harness();
    const changing = dispatcher.submit("/model made/up");
    await peer.awaitRequests(3);
    peer.respond(3, { type: "model_catalog", catalog: { models: [] } });

    const outcome = await changing;
    assert.equal(outcome.kind, "transient");
    if (outcome.kind === "transient") {
      assert.equal(outcome.level, "error");
      assert.match(outcome.text, /not in the runtime's catalog/);
    }
    // No model_set was attempted for an unknown reference.
    assert.equal(peer.requests.length, 3);
  });

  it("opens the model selector from the runtime catalog", async () => {
    const { peer, dispatcher } = await harness();
    const choosing = dispatcher.submit("/model");
    await peer.awaitRequests(3);
    assert.equal(
      peer.requests[2]?.method,
      "model_catalog_get",
      "the selector reads the runtime catalog, never models.json",
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
            credentialSource: { type: "environment", variable: "RUSTX_KEY" },
          },
        ],
      },
    });

    const outcome = await choosing;
    assert.equal(outcome.kind, "choose_model");
    if (outcome.kind === "choose_model") {
      assert.deepEqual(
        outcome.models.map((model) => model.model),
        ["beta/model-b"],
      );
    }
    // Opening the selector sends no model_set.
    assert.equal(peer.requests.length, 3);
  });

  it("reads authoritative Session metadata for /session", async () => {
    const { peer, dispatcher } = await harness();
    const reading = dispatcher.submit("/session");
    await peer.awaitRequests(3);
    assert.equal(peer.requests[2]?.method, "session_get");
    peer.respond(3, { type: "session", session: sessionView({ name: "review" }) });

    const outcome = await reading;
    assert.equal(outcome.kind, "inspect");
    if (outcome.kind === "inspect") {
      assert.equal(outcome.title, "Session");
      assert.match(outcome.body, /session review/);
      assert.match(outcome.body, /active node node-1/);
      assert.match(outcome.body, /conversation conv-test/);
    }
  });

  it("lists persisted Sessions for /resume and does not choose in the client", async () => {
    const { peer, dispatcher } = await harness();
    const resuming = dispatcher.submit("/resume");
    await peer.awaitRequests(3);
    assert.equal(peer.requests[2]?.method, "session_list");
    peer.respond(3, {
      type: "session_list",
      sessions: [
        {
          id: "session-1",
          name: "current",
          updated_at: "2026-08-21T00:00:00Z",
          active_node: "node-1",
          active: true,
        },
        {
          id: "session-2",
          name: "saved review",
          updated_at: "2026-08-20T00:00:00Z",
          active_node: "node-2",
          active: false,
        },
      ],
    });

    const outcome = await resuming;
    assert.equal(outcome.kind, "choose_session");
    if (outcome.kind === "choose_session") {
      assert.deepEqual(outcome.sessions.map((session) => session.id), [
        "session-1",
        "session-2",
      ]);
    }
  });

  it("turns native Session selection failures into error outcomes", async () => {
    const { peer, connection, dispatcher } = await harness();
    const boundary = {
      surface_revision: 4,
      message: {
        id: "user-c",
        content: [{ type: "text" as const, text: "try again" }],
        source: "human" as const,
        kind: "message" as const,
      },
    };

    const selecting = dispatcher.selectSession("missing");
    await peer.awaitRequests(3);
    peer.respondError(3, { type: "session_failure", message: "unknown session" });
    assert.deepEqual(await selecting, {
      kind: "transient",
      level: "error",
      text: "session operation failed: unknown session",
    });

    const selectingNode = dispatcher.selectTreeNode("session-1", "missing-node");
    await peer.awaitRequests(4);
    peer.respondError(4, { type: "session_failure", message: "unknown node" });
    assert.equal((await selectingNode).kind, "transient");

    const forking = dispatcher.forkAt(boundary);
    await peer.awaitRequests(5);
    peer.respondError(5, { type: "session_failure", message: "stale boundary" });
    assert.equal((await forking).kind, "transient");

    const branching = dispatcher.branchAt(boundary);
    await peer.awaitRequests(6);
    peer.respondError(6, { type: "session_failure", message: "catalog failure" });
    assert.equal((await branching).kind, "transient");

    // A semantic Session failure is a healthy protocol response, so the TUI
    // connection remains usable for the next overlay request.
    assert.equal(connection.closed, undefined);
  });

  it("creates a new Session through the native control request", async () => {
    const { peer, dispatcher } = await harness();
    const creating = dispatcher.submit("/new");
    await peer.awaitRequests(3);
    assert.equal(peer.requests[2]?.method, "session_new");
    peer.respond(3, {
      type: "session_changed",
      session: sessionView({ id: "session-2", name: "New session" }),
      restart_required: true,
    });

    const outcome = await creating;
    assert.equal(outcome.kind, "session_switch");
    if (outcome.kind === "session_switch") {
      assert.equal(outcome.change.session.id, "session-2");
      assert.equal(outcome.change.restartRequired, true);
    }
  });

  it("renames Session metadata without emitting a conversation message", async () => {
    const { peer, dispatcher } = await harness();
    const renaming = dispatcher.submit("/name design review");
    await peer.awaitRequests(3);
    assert.deepEqual(peer.requests[2], {
      method: "session_name",
      id: peer.requests[2]?.id,
      name: "design review",
    });
    peer.respond(3, {
      type: "session_changed",
      session: sessionView({ name: "design review" }),
      restart_required: false,
    });

    const outcome = await renaming;
    assert.equal(outcome.kind, "transient");
    if (outcome.kind === "transient") {
      assert.match(outcome.text, /session renamed to design review/);
    }
    assert.equal(
      peer.requests.some((request) => request.method === "submit_inbound"),
      false,
    );
  });

  it("returns native fork boundaries for the Pi-style picker", async () => {
    const { peer, dispatcher } = await harness();
    const forking = dispatcher.submit("/fork");
    await peer.awaitRequests(3);
    assert.equal(peer.requests[2]?.method, "session_tree_get");
    peer.respond(3, {
      type: "session_tree",
      session: sessionView(),
      nodes: [],
      branchable_messages: [
        {
          surface_revision: 3,
          message: {
            id: "user-c",
            content: [{ type: "text", text: "C" }],
            source: "human",
            kind: "message",
          },
        },
      ],
    });

    const outcome = await forking;
    assert.deepEqual(outcome, {
      kind: "choose_fork",
      boundaries: [
        {
          surface_revision: 3,
          message: {
            id: "user-c",
            content: [{ type: "text", text: "C" }],
            source: "human",
            kind: "message",
          },
        },
      ],
      nextOffset: undefined,
    });
  });

  it("treats /reasoning and /expand as client display preferences", async () => {
    const { peer, dispatcher } = await harness();

    assert.deepEqual(await dispatcher.submit("/reasoning"), {
      kind: "preference",
      preference: { type: "reasoning" },
    });
    assert.deepEqual(await dispatcher.submit("/reasoning off"), {
      kind: "preference",
      preference: { type: "reasoning", visible: false },
    });
    assert.deepEqual(await dispatcher.submit("/reasoning on"), {
      kind: "preference",
      preference: { type: "reasoning", visible: true },
    });
    assert.deepEqual(await dispatcher.submit("/expand"), {
      kind: "preference",
      preference: { type: "expand", target: "latest" },
    });
    assert.deepEqual(await dispatcher.submit("/expand all"), {
      kind: "preference",
      preference: { type: "expand", target: "all" },
    });
    assert.deepEqual(await dispatcher.submit("/expand none"), {
      kind: "preference",
      preference: { type: "expand", target: "none" },
    });
    assert.deepEqual(await dispatcher.submit("/expand call-7"), {
      kind: "preference",
      preference: { type: "expand_call", callId: "call-7" },
    });

    // Not one of them reached the runtime: display is not a request.
    assert.equal(peer.requests.length, 2);
  });

  it("rejects an unusable /reasoning argument instead of guessing", async () => {
    const { dispatcher } = await harness();
    const outcome = await dispatcher.submit("/reasoning maybe");
    assert.equal(outcome.kind, "transient");
    if (outcome.kind === "transient") {
      assert.equal(outcome.level, "error");
      assert.match(outcome.text, /usage: \/reasoning \[on\|off\]/);
    }
  });

  it("renders /tools generically from the capability projection", async () => {
    const { dispatcher } = await harness(
      snapshot({ capabilities: capabilities(5) }),
    );
    const outcome = await dispatcher.submit("/tools");
    assert.equal(outcome.kind, "inspect");
    if (outcome.kind !== "inspect") {
      return;
    }
    assert.match(outcome.body, /capability revision 5/);
    assert.match(outcome.body, /### Active tools/);
    assert.match(outcome.body, /### Available but inactive/);
    assert.match(outcome.body, /`bash`/);
    assert.match(outcome.body, /mcp:corpus/);
    // Policies come from the runtime; nothing is inferred from the name.
    assert.match(outcome.body, /execution: model_selectable/);
  });

  it("shows available-but-inactive Tools without duplicating active Tools", async () => {
    const base = capabilities(6);
    const inactive = {
      ...base.available_tools![0]!,
      id: "tool-lint",
      name: "lint",
    };
    const { dispatcher } = await harness(
      snapshot({
        capabilities: {
          ...base,
          tools: [base.tools![0]!],
          available_tools: [...base.available_tools!, inactive],
        },
      }),
    );
    const outcome = await dispatcher.submit("/tools");
    assert.equal(outcome.kind, "inspect");
    if (outcome.kind !== "inspect") {
      return;
    }
    assert.match(outcome.body, /### Active tools/);
    assert.match(outcome.body, /### Available but inactive/);
    assert.match(outcome.body, /`lint`/);
    assert.equal((outcome.body.match(/`bash`/g) ?? []).length, 1);
    assert.equal((outcome.body.match(/`search`/g) ?? []).length, 1);
  });

  it("reports available Tools when the active registry is empty", () => {
    const base = capabilities(7);
    const rendered = renderTools({
      ...emptyPresentationState(sessionModel("alpha/model-a")),
      capabilities: { ...base, tools: [], available_tools: base.available_tools },
    });
    assert.match(rendered, /### Active tools\n- none/);
    assert.match(rendered, /### Available but inactive/);
    assert.match(rendered, /`bash`/);
    assert.match(rendered, /`search`/);
  });

  it("renders /skills from the runtime's Skill projection", async () => {
    const { dispatcher } = await harness(
      snapshot({ capabilities: capabilities(2) }),
    );
    const outcome = await dispatcher.submit("/skills");
    assert.equal(outcome.kind, "inspect");
    if (outcome.kind === "inspect") {
      assert.match(outcome.body, /`review` \(skill-review@1\)/);
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
    assert.equal(outcome.kind, "inspect");
    if (outcome.kind !== "inspect") {
      return;
    }
    // The rendering is the runtime's, verbatim.
    assert.ok(outcome.body.includes(rendered));
    assert.match(outcome.body, /inbound pending: 1/);
    assert.match(outcome.body, /last drain: watermark 2, 2 item\(s\)/);
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
    assert.equal(outcome.kind, "inspect");
    if (outcome.kind !== "inspect") {
      return;
    }
    assert.match(outcome.body, /attachment: `att-1`/);
    assert.match(outcome.body, /authoritative repairs \(resync\): 2/);
    assert.match(outcome.body, /1024 dropped/);
    assert.ok(!/sk-|api[_-]?key|secret/i.test(outcome.body));
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
    assert.equal(outcome.kind, "transient");
    if (outcome.kind === "transient") {
      assert.match(outcome.text, /acceptance/);
      assert.match(outcome.text, /runtime owns the terminal settlement/);
    }
  });

  it("sends /approve through Runtime Client without local outcome state", async () => {
    const { peer, dispatcher } = await harness();
    const responding = dispatcher.submit(
      "/approve attempt-1-interaction-1 deny human said no",
    );
    await peer.awaitRequests(3);

    const request = peer.requests[2];
    assert.equal(request?.method, "interaction_respond");
    assert.deepEqual(
      request?.method === "interaction_respond"
        ? {
            interaction_id: request.interaction_id,
            response: request.response,
          }
        : null,
      {
        interaction_id: "attempt-1-interaction-1",
        response: {
          type: "approval",
          decision: { type: "deny", reason: "human said no" },
        },
      },
    );

    peer.respond(3, {
      type: "interaction_response_accepted",
      interaction_id: "attempt-1-interaction-1",
    });
    const outcome = await responding;
    assert.equal(outcome.kind, "transient");
    if (outcome.kind === "transient") {
      assert.match(outcome.text, /response accepted/);
    }
  });

  it("sends a typed Question answer through Runtime Client", async () => {
    const { peer, dispatcher } = await harness();
    const responding = dispatcher.submit(
      "/answer attempt-1-interaction-1 choice alpha value",
    );
    await peer.awaitRequests(3);

    assert.deepEqual(
      peer.requests[2]?.method === "interaction_respond"
        ? peer.requests[2].response
        : null,
      {
        type: "question",
        answer: { type: "choice", value: "alpha value" },
      },
    );
    peer.respond(3, {
      type: "interaction_response_accepted",
      interaction_id: "attempt-1-interaction-1",
    });
    assert.equal((await responding).kind, "transient");
  });

  it("requests ApprovalMode through the runtime and reports pending reconciliation", async () => {
    const { peer, dispatcher } = await harness();
    const changing = dispatcher.submit("/approval full_access");
    await peer.awaitRequests(3);
    assert.deepEqual(
      peer.requests[2]?.method === "approval_mode_set"
        ? peer.requests[2].mode
        : null,
      "full_access",
    );
    peer.respond(3, {
      type: "approval_mode_set",
      effective_approval_mode: "policy",
      pending_approval_mode: "full_access",
      revision: 1,
    });
    const outcome = await changing;
    assert.equal(outcome.kind, "transient");
    if (outcome.kind === "transient") {
      assert.match(outcome.text, /pending full_access/);
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
    assert.equal(outcome.kind, "transient");
    if (outcome.kind === "transient") {
      assert.match(outcome.text, /acceptance, not settlement/);
    }
  });

  it("surfaces a typed protocol error as an error message", async () => {
    const { peer, dispatcher } = await harness();
    const cancelling = dispatcher.submit("/cancel");
    await peer.awaitRequests(3);
    peer.respondError(3, { type: "no_current_attempt" });

    const outcome = await cancelling;
    assert.equal(outcome.kind, "transient");
    if (outcome.kind === "transient") {
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
    assert.equal(outcome.kind, "transient");
    if (outcome.kind === "transient") {
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
    "--config",
    "/rustx.json",
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
      config: "/rustx.json",
      workspace: "/ws",
      runtimeRoot: "/private",
    });
    assert.deepEqual(parsed.startup, {
      skillPaths: [],
      noSkills: false,
      noBuiltinTools: false,
      noTools: false,
      tools: undefined,
      excludeTools: undefined,
    });
  });

  it("parses repeatable Skills and forwards startup controls without interpretation", () => {
    const parsed = parseArguments([
      ...complete,
      "--skill",
      "/user/skills/one",
      "--skill",
      "relative/skills/two",
      "--no-skills",
      "--no-builtin-tools",
      "--no-tools",
      "--tools",
      "read,search",
      "--exclude-tools",
      "search",
    ]);
    assert.deepEqual(parsed.startup, {
      skillPaths: ["/user/skills/one", "relative/skills/two"],
      noSkills: true,
      noBuiltinTools: true,
      noTools: true,
      tools: "read,search",
      excludeTools: "search",
    });
  });

  it("fails explicitly on malformed arguments", () => {
    const cases: string[][] = [
      [],
      ["--models"],
      ["--future", "x"],
      [...complete, "--models", "/again.json"],
      [...complete, "--tools", "read", "--tools", "search"],
      [...complete, "--no-tools", "--no-tools"],
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

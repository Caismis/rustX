/**
 * The command layer, tested without a terminal.
 *
 * Every command either renders projection state or invokes exactly one
 * canonical App Server operation. These cases assert both halves: what the user
 * sees, and which protocol request (if any) reached the wire.
 *
 * Nothing here sleeps: each case awaits the exact request it expects.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { SlashCommandAutocompleteProvider, commandPrefix } from "../src/commands/autocomplete.ts";
import { CommandDispatcher, renderTools } from "../src/commands/dispatcher.ts";
import { COMMANDS, parseCommandLine } from "../src/commands/registry.ts";
import { emptyPresentationState } from "../src/presentation/projection.ts";
import { TransientFeedbackSurface } from "../src/ui/components/transient-feedback.ts";
import { ArgumentError, USAGE, parseArguments } from "../src/cli.ts";
import {
  agentStatus,
  attemptModel,
  catalogModel,
  capabilities,
  questionnaireInteraction,
  runtimeCursor,
  sessionModel,
  sessionView,
  snapshot,
  temporalSection,
  todoSection,
  transcriptCursor,
} from "./support/fixtures.ts";
import { paramsOf } from "./support/app-server-peer.ts";
import {
  NO_DIAGNOSTICS,
  SESSION_SETTINGS,
  harness,
  nextRequest,
} from "./support/app-server-harness.ts";
import type { AppServerSession } from "../src/app-server/session.ts";

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

describe("Goal commands", () => {
  it("creates through the typed control without an inbound turn", async () => {
    const h = await harness();
    const creating = h.dispatcher.submit("/goal create finish delivery");
    const control = await nextRequest(h, "goal/control");
    assert.deepEqual(paramsOf(control, "goal/control"), {
      target: h.target,
      control: { action: "create", objective: "finish delivery", budget: 10 },
    });
    h.transport.respond(control.id, {
      type: "goal",
      view: {
        current: {
          reference: { id: "goal-1", revision: "1" },
          objective: "finish delivery",
          phase: "active" as const,
          autonomous_round_budget: 10,
          autonomous_rounds_consumed: 0,
          blocked_reason: null,
          origin: { kind: "runtime_control" as const },
          last_round_message_id: null,
        },
      },
    });
    const created = await creating;
    if (created.kind !== "inspect") throw new Error(`expected an inspect outcome, got ${created.kind}`);
    // Issue #351: the surface is derived from durable GoalPhase alone. An
    // Active Goal continues on its own; there is no arm/play/start step to
    // report and no second control to send.
    assert.match(created.body, /Status: Active/);
    assert.match(created.body, /Progress: 0\/10 autonomous rounds/);
    assert.doesNotMatch(created.body, /\barm|disarm|Inactive/i);
    // Creating a Goal is a typed control, never a conversation message.
    assert.equal(h.transport.log.count("turn/start"), 0);
    assert.equal(h.transport.log.count("goal/control"), 1);
  });

  it("uses the observed revision for pause and never retries stale state", async () => {
    const h = await harness();
    const pausing = h.dispatcher.submit("/goal pause");
    const read = await nextRequest(h, "goal/control");
    const current = {
      reference: { id: "goal-1", revision: "7" },
      objective: "deliver",
      phase: "active" as const,
      autonomous_round_budget: 10,
      autonomous_rounds_consumed: 2,
      blocked_reason: null,
      origin: { kind: "runtime_control" as const },
      last_round_message_id: null,
    };
    h.transport.respond(read.id, {
      type: "goal",
      view: { current },
    });

    const mutate = await nextRequest(h, "goal/control");
    assert.deepEqual(paramsOf(mutate, "goal/control").control, {
      action: "mutate",
      expected: current.reference,
      mutation: { action: "pause" },
    });
    h.transport.respondError(mutate.id, {
      code: -32000,
      message: "Stale GoalRef; current revision is 8",
      data: { kind: "invalid_state" },
    });
    const outcome = await pausing;
    assert.equal(outcome.kind, "transient");
    // A stale expectation is reported, not retried with a guessed revision.
    assert.equal(h.transport.log.count("goal/control"), 2);
  });
});

describe("command registry", () => {
  it("declares exactly the bounded command surface", () => {
    assert.deepEqual(
      COMMANDS.map((command) => command.name),
      [
        "/capabilities", "/permissions", "/attach", "/queue",
        "/export",
        "/settings",
        "/help",
        "/goal",
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
        "/todos",
        "/status",
        "/compact",
        "/configuration",
        "/debug",
        "/show-reasoning",
        "/expand",
        "/cancel",
        "/quit",
      ],
    );
  });

  it("no longer declares /approve: the human-input surface owns answers", () => {
    // Issue #185 removed the command-driven approval path. Approvals are
    // answered in the unified human-input surface through the same typed
    // `interaction_respond` operation, never through a command.
    const names = COMMANDS.map((command) => command.name);
    assert.ok(!names.includes("/approve"));
  });

  it("declares no shell, file, or Skill-execution escape", () => {
    // These would bypass rustX semantics entirely. There is no `!bash`, no
    // `@file` attachment, and no client-side Skill invocation.
    const names = COMMANDS.map((command) => command.name);
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
      ["/model", "/permissions"],
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
  it("routes /export through the local byte consumer and reports its outcome", async () => {
    const h = await harness();
    const calls: string[][] = [];
    h.host.exportSession = async (id, destination) => { calls.push([id, destination]); return destination; };
    const result = await h.dispatcher.submit("/export /client/a path.zip");
    assert.deepEqual(calls, [[h.session.sessionId, "/client/a path.zip"]]);
    assert.deepEqual(result, { kind: "transient", level: "info", text: "Session archive saved to /client/a path.zip" });
    h.host.exportSession = async () => { throw new Error("Native preflight failed"); };
    const failure = await h.dispatcher.submit("/export");
    assert.equal(failure.kind, "transient");
    if (failure.kind === "transient") { assert.equal(failure.level, "error"); assert.match(failure.text, /Native preflight failed/); }
    assert.equal(h.transport.log.count("turn/start"), 0);
  });

  it("submits plain text as one inbound message", async () => {
    const h = await harness();
    const submitting = h.dispatcher.submit("hello runtime");
    const start = await nextRequest(h, "turn/start");
    const params = paramsOf(start, "turn/start");

    // Inbound content is carried verbatim, addressed at the full attachment
    // target. The client composes nothing and interprets nothing.
    assert.deepEqual(params.target, h.target);
    assert.deepEqual(params.content, [{ type: "text", text: "hello runtime" }]);

    h.transport.respond(start.id, {
      type: "inbound_accepted",
      message_id: "m1",
      inbound_sequence: "1",
    });
    assert.deepEqual(await submitting, { kind: "none" });
  });

  it("keeps ordinary editor text as inbound while a questionnaire is pending", async () => {
    const h = await harness(
      snapshot({ pending_interactions: [questionnaireInteraction()] }),
    );
    const responding = h.dispatcher.submit("production");
    const start = await nextRequest(h, "turn/start");

    // A pending Questionnaire never captures the editor: only the focused
    // human-input surface answers one, and it does so with a typed response.
    assert.deepEqual(paramsOf(start, "turn/start").content, [
      { type: "text", text: "production" },
    ]);
    assert.equal(h.transport.log.count("interaction/respond"), 0);
    h.transport.respond(start.id, {
      type: "inbound_accepted",
      message_id: "m-question-text",
      inbound_sequence: "1",
    });
    assert.deepEqual(await responding, { kind: "none" });
  });

  it("keeps questionnaire answers out of shell parsing", async () => {
    const h = await harness(
      snapshot({
        pending_interactions: [
          questionnaireInteraction("attempt-1-interaction-question-open"),
        ],
      }),
    );
    const responding = h.dispatcher.submit("a private environment");
    const start = await nextRequest(h, "turn/start");
    assert.deepEqual(paramsOf(start, "turn/start").content, [
      { type: "text", text: "a private environment" },
    ]);
    h.transport.respond(start.id, {
      type: "inbound_accepted",
      message_id: "m-free-text",
      inbound_sequence: "1",
    });
    assert.deepEqual(await responding, { kind: "none" });
  });

  it("does not route ordinary text to any pending questionnaire", async () => {
    const h = await harness(
      snapshot({
        pending_interactions: [
          questionnaireInteraction("attempt-1-interaction-z"),
          questionnaireInteraction("attempt-1-interaction-a"),
        ],
      }),
    );
    const responding = h.dispatcher.submit("production");
    const start = await nextRequest(h, "turn/start");
    assert.equal(h.transport.log.count("interaction/respond"), 0);
    h.transport.respond(start.id, {
      type: "inbound_accepted",
      message_id: "m-question-text",
      inbound_sequence: "1",
    });
    assert.deepEqual(await responding, { kind: "none" });
  });

  it("keeps questionnaire decisions inside the overlay protocol", async () => {
    const h = await harness(
      snapshot({ pending_interactions: [questionnaireInteraction()] }),
    );
    const submitting = h.dispatcher.submit("custom environment");
    const start = await nextRequest(h, "turn/start");
    assert.equal(h.transport.log.count("interaction/respond"), 0);
    h.transport.respond(start.id, {
      type: "inbound_accepted",
      message_id: "m-custom-text",
      inbound_sequence: "1",
    });
    assert.deepEqual(await submitting, { kind: "none" });
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
    assert.match(outcome.body, /human-input surface/);
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
          execution_settings: null,
          model: attemptModel("alpha/model-a"),
        },
      }),
    );
    assert.equal(session.state?.attempt?.model!.primary.model, "alpha/model-a");

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
    const h = await harness(
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
          execution_settings: null,
          model: attemptModel("alpha/model-a"),
        },
      }),
    );
    const changing = h.dispatcher.submit("/model beta/model-b");

    // The client reads the runtime's own catalog. It never opens rustx.toml.
    const catalog = await nextRequest(h, "settings/models");
    h.transport.respond(catalog.id, {
      type: "models",
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

    const modelSet = await nextRequest(h, "settings/setModel");
    // A deliberate whole-state replacement: primary overrides reset, while
    // the independent summary policy survives exactly.
    assert.deepEqual(paramsOf(modelSet, "settings/setModel").config, {
      model: "beta/model-b",
      reasoningProfile: "off",
      requestParams: {},
      summaryModel: summaryPolicy,
    });
    h.transport.respond(modelSet.id, {
      type: "model",
      model: sessionModel("beta/model-b"),
    });

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
    } as unknown as AppServerSession;
    const sessionB = {
      state: emptyPresentationState(sessionModel("alpha/model-a")),
      modelSet: async () => {
        bModelSet += 1;
        return sessionModel("beta/model-b");
      },
    } as unknown as AppServerSession;
    const dispatcher = new CommandDispatcher({
      host: {} as never,
      session: sessionA,
      sessionSettings: SESSION_SETTINGS,
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
    const h = await harness();
    const changing = h.dispatcher.submit("/model made/up");
    const catalog = await nextRequest(h, "settings/models");
    h.transport.respond(catalog.id, { type: "models", catalog: { models: [] } });

    const outcome = await changing;
    assert.equal(outcome.kind, "transient");
    if (outcome.kind === "transient") {
      assert.equal(outcome.level, "error");
      assert.match(outcome.text, /not in the runtime's catalog/);
    }
    // No mutation was attempted for an unknown reference.
    assert.equal(h.transport.log.count("settings/setModel"), 0);
  });

  it("opens the model selector from the runtime catalog", async () => {
    const h = await harness();
    const choosing = h.dispatcher.submit("/model");
    // The selector reads the runtime catalog, never rustx.toml.
    const catalog = await nextRequest(h, "settings/models");
    h.transport.respond(catalog.id, {
      type: "models",
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
    // Opening the selector mutates nothing.
    assert.equal(h.transport.log.count("settings/setModel"), 0);
  });

  it("reads authoritative Session metadata for /session", async () => {
    const h = await harness();
    const reading = h.dispatcher.submit("/session");
    const read = await nextRequest(h, "session/read");
    assert.equal(paramsOf(read, "session/read").session_id, "ses_84097828-fc31-78c8-9292-10df48901a85");
    h.transport.respond(read.id, {
      type: "session",
      session: sessionView({ name: "review" }),
    });

    const outcome = await reading;
    assert.equal(outcome.kind, "inspect");
    if (outcome.kind === "inspect") {
      assert.equal(outcome.title, "Session");
      assert.match(outcome.body, /name review/);
      assert.ok(outcome.body.includes(`session ${sessionView().id}`));
      assert.ok(outcome.body.includes(`active node ${sessionView().active_node}`));
      assert.ok(outcome.body.includes(`conversation ${sessionView().active_conversation_id}`));
    }
  });

  it("lists persisted Sessions for /resume and does not choose in the client", async () => {
    const h = await harness();
    const resuming = h.dispatcher.submit("/resume");
    const list = await nextRequest(h, "session/list");
    h.transport.respond(list.id, {
      type: "sessions",

      sessions: [
        {
          id: "ses_84097828-fc31-78c8-9292-10df48901a85",
          name: "current",
          updated_at: "2026-08-21T00:00:00Z",
          cwd: "/server/work", active_node: "node_35971be6-e9bb-724a-8955-82fe0e42e048",
        },
        {
          id: "ses_5d906140-8048-712d-8539-25aed45333a1",
          name: "saved review",
          updated_at: "2026-08-20T00:00:00Z",
          cwd: "/server/work", active_node: "node_1779f59f-4df2-71f6-b81a-eb08fb52a5d8",
        },
      ],
    });

    const outcome = await resuming;
    assert.equal(outcome.kind, "choose_session");
    if (outcome.kind === "choose_session") {
      assert.deepEqual(outcome.sessions.map((session) => session.id), [
        "ses_84097828-fc31-78c8-9292-10df48901a85",
        "ses_5d906140-8048-712d-8539-25aed45333a1",
      ]);
    }
    // The durable list is a read: choosing is the user's, and focusing is a
    // separate step that this command does not take.
    assert.equal(h.transport.log.count("session/attach"), 1);
  });

  it("selecting a Session is a focus intent, with no request of its own", async () => {
    const h = await harness();
    const before = h.transport.log.requests.length;

    // Choosing a row in the picker names the Session to show. It does not
    // detach, cancel, unload, or replace anything, so it writes nothing here:
    // the app performs the attach when it changes focus.
    assert.deepEqual(h.dispatcher.selectSession("ses_5d906140-8048-712d-8539-25aed45333a1"), {
      kind: "focus_session",
      sessionId: "ses_5d906140-8048-712d-8539-25aed45333a1",
    });
    assert.deepEqual(h.dispatcher.selectTreeNode("ses_5d906140-8048-712d-8539-25aed45333a1", "node_cda805b6-0c45-73dd-81b4-8a4571613b8e"), {
      kind: "focus_session",
      sessionId: "ses_5d906140-8048-712d-8539-25aed45333a1",
      nodeId: "node_cda805b6-0c45-73dd-81b4-8a4571613b8e",
    });
    assert.equal(h.transport.log.requests.length, before);
  });

  it("turns durable Session failures into error outcomes without ending the connection", async () => {
    const h = await harness();
    const boundary = {
      surface_revision: "4",
      message: {
        id: "user-c",
        content: [{ type: "text" as const, text: "try again" }],
        source: "human" as const,
        kind: "message" as const,
      },
    };

    const forking = h.dispatcher.forkAt(boundary);
    const fork = await nextRequest(h, "session/fork");
    h.transport.respondError(fork.id, {
      code: -32000,
      message: "stale boundary",
      data: { kind: "invalid_state" },
    });
    assert.equal((await forking).kind, "transient");

    const branching = h.dispatcher.branchAt(boundary);
    const read = await nextRequest(h, "session/read");
    h.transport.respond(read.id, { type: "session", session: sessionView() });
    const branch = await nextRequest(h, "session/branch");
    h.transport.respondError(branch.id, {
      code: -32000,
      message: "catalog failure",
      data: { kind: "operation_failed" },
    });
    assert.equal((await branching).kind, "transient");

    // A semantic Session failure is a healthy protocol response, so the
    // connection remains usable for the next overlay request.
    assert.equal(h.client.closed, undefined);
  });

  it("creates a new Session from this client's Session settings", async () => {
    const h = await harness();
    const creating = h.dispatcher.submit("/new");
    const create = await nextRequest(h, "session/create");
    // The Session's cwd is a Session selection, supplied here. The App Server
    // process's own launch directory is never substituted for it.
    assert.deepEqual(paramsOf(create, "session/create").settings, SESSION_SETTINGS);
    h.transport.respond(create.id, {
      type: "session_transition",
      session: sessionView({ id: "ses_5d906140-8048-712d-8539-25aed45333a1", name: "New session" }),
    });

    const outcome = await creating;
    assert.equal(outcome.kind, "focus_session");
    if (outcome.kind === "focus_session") {
      assert.equal(outcome.sessionId, "ses_5d906140-8048-712d-8539-25aed45333a1");
      assert.match(outcome.notice ?? "", /created session/);
    }
    // Creating a Session replaces no process and stops nothing.
    const methods = h.transport.log.requests.map((request) => request.method);
    assert.ok(!methods.includes("session/switchNode"));
    assert.ok(!methods.includes("turn/cancel"));
  });

  it("clones the exact committed head, read authoritatively first", async () => {
    const h = await harness();
    const cloning = h.dispatcher.submit("/clone");
    const head = await nextRequest(h, "session/boundaries");
    h.transport.respond(head.id, {
      type: "boundaries",
      surface_revision: "17",
      boundaries: [],
      next_offset: null,
    });
    const fork = await nextRequest(h, "session/fork");
    const params = paramsOf(fork, "session/fork");
    // A clone is a fork with no boundary, at the exact revision just read.
    assert.equal(params.surface_revision, "17");
    assert.equal(params.boundary, null);
    h.transport.respond(fork.id, {
      type: "session_transition",
      session: sessionView({ id: "ses_eb278475-f606-7143-97df-8cb657e1c7ee" }),
    });

    const outcome = await cloning;
    assert.equal(outcome.kind, "focus_session");
  });

  it("names the Session as metadata without emitting a conversation message", async () => {
    const h = await harness();
    const naming = h.dispatcher.submit("/name design review");
    const rename = await nextRequest(h, "session/name");
    assert.deepEqual(paramsOf(rename, "session/name"), {
      session_id: "ses_84097828-fc31-78c8-9292-10df48901a85",
      name: "design review",
    });
    h.transport.respond(rename.id, {
      type: "session",
      session: sessionView({ name: "design review" }),
    });

    const outcome = await naming;
    assert.equal(outcome.kind, "transient");
    if (outcome.kind === "transient") {
      assert.match(outcome.text, /session named design review/);
    }
    assert.equal(h.transport.log.count("turn/start"), 0);
  });

  // A Session is unnamed until someone names it, so a bare `/name` answers
  // with the fact rather than a usage error, and it never writes metadata.
  it("reports the active Session name instead of naming it", async () => {
    const h = await harness();
    const asking = h.dispatcher.submit("/name");
    const read = await nextRequest(h, "session/read");
    h.transport.respond(read.id, { type: "session", session: sessionView() });

    const unnamed = await asking;
    assert.equal(unnamed.kind, "transient");
    if (unnamed.kind === "transient") {
      assert.ok(unnamed.text.includes(`${sessionView().id} is unnamed`));
    }

    const again = h.dispatcher.submit("/name");
    const second = await nextRequest(h, "session/read");
    h.transport.respond(second.id, {
      type: "session",
      session: sessionView({ name: "design review" }),
    });
    const named = await again;
    assert.equal(named.kind, "transient");
    if (named.kind === "transient") {
      assert.match(named.text, /session name: design review/);
    }
    assert.equal(h.transport.log.count("session/name"), 0);
  });

  it("returns native fork boundaries for the picker", async () => {
    const h = await harness();
    const forking = h.dispatcher.submit("/fork");
    const boundaries = await nextRequest(h, "session/boundaries");
    // Boundaries are a live Surface read against the attached target, so the
    // revision a selection carries is the one the reader is looking at.
    assert.deepEqual(paramsOf(boundaries, "session/boundaries").target, h.target);
    h.transport.respond(boundaries.id, {
      type: "boundaries",
      surface_revision: "3",
      boundaries: [
        {
          surface_revision: "3",
          message: {
            id: "user-c",
            content: [{ type: "text", text: "C" }],
            source: "human",
            kind: "message",
          },
        },
      ],
      next_offset: null,
    });

    const outcome = await forking;
    assert.deepEqual(outcome, {
      kind: "choose_fork",
      boundaries: [
        {
          surface_revision: "3",
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

  it("treats /show-reasoning and /expand as client display preferences", async () => {
    const h = await harness();
    const before = h.transport.log.requests.length;

    assert.deepEqual(await h.dispatcher.submit("/show-reasoning"), {
      kind: "preference",
      preference: { type: "reasoning" },
    });
    assert.deepEqual(await h.dispatcher.submit("/show-reasoning off"), {
      kind: "preference",
      preference: { type: "reasoning", visible: false },
    });
    assert.deepEqual(await h.dispatcher.submit("/show-reasoning on"), {
      kind: "preference",
      preference: { type: "reasoning", visible: true },
    });
    assert.deepEqual(await h.dispatcher.submit("/expand"), {
      kind: "preference",
      preference: { type: "expand", target: "latest" },
    });
    assert.deepEqual(await h.dispatcher.submit("/expand all"), {
      kind: "preference",
      preference: { type: "expand", target: "all" },
    });
    assert.deepEqual(await h.dispatcher.submit("/expand none"), {
      kind: "preference",
      preference: { type: "expand", target: "none" },
    });
    assert.deepEqual(await h.dispatcher.submit("/expand call-7"), {
      kind: "preference",
      preference: { type: "expand_call", callId: "call-7" },
    });

    // Not one of them reached the server: display is not a request.
    assert.equal(h.transport.log.requests.length, before);
  });

  it("CFG238 rejects obsolete /reasoning and keeps display toggles out of semantic state", async () => {
    const h = await harness();
    const before = structuredClone(h.session.state);
    const written = h.transport.log.requests.length;
    for (const command of ["/show-reasoning on", "/show-reasoning off"]) {
      assert.equal((await h.dispatcher.submit(command)).kind, "preference");
      assert.deepEqual(h.session.state, before);
    }
    const obsolete = await h.dispatcher.submit("/reasoning on");
    assert.equal(obsolete.kind, "transient");
    if (obsolete.kind === "transient") assert.match(obsolete.text, /unknown command/);
    assert.deepEqual(h.session.state, before);
    assert.equal(
      h.transport.log.requests.length,
      written,
      "no model, tool, history, or save mutation reaches the server",
    );
    assert.ok(!COMMANDS.some((command) => command.name === "/reasoning"));
  });

  for (const profile of ["default", "clear", "off", "set", "on", null]) {
    it(`CFG238 profile grammar selects ${profile ?? "catalog default"} through a whole-state replacement`, async () => {
      const h = await harness();
      const before = structuredClone(h.session.state);
      const command = h.dispatcher.submit(
        profile === null ? "/model profile clear" : `/model profile set ${profile}`,
      );
      const read = await nextRequest(h, "settings/model");
      const current = sessionModel("alpha/model-a");
      current.configured.reasoningProfile = "previous";
      current.configured.requestParams = { temperature: 0.5 };
      current.configured.maxOutputTokens = 2048;
      current.configured.summaryModel = {
        mode: "explicit",
        model: "alpha/summary",
        request_params: { temperature: 0.2 },
      };
      h.transport.respond(read.id, { type: "model", model: current });

      const set = await nextRequest(h, "settings/setModel");
      const expected = { ...current.configured };
      if (profile === null) delete expected.reasoningProfile;
      else expected.reasoningProfile = profile;
      assert.deepEqual(paramsOf(set, "settings/setModel").config, expected);
      h.transport.respond(set.id, {
        type: "model",
        model: { ...current, configured: expected },
      });
      assert.equal((await command).kind, "transient");
      assert.equal(h.transport.log.count("settings/setModel"), 1);
      assert.deepEqual(
        h.session.state,
        before,
        "responses never mutate the semantic projection",
      );
    });
  }

  it("CFG238 rejects obsolete ambiguous profile syntax without a native operation", async () => {
    const h = await harness();
    const before = h.transport.log.requests.length;
    for (const text of [
      "/model profile default",
      "/model profile off",
      "/model profile set",
      "/model profile",
      "/model profile clear extra",
    ]) {
      const result = await h.dispatcher.submit(text);
      assert.equal(result.kind, "transient");
      if (result.kind === "transient")
        assert.equal(
          result.text,
          "usage: /model profile set <id> | /model profile clear",
        );
    }
    assert.equal(h.transport.log.requests.length, before);
  });

  it("removed Session approval and save-default commands never reach the wire", async () => {
    const h = await harness(); const before = h.transport.log.requests.length;
    for (const command of ["/approval", "/defaults", "/save-default user model revision"]) {
      const result = await h.dispatcher.submit(command);
      assert.equal(result.kind, "transient");
      if (result.kind === "transient") assert.equal(result.level, "error");
    }
    assert.equal(h.transport.log.requests.length, before);
  });

  it("rejects an unusable /show-reasoning argument instead of guessing", async () => {
    const { dispatcher } = await harness();
    const outcome = await dispatcher.submit("/show-reasoning maybe");
    assert.equal(outcome.kind, "transient");
    if (outcome.kind === "transient") {
      assert.equal(outcome.level, "error");
      assert.match(outcome.text, /usage: \/show-reasoning \[on\|off\]/);
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
      assert.match(outcome.body, /\.agents\/skills\/review\/SKILL\.md/);
    }
  });

  it("renders /status from typed sections, never the rendered body", async () => {
    // The model-facing body says something the typed sections do not, so a
    // renderer that parsed it would be caught here.
    const rendered = "<system-reminder>\nTimezone: UTC\nsentinel-from-rendered\n</system-reminder>";
    const { dispatcher } = await harness(
      snapshot({
        statuses: [
          agentStatus({
            status_message_id: "status-1",
            turn: 2,
            opportunities: { fresh_inbound: { target_message_id: "m1" } },
            sections: [
              temporalSection("2026-08-14T15:42:00Z"),
              todoSection({ active_count: 3, blocked_count: 1 }),
            ],
            rendered,
          }),
        ],
      }),
    );

    const outcome = await dispatcher.submit("/status");
    assert.equal(outcome.kind, "inspect");
    if (outcome.kind !== "inspect") {
      return;
    }
    assert.match(outcome.body, /### Agent Status/);
    assert.match(outcome.body, /\*\*time\*\* — 15:42 UTC/);
    assert.match(outcome.body, /\*\*todo\*\* — 3 active · 1 blocked/);
    assert.ok(
      !outcome.body.includes("sentinel-from-rendered"),
      "/status renders typed sections; the model-facing body is not a source",
    );
    // Provenance about the composition is kept and separated; generic
    // runtime diagnostics moved to the one diagnostic surface.
    assert.match(outcome.body, /Composed by attempt `a1` on turn 2/);
    assert.match(outcome.body, /`\/debug`/);
  });

  it("says so plainly when no Agent Status has been composed", async () => {
    const { dispatcher } = await harness(snapshot());
    const outcome = await dispatcher.submit("/status");
    assert.equal(outcome.kind, "inspect");
    if (outcome.kind !== "inspect") {
      return;
    }
    assert.match(outcome.body, /No Agent Status has been composed yet/);
  });

  it("keeps runtime and mailbox diagnostics on /debug", async () => {
    const { dispatcher } = await harness(
      snapshot({
        statuses: [agentStatus({ status_message_id: "status-1" })],
        inbound: {
          pending: [
            {
              revision: "0", sequence: "3",
              message: {
                id: "m2",
                content: [{ type: "text", text: "queued" }],
                source: "human",
                kind: "message",
              },
            },
          ],
          last_drain: { watermark: "2", count: 2 },
        },
      }),
    );

    const outcome = await dispatcher.submit("/debug");
    assert.equal(outcome.kind, "inspect");
    if (outcome.kind !== "inspect") {
      return;
    }
    assert.match(outcome.body, /inbound pending: 1/);
    assert.match(outcome.body, /last drain: watermark 2, 2 item\(s\)/);
    assert.match(outcome.body, /composed Agent Statuses: 1/);
    assert.match(outcome.body, /attempt: none/);
  });

  it("runs /compact through one canonical App Server operation", async () => {
    const h = await harness();
    const compacting = h.dispatcher.submit("/compact");
    const compact = await nextRequest(h, "context/compact");
    assert.deepEqual(paramsOf(compact, "context/compact").target, h.target);
    h.transport.respond(compact.id, {
      type: "context",
      context: {
        compaction_in_progress: false,
        compaction_count: 1,
        latest_compaction: {
          generation: "1",
          summary_message_id: "conv_8498648a-66ba-716f-8243-8cf147533e52",
          surface_revision: "4",
          tokens_before: { input_tokens: 8_400, source: "estimated" },
          estimated_tokens_after: 1_900,
        },
      },
    });

    const outcome = await compacting;
    assert.equal(outcome.kind, "transient");
    if (outcome.kind === "transient") {
      assert.equal(outcome.level, "info");
      assert.match(outcome.text, /generation 1/);
      assert.match(outcome.text, /8400 → 1900 tokens/);
    }
  });

  it("rejects /compact arguments without reaching the server", async () => {
    const h = await harness();
    const before = h.transport.log.requests.length;
    const outcome = await h.dispatcher.submit("/compact custom instructions");
    assert.equal(outcome.kind, "transient");
    if (outcome.kind === "transient") {
      assert.equal(outcome.level, "error");
      assert.match(outcome.text, /usage: \/compact/);
    }
    assert.equal(h.transport.log.requests.length, before);
  });

  it("T16 rescans only through native reconciliation", async () => {
    const h = await harness();
    const operation = h.dispatcher.submit("/configuration rescan");
    const read = await nextRequest(h, "configuration/sourcesRead");
    h.transport.respond(read.id, { type: "source_settings", session_revision: "1", projection: {
      absent_resource_revision: "absent", resource_revisions: {}, user: { path: "/user", revision: "u" }, workspace: { path: "/workspace", revision: "w" },
      user_resource_root: "/ur", workspace_resource_root: "/wr", runtime_root: "/r", user_mcp: { path: "/um", revision: "um" }, workspace_mcp: { path: "/wm", revision: "wm" }, agents: [],
    } });
    const reconcile = await nextRequest(h, "configuration/reconcile");
    h.transport.respond(reconcile.id, { type: "configuration_application", application: { scope: h.session.sessionId, version: "2", desired: { input_revision: "u", attempt: "1" }, units: { execution_policy: { status: "applied" } } } });
    assert.deepEqual(await operation, { kind: "transient", level: "info", text: "Native reconciliation started. Use /configuration to inspect application." });
  });

  it("renders bounded /debug diagnostics without any credential", async () => {
    const h = await harness();
    const dispatcher = new CommandDispatcher({
      host: h.host,
      session: h.session,
      sessionSettings: SESSION_SETTINGS,
      diagnostics: () => ({
        connection: "external App Server at ws://127.0.0.1:8080",
        ownership: "an external owner runs the App Server; exiting only disconnects",
        sessionId: "ses_84097828-fc31-78c8-9292-10df48901a85",
        attachmentId: "att-1",
        conversationId: "conv_01900000-0000-7000-8000-000000000002",
        runtimeIncarnation: "9007199254740993",
        cursor: runtimeCursor(0),
        attachedSessions: 2,
        connectionState: "connected",
        childStatus: "not applicable (external App Server)",
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
    // The identity domains are reported separately, because they are separate.
    assert.match(outcome.body, /runtime incarnation: `9007199254740993`/);
    assert.match(outcome.body, /attached sessions: 2/);
    assert.match(outcome.body, /process ownership: an external owner/);
    assert.match(outcome.body, /authoritative repairs \(resync\): 2/);
    assert.match(outcome.body, /1024 dropped/);
    assert.ok(!/sk-|api[_-]?key|secret/i.test(outcome.body));
  });

  it("treats attempt cancellation as acceptance", async () => {
    const h = await harness();
    const cancelling = h.dispatcher.submit("/cancel");
    const cancel = await nextRequest(h, "turn/cancel");
    assert.deepEqual(paramsOf(cancel, "turn/cancel").target, h.target);
    h.transport.respond(cancel.id, {
      type: "cancellation_accepted",
      attempt_id: "a1",
    });

    const outcome = await cancelling;
    assert.equal(outcome.kind, "transient");
    if (outcome.kind === "transient") {
      assert.match(outcome.text, /acceptance/);
      assert.match(outcome.text, /runtime owns the terminal settlement/);
    }
  });

  it("removed /approve: a stale spelling is an unknown command off the wire", async () => {
    const h = await harness();
    const before = h.transport.log.requests.length;
    const outcome = await h.dispatcher.submit(
      "/approve conv_01900000-0000-7000-8000-000000000002::attempt-1-interaction-1 allow",
    );
    assert.equal(outcome.kind, "transient");
    if (outcome.kind === "transient") {
      assert.equal(outcome.level, "error");
      assert.match(outcome.text, /unknown command/);
    }
    assert.equal(h.transport.log.requests.length, before);
    assert.equal(h.transport.log.count("interaction/respond"), 0);
  });

  it("cancels one background execution by its runtime identity", async () => {
    const h = await harness();
    const cancelling = h.dispatcher.submit("/cancel exec_05cd92d5-1932-7bdc-beaf-0d97db6118c5");
    const cancel = await nextRequest(h, "background/cancel");
    assert.equal(
      paramsOf(cancel, "background/cancel").execution_id,
      "exec_05cd92d5-1932-7bdc-beaf-0d97db6118c5",
    );
    h.transport.respond(cancel.id, {
      type: "background",
      execution: {
        execution_id: "exec_05cd92d5-1932-7bdc-beaf-0d97db6118c5",
        tool_id: "tool-background",
        tool_name: "bash",
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
    const h = await harness();
    const cancelling = h.dispatcher.submit("/cancel");
    const cancel = await nextRequest(h, "turn/cancel");
    h.transport.respondError(cancel.id, {
      code: -32000,
      message: "no attempt is currently cancellable",
      data: { kind: "invalid_state" },
    });

    const outcome = await cancelling;
    assert.equal(outcome.kind, "transient");
    if (outcome.kind === "transient") {
      assert.equal(outcome.level, "error");
      assert.match(outcome.text, /no attempt is currently cancellable/);
    }
  });

  it("reports an uncertain cancellation without claiming it failed", async () => {
    const h = await harness();
    const cancelling = h.dispatcher.submit("/cancel");
    await nextRequest(h, "turn/cancel");
    // The response is lost. Cancellation may well have been accepted.
    h.transport.fail("input_eof", "the connection dropped");

    const outcome = await cancelling;
    assert.equal(outcome.kind, "transient");
    if (outcome.kind === "transient") {
      assert.equal(outcome.level, "error");
      assert.match(outcome.text, /whether the server accepted it is unknown/);
      assert.match(outcome.text, /check the authoritative state/);
    }
    assert.equal(
      h.transport.log.count("turn/cancel"),
      1,
      "the uncertain mutation was never resent",
    );
  });

  it("reports /quit as a quit intent rather than acting itself", async () => {
    const h = await harness();
    assert.deepEqual(await h.dispatcher.submit("/quit"), { kind: "quit" });
  });

  it("rejects an unknown command without reaching the wire", async () => {
    const h = await harness();
    const before = h.transport.log.requests.length;
    const outcome = await h.dispatcher.submit("/definitely-not-a-command");
    assert.equal(outcome.kind, "transient");
    if (outcome.kind === "transient") {
      assert.equal(outcome.level, "error");
    }
    assert.equal(h.transport.log.requests.length, before, "no request was issued");
  });
});

describe("CLI arguments", () => {
  const localArgv = [
    "--binary",
    "/usr/bin/rustx",
    "--runtime-root",
    "/private/state",
    "--workspace",
    "/work/project",
    "--config",
    "/private/user/rustx.toml",
    "--model",
    "local/dev",
    "--name",
    "auth refactor",
  ];

  it("parses the complete local self-hosted argument set", () => {
    const parsed = parseArguments(localArgv);

    assert.equal(parsed.mode.kind, "local");
    if (parsed.mode.kind !== "local") throw new Error("local mode");
    assert.equal(parsed.mode.binary, "/usr/bin/rustx");
    // Process-level source bindings configure the App Server process.
    assert.deepEqual(parsed.mode.launch, {
      config: "/private/user/rustx.toml",
      runtimeRoot: "/private/state",
    });
    // Session settings are `session/create` inputs, and stay separate.
    assert.deepEqual(parsed.sessionSettings, {
      cwd: "/work/project",
      model: { model: "local/dev" },
    });
    assert.equal(parsed.sessionName, "auth refactor");
    assert.deepEqual(parsed.routing, {
      session: undefined,
      node: undefined,
      openSessionSelector: false,
    });
  });

  it("parses a minimal local launch without inventing defaults", () => {
    const parsed = parseArguments(["--binary", "/usr/bin/rustx"]);
    assert.equal(parsed.mode.kind, "local");
    if (parsed.mode.kind !== "local") throw new Error("local mode");
    // Every omitted binding keeps the App Server's canonical default; the
    // client never fabricates a path, and never reads one.
    assert.deepEqual(parsed.mode.launch, {
      config: undefined,
      runtimeRoot: undefined,
    });
    assert.equal(parsed.sessionSettings.model, null);
    assert.equal(parsed.sessionName, undefined);
  });

  it("defaults only local Session cwd from the controlled client cwd", (t) => {
    t.mock.method(process, "cwd", () => "/client/workspace");
    assert.equal(parseArguments(["--binary", "rustx"]).sessionSettings.cwd, "/client/workspace");
    assert.equal(parseArguments(["--binary", "rustx", "--workspace", "/explicit/local"]).sessionSettings.cwd, "/explicit/local");
  });

  it("requires an explicit absolute remote cwd without consulting client cwd", (t) => {
    t.mock.method(process, "cwd", () => { throw new Error("remote parsing must not read client cwd"); });
    const remote = ["--connect", "wss://server.test", "--token-file", "/client/token"];
    for (const suffix of [[], ["--workspace", "relative/project"], ["--workspace", ""]]) {
      assert.throws(() => parseArguments([...remote, ...suffix]), /remote Session cwd requires an explicit --workspace absolute path on the App Server host/);
    }
    assert.equal(parseArguments([...remote, "--workspace", "/server/work/../project"]).sessionSettings.cwd, "/server/work/../project");
  });

  it("parses an existing/remote App Server connection", () => {
    const parsed = parseArguments([
      "--connect",
      "ws://127.0.0.1:8080",
      "--token-file",
      "/private/user/socket-token",
      "--workspace",
      "/srv/project",
    ]);
    assert.equal(parsed.mode.kind, "remote");
    if (parsed.mode.kind !== "remote") throw new Error("remote mode");
    assert.equal(parsed.mode.endpoint, "ws://127.0.0.1:8080");
    assert.equal(parsed.mode.tokenFile, "/private/user/socket-token");
    // Session settings still apply: they are resolved by the server host.
    assert.equal(parsed.sessionSettings.cwd, "/srv/project");
  });

  it("requires exactly one mode", () => {
    assert.throws(() => parseArguments([]), ArgumentError);
    assert.throws(
      () =>
        parseArguments([
          "--binary",
          "/usr/bin/rustx",
          "--connect",
          "ws://127.0.0.1:8080",
        ]),
      /cannot be combined/,
    );
  });

  it("refuses process-level bindings against an external App Server", () => {
    // These configure a *process*. A remote App Server was launched by someone
    // else and already has its own; accepting them would be a flag that
    // pretends to configure a server it cannot reach.
    for (const flag of ["--config", "--runtime-root"]) {
      assert.throws(
        () =>
          parseArguments([
            "--connect",
            "ws://127.0.0.1:8080",
            "--token-file",
            "/t",
            flag,
            "/value",
          ]),
        /configures an App Server process/,
        flag,
      );
    }
  });

  it("requires a transport credential for WebSocket and refuses one for stdio", () => {
    assert.throws(
      () => parseArguments(["--connect", "ws://127.0.0.1:8080"]),
      /requires --token-file/,
    );
    assert.throws(
      () =>
        parseArguments(["--binary", "/usr/bin/rustx", "--token-file", "/t"]),
      /applies only to --connect/,
    );
  });

  it("rejects an endpoint that is not a WebSocket URL", () => {
    for (const endpoint of ["http://127.0.0.1:8080", "127.0.0.1:8080", "stdio"]) {
      assert.throws(
        () => parseArguments(["--connect", endpoint, "--token-file", "/t"]),
        /ws:\/\/ or wss:\/\/ endpoint/,
        endpoint,
      );
    }
    assert.equal(
      parseArguments(["--connect", "wss://example.test", "--token-file", "/t", "--workspace", "/srv/project"])
        .mode.kind,
      "remote",
    );
  });

  it("routes to one Session, or opens the picker, but never both", () => {
    const explicit = parseArguments([
      "--binary",
      "/usr/bin/rustx",
      "--session",
      "ses_eb278475-f606-7143-97df-8cb657e1c7ee",
      "--node",
      "node_c346d387-9a21-70f0-ae5c-7422521183b3",
    ]);
    assert.deepEqual(explicit.routing, {
      session: "ses_eb278475-f606-7143-97df-8cb657e1c7ee",
      node: "node_c346d387-9a21-70f0-ae5c-7422521183b3",
      openSessionSelector: false,
    });
    const picker = parseArguments(["--binary", "/usr/bin/rustx", "--resume"]);
    assert.equal(picker.routing.openSessionSelector, true);
    assert.equal(picker.routing.session, undefined);

    assert.throws(
      () =>
        parseArguments([
          "--binary",
          "/usr/bin/rustx",
          "--session",
          "s",
          "--resume",
        ]),
      /cannot be combined/,
    );
    assert.throws(
      () => parseArguments(["--binary", "/usr/bin/rustx", "--node", "node_c346d387-9a21-70f0-ae5c-7422521183b3"]),
      /--node requires --session/,
    );
  });

  it("removes the obsolete global-active and inspection flags", () => {
    // There is no global active Session to continue, and conversation
    // inspection was a Runtime Client process capability with no App Server
    // method behind it. Both are unknown arguments rather than aliases.
    for (const flag of ["--continue", "--inspect-conversation", "--cwd", "--trust"]) {
      assert.throws(
        () => parseArguments(["--binary", "/usr/bin/rustx", flag, "x"]),
        /unknown argument/,
        flag,
      );
    }
  });

  it("fails explicitly on malformed arguments", () => {
    assert.throws(
      () => parseArguments(["--binary"]),
      /--binary requires a value/,
    );
    assert.throws(
      () => parseArguments(["--binary", "a", "--binary", "b"]),
      /supplied more than once/,
    );
    assert.throws(
      () => parseArguments(["--binary", "a", "--resume", "--resume"]),
      /supplied more than once/,
    );
    assert.throws(() => parseArguments(["nonsense"]), /unknown argument/);
  });

  it("documents both modes in its usage text", () => {
    assert.match(USAGE, /local self-hosted/);
    assert.match(USAGE, /existing \/ remote App Server/);
    assert.match(USAGE, /--connect/);
    assert.match(USAGE, /--token-file/);
  });
});

it("manual runtime unload is not a product command", async () => {
  const h = await harness();
  const result = await h.dispatcher.submit("/unload background");
  assert.equal(result.kind, "transient");
  assert.equal(h.transport.log.count("session/delete"), 0);
  assert.equal(h.transport.log.count("session/unload"), 0);
});

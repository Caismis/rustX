import { followAttachment } from './support/app-server-peer.ts';
import { checkpoint } from "./support/fixtures.ts";
/** Startup ownership, driven by explicit wire replies and user selections. */
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { it, type TestContext } from "node:test";
import { TUI, Editor } from "@earendil-works/pi-tui";
import { AppServerClient } from "../src/app-server/client.ts";
import { AppServerHost } from "../src/app-server/host.ts";
import { parseArguments } from "../src/cli.ts";
import { prepareStartup, type StartupFocus } from "../src/startup.ts";
import { RustxTuiApp } from "../src/ui/app.ts";
import { CommandDispatcher } from "../src/commands/dispatcher.ts";
import { ConfirmationView } from "../src/ui/components/confirmation.ts";
import { ComposerCommand } from "../src/ui/components/composer-command.ts";
import { PopupFrame } from "../src/ui/components/popup-frame.ts";
import { ResumeSelector } from "../src/ui/components/resume-selector.ts";
import { TransientFeedbackSurface } from "../src/ui/components/transient-feedback.ts";
import { FakeTransport, paramsOf, tick } from "./support/app-server-peer.ts";
import { SERVER_CAPABILITIES } from "./support/app-server-harness.ts";
import { snapshot, sessionView } from "./support/fixtures.ts";

const SESSION_A = "ses_01900000-0000-7000-8000-000000001001";
const SESSION_B = "ses_01900000-0000-7000-8000-000000001002";
const SESSION_NEW = "ses_01900000-0000-7000-8000-000000001003";
const SESSION_CREATED = "ses_01900000-0000-7000-8000-000000001004";
const parsedResume = () => parseArguments(["--binary", "rustx", "--resume", "--workspace", "/server/work"]);
const rows = [SESSION_A, SESSION_B].map((id) => ({ id, name: `Session ${id}`, cwd: "/server/work", ownership_generation: "1", active_node: `node_${id.slice(4)}`, updated_at: "2026-09-14T00:00:00Z" }));

async function connected(authority = 'fixture-app-server-authority') {
  const transport = new FakeTransport();
  const pending = AppServerClient.initialize({ transport });
  const [request] = await transport.log.awaitMethod("initialize");
  transport.respond(request!.id, { type: "initialized", authority_id: authority, protocol_version: 57, capabilities: SERVER_CAPABILITIES });
  return { transport, host: new AppServerHost({ client: await pending, ownership: "external" }) };
}
async function catalog(transport: FakeTransport, sessions = rows, count = 1) {
  const request = (await transport.log.awaitMethod("session/list", count)).at(-1)!;
  transport.respond(request.id, { type: "sessions", sessions, });
}
async function pendingAttachment(transport: FakeTransport, count = 1) {
  if (transport.log.count('session/attach') < count) {
    await tick();
    if (transport.log.count('session/attach') < count) {
      const metadata = transport.log.matching('session/read').find(request => !transport.answered.has(request.id));
      if (metadata) transport.respond(metadata.id, { type: 'session', session: sessionView({ id: paramsOf(metadata, 'session/read').session_id }) });
    }
  }
  return transport.log.awaitMethod('session/attach', count);
}
async function attachment(transport: FakeTransport, id: string, count = 1) {
  const request = (await pendingAttachment(transport, count)).at(-1)!;
  assert.equal(paramsOf(request, "session/attach").session_id, id);
  transport.respond(request.id, {type:'attached', target: {
    session_id: id, conversation_id: "conv_01900000-0000-7000-8000-000000000002", attachment_id: `att-${id}`, runtime_incarnation: "1",
  }, configuration_revision:'0', attachment_kind:'controller', observation_source:'live'});
  await followAttachment(transport,{session_id:id,conversation_id:"conv_01900000-0000-7000-8000-000000000002",attachment_id:`att-${id}`,runtime_incarnation:'1'},snapshot(),"0");
  return request;
}
async function finishFocus(transport: FakeTransport) {
  const read = await transport.nextPending("session/read");
  transport.respond(read.id, { type: "session", session: sessionView({ id: paramsOf(read, "session/read").session_id }) });
  const repair = await transport.nextPending("session/checkpoint");
  transport.respond(repair.id, {type:'checkpoint', checkpoint:checkpoint(snapshot(),"0")});
  const subscribe = await transport.nextPending("session/subscribe");
  transport.respond(subscribe.id, { type: "subscribed", after_cursor: "0" });
  await tick();
}
function noControl(transport: FakeTransport) {
  for (const method of ["session/attach", "session/create", "session/unload", "session/detach", "turn/cancel", "turn/start"]) {
    assert.equal(transport.log.count(method), 0, method);
  }
}
function appFor(t: TestContext, host: AppServerHost, focus: StartupFocus, reconnect?: () => Promise<AppServerHost>) {
  let editor!: Editor;
  const autocomplete = Editor.prototype.setAutocompleteProvider;
  t.mock.method(Editor.prototype, "setAutocompleteProvider", function(this: Editor, provider: Parameters<Editor["setAutocompleteProvider"]>[0]) {
    editor = this; return autocomplete.call(this, provider);
  });
  let surface!: TUI;
  let input!: Parameters<TUI["addInputListener"]>[0];
  const addInput = TUI.prototype.addInputListener;
  t.mock.method(TUI.prototype, "addInputListener", function(this: TUI, listener: typeof input) {
    surface = this; input = listener; return addInput.call(this, listener);
  });
  const hidden: ResumeSelector[] = [];
  const surfaces: ResumeSelector[] = [];
  const confirmations: ConfirmationView[] = [];
  const commandEntries: ComposerCommand[] = [];
  const feedback: string[] = [];
  const original = TUI.prototype.showOverlay;
  t.mock.method(TUI.prototype, "showOverlay", function(this: TUI, content: Parameters<TUI["showOverlay"]>[0], options: Parameters<TUI["showOverlay"]>[1]) {
    const handle = original.call(this, content, options);
    if (content instanceof PopupFrame && content.content instanceof ConfirmationView) confirmations.push(content.content);
    if (content instanceof PopupFrame && content.content instanceof ComposerCommand) commandEntries.push(content.content);
    if (content instanceof PopupFrame && content.content instanceof ResumeSelector) {
      const surface = content.content;
      surfaces.push(surface);
      const hide = handle.hide;
      t.mock.method(handle, "hide", () => { hidden.push(surface); hide(); });
    }
    return handle;
  });
  const replace = TransientFeedbackSurface.prototype.replace;
  t.mock.method(TransientFeedbackSurface.prototype, "replace", function(this: TransientFeedbackSurface, value: Parameters<TransientFeedbackSurface["replace"]>[0]) {
    feedback.push(value.text); return replace.call(this, value);
  });
  const app = new RustxTuiApp({ host, ...focus, reconnect, sessionSettings: parsedResume().sessionSettings });
  const running = app.run();
  t.after(async () => { await app.quit(); await running; });
  return { app, surfaces, confirmations, commandEntries, hidden, feedback, editor, render: () => surface.render(100).join("\n"), input: (data: string) => input(data) };
}

it("remote missing cwd fails at argument parsing before token reading or connection", () => {
  const result = spawnSync(process.execPath, [fileURLToPath(new URL("../src/main.ts", import.meta.url)),
    "--connect", "ws://127.0.0.1:1", "--token-file", "/nonexistent/token"], { encoding: "utf8" });
  assert.equal(result.status, 2);
  assert.match(result.stderr, /remote Session cwd requires an explicit --workspace/);
  assert.doesNotMatch(result.stderr, /ENOENT|ECONNREFUSED/);
});

it("resume browses without control and attaches only the selected B", async (t) => {
  const { host, transport } = await connected();
  const starting = prepareStartup(host, parsedResume());
  await catalog(transport);
  const focus = await starting;
  assert.equal(focus.session, undefined);
  noControl(transport);
  const h = appFor(t, host, focus);
  assert.equal(h.surfaces.length, 1, "picker exists without an attached projection");
  noControl(transport);
  assert.equal(h.editor.disableSubmit, false);
  h.editor.onSubmit?.("must not reach a runtime");
  await tick();
  noControl(transport);
  h.surfaces[0]!.onSelect!(rows[1]!);
  await attachment(transport, SESSION_B);
  await finishFocus(transport);
  assert.deepEqual(host.attached.map((s) => s.sessionId), [SESSION_B]);
  assert.equal(transport.log.count("session/attach"), 1);
  assert.equal(transport.log.count("session/unload"), 0);
  assert.equal(transport.log.count("turn/cancel"), 0);
  assert.equal(h.feedback.at(-1), `showing session ${SESSION_B}`);
  assert.equal(h.editor.disableSubmit, false);
});

it("a controlled A does not block browsing; its conflict occurs only on selection and B remains selectable", async (t) => {
  const { host, transport } = await connected();
  const starting = prepareStartup(host, parsedResume());
  await catalog(transport);
  const h = appFor(t, host, await starting);
  noControl(transport);
  assert.deepEqual(h.feedback, []);
  const selector = h.surfaces[0]!;
  selector.onSelect!(rows[0]!);
  const [request] = await pendingAttachment(transport);
  assert.equal(paramsOf(request!, "session/attach").session_id, SESSION_A);
  transport.respondError(request!.id, { code: -32000, message: "A is controlled", data: { kind: "controller_in_use" } });
  await tick();
  assert.equal(h.feedback.at(-1), `could not open Session ${SESSION_A}: another client already controls this Session · Input retained; nothing sent.`);
  assert.equal(host.client.closed, undefined);
  assert.equal(host.attached.length, 0);
  assert.equal(h.surfaces.at(-1), selector, "the same picker remains recoverable");
  selector.onSelect!(rows[1]!);
  await attachment(transport, SESSION_B, 2);
  await finishFocus(transport);
  assert.deepEqual(host.attached.map((s) => s.sessionId), [SESSION_B]);
  assert.equal(transport.log.count("session/attach"), 2, "one explicit attempt per chosen identity");
});

for (const resume of [true, false]) for (const model of [undefined, "local/initial"]) {
  it(`${resume ? "empty-catalog resume" : "ordinary startup"} creates and attaches exactly one Session with unchanged remote cwd (model=${model ?? "native default"})`, async (t) => {
    const { host, transport } = await connected();
    const parsed = parseArguments(["--connect", "wss://server.test", "--token-file", "/client/token", "--workspace", "/server/work/../project", ...(resume ? ["--resume"] : []), ...(model ? ["--model", model] : [])]);
    const starting = prepareStartup(host, parsed);
    if (resume) await catalog(transport, []);
    const [create] = await transport.log.awaitMethod("session/create");
    assert.equal(paramsOf(create!, "session/create").settings.cwd, "/server/work/../project");
    assert.deepEqual(paramsOf(create!, "session/create").settings.model, model ? { model } : null);
    transport.respond(create!.id, { type: "session_transition", session: sessionView({ id: SESSION_NEW }) });
    const focus = await starting;
    assert.equal(transport.log.count("session/attach"), 0);
    appFor(t, host, focus);
    await attachment(transport, SESSION_NEW);
    await finishFocus(transport);
    assert.equal(transport.log.count("session/create"), 1);
    assert.equal(transport.log.count("session/attach"), 1);
    assert.equal(transport.log.count("session/list"), resume ? 1 : 0);
    assert.equal(transport.log.count("session/setModel"), 0, "creation is the sole initial model owner");
    await host.shutdown();
  });
}

it("explicit --session/--node attaches that identity directly without browsing or creating", async (t) => {
  const { host, transport } = await connected();
  const starting = prepareStartup(host, parseArguments(["--binary", "rustx", "--session", SESSION_B, "--node", "node_46e1cc43-3b60-768f-a449-f55af17cbce3"]));
  appFor(t, host, await starting);
  const request = await attachment(transport, SESSION_B);
  await finishFocus(transport);
  assert.equal(paramsOf(request, "session/attach").node_id, "node_46e1cc43-3b60-768f-a449-f55af17cbce3");
  assert.equal(transport.log.count("session/list"), 0);
  assert.equal(transport.log.count("session/create"), 0);
  await host.shutdown();
});

it("unfocused reconnect refreshes only the catalog and fences the old picker", async (t) => {
  const first = await connected();
  const next = await connected();
  const starting = prepareStartup(first.host, parsedResume());
  await catalog(first.transport);
  const h = appFor(t, first.host, await starting, async () => next.host);
  const old = h.surfaces[0]!;
  first.transport.fail("socket_error");
  await catalog(next.transport);
  await tick();
  noControl(next.transport);
  assert.equal(h.surfaces.length, 2);
  old.onSelect!(rows[0]!);
  await tick();
  noControl(next.transport);
  h.surfaces[1]!.onSelect!(rows[1]!);
  await attachment(next.transport, SESSION_B);
  await finishFocus(next.transport);
  assert.equal(next.transport.log.count("session/attach"), 1);
});

it("a lost first attachment response returns to unfocused browsing without replay", async (t) => {
  const first = await connected();
  const next = await connected();
  const starting = prepareStartup(first.host, parsedResume());
  await catalog(first.transport);
  const h = appFor(t, first.host, await starting, async () => next.host);
  h.surfaces[0]!.onSelect!(rows[0]!);
  const [pending] = await pendingAttachment(first.transport);
  assert.equal(paramsOf(pending!, "session/attach").session_id, SESSION_A);
  first.transport.fail("socket_error");
  await catalog(next.transport);
  await tick();
  assert.equal(first.transport.log.count("session/attach"), 1);
  noControl(next.transport);
  assert.equal(next.host.attached.length, 0);
  assert.equal(h.editor.disableSubmit, false);
  assert.match(h.feedback.at(-1)!, /no unanswered mutations were resent/);
  h.surfaces.at(-1)!.onSelect!(rows[1]!);
  await attachment(next.transport, SESSION_B);
  await finishFocus(next.transport);
  assert.deepEqual(next.host.attached.map((s) => s.sessionId), [SESSION_B]);
});

async function createFromEmpty(h: ReturnType<typeof appFor>, host: AppServerHost, transport: FakeTransport) {
  const selector = h.surfaces.at(-1)!;
  assert.deepEqual(selector.selector.visibleSessions(), []);
  assert.match(selector.render(80).join("\n"), /No Sessions available.*\n.*\n.*New Session/);
  assert.match(selector.popupFooter().join(), /Enter New Session/);
  assert.equal(h.editor.disableSubmit, false);
  noControl(transport);
  selector.handleInput("\r");
  const [create] = await transport.log.awaitMethod("session/create");
  selector.handleInput("\r");
  assert.equal(transport.log.count("session/create"), 1, "pending action is single-submit");
  assert.deepEqual(paramsOf(create!, "session/create").settings, parsedResume().sessionSettings);
  transport.respond(create!.id, { type: "session_transition", session: sessionView({ id: SESSION_CREATED }),
    editor:transport.editor(sessionView({ id: SESSION_CREATED }),[{ type: "text", text: "transition draft" }]), durability_diagnostic: "durability test notice" });
  const attach = await attachment(transport, SESSION_CREATED);
  assert.equal(paramsOf(attach, "session/attach").node_id, sessionView().active_node);
  await finishFocus(transport);
  assert.deepEqual(host.attached.map((s) => s.sessionId), [SESSION_CREATED]);
  assert.equal(h.editor.disableSubmit, false);
  assert.equal(h.editor.getText(), "transition draft");
  assert.match(h.feedback.at(-1)!, /durability test notice/);
  assert.equal(transport.log.count("session/create"), 1);
  assert.equal(transport.log.count("session/attach"), 1);
  for (const method of ["session/unload", "session/detach", "turn/cancel", "turn/start"]) assert.equal(transport.log.count(method), 0);
  assert.equal(transport.disposed, false, "creation preserves the connected host");
  selector.handleInput("\r");
  assert.equal(transport.log.count("session/create"), 1, "closed picker cannot create again");
}

it("delete last Session leaves an empty unfocused picker until explicit New Session", async (t) => {
  const { host, transport } = await connected();
  const starting = prepareStartup(host, parsedResume());
  await catalog(transport, rows.slice(0, 1));
  const h = appFor(t, host, await starting);
  const selector = h.surfaces[0]!;
  selector.handleInput("\x04");
  const [preview] = await transport.log.awaitMethod("session/deletePreview");
  transport.respond(preview!.id, { type: "deletion", result: { status: "preview", preview: {
    session_id: SESSION_A, target_revision: "revision-A", owned_node_count: 1, owned_conversation_count: 1, owned_child_count: 0,
  } } });
  await tick();
  selector.handleInput("\t"); selector.handleInput("\r");
  const [deletion] = await transport.log.awaitMethod("session/delete");
  assert.deepEqual(paramsOf(deletion!, "session/delete"), { session_id: SESSION_A, expected_target_revision: "revision-A" });
  transport.respond(deletion!.id, { type: "deletion", result: { status: "deleted", session_id: SESSION_A } });
  await catalog(transport, [], 2);
  await tick();
  assert.equal(h.surfaces.at(-1), selector);
  await createFromEmpty(h, host, transport);
  assert.equal(transport.log.count("session/delete"), 1);
});

it("unfocused reconnect to empty catalog initializes and reads before explicit creation", async (t) => {
  const first = await connected();
  let next!: Awaited<ReturnType<typeof connected>>;
  let reconnects = 0;
  const starting = prepareStartup(first.host, parsedResume());
  await catalog(first.transport);
  const h = appFor(t, first.host, await starting, async () => { reconnects++; next = await connected(); return next.host; });
  first.transport.fail("socket_error");
  await tick();
  await catalog(next.transport, []);
  await tick();
  assert.equal(next.transport.log.count("initialize"), 1);
  assert.equal(next.transport.log.count("session/list"), 1);
  noControl(first.transport);
  h.surfaces[0]!.onCreate!(); // retired surface is fenced
  await createFromEmpty(h, next.host, next.transport);
  assert.equal(reconnects, 1, "creation does not replace the host");
});

it("create failure retains the authoritative empty picker without attach or retry", async (t) => {
  const { host, transport } = await connected();
  const h = appFor(t, host, { resumePage: { sessions: [] } });
  const selector = h.surfaces[0]!;
  selector.handleInput("\r");
  const [create] = await transport.log.awaitMethod("session/create");
  transport.respondError(create!.id, { code: -32000, message: "create rejected", data: { kind: "operation_failed" } });
  await tick();
  assert.match(h.feedback.at(-1)!, /create rejected/);
  assert.match(selector.render(80).join(), /New Session/);
  assert.deepEqual(h.hidden, []);
  assert.equal(host.attached.length, 0);
  assert.equal(h.editor.disableSubmit, false);
  assert.equal(transport.log.count("session/create"), 1);
  assert.equal(transport.log.count("session/attach"), 0);
  assert.equal(transport.log.count("session/delete"), 0);
});

it("committed create retires empty authority before failed attach; reopening reads the durable Session", async (t) => {
  const { host, transport } = await connected();
  const h = appFor(t, host, { resumePage: { sessions: [] } });
  const stale = h.surfaces[0]!;
  stale.handleInput("\r");
  const [create] = await transport.log.awaitMethod("session/create");
  transport.respond(create!.id, { type: "session_transition", session: sessionView({ id: SESSION_CREATED }) });
  const [attach] = await pendingAttachment(transport);
  assert.deepEqual(h.hidden, [stale], "empty authority retires before attach settles");
  transport.respondError(attach!.id, { code: -32000, message: "attach rejected", data: { kind: "operation_failed" } });
  await tick();
  assert.match(h.feedback.at(-1)!, /attach rejected/);
  assert.equal(host.attached.length, 0);
  assert.equal(h.editor.disableSubmit, false);
  stale.handleInput("\r");
  await stale.onCreate!();
  assert.equal(transport.log.count("session/create"), 1, "retired picker cannot create again");
  assert.equal(transport.log.count("session/attach"), 1);
  assert.equal(transport.log.count("session/list"), 0, "no implicit reconciliation or mutation");
  for (const method of ["session/delete", "session/unload", "session/detach", "turn/cancel"]) assert.equal(transport.log.count(method), 0);
  assert.equal(transport.disposed, false, "same host is retained");

  h.input("\r"); // Explicitly reopen resume through the unfocused input path.
  const createdRow = { ...rows[0]!, id: SESSION_CREATED };
  await catalog(transport, [createdRow]);
  await tick();
  const fresh = h.surfaces.at(-1)!;
  assert.notEqual(fresh, stale);
  assert.deepEqual(fresh.selector.visibleSessions(), [createdRow]);
  assert.doesNotMatch(fresh.render(80).join(), /No Sessions available|New Session/);
  assert.equal(transport.log.count("session/create"), 1);
  assert.equal(transport.log.count("session/attach"), 1, "catalog refresh does not retry attachment");
  fresh.handleInput("\r");
  await attachment(transport, SESSION_CREATED, 2);
  await finishFocus(transport);
  assert.equal(transport.log.count("session/create"), 1);
  assert.equal(h.editor.disableSubmit, false);
  assert.deepEqual(host.attached.map((session) => session.sessionId), [SESSION_CREATED]);
});

for (const stage of ["create", "attach"] as const) {
  it(`lost empty-picker ${stage} response reconnects read-only without mutation replay`, async (t) => {
    const first = await connected();
    const next = await connected();
    const h = appFor(t, first.host, { resumePage: { sessions: [] } }, async () => next.host);
    const old = h.surfaces[0]!;
    old.handleInput("\r");
    const [create] = await first.transport.log.awaitMethod("session/create");
    if (stage === "attach") {
      first.transport.respond(create!.id, { type: "session_transition", session: sessionView({ id: SESSION_CREATED }) });
      await pendingAttachment(first.transport);
    }
    first.transport.fail("socket_error");
    await catalog(next.transport, []);
    await tick();
    assert.equal(first.transport.log.count("session/create"), 1);
    assert.equal(first.transport.log.count("session/attach"), stage === "attach" ? 1 : 0);
    noControl(next.transport);
    assert.equal(h.editor.disableSubmit, false);
    assert.match(h.surfaces.at(-1)!.render(80).join(), /New Session/);
    assert.match(h.feedback.at(-1)!, /no unanswered mutations were resent/);
    await old.onCreate!();
    noControl(next.transport);
  });
}

it('cold TUI navigation resolves one exact node before attachment and first submission intent', async t => {
  const { host, transport } = await connected();
  const focus = await prepareStartup(host, parseArguments(['--binary', 'rustx', '--session', SESSION_A]));
  const h = appFor(t, host, focus);
  const metadata = await transport.nextPending('session/read');
  assert.equal(transport.log.count('session/attach'), 0);
  assert.equal(transport.log.count('session/submissionIntent'), 0);
  h.editor.setText('draft during metadata');
  const node = 'node_01900000-0000-7000-8000-000000001006';
  transport.respond(metadata.id, { type: 'session', session: sessionView({ id: SESSION_A, active_node: node }) });
  const [attach] = await transport.log.awaitMethod('session/attach');
  assert.equal(paramsOf(attach!, 'session/attach').node_id, node);
  assert.equal(h.editor.getText(), 'draft during metadata');
  assert.equal(h.editor.disableSubmit, false);
  h.editor.handleInput('\r');
  const [intent] = await transport.log.awaitMethod('session/submissionIntent');
  assert.deepEqual(paramsOf(intent!, 'session/submissionIntent'), { session_id: SESSION_A, node_id: node });
  assert.equal(transport.answered.has(attach!.id), false, 'metadata and intent never wait for MCP attachment');
  assert.equal(transport.log.count('turn/start'), 0);
});

for (const outcome of ["ready", "withdraw", "disconnect", "rejection", "intent-rejection"] as const) {
  it(`startup accepts input before attachment and handles ${outcome} without replay`, async (t) => {
    const { host, transport } = await connected();
    const focus = await prepareStartup(host, parseArguments(["--binary", "rustx", "--session", SESSION_A]));
    const h = appFor(t, host, focus);
    await pendingAttachment(transport);
    assert.equal(h.editor.disableSubmit, false);
    assert.equal(transport.log.count('session/submissionIntent'), 0, 'early attachment does not consume submission wait');
    assert.doesNotMatch(h.render(), /Connecting/);
    const [history] = await transport.log.awaitMethod("session/history");
    transport.respond(history!.id, { type: "session_history", conversation_id: "conv_01900000-0000-7000-8000-000000000002", window: { cut: { conversation_id: 'conv_01900000-0000-7000-8000-000000000002', journal: '1000', transcript: '1000', mutation_revision: '0' ,ledger_through:'0',surface_revision:'0',transcript_total:'0',pending_through:'0',pending_total:'0'}, page: { entries: [{ cursor: "1", item: { type: "message", message: { id: "saved", role: "user", source: "human", content: [{ type: "text", text: "Previously saved message" }] } } }] } } });
    await tick();
    assert.match(h.render(), /Previously saved message/);
    assert.doesNotMatch(h.render(), /Connecting/);
    h.editor.setText("queued while connecting");
    h.editor.handleInput("\r");
    const [intent] = await transport.log.awaitMethod('session/submissionIntent');
    assert.deepEqual(paramsOf(intent!, 'session/submissionIntent'), { session_id: SESSION_A, node_id: sessionView().active_node });
    if (outcome !== 'ready' && outcome !== 'intent-rejection') transport.respond(intent!.id, { type: 'submission_intent_accepted' });
    assert.equal(transport.log.count("turn/start"), 0);
    assert.equal(h.editor.disableSubmit, true);
    assert.match(h.render(), /Connecting/);
    assert.match(h.render(), /Previously saved message/);
    if (outcome === 'intent-rejection') {
      transport.respondError(intent!.id, { code: -32000, message: 'Submission preparation rejected' });
      await tick();
      assert.equal(h.editor.getText(), 'queued while connecting');
      assert.equal(h.editor.disableSubmit, false);
      await attachment(transport, SESSION_A);
      await finishFocus(transport);
    } else if (outcome === "rejection") {
      const [request] = await pendingAttachment(transport);
      transport.respondError(request!.id, { code: -32000, message: "Preparation failed" });
      await tick();
      assert.equal(h.editor.getText(), "queued while connecting");
    } else if (outcome === "disconnect") {
      transport.fail("socket_error"); await tick();
      assert.equal(h.editor.getText(), "queued while connecting");
    } else {
      if (outcome === "withdraw") h.input("\x1b");
      await attachment(transport, SESSION_A);
      await finishFocus(transport);
      if (outcome === "ready") {
        assert.equal(transport.log.count('turn/start'), 0, 'a held intent ACK cannot dispatch retained text');
        transport.respond(intent!.id, { type: 'submission_intent_accepted' });
        const [request] = await transport.log.awaitMethod("turn/start");
        assert.deepEqual(paramsOf(request!, "turn/start").content, [{ type: "text", text: "queued while connecting" }]);
      } else assert.equal(h.editor.getText(), "queued while connecting");
    }
    assert.equal(transport.log.count("turn/start"), outcome === "ready" ? 1 : 0);
    assert.equal(transport.log.count("session/attach"), 1);
    assert.equal(transport.log.count('session/submissionIntent'), 1);
  });
}

for (const outcome of ['ready', 'withdraw', 'intent-rejection', 'attachment-rejection', 'disconnect', 'quit', 'authority'] as const) {
  it(`confirmed cold reconnect accepts a new Send before attachment (${outcome})`, async t => {
    const first = await connected();
    const next = await connected(outcome === 'authority' ? 'replacement-native-authority' : undefined);
    const node = sessionView().active_node;
    const attaching = first.host.attach(SESSION_A, node);
    await attachment(first.transport, SESSION_A);
    const previous = await attaching;
    let reconnects = 0;
    const h = appFor(t, first.host, { session: previous }, async () => { reconnects++; return next.host; });
    const info = await first.transport.nextPending('session/read');
    first.transport.respond(info.id, { type: 'session', session: sessionView({ id: SESSION_A }) });
    if (outcome === 'ready') {
      h.editor.setText('unanswered old request');
      h.editor.handleInput('\r');
      await first.transport.log.awaitMethod('turn/start');
    }
    h.editor.setText('unsent draft before disconnect');
    first.transport.fail('socket_error');
    const pending = (await next.transport.log.awaitMethod('session/attach')).at(-1)!;
    assert.deepEqual(paramsOf(pending, 'session/attach'), { session_id: SESSION_A, node_id: node });
    assert.equal(h.editor.disableSubmit, false, 'initialized replacement owns an editable cold connecting surface');
    assert.equal(h.editor.getText(), 'unsent draft before disconnect');
    assert.equal(next.transport.log.count('session/submissionIntent'), 0, 'recovery itself does not arm startup');
    assert.equal(next.transport.log.count('turn/start'), 0);
    h.editor.setText('new intentional submission after disconnect');
    h.editor.handleInput('\r');
    const intent = (await next.transport.log.awaitMethod('session/submissionIntent')).at(-1)!;
    assert.deepEqual(paramsOf(intent, 'session/submissionIntent'), { session_id: SESSION_A, node_id: node });
    assert.equal(next.transport.answered.has(pending.id), false, 'intent precedes held MCP attachment completion');
    assert.equal(h.editor.disableSubmit, true);
    if (outcome === 'intent-rejection') {
      next.transport.respondError(intent.id, { code: -32000, message: 'Submission preparation rejected' });
      await tick();
      assert.equal(h.editor.disableSubmit, false);
      assert.equal(h.editor.getText(), 'new intentional submission after disconnect');
    } else if (outcome !== 'ready' && outcome !== 'authority') {
      next.transport.respond(intent.id, { type: 'submission_intent_accepted' });
    }
    if (outcome === 'withdraw') h.input('\x1b');
    if (outcome === 'disconnect' || outcome === 'quit') {
      if (outcome === 'quit') await h.app.quit();
      else next.transport.fail('socket_error');
      await tick();
      assert.equal(next.transport.log.count('turn/start'), 0);
      assert.equal(reconnects, 1, 'a lost continuation never automatically replays a submission');
      next.transport.respond(pending.id, { type: 'attached', target: previous.target, configuration_revision: '0', attachment_kind: 'controller', observation_source: 'live' });
      await tick();
      assert.equal(next.transport.log.count('turn/start'), 0, 'late old-carrier success cannot forward retained text');
    } else if (outcome === 'attachment-rejection') {
      next.transport.respondError(pending.id, { code: -32000, message: 'Preparation failed' });
      await tick();
      assert.equal(h.editor.getText(), 'new intentional submission after disconnect');
      assert.equal(next.transport.log.count('turn/start'), 0);
      assert.match(h.feedback.at(-1)!, /could not open Session.*Input retained/);
    } else {
      const target = { ...previous.target, attachment_id: 'cold-reconnect-attachment', runtime_incarnation: '2' };
      next.transport.respond(pending.id, { type: 'attached', target, configuration_revision: '0', attachment_kind: 'controller', observation_source: 'live' });
      await followAttachment(next.transport, target, snapshot(), '0');
      await finishFocus(next.transport);
      assert.equal(next.transport.log.count('turn/start'), 0, 'held intent acknowledgement fences forwarding');
      if (outcome === 'ready' || outcome === 'authority') {
        next.transport.respond(intent.id, { type: 'submission_intent_accepted' });
        const submitted = (await next.transport.log.awaitMethod('turn/start')).at(-1)!;
        assert.deepEqual(paramsOf(submitted, 'turn/start'), { target, content: [{ type: 'text', text: 'new intentional submission after disconnect' }] });
        next.transport.respond(submitted.id, { type: 'inbound_accepted', message_id: 'new-input', inbound_sequence: '1' });
        await tick();
      } else assert.equal(h.editor.getText(), 'new intentional submission after disconnect');
    }
    assert.equal(first.transport.log.count('turn/start'), outcome === 'ready' ? 1 : 0, 'an unanswered old request is never replayed');
    assert.equal(next.transport.log.count('session/attach'), 1);
    assert.equal(next.transport.log.count('session/submissionIntent'), 1);
    assert.equal(next.transport.log.count('turn/start'), outcome === 'ready' || outcome === 'authority' ? 1 : 0);
    if (outcome === 'ready' || outcome === 'authority') assert.equal(h.editor.getText(), '', 'accepted recovery input cannot restore the earlier unsent draft');
  });
}

for (const edit of ['send P', 'replace with Q and send', 'clear without Send', 'send then type a newer draft'] as const) {
  it(`cold reconnect draft ownership: ${edit}`, async t => {
    const first = await connected();
    const next = await connected();
    const node = sessionView().active_node;
    const attaching = first.host.attach(SESSION_A, node);
    await attachment(first.transport, SESSION_A);
    const previous = await attaching;
    let reconnects = 0;
    const h = appFor(t, first.host, { session: previous }, async () => { reconnects++; return next.host; });
    const info = await first.transport.nextPending('session/read');
    first.transport.respond(info.id, { type: 'session', session: sessionView({ id: SESSION_A }) });
    h.editor.setText('unanswered old input');
    h.editor.handleInput('\r');
    await first.transport.log.awaitMethod('turn/start');
    const beforeDisconnect = 'draft P before disconnect';
    h.editor.setText(beforeDisconnect);
    first.transport.fail('socket_error');
    const pending = (await next.transport.log.awaitMethod('session/attach')).at(-1)!;
    assert.deepEqual(paramsOf(pending, 'session/attach'), { session_id: SESSION_A, node_id: node });
    assert.equal(h.editor.disableSubmit, false);
    assert.equal(h.editor.getText(), beforeDisconnect);
    assert.equal(next.transport.log.count('session/submissionIntent'), 0);
    assert.equal(next.transport.log.count('turn/start'), 0);

    const sends = edit !== 'clear without Send';
    const submittedText = edit === 'replace with Q and send' ? 'replacement draft Q' : beforeDisconnect;
    let intent: Awaited<ReturnType<typeof next.transport.nextPending>> | undefined;
    if (sends) {
      h.editor.setText(submittedText);
      h.editor.handleInput('\r');
      intent = (await next.transport.log.awaitMethod('session/submissionIntent')).at(-1)!;
      assert.deepEqual(paramsOf(intent, 'session/submissionIntent'), { session_id: SESSION_A, node_id: node });
      assert.equal(h.editor.getText(), '', 'the explicit Send transfers only its captured draft');
      assert.equal(next.transport.answered.has(pending.id), false, 'Send records intent while Attach is still held');
    } else h.editor.setText('');
    const newer = 'new text typed after the captured Send';
    if (edit === 'send then type a newer draft') {
      h.editor.handleInput(newer);
      assert.equal(h.editor.getText(), newer);
    }

    const target = { ...previous.target, attachment_id: 'draft-reconnect-attachment', runtime_incarnation: '2' };
    next.transport.respond(pending.id, { type: 'attached', target, configuration_revision: '0', attachment_kind: 'controller', observation_source: 'live' });
    await followAttachment(next.transport, target, snapshot(), '0');
    await finishFocus(next.transport);
    assert.equal(next.transport.log.count('turn/start'), 0, 'Attach cannot forward a queued input before its intent ACK');
    if (edit === 'send then type a newer draft') assert.equal(h.editor.getText(), newer, 'Attach preserves typing after the captured Send');
    let expectedDraft = edit === 'send then type a newer draft' ? newer : '';
    if (intent) {
      next.transport.respond(intent.id, { type: 'submission_intent_accepted' });
      const submitted = (await next.transport.log.awaitMethod('turn/start')).at(-1)!;
      assert.deepEqual(paramsOf(submitted, 'turn/start'), { target, content: [{ type: 'text', text: submittedText }] });
      if (edit === 'send then type a newer draft') {
        assert.equal(h.editor.getText(), newer);
        const whileAcceptanceHeld = ' and S typed before the inbound acknowledgement';
        h.editor.handleInput(whileAcceptanceHeld);
        expectedDraft += whileAcceptanceHeld;
        assert.equal(h.editor.getText(), expectedDraft);
        assert.equal(next.transport.answered.has(submitted.id), false, 'new typing belongs to the editor while the old input ACK is held');
      }
      next.transport.respond(submitted.id, { type: 'inbound_accepted', message_id: 'new-draft-input', inbound_sequence: '1' });
      await tick();
    }
    assert.equal(reconnects, 1);
    assert.equal(first.transport.log.count('turn/start'), 1, 'the old uncertain input is never automatically replayed');
    assert.equal(next.transport.log.count('session/attach'), 1);
    assert.equal(next.transport.log.count('session/submissionIntent'), sends ? 1 : 0);
    assert.equal(next.transport.log.count('turn/start'), sends ? 1 : 0, 'one owned continuation for each explicit Send');
    assert.equal(h.editor.getText(), expectedDraft, 'attachment completion must not resurrect the pre-disconnect draft or clear newer typing');
  });
}

it('ordinary A to B to A focus restores each cached Session draft without execution', async t => {
  const { host, transport } = await connected();
  const node = sessionView().active_node;
  const attaching = host.attach(SESSION_A, node);
  await attachment(transport, SESSION_A);
  const initial = await attaching;
  const observedRows = rows.map(row => ({ ...row, active_node: node }));
  const h = appFor(t, host, { session: initial });
  const info = await transport.nextPending('session/read');
  transport.respond(info.id, { type: 'session', session: sessionView({ id: SESSION_A }) });
  const draftA = 'unsent text owned by A';
  const draftB = 'different unsent text owned by B';
  h.editor.setText(draftA);
  const select = async (id: string, count: number) => {
    h.input('\x10');
    const command = h.commandEntries.at(-1)!;
    command.handleInput('resume');
    command.handleInput('\r');
    await catalog(transport, observedRows, count);
    await tick();
    h.surfaces.at(-1)!.onSelect!(observedRows.find(row => row.id === id)!);
    const metadata = await transport.nextPending('session/read');
    assert.equal(paramsOf(metadata, 'session/read').session_id, id);
    transport.respond(metadata.id, { type: 'session', session: sessionView({ id, active_node: node }) });
  };
  await select(SESSION_B, 1);
  await attachment(transport, SESSION_B, 2);
  await finishFocus(transport);
  assert.equal(h.editor.getText(), '', 'a first visit to B cannot borrow A input');
  h.editor.setText(draftB);
  await select(SESSION_A, 2);
  await finishFocus(transport);
  assert.equal(h.editor.getText(), draftA, 'returning to A restores only its cached draft');
  await select(SESSION_B, 3);
  await finishFocus(transport);
  assert.equal(h.editor.getText(), draftB, 'returning to B preserves its independent cached draft too');
  assert.deepEqual(host.attached.map(session => session.sessionId), [SESSION_A, SESSION_B]);
  assert.equal(transport.log.count('session/attach'), 2, 'ordinary refocus reuses each confirmed attachment');
  for (const method of ['session/submissionIntent', 'turn/start', 'turn/cancel', 'session/detach', 'session/unload']) {
    assert.equal(transport.log.count(method), 0, 'focus alone owns no execution or retirement');
  }
});

it('a selector retained from before disconnect cannot switch the cold reconnect submission target', async t => {
  const first = await connected();
  const next = await connected();
  const node = sessionView().active_node;
  const attaching = first.host.attach(SESSION_A, node);
  await attachment(first.transport, SESSION_A);
  const h = appFor(t, first.host, { session: await attaching }, async () => next.host);
  const info = await first.transport.nextPending('session/read');
  first.transport.respond(info.id, { type: 'session', session: sessionView({ id: SESSION_A }) });
  h.editor.setText('/resume'); h.editor.handleInput('\r');
  await catalog(first.transport); await tick();
  const oldSelector = h.surfaces.at(-1)!;
  first.transport.fail('socket_error');
  const pending = (await next.transport.log.awaitMethod('session/attach')).at(-1)!;
  oldSelector.onSelect!(rows[1]!); await tick();
  assert.equal(next.transport.log.count('session/attach'), 1);
  assert.deepEqual(paramsOf(pending, 'session/attach'), { session_id: SESSION_A, node_id: node });
  h.editor.setText('new request stays with the confirmed target'); h.editor.handleInput('\r');
  const intent = (await next.transport.log.awaitMethod('session/submissionIntent')).at(-1)!;
  assert.deepEqual(paramsOf(intent, 'session/submissionIntent'), { session_id: SESSION_A, node_id: node });
  next.transport.respond(intent.id, { type: 'submission_intent_accepted' });
  next.transport.respondError(pending.id, { code: -32000, message: 'End controlled preparation' });
  await tick();
  assert.equal(next.transport.log.count('turn/start'), 0);
  assert.equal(next.transport.log.count('session/create'), 0);
  assert.equal(h.editor.getText(), 'new request stays with the confirmed target');
});

for (const outcome of ['ready', 'rejected', 'unknown'] as const) {
  it(`branch switch exposes the new submission target only after acknowledgement (${outcome})`, async t => {
    const { host, transport } = await connected();
    const attaching = host.attach(SESSION_A);
    await attachment(transport, SESSION_A);
    const h = appFor(t, host, { session: await attaching });
    const read = await transport.nextPending('session/read');
    transport.respond(read.id, { type: 'session', session: sessionView({ id: SESSION_A }) });
    const nextNode = 'node_01900000-0000-7000-8000-000000001005';
    const submit = CommandDispatcher.prototype.submit;
    t.mock.method(CommandDispatcher.prototype, 'submit', function(this: CommandDispatcher, ...args: Parameters<CommandDispatcher['submit']>) {
      if (args[0] === '/tree') return Promise.resolve({ kind: 'focus_session' as const, sessionId: SESSION_A, nodeId: nextNode });
      return submit.apply(this, args);
    });
    h.editor.setText('/tree');
    h.editor.handleInput('\r');
    await tick();
    const confirmation = h.confirmations.at(-1)!;
    assert.ok(confirmation);
    confirmation.handleInput('\t');
    confirmation.handleInput('\r');
    const [switched] = await transport.log.awaitMethod('session/switchNode');
    assert.equal(paramsOf(switched!, 'session/switchNode').node_id, nextNode);
    assert.equal(h.editor.disableSubmit, true, 'unknown switch target cannot accept Send');
    h.editor.setText('retained before switch confirmation');
    h.editor.handleInput('\r');
    assert.equal(transport.log.count('session/submissionIntent'), 0);
    assert.equal(transport.log.count('turn/start'), 0);
    assert.equal(transport.log.count('session/attach'), 1);

    if (outcome === 'ready') {
      transport.respond(switched!.id, { type: 'session', session: sessionView({ id: SESSION_A, active_node: nextNode }) });
      const successor = (await transport.log.awaitMethod('session/attach', 2)).at(-1)!;
      assert.equal(paramsOf(successor, 'session/attach').node_id, nextNode);
      assert.equal(h.editor.disableSubmit, false, 'confirmed target remains editable while native preparation holds attach');
      h.editor.setText('first message on new branch');
      assert.equal(h.editor.getText(), 'first message on new branch');
      h.editor.handleInput('\r');
      const [intent] = await transport.log.awaitMethod('session/submissionIntent');
      assert.deepEqual(paramsOf(intent!, 'session/submissionIntent'), { session_id: SESSION_A, node_id: nextNode });
      transport.respond(intent!.id, { type: 'submission_intent_accepted' });
      assert.equal(transport.answered.has(successor.id), false);
      assert.equal(transport.log.count('turn/start'), 0);
      await attachment(transport, SESSION_A, 2);
      await finishFocus(transport);
      const [started] = await transport.log.awaitMethod('turn/start');
      assert.deepEqual(paramsOf(started!, 'turn/start').content, [{ type: 'text', text: 'first message on new branch' }]);
      assert.equal(host.attachment(SESSION_A)?.nodeId, nextNode);
      assert.equal(transport.log.count('turn/start'), 1);
      assert.equal(transport.log.count('session/submissionIntent'), 1);
    } else {
      if (outcome === 'rejected') transport.respondError(switched!.id, { code: -32000, message: 'Switch rejected' });
      else transport.fail('socket_error');
      await tick();
      assert.equal(h.editor.getText(), 'retained before switch confirmation');
      assert.equal(transport.log.count('session/attach'), 1);
      assert.equal(transport.log.count('session/submissionIntent'), 0);
      assert.equal(transport.log.count('turn/start'), 0);
    }
  });
}

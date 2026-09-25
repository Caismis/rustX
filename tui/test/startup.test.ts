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
const rows = [SESSION_A, SESSION_B].map((id) => ({ id, name: `Session ${id}`, cwd: "/server/work", active_node: `node_${id.slice(4)}`, updated_at: "2026-09-14T00:00:00Z" }));

async function connected() {
  const transport = new FakeTransport();
  const pending = AppServerClient.initialize({ transport });
  const [request] = await transport.log.awaitMethod("initialize");
  transport.respond(request!.id, { type: "initialized", protocol_version: 24, capabilities: SERVER_CAPABILITIES });
  return { transport, host: new AppServerHost({ client: await pending, ownership: "external" }) };
}
async function catalog(transport: FakeTransport, sessions = rows, count = 1) {
  const request = (await transport.log.awaitMethod("session/list", count)).at(-1)!;
  transport.respond(request.id, { type: "sessions", sessions, });
}
async function attachment(transport: FakeTransport, id: string, count = 1) {
  const request = (await transport.log.awaitMethod("session/attach", count)).at(-1)!;
  assert.equal(paramsOf(request, "session/attach").session_id, id);
  transport.respond(request.id, { type: "attached", target: {
    session_id: id, conversation_id: "conv_01900000-0000-7000-8000-000000000002", attachment_id: `att-${id}`, runtime_incarnation: "1",
  }, snapshot: snapshot(), cursor: "0" });
  return request;
}
async function finishFocus(transport: FakeTransport) {
  const read = (await transport.log.awaitMethod("session/read")).at(-1)!;
  transport.respond(read.id, { type: "session", session: sessionView({ id: paramsOf(read, "session/read").session_id }) });
  const repair = (await transport.log.awaitMethod("session/snapshot")).at(-1)!;
  transport.respond(repair.id, { type: "snapshot", snapshot: snapshot(), cursor: "0" });
  const subscribe = (await transport.log.awaitMethod("session/subscribe")).at(-1)!;
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
  let input!: Parameters<TUI["addInputListener"]>[0];
  const addInput = TUI.prototype.addInputListener;
  t.mock.method(TUI.prototype, "addInputListener", function(this: TUI, listener: typeof input) {
    input = listener; return addInput.call(this, listener);
  });
  const hidden: ResumeSelector[] = [];
  const surfaces: ResumeSelector[] = [];
  const feedback: string[] = [];
  const original = TUI.prototype.showOverlay;
  t.mock.method(TUI.prototype, "showOverlay", function(this: TUI, content: Parameters<TUI["showOverlay"]>[0], options: Parameters<TUI["showOverlay"]>[1]) {
    const handle = original.call(this, content, options);
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
  return { app, surfaces, hidden, feedback, editor, input: (data: string) => input(data) };
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
  const [request] = await transport.log.awaitMethod("session/attach");
  assert.equal(paramsOf(request!, "session/attach").session_id, SESSION_A);
  transport.respondError(request!.id, { code: -32000, message: "A is controlled", data: { kind: "controller_in_use" } });
  await tick();
  assert.equal(h.feedback.at(-1), `could not open Session ${SESSION_A}: another client already controls this Session`);
  assert.equal(host.client.closed, undefined);
  assert.equal(host.attached.length, 0);
  assert.equal(h.surfaces.at(-1), selector, "the same picker remains recoverable");
  selector.onSelect!(rows[1]!);
  await attachment(transport, SESSION_B, 2);
  await finishFocus(transport);
  assert.deepEqual(host.attached.map((s) => s.sessionId), [SESSION_B]);
  assert.equal(transport.log.count("session/attach"), 2, "one explicit attempt per chosen identity");
});

for (const resume of [true, false]) {
  it(`${resume ? "empty-catalog resume" : "ordinary startup"} creates and attaches exactly one Session with unchanged remote cwd`, async () => {
    const { host, transport } = await connected();
    const parsed = parseArguments(["--connect", "wss://server.test", "--token-file", "/client/token", "--workspace", "/server/work/../project", ...(resume ? ["--resume"] : [])]);
    const starting = prepareStartup(host, parsed);
    if (resume) await catalog(transport, []);
    const [create] = await transport.log.awaitMethod("session/create");
    assert.equal(paramsOf(create!, "session/create").settings.cwd, "/server/work/../project");
    transport.respond(create!.id, { type: "session_transition", session: sessionView({ id: SESSION_NEW }) });
    await attachment(transport, SESSION_NEW);
    assert.equal((await starting).session?.sessionId, SESSION_NEW);
    assert.equal(transport.log.count("session/create"), 1);
    assert.equal(transport.log.count("session/attach"), 1);
    assert.equal(transport.log.count("session/list"), resume ? 1 : 0);
    await host.shutdown();
  });
}

it("explicit --session/--node attaches that identity directly without browsing or creating", async () => {
  const { host, transport } = await connected();
  const starting = prepareStartup(host, parseArguments(["--binary", "rustx", "--session", SESSION_B, "--node", "node_46e1cc43-3b60-768f-a449-f55af17cbce3"]));
  const request = await attachment(transport, SESSION_B);
  assert.equal(paramsOf(request, "session/attach").node_id, "node_46e1cc43-3b60-768f-a449-f55af17cbce3");
  assert.equal((await starting).session?.sessionId, SESSION_B);
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
  const [pending] = await first.transport.log.awaitMethod("session/attach");
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
    editor_content: [{ type: "text", text: "transition draft" }], durability_diagnostic: "durability test notice" });
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
  const [attach] = await transport.log.awaitMethod("session/attach");
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
      await first.transport.log.awaitMethod("session/attach");
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

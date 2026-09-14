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
import { fixtures } from "../../protocol/app-server/fixtures.ts";
import { FakeTransport, paramsOf, tick } from "./support/app-server-peer.ts";
import { SERVER_CAPABILITIES } from "./support/app-server-harness.ts";
import { snapshot, sessionView } from "./support/fixtures.ts";

const parsedResume = () => parseArguments(["--binary", "rustx", "--resume", "--cwd", "/server/work"]);
const rows = ["A", "B"].map((id) => ({ id, name: `Session ${id}`, active_node: `node-${id}`, updated_at: "2026-09-14T00:00:00Z" }));
const diagnostics = fixtures.flatMap((f) => "result" in f && f.result?.type === "diagnostics" ? [f.result] : [])[0]!;

async function connected() {
  const transport = new FakeTransport();
  const pending = AppServerClient.initialize({ transport });
  const [request] = await transport.log.awaitMethod("initialize");
  transport.respond(request!.id, { type: "initialized", protocol_version: 1, capabilities: SERVER_CAPABILITIES });
  return { transport, host: new AppServerHost({ client: await pending, ownership: "external" }) };
}
async function catalog(transport: FakeTransport, sessions = rows, count = 1) {
  const request = (await transport.log.awaitMethod("session/list", count)).at(-1)!;
  transport.respond(request.id, { type: "sessions", sessions });
  const diagnostic = (await transport.log.awaitMethod("server/diagnostics", count)).at(-1)!;
  transport.respond(diagnostic.id, diagnostics);
}
async function attachment(transport: FakeTransport, id: string, count = 1) {
  const request = (await transport.log.awaitMethod("session/attach", count)).at(-1)!;
  assert.equal(paramsOf(request, "session/attach").session_id, id);
  transport.respond(request.id, { type: "attached", target: {
    session_id: id, conversation_id: "conv-test", attachment_id: `att-${id}`, runtime_incarnation: "1",
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
  const surfaces: ResumeSelector[] = [];
  const feedback: string[] = [];
  const original = TUI.prototype.showOverlay;
  t.mock.method(TUI.prototype, "showOverlay", function(this: TUI, content: Parameters<TUI["showOverlay"]>[0], options: Parameters<TUI["showOverlay"]>[1]) {
    if (content instanceof PopupFrame && content.content instanceof ResumeSelector) surfaces.push(content.content);
    return original.call(this, content, options);
  });
  const replace = TransientFeedbackSurface.prototype.replace;
  t.mock.method(TransientFeedbackSurface.prototype, "replace", function(this: TransientFeedbackSurface, value: Parameters<TransientFeedbackSurface["replace"]>[0]) {
    feedback.push(value.text); return replace.call(this, value);
  });
  const app = new RustxTuiApp({ host, ...focus, reconnect, sessionSettings: parsedResume().sessionSettings });
  const running = app.run();
  t.after(async () => { await app.quit(); await running; });
  return { app, surfaces, feedback, editor };
}

it("remote missing cwd fails at argument parsing before token reading or connection", () => {
  const result = spawnSync(process.execPath, [fileURLToPath(new URL("../src/main.ts", import.meta.url)),
    "--connect", "ws://127.0.0.1:1", "--token-file", "/nonexistent/token"], { encoding: "utf8" });
  assert.equal(result.status, 2);
  assert.match(result.stderr, /remote Session cwd requires an explicit --cwd/);
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
  assert.equal(h.editor.disableSubmit, true);
  h.editor.onSubmit?.("must not reach a runtime");
  await tick();
  noControl(transport);
  h.surfaces[0]!.onSelect!(rows[1]!);
  await attachment(transport, "B");
  await finishFocus(transport);
  assert.deepEqual(host.attached.map((s) => s.sessionId), ["B"]);
  assert.equal(transport.log.count("session/attach"), 1);
  assert.equal(transport.log.count("session/unload"), 0);
  assert.equal(transport.log.count("turn/cancel"), 0);
  assert.match(h.feedback.at(-1)!, /showing session B/);
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
  assert.equal(paramsOf(request!, "session/attach").session_id, "A");
  transport.respondError(request!.id, { code: -32000, message: "A is controlled", data: { kind: "controller_in_use" } });
  await tick();
  assert.match(h.feedback.at(-1)!, /could not open Session A: another client already controls this Session/);
  assert.equal(host.client.closed, undefined);
  assert.equal(host.attached.length, 0);
  assert.equal(h.surfaces.at(-1), selector, "the same picker remains recoverable");
  selector.onSelect!(rows[1]!);
  await attachment(transport, "B", 2);
  await finishFocus(transport);
  assert.deepEqual(host.attached.map((s) => s.sessionId), ["B"]);
  assert.equal(transport.log.count("session/attach"), 2, "one explicit attempt per chosen identity");
});

for (const resume of [true, false]) {
  it(`${resume ? "empty-catalog resume" : "ordinary startup"} creates and attaches exactly one Session with unchanged remote cwd`, async () => {
    const { host, transport } = await connected();
    const parsed = parseArguments(["--connect", "wss://server.test", "--token-file", "/client/token", "--cwd", "/server/work/../project", ...(resume ? ["--resume"] : [])]);
    const starting = prepareStartup(host, parsed);
    if (resume) await catalog(transport, []);
    const [create] = await transport.log.awaitMethod("session/create");
    assert.equal(paramsOf(create!, "session/create").settings.cwd, "/server/work/../project");
    transport.respond(create!.id, { type: "session_transition", session: sessionView({ id: "new" }) });
    await attachment(transport, "new");
    assert.equal((await starting).session?.sessionId, "new");
    assert.equal(transport.log.count("session/create"), 1);
    assert.equal(transport.log.count("session/attach"), 1);
    assert.equal(transport.log.count("session/list"), resume ? 1 : 0);
    await host.shutdown();
  });
}

it("explicit --session/--node attaches that identity directly without browsing or creating", async () => {
  const { host, transport } = await connected();
  const starting = prepareStartup(host, parseArguments(["--binary", "rustx", "--session", "B", "--node", "node-B"]));
  const request = await attachment(transport, "B");
  assert.equal(paramsOf(request, "session/attach").node_id, "node-B");
  assert.equal((await starting).session?.sessionId, "B");
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
  await attachment(next.transport, "B");
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
  assert.equal(paramsOf(pending!, "session/attach").session_id, "A");
  first.transport.fail("socket_error");
  await catalog(next.transport);
  await tick();
  assert.equal(first.transport.log.count("session/attach"), 1);
  noControl(next.transport);
  assert.equal(next.host.attached.length, 0);
  assert.equal(h.editor.disableSubmit, true);
  assert.match(h.feedback.at(-1)!, /no unanswered mutations were resent/);
  h.surfaces.at(-1)!.onSelect!(rows[1]!);
  await attachment(next.transport, "B");
  await finishFocus(next.transport);
  assert.deepEqual(next.host.attached.map((s) => s.sessionId), ["B"]);
});

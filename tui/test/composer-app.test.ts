import assert from "node:assert/strict";
import { test, type TestContext } from "node:test";
import { writeFileSync } from "node:fs";
import { Editor, TUI, type Component } from "@earendil-works/pi-tui";
import { RustxTuiApp } from "../src/ui/app.ts";
import { ComposerDraft } from "../src/ui/composer-draft.ts";
import { PopupFrame } from "../src/ui/components/popup-frame.ts";
import { TransientFeedbackSurface } from "../src/ui/components/transient-feedback.ts";
import { harness, nextRequest } from "./support/app-server-harness.ts";
import { snapshot, sessionView, attemptView } from "./support/fixtures.ts";
import { TempFixture } from "./support/temp-fixture.ts";
import type { RuntimeClientSnapshot, UserInputBlock } from "../src/protocol/app-server.ts";

const continuation = () => new Promise<void>(resolve => setImmediate(resolve));
async function appHarness(t: TestContext, initial: RuntimeClientSnapshot = snapshot()) {
  const h = await harness(initial);
  t.mock.method(TUI.prototype, "start", () => {});
  t.mock.method(TUI.prototype, "stop", () => {});
  t.mock.method(TUI.prototype, "requestRender", () => {});
  let focus: Component | undefined;
  let editor!: Editor;
  let listener!: (data: string) => { consume?: boolean } | undefined;
  t.mock.method(TUI.prototype, "setFocus", (component: Component) => { focus = component; if (component instanceof Editor) editor = component; });
  t.mock.method(TUI.prototype, "addInputListener", (callback: typeof listener) => { listener = callback; return () => {}; });
  t.mock.method(h.host, "shutdown", async () => undefined);
  t.mock.method(h.host, "readSession", async () => sessionView());
  const notices: string[] = [];
  t.mock.method(TransientFeedbackSurface.prototype, "replace", (value: { text: string }) => { notices.push(value.text); });
  const app = new RustxTuiApp({ host: h.host, session: h.session, sessionSettings: { cwd: "/work/project" } });
  const running = app.run();
  t.after(async () => { await app.quit(); await running; h.client.close(); });
  return { ...h, app, notices, get editor() { return editor; }, get focus() { return focus; }, input(data: string) { if (!listener(data)?.consume) focus?.handleInput?.(data); } };
}

test("actual app uses Send/Steer/Queue and never clears a newly typed draft after admission", async t => {
  const h = await appHarness(t);
  h.editor.setText("idle"); h.input("\r");
  const first = await nextRequest(h, "turn/start", 0);
  h.editor.setText("typed during transport");
  h.transport.respond(first.id, { type: "inbound_accepted", message_id: "m1", inbound_sequence: "1" });
  await continuation(); assert.equal(h.editor.getExpandedText(), "typed during transport");
  h.session.updateState(state => ({ ...state, attempt: { attemptId: "a", phase: { type: "running" }, turn: 1, foreground: [] } }));
  h.input("\r"); const steer = await nextRequest(h, "turn/steer", 0);
  h.transport.respond(steer.id, { type: "inbound_accepted", message_id: "m2", inbound_sequence: "2" }); await continuation();
  h.editor.setText("queue"); h.input("\t"); const queue = await nextRequest(h, "turn/start", 1);
  h.transport.respond(queue.id, { type: "inbound_accepted", message_id: "m3", inbound_sequence: "3" }); await continuation();
  assert.equal(h.transport.transportCount("turn/start"), 2); assert.equal(h.transport.transportCount("turn/steer"), 1);
});

test("actual app Ctrl+R searches local prompts, excludes commands, Esc preserves exact cursor draft", async t => {
  const h = await appHarness(t);
  h.editor.setText("searchable prompt"); h.input("\r");
  const request = await nextRequest(h, "turn/start", 0); h.transport.respond(request.id, { type: "inbound_accepted", message_id: "m", inbound_sequence: "1" }); await continuation();
  h.editor.setText("draft中👩‍💻e\u0301"); h.input("\x1b[D");
  h.input("\x12"); h.input("searchable");
  assert.ok(h.focus instanceof PopupFrame); assert.match(h.focus.render(80).join("\n"), /searchable prompt/);
  h.input("\x1b"); assert.equal(h.editor.getExpandedText(), "draft中👩‍💻e\u0301");
  h.input("\x7f"); assert.equal(h.editor.getExpandedText(), "draft中e\u0301", "cursor was preserved too");
  h.input("\x12"); h.input("\r"); assert.equal(h.editor.getExpandedText(), "searchable prompt");
  assert.equal(h.transport.transportCount("turn/start"), 1);
});

test("actual app command overlay uploads selected bytes without losing text order or local draft", async t => {
  const temp = TempFixture.create("rustx-tui-373-"); t.after(() => temp.cleanup());
  const path = temp.path("selected.txt"); writeFileSync(path, "selected bytes");
  const h = await appHarness(t);
  h.editor.setText("before\n"); h.input("\x10"); // Ctrl+P command, normal draft stays intact
  h.input(`attach ${path}`); h.input("\r");
  const upload = await nextRequest(h, "session/upload", 0);
  assert.deepEqual(upload.params, { target: h.target, files: [{ name: "selected.txt", data: Buffer.from("selected bytes").toString("base64") }] });
  h.editor.setText("after"); // typing after the upload action is a separate ordered region
  const receipt = { session_id: h.session.sessionId, batch_id: "batch", token: "receipt" };
  h.transport.respond(upload.id, { type: "session_uploaded", files: [{ receipt, file: { batch_id: "batch", name: "selected.txt" }, path: "/server/native/path" }] });
  await continuation(); assert.equal(h.editor.getExpandedText(), "after");
  // Native projection replacement must leave the editor and receipt untouched.
  const repair = h.session.resync();
  const read = await nextRequest(h, "session/snapshot", 0);
  h.transport.respond(read.id, { type: "snapshot", snapshot: snapshot(), cursor: "2" });
  const subscribe = await nextRequest(h, "session/subscribe", 0);
  h.transport.respond(subscribe.id, { type: "subscribed", after_cursor: "2" }); await repair;
  h.input("\r"); const sent = await nextRequest(h, "turn/start", 0);
  assert.deepEqual(sent.params, { target: h.target, content: [{ type: "text", text: "before\n" }, { type: "upload", ...receipt }, { type: "text", text: "after" }] });
  assert.ok(!JSON.stringify(sent.params).includes(path)); assert.ok(!JSON.stringify(sent.params).includes("/server/native/path"));
  h.transport.respond(sent.id, { type: "inbound_accepted", message_id: "m", inbound_sequence: "3" }); await continuation();
});

test("restored interleaved drafts cannot be flattened", () => {
  const blocks: UserInputBlock[] = [{ type: "text", text: "one" }, { type: "upload", session_id: "s", batch_id: "a", token: "x" }, { type: "text", text: "two" }];
  const draft = new ComposerDraft(); draft.restore(blocks);
  assert.deepEqual(draft.submission(), blocks);
  assert.throws(() => draft.submission("changed"), /ordering/);

});

test("committed IME text and paste are content at the app boundary, never interrupt or submit", async t => {
  const h = await appHarness(t, snapshot({ attempt: attemptView() }));
  h.input("中文入力👩‍💻e\u0301");
  h.input("\x1b[200~"); h.input("\r"); h.input("\t"); h.input("\x1b"); h.input("\x1b[201~");
  assert.equal(h.transport.transportCount("turn/start"), 0); assert.equal(h.transport.transportCount("turn/steer"), 0); assert.equal(h.transport.transportCount("turn/cancel"), 0);
  assert.match(h.editor.getExpandedText(), /中文入力/);
});

test("queue and capability overlays keep the draft intact and command history stays separate", async t => {
  const h = await appHarness(t);
  for (const command of ["queue", "capabilities"]) {
    h.editor.setText("unsubmitted 中👨‍👩‍👧‍👦"); h.input("\x10");
    h.input(command); h.input("\r"); await continuation();
    assert.ok(h.focus instanceof PopupFrame);
    h.input("\x1b"); assert.equal(h.editor.getExpandedText(), "unsubmitted 中👨‍👩‍👧‍👦");
  }
  h.input("\x12"); assert.ok(h.focus instanceof PopupFrame);
  assert.match(h.focus.render(80).join("\n"), /No matching prompts/);
  assert.equal(h.transport.transportCount("turn/start"), 0);
});

test("settlement keeps the actual history overlay; resyncRequired invalidates it and preserves the draft", async t => {
  const h = await appHarness(t);
  h.editor.setText("unsubmitted 中文"); h.input("\x12"); h.input("query");
  const overlay = h.focus; assert.ok(overlay instanceof PopupFrame);
  const before = overlay.render(80);
  h.session.applyNotification({ jsonrpc: "2.0", method: "session/event", params: { target: h.target, cursor: "1", event: { type: "attempt_settled", attempt_id: "a1", outcome: { type: "completed", finish_reason: { type: "stop" } } } } });
  const live = await nextRequest(h, "session/snapshot", 0);
  const updated = new Promise<void>(resolve => { const stop = h.session.onState(() => { stop(); resolve(); }); });
  h.transport.respond(live.id, { type: "snapshot", snapshot: snapshot(), cursor: "1" }); await updated;
  assert.equal(h.focus, overlay); assert.deepEqual(overlay.render(80), before);
  assert.equal(h.transport.transportCount("session/subscribe"), 0);
  h.session.applyNotification({ jsonrpc: "2.0", method: "session/resyncRequired", params: { target: h.target, after_cursor: "1", earliest_serviceable: "2" } });
  const repair = await nextRequest(h, "session/snapshot", 1);
  h.transport.respond(repair.id, { type: "snapshot", snapshot: snapshot(), cursor: "3" });
  const subscribe = await nextRequest(h, "session/subscribe", 0);
  h.transport.respond(subscribe.id, { type: "subscribed", after_cursor: "3" });
  assert.equal(h.focus, h.editor); assert.equal(h.session.resyncCount, 1);
  assert.equal(h.editor.getExpandedText(), "unsubmitted 中文");
});

test("pasted leading command tokens dispatch as Agent input, including typed suffixes", async t => {
  const h = await appHarness(t);
  for (const [pasted, typed] of [["/permissions", ""], ["/attach /tmp/a", ""], ["/attach ", "foo.txt"], ["ordinary pasted content", ""]] as const) {
    h.input(`\x1b[200~${pasted}\x1b[201~`); if (typed) h.input(typed);
    const count = h.transport.transportCount("turn/start");
    h.input("\r");
    const request = await nextRequest(h, "turn/start", count);
    assert.deepEqual(request.params, { target: h.target, content: [{ type: "text", text: pasted + typed }] });
    h.transport.respond(request.id, { type: "inbound_accepted", message_id: `m-${count}`, inbound_sequence: String(count + 1) });
    await continuation();
  }
  assert.equal(h.transport.transportCount("session/upload"), 0);
  assert.equal(h.transport.transportCount("configuration/sourcesRead"), 0);
});

test("typed attach token with pasted path arguments executes upload, including spaces", async t => {
  const temp = TempFixture.create("rustx-tui-paste-arguments-"); t.after(() => temp.cleanup());
  const h = await appHarness(t);
  for (const name of ["a.txt", "my file.txt"]) {
    const path = temp.path(name); writeFileSync(path, name);
    h.input("/attach ");
    h.input("\x1b[200~"); h.input(path); h.input("\x1b[201~");
    const count = h.transport.transportCount("session/upload");
    h.input("\r");
    const upload = await nextRequest(h, "session/upload", count);
    assert.deepEqual(upload.params, { target: h.target, files: [{ name, data: Buffer.from(name).toString("base64") }] });
    h.transport.respond(upload.id, { type: "session_uploaded", files: [{ receipt: { session_id: h.session.sessionId, batch_id: `batch-${count}`, token: "receipt" }, file: { batch_id: `batch-${count}`, name }, path: "/server/native/path" }] });
    await continuation();
  }
  assert.equal(h.transport.transportCount("turn/start"), 0);
});

test("typed permissions remains a command and clearing or replacing resets paste provenance", async t => {
  const h = await appHarness(t);
  for (const reset of ["none", "clear", "replace"]) {
    if (reset !== "none") h.input("\x1b[200~/attach /tmp/a\x1b[201~");
    if (reset === "clear") h.editor.setText("");
    if (reset === "replace") h.editor.setText("/permissions");
    else h.input("/permissions");
    const count = h.transport.transportCount("configuration/sourcesRead");
    h.input("\r");
    const request = await nextRequest(h, "configuration/sourcesRead", count);
    h.transport.respondError(request.id, { code: -32603, message: "scripted read failure" });
    await continuation();
  }
  assert.equal(h.transport.transportCount("configuration/sourcesRead"), 3);
  assert.equal(h.transport.transportCount("turn/start"), 0);
});

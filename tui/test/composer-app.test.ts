import { registrationId } from "./support/app-server-peer.ts";
import { checkpoint } from "./support/fixtures.ts";
import assert from "node:assert/strict";
import { test, type TestContext } from "node:test";
import { writeFileSync } from "node:fs";
import { Editor, TUI, type Component } from "@earendil-works/pi-tui";
import { RustxTuiApp } from "../src/ui/app.ts";
import { AppServerSession } from "../src/app-server/session.ts";
import { ComposerDraft } from "../src/ui/composer-draft.ts";
import { PopupFrame } from "../src/ui/components/popup-frame.ts";
import { TransientFeedbackSurface } from "../src/ui/components/transient-feedback.ts";
import { harness, nextRequest } from "./support/app-server-harness.ts";
import { snapshot, sessionView, attemptView, subagent, userMessage, nativeInteraction, questionnaireInteraction, approvalInteraction } from "./support/fixtures.ts";
import { TempFixture } from "./support/temp-fixture.ts";
import type { RuntimeClientSnapshot, UserInputBlock } from "../src/protocol/app-server.ts";

function childObservation(child: ReturnType<typeof subagent>, text?: string): Extract<import('../../protocol/app-server/v57.ts').MethodResult,{type:'agent_observe'}> {
 const value=snapshot({conversation_id:child.child_conversation_id,transcript:{entries:text?[{cursor:'1',item:{type:'message',message:userMessage('child-message',text)}}]:[]}});
 const bounded=checkpoint(value);bounded.read_cut.activation_id=child.activation_id;
 return {type:'agent_observe',observation:{type:'checkpoint',agent_id:child.agent_id,conversation_id:child.child_conversation_id,activation_id:child.activation_id,source:'durable_only',cursor:null,checkpoint:bounded}};
}
const continuation = () => new Promise<void>(resolve => setImmediate(resolve));

test("T11 a late bounded content read cannot replace its successor view or Applied cursor", async t => {
  const h = await appHarness(t);
  const ref: import("../../protocol/app-server/v57.ts").ContentRef = {
    conversation_id: h.session.state.conversationId,
    source: { type: "canonical_message_block", component: "payload", locator:{type:"by_message_id",message_id:"range-source"}, block_index: 0 },
    representation: "utf8_text", version: 1, content_revision: 1, readable_end: 5,
  };
  Object.defineProperty(h.session, "deferredContent", { get: () => [{ path: ["range"], content_ref: ref, deferred: true }] });
  let entered!: () => void;
  const called = new Promise<void>(resolve => { entered = resolve; });
  let release!: (value: import("../../protocol/app-server/v57.ts").ContentChunk) => void;
  const pending = new Promise<import("../../protocol/app-server/v57.ts").ContentChunk>(resolve => { release = resolve; });
  t.mock.method(h.session, "readContent", () => { entered(); return pending; });
  const applied = h.session.state.cursor;
  h.editor.setText("/content 1"); h.input("\r");
  await called;
  h.editor.setText("/help"); h.input("\r");
  await continuation();
  assert.ok(h.focus instanceof PopupFrame);
  assert.match(h.focus.render(100).join("\n"), /Help/);
  release({ content_ref: ref, offset: 0, data_base64: Buffer.from("range").toString("base64"), end_offset: 5, readable_end: 5, next_offset: null, complete: true });
  await continuation();
  assert.match(h.focus.render(100).join("\n"), /Help/);
  assert.doesNotMatch(h.focus.render(100).join("\n"), /Deferred content bytes/);
  assert.equal(h.session.state.cursor, applied);
});
async function appHarness(t: TestContext, initial: RuntimeClientSnapshot = snapshot(), holdInitialReceipt = false) {
  const h = await harness(initial);
  let session = h.session;
  let completeInitialReceipt = async () => {};
  if (holdInitialReceipt) {
    let prepared: AppServerSession | undefined;
    const target = { ...h.target, attachment_id: 'prepared-composer-attachment' };
    const attaching = AppServerSession.attach(h.client, target.session_id, undefined, value => { prepared = value; });
    const attach = await nextRequest(h, 'session/attach');
    h.transport.respond(attach.id, { type: 'attached', target, configuration_revision: '0', attachment_kind: 'controller', observation_source: 'live' });
    const read = await nextRequest(h, 'session/checkpoint');
    h.transport.respond(read.id, { type: 'checkpoint', checkpoint: checkpoint(initial) });
    const subscribe = await nextRequest(h, 'session/subscribe');
    assert.ok(prepared); session = prepared;
    completeInitialReceipt = async () => {
      h.transport.respond(subscribe.id, { type: 'subscribed', target, registration_id: 'composer-registration', after_cursor: '0' });
      const configuration = await nextRequest(h, 'session/configuration');
      h.transport.respond(configuration.id, { type: 'session_configuration', projection: null });
      await attaching;
    };
  }
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
  const app = new RustxTuiApp({ host: h.host, session, sessionSettings: { cwd: "/work/project" } });
  const running = app.run();
  t.after(async () => { await app.quit(); await running; h.client.close(); });
  return { ...h, session, completeInitialReceipt, app, notices, get editor() { return editor; }, get focus() { return focus; }, input(data: string) { if (!listener(data)?.consume) focus?.handleInput?.(data); } };
}

for (const boundary of ['prepared subscription', 'repair checkpoint'] as const) test(`actual composer preserves its draft before ${boundary} readiness`, async t => {
  const h = await appHarness(t, snapshot(), boundary === 'prepared subscription');
  const repairing = boundary === 'repair checkpoint' ? h.session.resync() : undefined;
  const read = repairing ? await nextRequest(h, 'session/checkpoint') : undefined;
  assert.equal(h.session.controlReady, false);
  const records = h.transport.log.requests.length;
  const draft = 'original draft 中👩‍💻';
  h.editor.setText(draft); h.input('\r');
  assert.equal(h.editor.getExpandedText(), draft);
  assert.equal(h.transport.log.requests.length, records, 'refusal is known unsent with no repair or mutation replay');
  assert.match(h.notices.at(-1)!, /Request not sent; prompt draft is unchanged/);
  if (read) {
    h.transport.respond(read.id, { type: 'checkpoint', checkpoint: checkpoint(snapshot(), '2') });
    const subscribe = await nextRequest(h, 'session/subscribe');
    h.input('\r'); assert.equal(h.editor.getExpandedText(), draft);
    assert.equal(h.transport.log.count('turn/start'), 0);
    h.transport.respond(subscribe.id, { type: 'subscribed', target: h.target, registration_id: 'composer-repair', after_cursor: '2' });
    await repairing;
  } else await h.completeInitialReceipt();
  assert.equal(h.session.controlReady, true); assert.equal(h.editor.getExpandedText(), draft);
  h.input('\r'); const start = await nextRequest(h, 'turn/start');
  assert.deepEqual(start.params, { target: h.session.target, content: [{ type: 'text', text: draft }] });
  h.transport.respond(start.id, { type: 'inbound_accepted', message_id: 'original-admitted-message', inbound_sequence: '1' });
  await continuation(); assert.equal(h.editor.getExpandedText(), '');
  assert.equal(h.transport.log.count('turn/start'), 1);
});

const unavailableStates = [
  ['running', snapshot({ attempt: attemptView({ attempt_id: 'retained-A', phase: { type: 'running' } }) })],
  ['settled', snapshot({ attempt: attemptView({ phase: { type: 'settled', outcome: { type: 'cancelled', reason: 'user_requested' } } }) })],
  ['absent Attempt', snapshot()],
  ['shutdown and durability failure', snapshot({ shutting_down: true, durability_failure: { operation: 'retained', diagnostic: 'retained failure' } })],
  ['pending interaction', snapshot({ pending_interactions: [questionnaireInteraction()] })],
] as const;
for (const [stateName, initial] of unavailableStates) for (const gesture of (stateName === 'pending interaction' ? ['Escape', 'Ctrl+C'] as const : ['Escape', 'Ctrl+C', '/cancel'] as const)) test(`actual ${gesture} cancels after Source becomes unavailable with retained ${stateName} without enabling Send`, async t => {
  const h = await appHarness(t, initial);
  h.editor.setText('retained draft 中');
  const before = h.session.state, reads = h.transport.log.count('session/checkpoint');
  h.transport.notify({ jsonrpc: '2.0', method: 'session/observationUnavailable', params: { target: h.target, registration_id: 'harness-registration-1', message: 'held source' } });
  assert.equal(h.session.observationPhase, 'unavailable'); assert.equal(h.session.controlReady, false);
  if (stateName !== 'pending interaction') h.input('\r');
  assert.equal(h.editor.getExpandedText(), 'retained draft 中');
  assert.equal(h.transport.log.count('turn/start'), 0); assert.equal(h.transport.log.count('turn/steer'), 0);
  if (gesture === '/cancel') { h.editor.setText('/cancel'); h.input('\r'); }
  else h.input(gesture === 'Escape' ? '\x1b' : '\x03');
  const request = await nextRequest(h, 'turn/cancel');
  assert.equal(request.method, 'turn/cancel'); if (request.method !== 'turn/cancel') assert.fail('native cancellation');
  assert.deepEqual(request.params.target, h.target);
  h.transport.respond(request.id, { type: 'cancellation_accepted', attempt_id: 'native-current-B' });
  await continuation();
  assert.equal(h.session.state, before); assert.equal(h.session.observationPhase, 'unavailable');
  assert.match(h.notices.at(-1)!, /native-current-B/);
  if (gesture !== '/cancel') assert.equal(h.editor.getExpandedText(), 'retained draft 中');
  assert.equal(h.transport.log.count('turn/cancel'), 1); assert.equal(h.transport.log.count('session/checkpoint'), reads);
});

test('actual Escape cancels stale idle while an explicit observation repair is held', async t => {
  const h = await appHarness(t);
  const before = h.session.state;
  const repairing = h.session.resync(), read = await nextRequest(h, 'session/checkpoint');
  assert.equal(h.session.controlReady, false); assert.equal(h.session.cancellationReady, true);
  h.input('\x1b');
  const request = await nextRequest(h, 'turn/cancel');
  assert.deepEqual(request.params, { target: h.target });
  h.transport.respond(request.id, { type: 'cancellation_accepted', attempt_id: 'native-current-B' });
  await continuation();
  assert.equal(h.session.state, before); assert.match(h.notices.at(-1)!, /native-current-B/);
  h.transport.respond(read.id, { type: 'checkpoint', checkpoint: checkpoint(snapshot(), '2') });
  const subscribe = await nextRequest(h, 'session/subscribe');
  h.transport.respond(subscribe.id, { type: 'subscribed', target: h.target, registration_id: 'explicit-repair', after_cursor: '2' });
  await repairing; assert.equal(h.transport.log.count('turn/cancel'), 1);
});

for (const retirement of ['attachment', 'connection'] as const) test(`unavailable TUI gestures cannot dispatch through a retired ${retirement}`, async t => {
  const h = await appHarness(t, snapshot({ attempt: attemptView({ phase: { type: 'running' } }) }));
  h.transport.notify({ jsonrpc: '2.0', method: 'session/observationUnavailable', params: { target: h.target, registration_id: 'harness-registration-1', message: 'held source' } });
  if (retirement === 'attachment') {
    const detaching = h.session.detach(), request = await nextRequest(h, 'session/detach');
    h.transport.respond(request.id, { type: 'detached' }); await detaching;
  } else h.client.close();
  h.input('\x1b'); await continuation();
  assert.equal(h.transport.log.count('turn/cancel'), 0);
  assert.equal(h.session.released, true);
});

test("actual app uses Send/Steer/Queue and never clears a newly typed draft after admission", async t => {
  const h = await appHarness(t);
  h.editor.setText("idle"); h.input("\r");
  const first = await nextRequest(h, "turn/start", 0);
  h.editor.setText("typed during transport");
  h.transport.respond(first.id, { type: "inbound_accepted", message_id: "m1", inbound_sequence: "1" });
  await continuation(); assert.equal(h.editor.getExpandedText(), "typed during transport");
  h.session.applyNotification({ jsonrpc: "2.0", method: "session/event", params: { registration_id: registrationId(h.target), target: h.target, cursor: "1", event: { type: "attempt_started", attempt_id: "a", model: null, execution_settings: null } } });
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
  const upload = await nextRequest(h, "session/uploadPrepare", 0);
  assert.deepEqual((upload.params as { files: unknown }).files, [{ name: "selected.txt", size: 14 }]);
  h.editor.setText("after"); // typing after the upload action is a separate ordered region
  const receipt = { session_id: h.session.sessionId, batch_id: "batch", token: "receipt" };
  h.transport.respond(upload.id, { type: "upload_prepared", transfer: { path: "/session-upload/" + "a".repeat(43), loopback_port: 1234, expires_in_seconds: 60 } });
  const status = await nextRequest(h, "session/uploadStatus", h.transport.transportCount("session/uploadStatus"));
  h.transport.respond(status.id, { type: "upload_status", outcome: { state: "ready", files: [{ receipt, file: { batch_id: "batch", name: "selected.txt" }, path: "/server/native/path" }] } });
  await continuation(); assert.equal(h.editor.getExpandedText(), "after");
  // Native projection replacement must leave the editor and receipt untouched.
  const repair = h.session.resync();
  const read = await nextRequest(h, "session/checkpoint", 0);
  h.transport.respond(read.id, {type:'checkpoint', checkpoint:checkpoint(snapshot(),"2")});
  const subscribe = await nextRequest(h, "session/subscribe", 0);
  h.transport.respond(subscribe.id, { type: "subscribed", target:h.target,registration_id:"00000000-0000-4000-8000-000000000099", after_cursor: "2" }); await repair;
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
  h.session.applyNotification({ jsonrpc: "2.0", method: "session/event", params: {registration_id:registrationId(h.target), target: h.target, cursor: "1", event: { type: "attempt_settled", attempt_id: "a1", outcome: { type: "completed", finish_reason: { type: "stop" } } } } });
  assert.equal(h.transport.transportCount("session/checkpoint"), 1);
  assert.equal(h.focus, overlay); assert.deepEqual(overlay.render(80), before);
  assert.equal(h.transport.transportCount("session/subscribe"), 1);
  h.session.applyNotification({ jsonrpc: "2.0", method: "session/resyncRequired", params: {registration_id:registrationId(h.target),reason:'replay_unavailable',checkpoint_required:true, target: h.target, after_cursor: "1", earliest_serviceable: "2" } });
  const repair = await nextRequest(h, "session/checkpoint", 0);
  h.transport.respond(repair.id, {type:'checkpoint', checkpoint:checkpoint(snapshot(),"3")});
  const subscribe = await nextRequest(h, "session/subscribe", 0);
  h.transport.respond(subscribe.id, { type: "subscribed", target:h.target,registration_id:"00000000-0000-4000-8000-000000000099", after_cursor: "3" });
  assert.equal(h.focus, h.editor); assert.equal(h.session.resyncCount, 1);
  assert.equal(h.editor.getExpandedText(), "unsubmitted 中文");
});

test("pasted leading command tokens dispatch as Agent input, including typed suffixes", async t => {
  const h = await appHarness(t);
  for (const [pasted, typed] of [["/settings", ""], ["/attach /tmp/a", ""], ["/attach ", "foo.txt"], ["ordinary pasted content", ""]] as const) {
    h.input(`\x1b[200~${pasted}\x1b[201~`); if (typed) h.input(typed);
    const count = h.transport.transportCount("turn/start");
    h.input("\r");
    const request = await nextRequest(h, "turn/start", count);
    assert.deepEqual(request.params, { target: h.target, content: [{ type: "text", text: pasted + typed }] });
    h.transport.respond(request.id, { type: "inbound_accepted", message_id: `m-${count}`, inbound_sequence: String(count + 1) });
    await continuation();
  }
  assert.equal(h.transport.transportCount("session/uploadPrepare"), 0);
  assert.equal(h.transport.transportCount("configuration/sourcesRead"), 0);
});

test("typed attach token with pasted path arguments executes upload, including spaces", async t => {
  const temp = TempFixture.create("rustx-tui-paste-arguments-"); t.after(() => temp.cleanup());
  const h = await appHarness(t);
  for (const name of ["a.txt", "my file.txt"]) {
    const path = temp.path(name); writeFileSync(path, name);
    h.input("/attach ");
    h.input("\x1b[200~"); h.input(path); h.input("\x1b[201~");
    const count = h.transport.transportCount("session/uploadPrepare");
    h.input("\r");
    const upload = await nextRequest(h, "session/uploadPrepare", count);
    assert.deepEqual((upload.params as { files: unknown }).files, [{ name, size: Buffer.byteLength(name) }]);
    h.transport.respond(upload.id, { type: "upload_prepared", transfer: { path: "/session-upload/" + "a".repeat(43), loopback_port: 1234, expires_in_seconds: 60 } });
  const status = await nextRequest(h, "session/uploadStatus", h.transport.transportCount("session/uploadStatus"));
  h.transport.respond(status.id, { type: "upload_status", outcome: { state: "ready", files: [{ receipt: { session_id: h.session.sessionId, batch_id: `batch-${count}`, token: "receipt" }, file: { batch_id: `batch-${count}`, name }, path: "/server/native/path" }] } });
    await continuation();
  }
  assert.equal(h.transport.transportCount("turn/start"), 0);
});

test("typed settings remains a command and clearing or replacing resets paste provenance", async t => {
  const h = await appHarness(t);
  for (const reset of ["none", "clear", "replace"]) {
    if (reset !== "none") h.input("\x1b[200~/attach /tmp/a\x1b[201~");
    if (reset === "clear") h.editor.setText("");
    if (reset === "replace") h.editor.setText("/settings");
    else h.input("/settings");
    const count = h.transport.transportCount("configuration/sourcesRead");
    h.input("\r");
    const request = await nextRequest(h, "configuration/sourcesRead", count);
    h.transport.respondError(request.id, { code: -32603, message: "scripted read failure" });
    await continuation();
  }
  assert.equal(h.transport.transportCount("configuration/sourcesRead"), 3);
  assert.equal(h.transport.transportCount("turn/start"), 0);
});

test("typed attach token accepts a pasted separator and path across whole and fragmented paste", async t => {
  const temp = TempFixture.create("rustx-tui-token-boundary-"); t.after(() => temp.cleanup());
  const h = await appHarness(t);
  const path = temp.path("my file.txt"); writeFileSync(path, "selected bytes");
  for (const [typed, fragments] of [["/attach", [`\x1b[200~ ${path}\x1b[201~`]], ["  /attach", ["\x1b[200~", " ", path, "\x1b[20", "1~"]]] as const) {
    h.input(typed); for (const fragment of fragments) h.input(fragment);
    const count = h.transport.transportCount("session/uploadPrepare"); h.input("\r");
    const upload = await nextRequest(h, "session/uploadPrepare", count);
    assert.deepEqual((upload.params as { files: unknown }).files, [{ name: "my file.txt", size: 14 }]);
    h.transport.respond(upload.id, { type: "upload_prepared", transfer: { path: "/session-upload/" + "a".repeat(43), loopback_port: 1234, expires_in_seconds: 60 } });
  const status = await nextRequest(h, "session/uploadStatus", h.transport.transportCount("session/uploadStatus"));
  h.transport.respond(status.id, { type: "upload_status", outcome: { state: "ready", files: [{ receipt: { session_id: h.session.sessionId, batch_id: `batch-${count}`, token: "receipt" }, file: { batch_id: `batch-${count}`, name: "my file.txt" }, path: "/server/native/path" }] } });
    await continuation();
  }
  assert.equal(h.transport.transportCount("turn/start"), 0);
});

test("pasted token extensions and a pasted leading slash remain literal Agent input", async t => {
  const h = await appHarness(t);
  for (const [typed, pasted, suffix] of [["/att", "ach /tmp/a", ""], ["  /att", "ach /tmp/a", ""], ["", "/", "attach /tmp/a"]] as const) {
    if (typed) h.input(typed);
    h.input("\x1b[200~"); h.input(pasted); h.input("\x1b[201~");
    if (suffix) h.input(suffix);
    const count = h.transport.transportCount("turn/start"); h.input("\r");
    const request = await nextRequest(h, "turn/start", count);
    assert.deepEqual(request.params, { target: h.target, content: [{ type: "text", text: typed + pasted + suffix }] });
    h.transport.respond(request.id, { type: "inbound_accepted", message_id: `m-${count}`, inbound_sequence: String(count + 1) });
    await continuation();
  }
  assert.equal(h.transport.transportCount("session/uploadPrepare"), 0);
});


test("child inspection preserves exact parent draft and cursor; Esc sends no request", async t => {
  const child = subagent("researcher", "sha256:child");
  const h = await appHarness(t, snapshot({ agents: [child] }));
  h.editor.setText("draft中👩‍💻e\u0301"); h.input("\x1b[D");
  const parent = h.session.state;
  h.input("\x1b[1;5B"); h.input("\r");
  const read = await nextRequest(h, "agent/observe", 0);
  h.transport.respond(read.id, childObservation(child)); await continuation();
  assert.ok(h.focus instanceof PopupFrame);
  assert.match(h.focus.render(100).join("\n"), /read only/);
  assert.equal(h.editor.getExpandedText(), "draft中👩‍💻e\u0301");
  const requests = [...h.transport.log.requests];
  h.input("\x1b");
  assert.deepEqual(h.transport.log.requests, requests, "Esc is presentation-only");
  assert.equal(h.session.state, parent);
  h.input("\x7f"); assert.equal(h.editor.getExpandedText(), "draft中e\u0301");
  assert.equal(h.transport.transportCount("session/attach"), 1);
  for (const method of ["turn/start", "turn/steer", "agent/interrupt", "interaction/respond"]) assert.equal(h.transport.transportCount(method), 0);
});


test("actual child A to B navigation rejects late A without Session or control requests", async t => {
  const children = [subagent("researcher", "sha256:a", "active", { agent_id: "A" }), subagent("planner", "sha256:b", "inactive", { agent_id: "B" })];
  const h = await appHarness(t, snapshot({ agents: children }));
  h.input("\x1b[1;5B"); h.input("\r");
  const a = await nextRequest(h, "agent/observe", 0);
  const previousView = h.focus;
  h.input("\x1b"); h.input("\x1b[1;5B"); h.input("\r");
  const b = await nextRequest(h, "agent/observe", 1);
  assert.equal((a.params as { agent_id: string }).agent_id, "A");
  assert.equal((b.params as { agent_id: string }).agent_id, "B");
  h.transport.respond(b.id, childObservation(children[1]!, "ONLY-B"));
  await continuation();
  h.transport.respond(a.id, childObservation(children[0]!, "STALE-A"));
  await continuation();
  previousView?.handleInput?.("\x1b"); // A stale view callback cannot close B.
  assert.ok(h.focus instanceof PopupFrame);
  const rendered = h.focus.render(100).join("\n");
  assert.match(rendered, /ONLY-B/); assert.doesNotMatch(rendered, /STALE-A/);
  assert.deepEqual(h.transport.log.requests.map(request => request.method), ["initialize", "session/attach", "session/checkpoint", "session/subscribe", "session/configuration", "agent/observe", "agent/observeCancel", "agent/observe"]);
});

for (const kind of ["agent_delivery_unknown", "agent_not_delivered"] as const) {
  test(`actual app preserves Agent guidance draft after ${kind} without replay`, async t => {
    const h = await appHarness(t, snapshot({ agents: [subagent("worker", "frozen", "active")] }));
    const draft = "/send-message agent-child keep this guidance";
    h.editor.setText(draft); h.input("\r");
    const request = await nextRequest(h, "agent/sendMessage", 0);
    h.transport.respondError(request.id, { code: -32000, message: "Delivery failed", data: { kind, agent_id: "agent-child" } });
    await continuation();
    assert.equal(h.editor.getExpandedText(), draft);
    assert.equal(h.transport.transportCount("agent/sendMessage"), 1);
    assert.ok(h.notices.some(text => text.includes(kind === "agent_delivery_unknown" ? "delivery is unknown" : "input was not delivered")));
  });
}

for (const outcome of ["success", "agent_not_delivered", "agent_delivery_unknown"] as const) {
  for (const edited of [false, true]) {
    test(`one Agent message submission owns its pending draft: ${outcome}, edited=${edited}`, async t => {
      const h = await appHarness(t, snapshot({ agents: [subagent("worker", "frozen", "active")] }));
      const draft = "/send-message agent-child one guidance";
      h.editor.setText(draft); h.input("\r");
      const request = await nextRequest(h, "agent/sendMessage", 0);
      h.input("\r"); await continuation();
      assert.equal(h.transport.transportCount("agent/sendMessage"), 1, "Enter cannot duplicate the pending submission");
      if (edited) h.editor.setText("new user typing");
      if (outcome === "success") h.transport.respond(request.id, { type: "agent_message", agent_id: "agent-child", activation_id: "activation-1", resumed: false });
      else h.transport.respondError(request.id, { code: -32000, message: "Delivery failed", data: { kind: outcome, agent_id: "agent-child" } });
      await continuation();
      assert.equal(h.editor.getExpandedText(), edited ? "new user typing" : outcome === "success" ? "" : draft);
      assert.equal(h.transport.transportCount("agent/sendMessage"), 1, "classification never replays input");
      if (outcome === "agent_delivery_unknown") assert.ok(h.notices.some(text => text.includes("delivery is unknown")));
    });
  }
}

test("pending Agent message leaves explicit interruption admissible", async t => {
  const agent = subagent("worker", "frozen", "active");
  const h = await appHarness(t, snapshot({ agents: [agent] }));
  h.editor.setText("/send-message agent-child guidance"); h.input("\r");
  const message = await nextRequest(h, "agent/sendMessage", 0);
  h.editor.setText("/interrupt-agent agent-child"); h.input("\r");
  const interrupt = await nextRequest(h, "agent/interrupt", 0);
  h.transport.respond(interrupt.id, { type: "agent_wait", agent_id: agent.agent_id, activation_id: agent.activation_id, outcome: "cancelled" });
  await continuation();
  h.editor.setText("next prompt");
  h.transport.respond(message.id, { type: "agent_message", agent_id: agent.agent_id, activation_id: agent.activation_id, resumed: false });
  await continuation();
  assert.equal(h.editor.getExpandedText(), "next prompt");
  assert.equal(h.transport.transportCount("agent/sendMessage"), 1);
});

test("T15 selected deferred questionnaire loads its exact body without changing Applied or answering", async t => {
  const routed=questionnaireInteraction('selected-body');
  const header={...nativeInteraction(routed),body:null,body_complete:false};
  const h=await appHarness(t,snapshot({pending_interactions:[header]}));
  assert.ok(h.focus instanceof PopupFrame);
  assert.match(h.focus.render(100).join('\n'),/deferred/);
  const applied=h.session.state.cursor, original=h.session.state.pendingInteractions[0];
  h.input('\r');
  const read=await nextRequest(h,'conversation/checkpointValueRead');
  const params=read.params as import('../src/protocol/app-server.ts').MethodParams<'conversation/checkpointValueRead'>;
  const bytes=Buffer.from(JSON.stringify(routed));
  h.transport.respond(read.id,{type:'checkpoint_value_chunk',chunk:{representation:'canonical_json_v1',cut:params.cut,domain:params.domain,item_offset:0,path:[],offset:0,bytes_base64:bytes.toString('base64'),next_offset:null,readable_end:bytes.length}});
  await continuation();
  assert.ok(h.focus instanceof PopupFrame);
  assert.doesNotMatch(h.focus.render(100).join('\n'),/body is deferred/);
  assert.equal(h.session.state.cursor,applied);
  assert.equal(h.session.state.pendingInteractions[0],original,'native display header remains authoritative');
  assert.equal(h.transport.transportCount('interaction/respond'),0);
});

test("T15 interaction viewport advances 32 items without changing authoritative state or Applied", async t => {
  const rows=Array.from({length:40},(_,index)=>approvalInteraction('page-'+index));
  const h=await appHarness(t,snapshot({pending_interactions:rows.slice(0,32)}));
  const cut=h.session.checkpoint!;
  cut.collections.push({domain:'pending_interactions',revision:'0',total:'40',loaded:'32',next_offset:'32'});
  h.input('\x1b[5;5~'); // Ctrl+PageUp is already the first page.
  h.input('\x1b[6;5~');
  const request=await nextRequest(h,'conversation/checkpointPage');
  assert.equal((request.params as {offset:number}).offset,32);
  const headers=rows.slice(32).map(nativeInteraction);
  h.transport.respond(request.id,{type:'checkpoint_page',page:{domain:'pending_interactions',cursor:'0',read_cut:cut.read_cut,total:'40',items:headers.map((value,index)=>({offset:String(32+index),identity:value.interaction.interaction_id,value,content:[],deferred:false})),next_offset:null}});
  await continuation();
  assert.ok(h.focus instanceof PopupFrame);
  assert.match(h.focus.render(100).join('\n'),/page-32/);
  assert.equal(h.session.state.pendingInteractions.length,32);
  assert.equal(h.session.state.cursor,'0');
  assert.equal(h.transport.transportCount('interaction/respond'),0);
});

test("T11 late selected interaction body cannot reopen a hidden view", async t => {
 const routed=questionnaireInteraction('late-body');
 const h=await appHarness(t,snapshot({pending_interactions:[{...nativeInteraction(routed),body:null,body_complete:false}]}));
 h.input('\r');const read=await nextRequest(h,'conversation/checkpointValueRead');
 const params=read.params as import('../src/protocol/app-server.ts').MethodParams<'conversation/checkpointValueRead'>;
 h.input('\x1b');assert.equal(h.focus,h.editor);
 const bytes=Buffer.from(JSON.stringify(routed));
 h.transport.respond(read.id,{type:'checkpoint_value_chunk',chunk:{representation:'canonical_json_v1',cut:params.cut,domain:params.domain,item_offset:0,path:[],offset:0,bytes_base64:bytes.toString('base64'),next_offset:null,readable_end:bytes.length}});
 await continuation();assert.equal(h.focus,h.editor);
 assert.equal(h.session.state.cursor,'0');assert.equal(h.transport.transportCount('interaction/respond'),0);
});

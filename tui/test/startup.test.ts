import { followAttachment } from './support/app-server-peer.ts';
import { checkpoint } from "./support/fixtures.ts";
/** Startup ownership, driven by explicit wire replies and user selections. */
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { writeFileSync } from "node:fs";
import { it, type TestContext } from "node:test";
import { TUI, Editor } from "@earendil-works/pi-tui";
import { AppServerClient } from "../src/app-server/client.ts";
import { AppServerHost } from "../src/app-server/host.ts";
import { parseArguments } from "../src/cli.ts";
import { prepareStartup, type StartupFocus } from "../src/startup.ts";
import { RustxTuiApp } from "../src/ui/app.ts";
import { ComposerEditor } from "../src/ui/composer.ts";
import { CommandDispatcher } from "../src/commands/dispatcher.ts";
import { ConfirmationView } from "../src/ui/components/confirmation.ts";
import { ComposerCommand } from "../src/ui/components/composer-command.ts";
import { PopupFrame } from "../src/ui/components/popup-frame.ts";
import { ResumeSelector } from "../src/ui/components/resume-selector.ts";
import { TransientFeedbackSurface } from "../src/ui/components/transient-feedback.ts";
import { FakeTransport, paramsOf, tick } from "./support/app-server-peer.ts";
import { SERVER_CAPABILITIES } from "./support/app-server-harness.ts";
import { attemptView, snapshot, sessionView } from "./support/fixtures.ts";
import { TempFixture } from "./support/temp-fixture.ts";
import type { UserInputBlock } from "../src/protocol/app-server.ts";

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
  transport.respond(request!.id, { type: "initialized", authority_id: authority, protocol_version: 58, capabilities: SERVER_CAPABILITIES });
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
async function attachment(transport: FakeTransport, id: string, count = 1, initial = snapshot()) {
  const request = (await pendingAttachment(transport, count)).at(-1)!;
  assert.equal(paramsOf(request, "session/attach").session_id, id);
  transport.respond(request.id, {type:'attached', target: {
    session_id: id, conversation_id: "conv_01900000-0000-7000-8000-000000000002", attachment_id: `att-${id}`, runtime_incarnation: "1",
  }, configuration_revision:'0', attachment_kind:'controller', observation_source:'live'});
  await followAttachment(transport,{session_id:id,conversation_id:"conv_01900000-0000-7000-8000-000000000002",attachment_id:`att-${id}`,runtime_incarnation:'1'},initial,"0");
  return request;
}
async function finishFocus(transport: FakeTransport, initial = snapshot(), releaseIntent?: () => void) {
  const repair = await transport.nextPending("session/checkpoint");
  transport.respond(repair.id, {type:'checkpoint', checkpoint:checkpoint(initial,"0")});
  const subscribe = await transport.nextPending("session/subscribe");
  transport.respond(subscribe.id, { type: "subscribed", after_cursor: "0" });
  await tick();
  // App binding (and its footer metadata read) follows both repair and the
  // explicit intent fence. Tests that hold that fence inspect it here.
  releaseIntent?.();
  const read = await transport.nextPending("session/read");
  transport.respond(read.id, { type: "session", session: sessionView({ id: paramsOf(read, "session/read").session_id }) });
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

/** Replacement Attach and its first subscription are confirmed, but the
 * focus owner's additional repair is separately controlled by the test. */
async function reconnectInputSurface(t: TestContext, initial = snapshot()) {
  const first = await connected();
  const next = await connected();
  const node = sessionView().active_node;
  const attaching = first.host.attach(SESSION_A, node);
  await attachment(first.transport, SESSION_A, 1, initial);
  const previous = await attaching;
  const h = appFor(t, first.host, { session: previous }, async () => next.host);
  const info = await first.transport.nextPending('session/read');
  first.transport.respond(info.id, { type: 'session', session: sessionView({ id: SESSION_A }) });
  const reconnect = async () => {
    first.transport.fail('socket_error');
    const pending = await next.transport.nextPending('session/attach');
    assert.deepEqual(paramsOf(pending, 'session/attach'), { session_id: SESSION_A, node_id: node });
    assert.equal(h.editor.disableSubmit, false);
    const target = { ...previous.target, attachment_id: 'input-intent-reconnect', runtime_incarnation: '2' };
    const install = async () => {
      next.transport.respond(pending.id, { type: 'attached', target, configuration_revision: '0', attachment_kind: 'controller', observation_source: 'live' });
      await followAttachment(next.transport, target, initial, '0');
      const repair = await next.transport.nextPending('session/checkpoint');
      const finish = async () => {
        next.transport.respond(repair.id, { type: 'checkpoint', checkpoint: checkpoint(initial, '0') });
        const subscribe = await next.transport.nextPending('session/subscribe');
        next.transport.respond(subscribe.id, { type: 'subscribed', after_cursor: '0' });
        const read = await next.transport.nextPending('session/read');
        next.transport.respond(read.id, { type: 'session', session: sessionView({ id: SESSION_A }) });
        await tick();
      };
      return { repair, finish };
    };
    return { pending, target, install };
  };
  return { first, next, h, node, previous, reconnect };
}

/** Drive only public UI controls: Enter after failure, or the real /resume
 * command overlay while attached, then keyboard selection and native metadata. */
async function selectResumeByKeyboard(h: ReturnType<typeof appFor>, transport: FakeTransport, sessionId: string, node: string, attached = false) {
  if (attached) {
    h.input('\x10');
    const command = h.commandEntries.at(-1)!;
    command.handleInput('resume');
    command.handleInput('\r');
  } else h.input('\r');
  const list = await transport.nextPending('session/list');
  transport.respond(list.id, { type: 'sessions', sessions: rows.map(row => row.id === sessionId ? { ...row, active_node: node } : row) });
  await tick();
  const selector = h.surfaces.at(-1)!;
  assert.ok(selector instanceof ResumeSelector);
  if (sessionId === SESSION_B) selector.handleInput('\x1b[B');
  assert.equal(selector.selector.selectedSession()?.id, sessionId);
  selector.handleInput('\r');
  const metadata = await transport.nextPending('session/read');
  assert.equal(paramsOf(metadata, 'session/read').session_id, sessionId);
  return metadata;
}

async function resumeByKeyboard(h: ReturnType<typeof appFor>, host: AppServerHost, transport: FakeTransport, sessionId: string, node: string, attached = false) {
  const metadata = await selectResumeByKeyboard(h, transport, sessionId, node, attached);
  transport.respond(metadata.id, { type: 'session', session: sessionView({ id: sessionId, active_node: node }) });
  const existing = host.attachment(sessionId);
  let target = existing?.target;
  if (!target) {
    const attaching = await transport.nextPending('session/attach');
    assert.deepEqual(paramsOf(attaching, 'session/attach'), { session_id: sessionId, node_id: node });
    target = { session_id: sessionId, conversation_id: sessionId === SESSION_A ? snapshot().conversation_id : 'conv_01900000-0000-7000-8000-000000002002', attachment_id: `resume-${sessionId}`, runtime_incarnation: '2' };
    transport.respond(attaching.id, { type: 'attached', target, configuration_revision: '0', attachment_kind: 'controller', observation_source: 'live' });
    await followAttachment(transport, target, snapshot({ conversation_id: target.conversation_id }), '0');
  }
  await finishFocus(transport, snapshot({ conversation_id: target.conversation_id }));
  return target;
}

/** Author a real structured draft through the command overlay and the exact
 * native upload operation, then stop replacement attachment at its wire cut. */
async function uploadedReconnectDraft(t: TestContext, tail: string, prefix = 'prefix before selected file\n', laterReplacement?: AppServerHost) {
  const temp = TempFixture.create('rustx-tui-reconnect-upload-');
  t.after(() => temp.cleanup());
  const path = temp.path('selected.txt');
  writeFileSync(path, 'selected bytes');
  const first = await connected();
  const next = await connected();
  const node = sessionView().active_node;
  const attaching = first.host.attach(SESSION_A, node);
  await attachment(first.transport, SESSION_A);
  const previous = await attaching;
  let replacements = 0;
  const h = appFor(t, first.host, { session: previous }, async () => ++replacements === 1 ? next.host : laterReplacement ?? next.host);
  const info = await first.transport.nextPending('session/read');
  first.transport.respond(info.id, { type: 'session', session: sessionView({ id: SESSION_A }) });
  h.editor.setText(prefix);
  h.input('\x10');
  const command = h.commandEntries.at(-1)!;
  command.handleInput(`attach ${path}`);
  command.handleInput('\r');
  const upload = await first.transport.nextPending('session/uploadPrepare');
  assert.deepEqual(paramsOf(upload, 'session/uploadPrepare').files, [{ name: 'selected.txt', size: 14 }]);
  assert.deepEqual(paramsOf(upload, 'session/uploadPrepare').target, previous.target);
  h.editor.setText(tail);
  first.transport.respond(upload.id, { type: 'upload_prepared', transfer: { path: '/session-upload/' + 'a'.repeat(43), loopback_port: 1234, expires_in_seconds: 60 } });
  const status = await first.transport.nextPending('session/uploadStatus');
  assert.equal(paramsOf(status, 'session/uploadStatus').operation_id, paramsOf(upload, 'session/uploadPrepare').operation_id);
  const receipt = { session_id: SESSION_A, batch_id: 'confirmed-draft-batch', token: 'confirmed-draft-token' };
  first.transport.respond(status.id, { type: 'upload_status', outcome: { state: 'ready', files: [{ receipt, file: { batch_id: receipt.batch_id, name: 'selected.txt' }, path: '/native/selected.txt' }] } });
  await tick();
  assert.equal(h.editor.getExpandedText(), tail);
  assert.ok(h.feedback.includes('Attached selected.txt'));
  first.transport.fail('socket_error');
  const pending = (await next.transport.log.awaitMethod('session/attach')).at(-1)!;
  assert.deepEqual(paramsOf(pending, 'session/attach'), { session_id: SESSION_A, node_id: node });
  assert.equal(h.editor.disableSubmit, false);
  assert.equal(h.editor.getExpandedText(), tail);
  const expected: UserInputBlock[] = [{ type: 'text', text: prefix }, { type: 'upload', ...receipt }, ...(tail ? [{ type: 'text' as const, text: tail }] : [])];
  const complete = async (request = pending, releaseIntent?: () => void) => {
    const target = { ...previous.target, attachment_id: `structured-reconnect-${next.transport.log.count('session/attach')}`, runtime_incarnation: '2' };
    next.transport.respond(request.id, { type: 'attached', target, configuration_revision: '0', attachment_kind: 'controller', observation_source: 'live' });
    await followAttachment(next.transport, target, snapshot(), '0');
    await finishFocus(next.transport, snapshot(), releaseIntent);
    return target;
  };
  const sendConnecting = async () => {
    h.editor.handleInput('\r');
    await tick();
    assert.equal(next.transport.log.count('session/submissionIntent'), 1, 'a structured draft is executable even when its editable tail is empty');
    const intent = next.transport.log.matching('session/submissionIntent')[0]!;
    assert.deepEqual(paramsOf(intent, 'session/submissionIntent'), { session_id: SESSION_A, node_id: node });
    assert.equal(next.transport.answered.has(pending.id), false);
    assert.equal(next.transport.log.count('turn/start'), 0);
    return intent;
  };
  return { first, next, h, node, previous, pending, expected, complete, sendConnecting };
}

for (const boundary of ['unchanged controls', 'held Attach', 'ready controls'] as const) for (const suffix of ['', ' typed suffix']) for (const authored of ['leading paste', 'token-end extension'] as const) {
  it(`reconnect input intent: ${authored} cancel stays literal at ${boundary}${suffix ? ' with typed suffix' : ''}`, async t => {
    const s = await reconnectInputSurface(t);
    if (authored === 'leading paste') s.h.editor.handleInput('\x1b[200~/cancel\x1b[201~');
    else {
      s.h.editor.handleInput('/can');
      s.h.editor.handleInput('\x1b[200~cel\x1b[201~');
    }
    if (suffix && authored === 'leading paste') s.h.editor.handleInput(suffix);
    const text = '/cancel' + suffix;
    const beforeReconnect = authored === 'leading paste' ? text : '/cancel';
    assert.equal(s.h.editor.getExpandedText(), beforeReconnect);
    let transport = s.first.transport;
    let target = s.previous.target;
    if (boundary !== 'unchanged controls') {
      const recovery = await s.reconnect();
      assert.equal(s.h.editor.getExpandedText(), beforeReconnect);
      transport = s.next.transport;
      target = recovery.target;
      if (suffix && authored === 'token-end extension') s.h.editor.handleInput(suffix);
      if (boundary === 'held Attach') {
        s.h.editor.handleInput('\r');
        await tick();
        assert.equal(transport.log.count('session/submissionIntent'), 1, 'pasted slash text must reach the submission-intent boundary while Attach is held');
        const intent = await transport.nextPending('session/submissionIntent');
        assert.deepEqual(paramsOf(intent, 'session/submissionIntent'), { session_id: SESSION_A, node_id: s.node });
        assert.equal(transport.answered.has(recovery.pending.id), false);
        assert.equal(transport.log.count('turn/start'), 0);
        transport.respond(intent.id, { type: 'submission_intent_accepted' });
      }
      const repair = await recovery.install();
      await repair.finish();
    } else if (suffix && authored === 'token-end extension') s.h.editor.handleInput(suffix);
    if (boundary !== 'held Attach') s.h.editor.handleInput('\r');
    await tick();
    assert.equal(transport.log.count('turn/cancel'), 0, 'reconnection cannot promote pasted content into a cancellation command');
    assert.equal(transport.log.count('turn/steer'), 0);
    assert.equal(transport.log.count('turn/start'), 1);
    const sent = await transport.nextPending('turn/start');
    assert.deepEqual(paramsOf(sent, 'turn/start'), { target, content: [{ type: 'text', text }] });
    transport.respond(sent.id, { type: 'inbound_accepted', message_id: 'literal-reconnect-input', inbound_sequence: '1' });
    await tick();
    assert.equal(s.h.editor.getExpandedText(), '');
    assert.equal(s.first.transport.log.count('turn/cancel'), 0);
    assert.equal(s.next.transport.log.count('turn/cancel'), 0);
  });
}

for (const boundary of ['unchanged controls', 'ready controls'] as const) {
  it(`reconnect input intent: pasted separator after a typed token stays a command at ${boundary}`, async t => {
    const s = await reconnectInputSurface(t);
    s.h.editor.handleInput('/cancel');
    s.h.editor.handleInput('\x1b[200~ \x1b[201~');
    let transport = s.first.transport;
    let target = s.previous.target;
    if (boundary === 'ready controls') {
      const recovery = await s.reconnect();
      const repair = await recovery.install();
      await repair.finish();
      transport = s.next.transport;
      target = recovery.target;
    }
    s.h.editor.handleInput('\r');
    await tick();
    assert.equal(transport.log.count('turn/cancel'), 1, 'pasted argument whitespace does not replace the authored command token');
    assert.equal(transport.log.count('turn/start'), 0);
    assert.equal(transport.log.count('turn/steer'), 0);
    const cancel = await transport.nextPending('turn/cancel');
    assert.deepEqual(paramsOf(cancel, 'turn/cancel'), { target });
    transport.respond(cancel.id, { type: 'cancellation_accepted', attempt_id: 'native-current' });
    await tick();
  });
}

for (const replacement of ['clear and type', 'replace with a different value'] as const) {
  it(`reconnect input intent: ${replacement} restores authored command intent`, async t => {
    const s = await reconnectInputSurface(t);
    s.h.editor.handleInput('\x1b[200~/cancel pasted arguments\x1b[201~');
    const recovery = await s.reconnect();
    const repair = await recovery.install();
    await repair.finish();
    if (replacement === 'clear and type') {
      s.h.editor.setText('');
      s.h.editor.handleInput('/cancel');
    } else s.h.editor.setText('/cancel');
    s.h.editor.handleInput('\r');
    await tick();
    assert.equal(s.next.transport.log.count('turn/cancel'), 1);
    assert.equal(s.next.transport.log.count('turn/start'), 0);
    assert.equal(s.next.transport.log.count('turn/steer'), 0);
    assert.equal(s.next.transport.log.count('session/submissionIntent'), 0);
    const cancel = await s.next.transport.nextPending('turn/cancel');
    assert.deepEqual(paramsOf(cancel, 'turn/cancel'), { target: recovery.target });
    s.next.transport.respond(cancel.id, { type: 'cancellation_accepted', attempt_id: 'native-current' });
    await tick();
  });
}

for (const boundary of ['ready controls', 'held repair'] as const) for (const gesture of ['Tab', 'Enter'] as const) {
  it(`reconnect input intent: ${gesture} keeps its ${gesture === 'Tab' ? 'Queue' : 'Steer'} meaning at ${boundary}`, async t => {
    const initial = snapshot({ attempt: attemptView({ attempt_id: 'running-before-reconnect' }) });
    const s = await reconnectInputSurface(t, initial);
    let transport = s.first.transport;
    let target = s.previous.target;
    let finish: (() => Promise<void>) | undefined;
    if (boundary === 'held repair') {
      const recovery = await s.reconnect();
      const repair = await recovery.install();
      transport = s.next.transport;
      target = recovery.target;
      finish = repair.finish;
      assert.equal(transport.answered.has(recovery.pending.id), true, 'replacement Attach already succeeded');
      assert.equal(transport.answered.has(repair.repair.id), false, 'the additional focus repair alone is held');
      assert.equal(transport.log.count('session/subscribe'), 1, 'the initial checkpoint and registration already succeeded');
    }
    assert.ok(s.h.editor instanceof ComposerEditor);
    assert.equal(s.h.editor.running(), true);
    assert.equal(s.h.editor.disableSubmit, false);
    const text = gesture + ' while running';
    s.h.editor.handleInput(text);
    s.h.editor.handleInput(gesture === 'Tab' ? '\t' : '\r');
    await tick();
    if (finish) {
      assert.equal(transport.log.count('session/submissionIntent'), 1);
      const intent = await transport.nextPending('session/submissionIntent');
      assert.deepEqual(paramsOf(intent, 'session/submissionIntent'), { session_id: SESSION_A, node_id: s.node });
      assert.equal(transport.log.count('turn/start'), 0);
      assert.equal(transport.log.count('turn/steer'), 0);
      transport.respond(intent.id, { type: 'submission_intent_accepted' });
      await finish();
    }
    await tick();
    const method = gesture === 'Tab' ? 'turn/start' : 'turn/steer';
    const other = gesture === 'Tab' ? 'turn/steer' : 'turn/start';
    assert.equal(transport.log.count(other), 0, 'the captured gesture cannot be reinterpreted at the recovery continuation');
    assert.equal(transport.log.count(method), 1);
    const sent = await transport.nextPending(method);
    assert.deepEqual(paramsOf(sent, method), { target, content: [{ type: 'text', text }] });
    transport.respond(sent.id, { type: 'inbound_accepted', message_id: 'explicit-' + gesture, inbound_sequence: '1' });
    await tick();
    assert.equal(s.h.editor.getExpandedText(), '');
    assert.equal(transport.log.count('turn/cancel'), 0);
  });
}

for (const tail of ['queued tail after selected file', '']) for (const order of ['intent first', 'repair first'] as const) {
  it(`reconnect input intent: structured Queue keeps ordered blocks, ${tail ? 'nonempty' : 'empty'} tail, ${order}`, async t => {
    const s = await uploadedReconnectDraft(t, tail);
    const initial = snapshot({ attempt: attemptView({ attempt_id: 'running-replacement' }) });
    const target = { session_id: SESSION_A, conversation_id: initial.conversation_id, attachment_id: 'structured-queue-reconnect', runtime_incarnation: '2' };
    s.next.transport.respond(s.pending.id, { type: 'attached', target, configuration_revision: '0', attachment_kind: 'controller', observation_source: 'live' });
    await followAttachment(s.next.transport, target, initial, '0');
    const repair = await s.next.transport.nextPending('session/checkpoint');
    assert.equal(s.next.transport.answered.has(s.pending.id), true);
    assert.equal(s.next.transport.log.count('session/subscribe'), 1);
    assert.equal(s.next.transport.answered.has(repair.id), false);
    assert.ok(s.h.editor instanceof ComposerEditor);
    assert.equal(s.h.editor.running(), true);
    assert.equal(s.h.editor.disableSubmit, false);
    assert.equal(s.h.editor.getExpandedText(), tail);
    s.h.editor.handleInput('\t');
    await tick();
    assert.equal(s.next.transport.log.count('session/submissionIntent'), 1, 'a Queue with confirmed blocks remains executable with an empty tail');
    const intent = await s.next.transport.nextPending('session/submissionIntent');
    assert.deepEqual(paramsOf(intent, 'session/submissionIntent'), { session_id: SESSION_A, node_id: s.node });
    assert.equal(s.next.transport.log.count('turn/start'), 0);
    assert.equal(s.next.transport.log.count('turn/steer'), 0);
    if (order === 'intent first') s.next.transport.respond(intent.id, { type: 'submission_intent_accepted' });
    s.next.transport.respond(repair.id, { type: 'checkpoint', checkpoint: checkpoint(initial, '0') });
    const subscribe = await s.next.transport.nextPending('session/subscribe');
    s.next.transport.respond(subscribe.id, { type: 'subscribed', after_cursor: '0' });
    await tick();
    if (order === 'repair first') {
      assert.equal(s.next.transport.log.count('turn/start'), 0, 'completed observation cannot bypass the explicit intent ACK');
      assert.equal(s.next.transport.log.count('turn/steer'), 0);
      s.next.transport.respond(intent.id, { type: 'submission_intent_accepted' });
      await tick();
    }
    const read = await s.next.transport.nextPending('session/read');
    s.next.transport.respond(read.id, { type: 'session', session: sessionView({ id: SESSION_A }) });
    await tick();
    assert.equal(s.next.transport.log.count('turn/steer'), 0, 'Tab never becomes Steer when the running attachment finishes its repair');
    assert.equal(s.next.transport.log.count('turn/start'), 1);
    const sent = await s.next.transport.nextPending('turn/start');
    assert.deepEqual(paramsOf(sent, 'turn/start'), { target, content: s.expected });
    s.next.transport.respond(sent.id, { type: 'inbound_accepted', message_id: 'structured-queued-input', inbound_sequence: '1' });
    await tick();
    assert.equal(s.h.editor.getExpandedText(), '');
    assert.equal(s.first.transport.log.count('session/uploadPrepare'), 1);
    assert.equal(s.first.transport.log.count('session/uploadStatus'), 1);
    assert.equal(s.first.transport.log.count('turn/start'), 0);
    assert.equal(s.next.transport.log.count('session/uploadPrepare'), 0);
    assert.equal(s.next.transport.log.count('turn/cancel'), 0);
  });
}

for (const outcome of ['Escape', 'intent rejection'] as const) {
  it(`reconnect input intent: ${outcome} restores captured slash input as literal content`, async t => {
    const s = await reconnectInputSurface(t);
    s.h.editor.handleInput('\x1b[200~/settings\x1b[201~');
    const recovery = await s.reconnect();
    s.h.editor.handleInput('\r');
    await tick();
    assert.equal(s.next.transport.log.count('session/submissionIntent'), 1);
    const intent = await s.next.transport.nextPending('session/submissionIntent');
    assert.equal(s.next.transport.answered.has(recovery.pending.id), false);
    if (outcome === 'Escape') {
      s.next.transport.respond(intent.id, { type: 'submission_intent_accepted' });
      s.h.input('\x1b');
    } else s.next.transport.respondError(intent.id, { code: -32000, message: 'Submission intent rejected before dispatch' });
    await tick();
    assert.equal(s.h.editor.getExpandedText(), '/settings');
    assert.equal(s.h.editor.disableSubmit, false);
    assert.equal(s.next.transport.log.count('turn/start'), 0);
    const repair = await recovery.install();
    await repair.finish();
    assert.equal(s.h.editor.getExpandedText(), '/settings');
    assert.equal(s.next.transport.log.count('turn/start'), 0, 'known-unsent restoration never retries by itself');
    s.h.editor.handleInput('\r');
    await tick();
    assert.equal(s.next.transport.log.count('configuration/sourcesRead'), 0, 'restoring captured Agent input must not turn it into a Settings command');
    assert.equal(s.next.transport.log.count('turn/cancel'), 0);
    assert.equal(s.next.transport.log.count('turn/steer'), 0);
    assert.equal(s.next.transport.log.count('turn/start'), 1);
    assert.equal(s.next.transport.log.count('session/submissionIntent'), 1, 'the new explicit ready Send uses the already-ready source');
    const sent = await s.next.transport.nextPending('turn/start');
    assert.deepEqual(paramsOf(sent, 'turn/start'), { target: recovery.target, content: [{ type: 'text', text: '/settings' }] });
    s.next.transport.respond(sent.id, { type: 'inbound_accepted', message_id: 'explicit-restored-literal', inbound_sequence: '1' });
    await tick();
    assert.equal(s.h.editor.getExpandedText(), '');
  });
}

for (const outcome of ['successful Attach', 'failed Attach then reselect'] as const) {
  it(`failed reconnect reselection: pasted cancel remains literal after ${outcome}`, async t => {
    const s = await reconnectInputSurface(t);
    s.h.editor.handleInput('\x1b[200~/cancel\x1b[201~');
    const recovery = await s.reconnect();
    s.h.editor.handleInput('\r');
    const intent = await s.next.transport.nextPending('session/submissionIntent');
    assert.deepEqual(paramsOf(intent, 'session/submissionIntent'), { session_id: SESSION_A, node_id: s.node });
    s.next.transport.respond(intent.id, { type: 'submission_intent_accepted' });
    let target = recovery.target;
    if (outcome === 'successful Attach') {
      const repair = await recovery.install();
      await repair.finish();
    } else {
      s.next.transport.respondError(recovery.pending.id, { code: -32000, message: 'Cold attachment explicitly refused' });
      await tick();
      assert.equal(s.h.editor.getExpandedText(), '/cancel');
      assert.equal(s.next.transport.log.count('turn/start'), 0);
      assert.equal(s.next.transport.log.count('turn/cancel'), 0);
      assert.match(s.h.render(), /Press Enter to choose a Session/);
      s.h.input('\r');
      const chosen = { ...rows[0]!, active_node: s.node };
      await catalog(s.next.transport, [chosen]);
      await tick();
      const selector = s.h.surfaces.at(-1)!;
      assert.ok(selector instanceof ResumeSelector);
      selector.handleInput('\r');
      const metadata = await s.next.transport.nextPending('session/read');
      assert.equal(paramsOf(metadata, 'session/read').session_id, SESSION_A);
      s.next.transport.respond(metadata.id, { type: 'session', session: sessionView({ id: SESSION_A, active_node: s.node }) });
      const attaching = await s.next.transport.nextPending('session/attach');
      assert.deepEqual(paramsOf(attaching, 'session/attach'), { session_id: SESSION_A, node_id: s.node });
      target = { ...s.previous.target, attachment_id: 'reselected-after-refusal', runtime_incarnation: '2' };
      s.next.transport.respond(attaching.id, { type: 'attached', target, configuration_revision: '0', attachment_kind: 'controller', observation_source: 'live' });
      await followAttachment(s.next.transport, target, snapshot(), '0');
      await finishFocus(s.next.transport);
      assert.equal(s.h.editor.getExpandedText(), '/cancel');
      assert.equal(s.next.transport.log.count('turn/start'), 0, 'a refused input is never automatically replayed after reselecting');
      s.h.editor.handleInput('\r');
      await tick();
    }
    assert.equal(s.next.transport.log.count('turn/cancel'), 0, 'same-Conversation reselection cannot reinterpret captured literal content as a command');
    assert.equal(s.next.transport.log.count('turn/steer'), 0);
    assert.equal(s.next.transport.log.count('turn/start'), 1);
    const sent = await s.next.transport.nextPending('turn/start');
    assert.deepEqual(paramsOf(sent, 'turn/start'), { target, content: [{ type: 'text', text: '/cancel' }] });
    s.next.transport.respond(sent.id, { type: 'inbound_accepted', message_id: 'reselected-literal-input', inbound_sequence: '1' });
    await tick();
    assert.equal(s.next.transport.log.count('session/submissionIntent'), 1);
    assert.equal(s.h.editor.getExpandedText(), '');
  });
}

for (const edit of ['clear and type', 'replace with a different command'] as const) {
  it(`failed reconnect reselection: ${edit} deliberately authors a new command`, async t => {
    const s = await reconnectInputSurface(t);
    s.h.editor.handleInput('\x1b[200~/settings\x1b[201~');
    const recovery = await s.reconnect();
    s.h.editor.handleInput('\r');
    const intent = await s.next.transport.nextPending('session/submissionIntent');
    s.next.transport.respond(intent.id, { type: 'submission_intent_accepted' });
    s.next.transport.respondError(recovery.pending.id, { code: -32000, message: 'Attachment refused before message dispatch' });
    await tick();
    const target = await resumeByKeyboard(s.h, s.next.host, s.next.transport, SESSION_A, s.node);
    assert.equal(s.h.editor.getExpandedText(), '/settings');
    if (edit === 'clear and type') {
      s.h.editor.setText('');
      s.h.editor.handleInput('/cancel');
    } else s.h.editor.setText('/cancel');
    s.h.editor.handleInput('\r');
    await tick();
    assert.equal(s.next.transport.log.count('turn/start'), 0);
    assert.equal(s.next.transport.log.count('turn/cancel'), 1);
    assert.equal(s.next.transport.log.count('configuration/sourcesRead'), 0);
    const cancel = await s.next.transport.nextPending('turn/cancel');
    assert.deepEqual(paramsOf(cancel, 'turn/cancel'), { target });
    s.next.transport.respond(cancel.id, { type: 'cancellation_accepted', attempt_id: 'explicit-new-command' });
    await tick();
  });
}

for (const uploaded of [false, true]) {
  it(`failed reconnect reselection: A and B retain separate ${uploaded ? 'structured receipt' : 'literal'} drafts`, async t => {
    const s = await (async () => {
      if (uploaded) return uploadedReconnectDraft(t, '', '/cancel');
      const plain = await reconnectInputSurface(t);
      plain.h.editor.handleInput('\x1b[200~/cancel\x1b[201~');
      const recovery = await plain.reconnect();
      return { ...plain, pending: recovery.pending, expected: [{ type: 'text' as const, text: '/cancel' }] };
    })();
    s.h.editor.handleInput('\r');
    const intent = await s.next.transport.nextPending('session/submissionIntent');
    s.next.transport.respond(intent.id, { type: 'submission_intent_accepted' });
    s.next.transport.respondError(s.pending.id, { code: -32000, message: 'A attachment refused before captured input dispatch' });
    await tick();
    assert.equal(s.h.editor.getExpandedText(), '/cancel');
    assert.equal(s.next.transport.log.count('turn/start'), 0);
    const nodeB = 'node_01900000-0000-7000-8000-000000002002';
    const targetB = await resumeByKeyboard(s.h, s.next.host, s.next.transport, SESSION_B, nodeB);
    assert.equal(s.h.editor.getExpandedText(), '', 'A content and confirmed receipts cannot populate B');
    const draftB = 'B independently typed while A remains unsent';
    s.h.editor.handleInput(draftB);
    const targetA = await resumeByKeyboard(s.h, s.next.host, s.next.transport, SESSION_A, s.node, true);
    assert.equal(targetA.conversation_id, s.previous.target.conversation_id);
    assert.equal(s.h.editor.getExpandedText(), '/cancel');
    assert.equal(s.next.transport.log.count('turn/start'), 0, 'Session selection never replays the refused input');
    s.h.editor.handleInput('\r');
    await tick();
    assert.equal(s.next.transport.log.count('turn/cancel'), 0, 'returning from B retains A captured-message intent');
    assert.equal(s.next.transport.log.count('turn/start'), 1);
    const sentA = await s.next.transport.nextPending('turn/start');
    assert.deepEqual(paramsOf(sentA, 'turn/start'), { target: targetA, content: s.expected });
    s.next.transport.respond(sentA.id, { type: 'inbound_accepted', message_id: 'A-retained-input', inbound_sequence: '1' });
    await tick();
    const returnedB = await resumeByKeyboard(s.h, s.next.host, s.next.transport, SESSION_B, nodeB, true);
    assert.deepEqual(returnedB, targetB, 'ordinary live B attachment is reused');
    assert.equal(s.h.editor.getExpandedText(), draftB);
    s.h.editor.handleInput('\r');
    const sentB = await s.next.transport.nextPending('turn/start');
    assert.deepEqual(paramsOf(sentB, 'turn/start'), { target: targetB, content: [{ type: 'text', text: draftB }] });
    s.next.transport.respond(sentB.id, { type: 'inbound_accepted', message_id: 'B-own-input', inbound_sequence: '2' });
    await tick();
    assert.equal(s.next.transport.log.count('turn/start'), 2);
    assert.equal(s.next.transport.log.count('turn/cancel'), 0);
    assert.equal(s.next.transport.log.count('session/submissionIntent'), 1);
    assert.equal(s.first.transport.log.count('session/uploadPrepare'), uploaded ? 1 : 0);
    assert.equal(s.next.transport.log.count('session/uploadPrepare'), 0, 'reselecting and switching never reuploads A bytes');
  });
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
  assert.equal(h.surfaces.at(-1), selector);
  selector.onSelect!(rows[1]!);
  await tick();
  assert.equal(transport.log.count('session/attach'), 1, 'the old selection lease cannot attach another identity');
  h.input('\r');
  const list = await transport.nextPending('session/list');
  transport.respond(list.id, { type: 'sessions', sessions: rows });
  await tick();
  const reopened = h.surfaces.at(-1)!;
  assert.notEqual(reopened, selector, 'failure remains recoverable through a newly authorized picker');
  reopened.handleInput('\x1b[B');
  reopened.handleInput('\r');
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
      await finishFocus(transport, snapshot(), outcome === "ready" ? () => {
        assert.equal(transport.log.count('turn/start'), 0, 'a held intent ACK cannot dispatch retained text');
        transport.respond(intent!.id, { type: 'submission_intent_accepted' });
      } : undefined);
      if (outcome === "ready") {
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
      await finishFocus(next.transport, snapshot(), () => {
        assert.equal(next.transport.log.count('turn/start'), 0, 'held intent acknowledgement fences forwarding');
        if (outcome === 'ready' || outcome === 'authority') next.transport.respond(intent.id, { type: 'submission_intent_accepted' });
      });
      if (outcome === 'ready' || outcome === 'authority') {
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
    await finishFocus(next.transport, snapshot(), () => {
      assert.equal(next.transport.log.count('turn/start'), 0, 'Attach cannot forward a queued input before its intent ACK');
      if (edit === 'send then type a newer draft') assert.equal(h.editor.getText(), newer, 'Attach preserves typing after the captured Send');
      if (intent) next.transport.respond(intent.id, { type: 'submission_intent_accepted' });
    });
    let expectedDraft = edit === 'send then type a newer draft' ? newer : '';
    if (intent) {
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

for (const boundary of ['held Attach', 'ready controls'] as const) for (const tail of ['tail after selected file', '']) {
  it(`structured reconnect draft: ${boundary}, ${tail ? 'nonempty' : 'empty'} tail`, async t => {
    const s = await uploadedReconnectDraft(t, tail);
    const intent = boundary === 'held Attach' ? await s.sendConnecting() : undefined;
    const target = await s.complete(undefined, () => {
      assert.equal(s.next.transport.log.count('turn/start'), 0);
      if (intent) s.next.transport.respond(intent.id, { type: 'submission_intent_accepted' });
    });
    if (!intent) s.h.editor.handleInput('\r');
    const sent = await s.next.transport.nextPending('turn/start');
    assert.deepEqual(paramsOf(sent, 'turn/start'), { target, content: s.expected });
    s.next.transport.respond(sent.id, { type: 'inbound_accepted', message_id: 'structured-input', inbound_sequence: '1' });
    await tick();
    assert.equal(s.h.editor.getExpandedText(), '');
    assert.equal(s.first.transport.log.count('session/uploadPrepare'), 1);
    assert.equal(s.first.transport.log.count('session/uploadStatus'), 1);
    assert.equal(s.first.transport.log.count('turn/start'), 0);
    assert.equal(s.next.transport.log.count('session/uploadPrepare'), 0, 'confirmed bytes are never uploaded again');
    assert.equal(s.next.transport.log.count('session/submissionIntent'), intent ? 1 : 0);
    assert.equal(s.next.transport.log.count('turn/start'), 1);
  });
}

it('structured reconnect draft: a captured Send preserves newer typing without reusing its upload', async t => {
  const s = await uploadedReconnectDraft(t, 'captured tail');
  const intent = await s.sendConnecting();
  s.h.editor.handleInput('new independent draft');
  const target = await s.complete(undefined, () => {
    assert.equal(s.h.editor.getExpandedText(), 'new independent draft');
    assert.equal(s.next.transport.log.count('turn/start'), 0);
    s.next.transport.respond(intent.id, { type: 'submission_intent_accepted' });
  });
  const first = await s.next.transport.nextPending('turn/start');
  assert.deepEqual(paramsOf(first, 'turn/start'), { target, content: s.expected });
  s.h.editor.handleInput(' while acceptance is held');
  const newer = 'new independent draft while acceptance is held';
  s.next.transport.respond(first.id, { type: 'inbound_accepted', message_id: 'captured-structured-input', inbound_sequence: '1' });
  await tick();
  assert.equal(s.h.editor.getExpandedText(), newer);
  s.h.editor.handleInput('\r');
  const second = await s.next.transport.nextPending('turn/start');
  assert.deepEqual(paramsOf(second, 'turn/start'), { target, content: [{ type: 'text', text: newer }] });
  s.next.transport.respond(second.id, { type: 'inbound_accepted', message_id: 'later-independent-input', inbound_sequence: '2' });
  await tick();
  assert.equal(s.next.transport.log.count('turn/start'), 2);
  assert.equal(s.next.transport.log.count('session/uploadPrepare'), 0);
});

for (const outcome of ['intent rejection', 'attachment failure', 'withdrawal'] as const) for (const tail of ['tail to retain', '']) for (const newer of tail ? ['', 'newer independent text'] : ['']) {
  it(`structured reconnect draft: ${outcome} restores ordered input, ${tail ? 'nonempty' : 'empty'} tail${newer ? ', newer typing' : ''}`, async t => {
    const s = await uploadedReconnectDraft(t, tail);
    const intent = await s.sendConnecting();
    if (newer) s.h.editor.handleInput(newer);
    if (outcome === 'intent rejection') {
      s.next.transport.respondError(intent.id, { code: -32000, message: 'Submission preparation refused' });
      await tick();
    } else {
      s.next.transport.respond(intent.id, { type: 'submission_intent_accepted' });
      await tick();
      if (outcome === 'withdrawal') s.h.input('\x1b');
    }
    let target;
    if (outcome === 'attachment failure') {
      s.next.transport.respondError(s.pending.id, { code: -32000, message: 'Attachment refused' });
      await tick();
      assert.equal(s.next.transport.log.count('turn/start'), 0);
      s.h.input('\r');
      const observedRows = rows.map(row => ({ ...row, active_node: s.node }));
      await catalog(s.next.transport, observedRows);
      await tick();
      s.h.surfaces.at(-1)!.onSelect!(observedRows[0]!);
      const retry = (await pendingAttachment(s.next.transport, 2)).at(-1)!;
      target = await s.complete(retry);
    } else target = await s.complete();
    assert.equal(s.next.transport.log.count('turn/start'), 0, 'refusal and withdrawal never replay captured input');
    s.h.editor.handleInput('\r');
    const sent = await s.next.transport.nextPending('turn/start');
    assert.deepEqual(paramsOf(sent, 'turn/start'), { target, content: [...s.expected, ...(newer ? [{ type: 'text', text: newer }] : [])] });
    s.next.transport.respond(sent.id, { type: 'inbound_accepted', message_id: 'explicitly-retried-structured-input', inbound_sequence: '1' });
    await tick();
    assert.equal(s.next.transport.log.count('turn/start'), 1);
    assert.equal(s.next.transport.log.count('session/submissionIntent'), 1);
    assert.equal(s.next.transport.log.count('session/uploadPrepare'), 0);
    assert.equal(s.first.transport.log.count('session/uploadPrepare'), 1);
  });
}

for (const boundary of ['metadata', 'Attach'] as const) for (const input of ['typing', 'paste'] as const) for (const uploaded of [false, true]) {
  it(`focus metadata draft ownership: ${input} at held ${boundary}, ${uploaded ? 'confirmed A upload' : 'plain A draft'}`, async t => {
    const { host, transport } = await connected();
    const node = sessionView().active_node;
    const attaching = host.attach(SESSION_A, node);
    await attachment(transport, SESSION_A);
    const initial = await attaching;
    const h = appFor(t, host, { session: initial });
    const info = await transport.nextPending('session/read');
    transport.respond(info.id, { type: 'session', session: sessionView({ id: SESSION_A }) });
    const prefix = 'Private unsent draft for Session A. ';
    h.editor.handleInput(prefix);
    const receipt = { session_id: SESSION_A, batch_id: 'focus-A-batch', token: 'focus-A-token' };
    if (uploaded) {
      const temp = TempFixture.create('rustx-tui-focus-upload-');
      t.after(() => temp.cleanup());
      const path = temp.path('selected.txt');
      writeFileSync(path, 'selected bytes');
      h.input('\x10');
      const command = h.commandEntries.at(-1)!;
      command.handleInput(`attach ${path}`);
      command.handleInput('\r');
      const upload = await transport.nextPending('session/uploadPrepare');
      assert.deepEqual(paramsOf(upload, 'session/uploadPrepare').target, initial.target);
      transport.respond(upload.id, { type: 'upload_prepared', transfer: { path: '/session-upload/' + 'a'.repeat(43), loopback_port: 1234, expires_in_seconds: 60 } });
      const status = await transport.nextPending('session/uploadStatus');
      transport.respond(status.id, { type: 'upload_status', outcome: { state: 'ready', files: [{ receipt, file: { batch_id: receipt.batch_id, name: 'selected.txt' }, path: '/native/selected.txt' }] } });
      await tick();
      assert.ok(h.feedback.includes('Attached selected.txt'));
      h.editor.handleInput('\x1b[200~/cancel\x1b[201~');
    }
    const textA = uploaded ? '/cancel' : prefix;
    const expectedA: UserInputBlock[] = [{ type: 'text', text: prefix }, ...(uploaded ? [{ type: 'upload' as const, ...receipt }, { type: 'text' as const, text: textA }] : [])];
    h.input('\x10');
    const resume = h.commandEntries.at(-1)!;
    resume.handleInput('resume');
    resume.handleInput('\r');
    await catalog(transport, rows.map(row => ({ ...row, active_node: node })));
    await tick();
    const selector = h.surfaces.at(-1)!;
    selector.handleInput('\x1b[B');
    assert.equal(selector.selector.selectedSession()?.id, SESSION_B);
    selector.handleInput('\r');
    const metadata = await transport.nextPending('session/read');
    assert.equal(paramsOf(metadata, 'session/read').session_id, SESSION_B);
    const textB = 'New input intended for Session B.';
    const authorB = () => h.editor.handleInput(input === 'paste' ? `\x1b[200~${textB}\x1b[201~` : textB);
    if (boundary === 'metadata') authorB();
    transport.respond(metadata.id, { type: 'session', session: sessionView({ id: SESSION_B, active_node: node }) });
    const pending = await transport.nextPending('session/attach');
    assert.deepEqual(paramsOf(pending, 'session/attach'), { session_id: SESSION_B, node_id: node });
    if (boundary === 'Attach') authorB();
    const target = { session_id: SESSION_B, conversation_id: 'conv_01900000-0000-7000-8000-000000002002', attachment_id: 'focus-metadata-B', runtime_incarnation: '1' };
    transport.respond(pending.id, { type: 'attached', target, configuration_revision: '0', attachment_kind: 'controller', observation_source: 'live' });
    await followAttachment(transport, target, snapshot({ conversation_id: target.conversation_id }), '0');
    await finishFocus(transport, snapshot({ conversation_id: target.conversation_id }));
    assert.equal(transport.log.count('turn/start'), 0, 'focus and repair never execute either draft');
    h.editor.handleInput('\r');
    const sentB = await transport.nextPending('turn/start');
    assert.deepEqual(paramsOf(sentB, 'turn/start'), { target, content: [{ type: 'text', text: textB }] }, 'B receives only input authored after selecting B');
    transport.respond(sentB.id, { type: 'inbound_accepted', message_id: 'B-only-input', inbound_sequence: '1' });
    await tick();
    const returned = await resumeByKeyboard(h, host, transport, SESSION_A, node, true);
    assert.equal(h.editor.getExpandedText(), textA, 'A retains its original editable text and paste provenance');
    h.editor.handleInput('\r');
    const sentA = await transport.nextPending('turn/start');
    assert.deepEqual(paramsOf(sentA, 'turn/start'), { target: returned, content: expectedA }, 'A retains its complete ordered draft and exact confirmed receipt');
    transport.respond(sentA.id, { type: 'inbound_accepted', message_id: 'original-A-input', inbound_sequence: '1' });
    await tick();
    assert.equal(transport.log.count('turn/cancel'), 0);
    assert.equal(transport.log.count('session/uploadPrepare'), uploaded ? 1 : 0);
    assert.equal(transport.log.count('session/uploadStatus'), uploaded ? 1 : 0);
    assert.equal(transport.log.count('session/submissionIntent'), 0);
    assert.equal(transport.log.count('session/detach'), 0);
    assert.equal(transport.log.count('session/unload'), 0);
  });
}

for (const refusal of ['metadata', 'Attach'] as const) {
  it(`focus metadata draft ownership: ${refusal} refusal retains B through A and explicit B reselection`, async t => {
    const { host, transport } = await connected();
    const node = sessionView().active_node;
    const attaching = host.attach(SESSION_A, node);
    await attachment(transport, SESSION_A);
    const initial = await attaching;
    const h = appFor(t, host, { session: initial });
    const info = await transport.nextPending('session/read');
    transport.respond(info.id, { type: 'session', session: sessionView({ id: SESSION_A }) });
    const textA = 'Only A owns this retained draft.';
    h.editor.handleInput(textA);
    const metadata = await selectResumeByKeyboard(h, transport, SESSION_B, node, true);
    h.editor.handleInput('\x1b[200~/cancel\x1b[201~');
    let rejected = metadata;
    if (refusal === 'Attach') {
      transport.respond(metadata.id, { type: 'session', session: sessionView({ id: SESSION_B, active_node: node }) });
      rejected = await transport.nextPending('session/attach');
    }
    transport.respondError(rejected.id, { code: -32000, message: `${refusal} refused`, data: { kind: 'controller_in_use' } });
    await tick();
    assert.equal(h.editor.getExpandedText(), '/cancel');
    assert.equal(transport.log.count('turn/start'), 0);
    assert.equal(transport.log.count('turn/cancel'), 0);
    await resumeByKeyboard(h, host, transport, SESSION_A, node);
    assert.equal(h.editor.getExpandedText(), textA, 'failed B preparation cannot overwrite A');
    const targetB = await resumeByKeyboard(h, host, transport, SESSION_B, node, true);
    assert.equal(h.editor.getExpandedText(), '/cancel', 'a new explicit B selection restores only B input');
    assert.equal(transport.log.count('turn/start'), 0, 'reselecting never retries known-unsent input');
    h.editor.handleInput('\r');
    const sentB = await transport.nextPending('turn/start');
    assert.deepEqual(paramsOf(sentB, 'turn/start'), { target: targetB, content: [{ type: 'text', text: '/cancel' }] });
    transport.respond(sentB.id, { type: 'inbound_accepted', message_id: 'B-literal-reselected', inbound_sequence: '1' });
    await tick();
    const targetA = await resumeByKeyboard(h, host, transport, SESSION_A, node, true);
    h.editor.handleInput('\r');
    const sentA = await transport.nextPending('turn/start');
    assert.deepEqual(paramsOf(sentA, 'turn/start'), { target: targetA, content: [{ type: 'text', text: textA }] });
    transport.respond(sentA.id, { type: 'inbound_accepted', message_id: 'A-retained-after-B-refusal', inbound_sequence: '1' });
    await tick();
    assert.equal(transport.log.count('turn/cancel'), 0);
    assert.equal(transport.log.count('session/submissionIntent'), 0);
  });
}

it('focus metadata draft ownership: failed reconnect cannot carry its bound receipt into a newly selected Conversation', async t => {
  const s = await uploadedReconnectDraft(t, 'A original editable tail.');
  const intent = await s.sendConnecting();
  s.next.transport.respond(intent.id, { type: 'submission_intent_accepted' });
  s.next.transport.respondError(s.pending.id, { code: -32000, message: 'replacement refused', data: { kind: 'controller_in_use' } });
  await tick();
  const nodeY = 'node_01900000-0000-7000-8000-000000004004';
  const metadataY = await selectResumeByKeyboard(s.h, s.next.transport, SESSION_A, nodeY);
  s.h.editor.handleInput('Only newly selected Y input.');
  s.next.transport.respond(metadataY.id, { type: 'session', session: sessionView({ id: SESSION_A, active_node: nodeY }) });
  const pendingY = await s.next.transport.nextPending('session/attach');
  assert.equal(paramsOf(pendingY, 'session/attach').node_id, nodeY);
  const targetY = { ...s.previous.target, conversation_id: 'conv_01900000-0000-7000-8000-000000004004', attachment_id: 'newly-selected-Y', runtime_incarnation: '3' };
  s.next.transport.respond(pendingY.id, { type: 'attached', target: targetY, configuration_revision: '0', attachment_kind: 'controller', observation_source: 'live' });
  await followAttachment(s.next.transport, targetY, snapshot({ conversation_id: targetY.conversation_id }), '0');
  await finishFocus(s.next.transport, snapshot({ conversation_id: targetY.conversation_id }));
  assert.equal(s.h.editor.getExpandedText(), 'Only newly selected Y input.', 'a draft previously bound to X cannot become Y editor content');
  assert.equal(s.next.transport.log.count('turn/start'), 0, 'the rejected input cannot replay into Y');
  s.h.editor.handleInput('\r');
  const sentY = await s.next.transport.nextPending('turn/start');
  assert.deepEqual(paramsOf(sentY, 'turn/start'), { target: targetY, content: [{ type: 'text', text: 'Only newly selected Y input.' }] });
  s.next.transport.respond(sentY.id, { type: 'inbound_accepted', message_id: 'only-Y-input', inbound_sequence: '1' });
  await tick();
  s.next.transport.notify({ jsonrpc: '2.0', method: 'session/closed', params: { target: targetY } });
  await tick();
  const metadataX = await selectResumeByKeyboard(s.h, s.next.transport, SESSION_A, s.node, true);
  s.next.transport.respond(metadataX.id, { type: 'session', session: sessionView({ id: SESSION_A, active_node: s.node }) });
  const pendingX = await s.next.transport.nextPending('session/attach');
  const targetX = { ...s.previous.target, attachment_id: 'reselected-original-X', runtime_incarnation: '4' };
  s.next.transport.respond(pendingX.id, { type: 'attached', target: targetX, configuration_revision: '0', attachment_kind: 'controller', observation_source: 'live' });
  await followAttachment(s.next.transport, targetX, snapshot(), '0');
  await finishFocus(s.next.transport);
  s.h.editor.handleInput('\r');
  const sentX = await s.next.transport.nextPending('turn/start');
  assert.deepEqual(paramsOf(sentX, 'turn/start'), { target: targetX, content: s.expected }, 'the original receipt stays recoverable only under its actual X draft');
  s.next.transport.respond(sentX.id, { type: 'inbound_accepted', message_id: 'only-X-original-input', inbound_sequence: '1' });
  await tick();
  assert.equal(s.next.transport.log.count('session/uploadPrepare'), 0);
  assert.equal(s.first.transport.log.count('session/uploadPrepare'), 1);
  assert.equal(s.next.transport.log.count('turn/cancel'), 0);
});

for (const outcome of ['same Conversation', 'same Conversation with queued Q', 'different Conversation'] as const) {
  it(`focus metadata draft ownership: retired B restores ${outcome} without receipt crossover or replay`, async t => {
    const { host, transport } = await connected();
    const node = sessionView().active_node;
    const attaching = host.attach(SESSION_A, node);
    await attachment(transport, SESSION_A);
    const initial = await attaching;
    const h = appFor(t, host, { session: initial });
    const info = await transport.nextPending('session/read');
    transport.respond(info.id, { type: 'session', session: sessionView({ id: SESSION_A }) });
    h.editor.handleInput('A remains independent.');
    const oldB = await resumeByKeyboard(h, host, transport, SESSION_B, node, true);
    const prefix = 'B prefix before selected file. ';
    const tail = 'B retained tail.';
    h.editor.handleInput(prefix);
    const temp = TempFixture.create('rustx-tui-focus-cached-upload-');
    t.after(() => temp.cleanup());
    const path = temp.path('cached.txt');
    writeFileSync(path, 'cached B bytes');
    h.input('\x10');
    const command = h.commandEntries.at(-1)!;
    command.handleInput(`attach ${path}`);
    command.handleInput('\r');
    const upload = await transport.nextPending('session/uploadPrepare');
    assert.deepEqual(paramsOf(upload, 'session/uploadPrepare').target, oldB);
    transport.respond(upload.id, { type: 'upload_prepared', transfer: { path: '/session-upload/' + 'b'.repeat(43), loopback_port: 1234, expires_in_seconds: 60 } });
    const status = await transport.nextPending('session/uploadStatus');
    const receipt = { session_id: SESSION_B, batch_id: 'B-cached-batch', token: 'B-cached-token' };
    transport.respond(status.id, { type: 'upload_status', outcome: { state: 'ready', files: [{ receipt, file: { batch_id: receipt.batch_id, name: 'cached.txt' }, path: '/native/B-cached.txt' }] } });
    await tick();
    assert.ok(h.feedback.includes('Attached cached.txt'));
    h.editor.handleInput(tail);
    await resumeByKeyboard(h, host, transport, SESSION_A, node, true);
    assert.equal(h.editor.getExpandedText(), 'A remains independent.');
    transport.notify({ jsonrpc: '2.0', method: 'session/closed', params: { target: oldB } });
    await tick();
    assert.equal(host.attachment(SESSION_B), undefined);
    const metadata = await selectResumeByKeyboard(h, transport, SESSION_B, node, true);
    const text = 'New B input Q.';
    h.editor.handleInput(text);
    transport.respond(metadata.id, { type: 'session', session: sessionView({ id: SESSION_B, active_node: node }) });
    const pending = await transport.nextPending('session/attach');
    const queued = outcome === 'same Conversation with queued Q';
    if (queued) {
      h.editor.handleInput('\r');
      const intent = await transport.nextPending('session/submissionIntent');
      assert.deepEqual(paramsOf(intent, 'session/submissionIntent'), { session_id: SESSION_B, node_id: node });
      transport.respond(intent.id, { type: 'submission_intent_accepted' });
      await tick();
    }
    const target = { ...oldB, conversation_id: outcome === 'different Conversation' ? 'conv_01900000-0000-7000-8000-000000003003' : oldB.conversation_id, attachment_id: 'B-replacement', runtime_incarnation: '3' };
    transport.respond(pending.id, { type: 'attached', target, configuration_revision: '0', attachment_kind: 'controller', observation_source: 'live' });
    await followAttachment(transport, target, snapshot({ conversation_id: target.conversation_id }), '0');
    await finishFocus(transport, snapshot({ conversation_id: target.conversation_id }));
    const cached: UserInputBlock[] = [{ type: 'text', text: prefix }, { type: 'upload', ...receipt }, { type: 'text', text: tail }];
    if (!queued) h.editor.handleInput('\r');
    const sent = await transport.nextPending('turn/start');
    assert.deepEqual(paramsOf(sent, 'turn/start'), {
      target,
      content: outcome === 'same Conversation' ? [...cached, { type: 'text', text }] : [{ type: 'text', text }],
    }, 'only a live-confirmed matching Conversation can restore its old draft');
    transport.respond(sent.id, { type: 'inbound_accepted', message_id: 'B-new-input', inbound_sequence: '1' });
    await tick();
    assert.equal(transport.log.count('turn/start'), 1);
    if (queued) {
      assert.equal(h.editor.getExpandedText(), tail, 'the earlier editable tail and reserved prefix stay independent of captured Q');
      h.editor.handleInput('\r');
      const older = await transport.nextPending('turn/start');
      assert.deepEqual(paramsOf(older, 'turn/start'), { target, content: cached }, 'captured Q is never appended again to a later Send');
      transport.respond(older.id, { type: 'inbound_accepted', message_id: 'B-original-draft', inbound_sequence: '2' });
      await tick();
      assert.equal(transport.log.count('turn/start'), 2);
    }
    await resumeByKeyboard(h, host, transport, SESSION_A, node, true);
    assert.equal(h.editor.getExpandedText(), 'A remains independent.');
    assert.equal(transport.log.count('session/uploadPrepare'), 1);
    assert.equal(transport.log.count('session/uploadStatus'), 1);
    assert.equal(transport.log.count('turn/cancel'), 0);
  });
}

it('focus metadata draft ownership: disconnected metadata cannot activate its retired target', async t => {
  const first = await connected();
  const next = await connected();
  const node = sessionView().active_node;
  const attaching = first.host.attach(SESSION_A, node);
  await attachment(first.transport, SESSION_A);
  const initial = await attaching;
  const h = appFor(t, first.host, { session: initial }, async () => next.host);
  const info = await first.transport.nextPending('session/read');
  first.transport.respond(info.id, { type: 'session', session: sessionView({ id: SESSION_A }) });
  h.editor.handleInput('A retained before metadata.');
  const metadata = await selectResumeByKeyboard(h, first.transport, SESSION_B, node, true);
  h.editor.handleInput('B belongs to its selected target.');
  first.transport.fail('socket_error');
  await catalog(next.transport);
  await tick();
  first.transport.respond(metadata.id, { type: 'session', session: sessionView({ id: SESSION_B, active_node: node }) });
  await tick();
  assert.equal(next.transport.log.count('session/attach'), 0, 'a late old-authority metadata result cannot install a target');
  assert.equal(next.transport.log.count('turn/start'), 0);
  assert.equal(next.transport.log.count('session/submissionIntent'), 0);
  const selector = h.surfaces.at(-1)!;
  selector.handleInput('\r');
  const current = await next.transport.nextPending('session/read');
  assert.equal(paramsOf(current, 'session/read').session_id, SESSION_A);
  next.transport.respond(current.id, { type: 'session', session: sessionView({ id: SESSION_A, active_node: node }) });
  await attachment(next.transport, SESSION_A);
  await finishFocus(next.transport);
  assert.equal(h.editor.getExpandedText(), 'A retained before metadata.');
  assert.equal(next.transport.log.count('turn/start'), 0);
});

/** A bound receipt draft P stays cached while a selected unbound Q / R is
 * captured and the returned Session is held at its final observation repair. */
async function heldReceiptReturn(t: TestContext, captured: boolean, laterReplacement?: AppServerHost) {
  const s = await uploadedReconnectDraft(t, 'P tail before Q.', undefined, laterReplacement);
  const original = await s.complete();
  await resumeByKeyboard(s.h, s.next.host, s.next.transport, SESSION_B, s.node, true);
  s.h.editor.handleInput('B independent across retirement.');
  s.next.transport.notify({ jsonrpc: '2.0', method: 'session/closed', params: { target: original } });
  await tick();
  const metadata = await selectResumeByKeyboard(s.h, s.next.transport, SESSION_A, s.node, true);
  const textQ = '/cancel';
  const textR = ' R typed after Q.';
  s.h.editor.handleInput(`\x1b[200~${textQ}\x1b[201~`);
  s.next.transport.respond(metadata.id, { type: 'session', session: sessionView({ id: SESSION_A, active_node: s.node }) });
  const attach = await s.next.transport.nextPending('session/attach');
  if (captured) {
    s.h.editor.handleInput('\r');
    const intent = await s.next.transport.nextPending('session/submissionIntent');
    s.next.transport.respond(intent.id, { type: 'submission_intent_accepted' });
  }
  s.h.editor.handleInput(textR);
  const target = { ...original, attachment_id: 'prepared-before-final-repair', runtime_incarnation: '3' };
  s.next.transport.respond(attach.id, { type: 'attached', target, configuration_revision: '0', attachment_kind: 'controller', observation_source: 'live' });
  await followAttachment(s.next.transport, target, snapshot(), '0');
  const repair = await s.next.transport.nextPending('session/checkpoint');
  assert.equal(s.next.transport.log.count('turn/start'), 0);
  return { ...s, target, repair, textQ, textR, content: [...s.expected, ...(captured ? [{ type: 'text' as const, text: textQ }, { type: 'text' as const, text: textR }] : [{ type: 'text' as const, text: textQ + textR }])] };
}

for (const captured of [false, true]) {
  it(`captured draft order: a retired prepared Session cannot bind after final repair${captured ? ' with captured Q' : ' without a captured Send'}`, async t => {
    const s = await heldReceiptReturn(t, captured);
    const subscriptions = s.next.transport.log.count('session/subscribe');
    s.next.transport.notify({ jsonrpc: '2.0', method: 'session/closed', params: { target: s.target } });
    s.next.transport.respond(s.repair.id, { type: 'checkpoint', checkpoint: checkpoint(snapshot(), '0') });
    await tick();
    assert.match(s.h.feedback.at(-1)!, /prepared Session connection closed/);
    assert.equal(s.next.transport.log.count('session/subscribe'), subscriptions, 'the retired repair cannot register late observation authority');
    assert.equal(s.next.transport.log.count('turn/start'), 0);
    assert.equal(s.next.transport.log.count('turn/cancel'), 0);
    const selected = await resumeByKeyboard(s.h, s.next.host, s.next.transport, SESSION_A, s.node);
    s.h.editor.handleInput('\r');
    const sent = await s.next.transport.nextPending('turn/start');
    assert.deepEqual(paramsOf(sent, 'turn/start'), { target: selected, content: s.content });
    s.next.transport.respond(sent.id, { type: 'inbound_accepted', message_id: 'explicit-after-prepared-retirement', inbound_sequence: '1' });
    await tick();
    assert.equal(s.next.transport.log.count('session/uploadPrepare'), 0);
  });
}

for (const sameConversation of [true, false]) {
  it(`captured draft order: disconnect during final repair retains exact Conversation ownership (${sameConversation ? 'same' : 'changed'} replacement)`, async t => {
    const replacement = await connected();
    const s = await heldReceiptReturn(t, true, replacement.host);
    s.next.transport.fail('socket_error');
    const attach = await replacement.transport.nextPending('session/attach');
    assert.deepEqual(paramsOf(attach, 'session/attach'), { session_id: SESSION_A, node_id: s.node });
    assert.equal(replacement.transport.log.count('session/submissionIntent'), 0, 'recovery is observation, never a new explicit Send');
    assert.equal(replacement.transport.log.count('turn/start'), 0);
    const target = { ...s.target, conversation_id: sameConversation ? s.target.conversation_id : 'conv_01900000-0000-7000-8000-000000003003', attachment_id: 'after-final-repair-disconnect', runtime_incarnation: '4' };
    replacement.transport.respond(attach.id, { type: 'attached', target, configuration_revision: '0', attachment_kind: 'controller', observation_source: 'live' });
    await followAttachment(replacement.transport, target, snapshot({ conversation_id: target.conversation_id }), '0');
    let selected = target;
    if (sameConversation) await finishFocus(replacement.transport);
    else {
      await tick();
      assert.match(s.h.feedback.at(-1)!, /attached Conversation changed/);
      assert.equal(replacement.transport.log.count('session/checkpoint'), 1, 'changed authority never reaches App binding or its final repair');
      assert.equal(replacement.transport.log.count('turn/start'), 0, 'no old receipt or captured message crosses into the changed Conversation');
      replacement.transport.notify({ jsonrpc: '2.0', method: 'session/closed', params: { target } });
      selected = await resumeByKeyboard(s.h, replacement.host, replacement.transport, SESSION_A, s.node);
    }
    assert.equal(replacement.transport.log.count('turn/start'), 0, 'a successful replacement does not replay the captured input');
    assert.equal(replacement.transport.log.count('turn/steer'), 0);
    s.h.editor.handleInput('\r');
    const sent = await replacement.transport.nextPending('turn/start');
    assert.deepEqual(paramsOf(sent, 'turn/start'), { target: selected, content: s.content });
    replacement.transport.respond(sent.id, { type: 'inbound_accepted', message_id: 'new-explicit-after-repair-disconnect', inbound_sequence: '1' });
    await tick();
    assert.equal(s.next.transport.log.count('turn/start'), 0);
    assert.equal(replacement.transport.log.count('turn/start'), 1);
    assert.equal(replacement.transport.log.count('turn/cancel'), 0);
    assert.equal(replacement.transport.log.count('session/uploadPrepare'), 0);
  });
}

for (const { boundary, withdrawal } of [
  ...(['before attachment', 'held final repair'] as const).flatMap(boundary => (['Escape', 'intent refusal'] as const).map(withdrawal => ({ boundary, withdrawal }))),
  { boundary: 'held final repair', withdrawal: 'repair refusal' },
]) {
  it(`captured draft order: ${withdrawal} at ${boundary} retains bound P before Q before newer R`, async t => {
    const s = await uploadedReconnectDraft(t, 'P tail before Q.');
    const original = await s.complete();
    await resumeByKeyboard(s.h, s.next.host, s.next.transport, SESSION_B, s.node, true);
    s.h.editor.handleInput('B independent.');
    s.next.transport.notify({ jsonrpc: '2.0', method: 'session/closed', params: { target: original } });
    await tick();
    const metadata = await selectResumeByKeyboard(s.h, s.next.transport, SESSION_A, s.node, true);
    const textQ = '/cancel';
    const textR = ' R typed after captured Q.';
    s.h.editor.handleInput(`\x1b[200~${textQ}\x1b[201~`);
    s.next.transport.respond(metadata.id, { type: 'session', session: sessionView({ id: SESSION_A, active_node: s.node }) });
    const attach = await s.next.transport.nextPending('session/attach');
    s.h.editor.handleInput('\r');
    const intent = await s.next.transport.nextPending('session/submissionIntent');
    if (withdrawal !== 'intent refusal') s.next.transport.respond(intent.id, { type: 'submission_intent_accepted' });
    s.h.editor.handleInput(textR);
    const withdraw = async (repairId?: import('../src/protocol/app-server.ts').AppServerRequestId) => {
      if (withdrawal === 'Escape') s.h.input('\x1b');
      else if (withdrawal === 'intent refusal') s.next.transport.respondError(intent.id, { code: -32000, message: 'explicit submission-intent refusal' });
      else { assert.ok(repairId !== undefined); s.next.transport.respondError(repairId, { code: -32000, message: 'explicit final-repair refusal' }); }
      await tick();
      assert.equal(s.next.transport.log.count('turn/start'), 0);
      assert.equal(s.next.transport.log.count('turn/cancel'), 0);
    };
    if (boundary === 'before attachment') await withdraw();
    const target = { ...original, attachment_id: 'ordered-queue-return-A', runtime_incarnation: '3' };
    s.next.transport.respond(attach.id, { type: 'attached', target, configuration_revision: '0', attachment_kind: 'controller', observation_source: 'live' });
    await followAttachment(s.next.transport, target, snapshot(), '0');
    const repair = await s.next.transport.nextPending('session/checkpoint');
    if (boundary === 'held final repair') await withdraw(repair.id);
    if (withdrawal === 'repair refusal') {
      assert.match(s.h.feedback.at(-1)!, /could not open Session/);
      await resumeByKeyboard(s.h, s.next.host, s.next.transport, SESSION_A, s.node);
    } else {
      s.next.transport.respond(repair.id, { type: 'checkpoint', checkpoint: checkpoint(snapshot(), '0') });
      const subscribe = await s.next.transport.nextPending('session/subscribe');
      s.next.transport.respond(subscribe.id, { type: 'subscribed', after_cursor: '0' });
      const footer = await s.next.transport.nextPending('session/read');
      s.next.transport.respond(footer.id, { type: 'session', session: sessionView({ id: SESSION_A }) });
      await tick();
    }
    s.h.editor.handleInput('\r');
    const sent = await s.next.transport.nextPending('turn/start');
    assert.deepEqual(paramsOf(sent, 'turn/start'), { target, content: [...s.expected, { type: 'text', text: textQ }, { type: 'text', text: textR }] });
    s.next.transport.respond(sent.id, { type: 'inbound_accepted', message_id: 'ordered-P-Q-R', inbound_sequence: '1' });
    await tick();
    assert.equal(s.h.editor.getExpandedText(), '');
    assert.equal(s.next.transport.log.count('session/uploadPrepare'), 0);
    assert.equal(s.next.transport.log.count('turn/cancel'), 0);
    await resumeByKeyboard(s.h, s.next.host, s.next.transport, SESSION_B, s.node, true);
    assert.equal(s.h.editor.getExpandedText(), 'B independent.');
  });
}

for (const subscription of ['accepted', 'refused then reselected'] as const) for (const queued of [false, true]) {
  it(`prepared draft ownership: ${subscription} preserves bound P and pending Q${queued ? ' with newer typing' : ''}`, async t => {
    const { host, transport } = await connected();
    const node = sessionView().active_node;
    const attaching = host.attach(SESSION_A, node);
    await attachment(transport, SESSION_A);
    const initial = await attaching;
    const h = appFor(t, host, { session: initial });
    const info = await transport.nextPending('session/read');
    transport.respond(info.id, { type: 'session', session: sessionView({ id: SESSION_A }) });
    h.editor.handleInput('Independent A draft.');
    const oldB = await resumeByKeyboard(h, host, transport, SESSION_B, node, true);
    const textP = 'P bound to B. ';
    const textQ = '/cancel';
    const textR = 'R typed after captured Q.';
    h.editor.handleInput(textP);
    await resumeByKeyboard(h, host, transport, SESSION_A, node, true);
    transport.notify({ jsonrpc: '2.0', method: 'session/closed', params: { target: oldB } });
    await tick();
    const metadata = await selectResumeByKeyboard(h, transport, SESSION_B, node, true);
    h.editor.handleInput(`\x1b[200~${textQ}\x1b[201~`);
    transport.respond(metadata.id, { type: 'session', session: sessionView({ id: SESSION_B, active_node: node }) });
    const pending = await transport.nextPending('session/attach');
    if (queued) {
      h.editor.handleInput('\r');
      const intent = await transport.nextPending('session/submissionIntent');
      transport.respond(intent.id, { type: 'submission_intent_accepted' });
      h.editor.handleInput(textR);
    }
    const target = { ...oldB, attachment_id: 'prepared-B-subscription', runtime_incarnation: '3' };
    transport.respond(pending.id, { type: 'attached', target, configuration_revision: '0', attachment_kind: 'controller', observation_source: 'live' });
    if (subscription === 'accepted') {
      await followAttachment(transport, target, snapshot({ conversation_id: target.conversation_id }), '0');
      await finishFocus(transport, snapshot({ conversation_id: target.conversation_id }));
    } else {
      const read = await transport.nextPending('session/checkpoint');
      transport.respond(read.id, { type: 'checkpoint', checkpoint: checkpoint(snapshot({ conversation_id: target.conversation_id }), '0') });
      const subscribe = await transport.nextPending('session/subscribe');
      transport.respondError(subscribe.id, { code: -32000, message: 'subscription explicitly refused' });
      await tick();
      assert.ok(host.attachment(SESSION_B), 'the Host retains the physical prepared attachment before subscription acknowledgement');
      assert.equal(transport.log.count('turn/start'), 0);
      await resumeByKeyboard(h, host, transport, SESSION_B, node);
    }
    const automatic = subscription === 'accepted' && queued;
    if (!automatic) h.editor.handleInput('\r');
    const sent = await transport.nextPending('turn/start');
    const expected = automatic ? [{ type: 'text', text: textQ }] : [
      { type: 'text', text: textP }, { type: 'text', text: textQ }, ...(queued ? [{ type: 'text', text: textR }] : []),
    ];
    assert.deepEqual(paramsOf(sent, 'turn/start'), { target, content: expected }, 'pending input remains owned until it is actually merged or dispatched');
    transport.respond(sent.id, { type: 'inbound_accepted', message_id: 'prepared-B-preserved-input', inbound_sequence: '1' });
    await tick();
    assert.equal(transport.log.count('turn/cancel'), 0, 'restored pasted Q remains message content');
    if (automatic) {
      h.editor.handleInput('\r');
      const later = await transport.nextPending('turn/start');
      assert.deepEqual(paramsOf(later, 'turn/start'), { target, content: [{ type: 'text', text: textP }, { type: 'text', text: textR }] });
      transport.respond(later.id, { type: 'inbound_accepted', message_id: 'P-and-newer-R-only', inbound_sequence: '2' });
      await tick();
    }
    await resumeByKeyboard(h, host, transport, SESSION_A, node, true);
    assert.equal(h.editor.getExpandedText(), 'Independent A draft.');
    assert.equal(transport.log.count('session/attach'), 3, 'reselection of the prepared attachment never allocates another physical attachment');
  });
}

it('prepared draft ownership: navigating an edited restored upload draft does not validate Send', async t => {
  const s = await uploadedReconnectDraft(t, 'original tail');
  const intent = await s.sendConnecting();
  s.next.transport.respond(intent.id, { type: 'submission_intent_accepted' });
  s.h.input('\x1b');
  const original = await s.complete();
  const restored = 'prefix before selected file\noriginal tail';
  assert.equal(s.h.editor.getExpandedText(), restored);
  s.h.editor.handleInput(' edited');
  await resumeByKeyboard(s.h, s.next.host, s.next.transport, SESSION_B, s.node, true);
  s.next.transport.notify({ jsonrpc: '2.0', method: 'session/closed', params: { target: original } });
  await tick();
  const metadata = await selectResumeByKeyboard(s.h, s.next.transport, SESSION_A, s.node, true);
  s.next.transport.respond(metadata.id, { type: 'session', session: sessionView({ id: SESSION_A, active_node: s.node }) });
  const pending = await s.next.transport.nextPending('session/attach');
  const target = { ...original, attachment_id: 'restored-edited-A', runtime_incarnation: '3' };
  s.next.transport.respond(pending.id, { type: 'attached', target, configuration_revision: '0', attachment_kind: 'controller', observation_source: 'live' });
  await followAttachment(s.next.transport, target, snapshot(), '0');
  await tick();
  assert.ok(!s.h.feedback.at(-1)?.startsWith('could not open Session'), 'navigation cannot run the actual Send ordering validator');
  await finishFocus(s.next.transport);
  assert.equal(s.h.editor.getExpandedText(), restored + ' edited');
  s.h.editor.handleInput('\r');
  await tick();
  assert.equal(s.next.transport.log.count('turn/start'), 0);
  assert.match(s.h.feedback.at(-1)!, /Cannot edit restored text separated by uploads/);
  assert.equal(s.next.transport.log.count('session/uploadPrepare'), 0);
});

for (const { refused, edited, tail } of [
  ...[false, true].flatMap(refused => [false, true].map(edited => ({ refused, edited, tail: 'original tail' }))),
  { refused: true, edited: true, tail: '' },
]) {
  it(`prepared draft ownership: ${refused ? 'refused subscription retains' : 'successful attach merges'} ordered receipts${edited ? (tail ? ' without laundering edited P' : ' with unambiguous edits') : ''}`, async t => {
    const s = await uploadedReconnectDraft(t, tail);
    const intent = await s.sendConnecting();
    s.next.transport.respond(intent.id, { type: 'submission_intent_accepted' });
    s.h.input('\x1b');
    const original = await s.complete();
    const baseline = 'prefix before selected file\n' + tail;
    if (edited) s.h.editor.handleInput(' edited');
    await resumeByKeyboard(s.h, s.next.host, s.next.transport, SESSION_B, s.node, true);
    s.h.editor.handleInput('B stays independent.');
    s.next.transport.notify({ jsonrpc: '2.0', method: 'session/closed', params: { target: original } });
    await tick();
    const metadata = await selectResumeByKeyboard(s.h, s.next.transport, SESSION_A, s.node, true);
    const textQ = ' Q after the retained receipt draft.';
    s.h.editor.handleInput(`\x1b[200~${textQ}\x1b[201~`);
    s.next.transport.respond(metadata.id, { type: 'session', session: sessionView({ id: SESSION_A, active_node: s.node }) });
    const pending = await s.next.transport.nextPending('session/attach');
    const target = { ...original, attachment_id: 'joined-raw-draft-A', runtime_incarnation: '3' };
    s.next.transport.respond(pending.id, { type: 'attached', target, configuration_revision: '0', attachment_kind: 'controller', observation_source: 'live' });
    if (refused) {
      const read = await s.next.transport.nextPending('session/checkpoint');
      s.next.transport.respond(read.id, { type: 'checkpoint', checkpoint: checkpoint(snapshot(), '0') });
      const subscribe = await s.next.transport.nextPending('session/subscribe');
      s.next.transport.respondError(subscribe.id, { code: -32000, message: 'subscription explicitly refused before draft transfer' });
      await tick();
      assert.ok(s.next.host.attachment(SESSION_A));
      assert.equal(s.next.transport.log.count('turn/start'), 0);
      await resumeByKeyboard(s.h, s.next.host, s.next.transport, SESSION_A, s.node);
    } else {
      await followAttachment(s.next.transport, target, snapshot(), '0');
      await finishFocus(s.next.transport);
    }
    assert.equal(s.h.editor.getExpandedText(), baseline + (edited ? ' edited' : '') + textQ, 'all retained input remains visible and editable');
    assert.equal(s.next.transport.log.count('session/uploadPrepare'), 0, 'confirmed receipts are not uploaded a second time');
    s.h.editor.handleInput('\r');
    if (edited && tail) {
      await tick();
      assert.equal(s.next.transport.log.count('turn/start'), 0, 'navigation cannot turn an ambiguous restored edit into accepted blocks');
      assert.match(s.h.feedback.at(-1)!, /Cannot edit restored text separated by uploads/);
      s.h.editor.setText(baseline + textQ);
      s.h.editor.handleInput('\r');
    }
    const sent = await s.next.transport.nextPending('turn/start');
    const expected = edited && !tail ? [{ type: 'text', text: baseline + ' edited' }, ...s.expected.slice(1)] : s.expected;
    assert.deepEqual(paramsOf(sent, 'turn/start'), { target, content: [...expected, { type: 'text', text: textQ }] });
    s.next.transport.respond(sent.id, { type: 'inbound_accepted', message_id: 'retained-receipt-P-and-Q', inbound_sequence: '1' });
    await tick();
    assert.equal(s.h.editor.getExpandedText(), '');
    await resumeByKeyboard(s.h, s.next.host, s.next.transport, SESSION_B, s.node, true);
    assert.equal(s.h.editor.getExpandedText(), 'B stays independent.');
    assert.equal(s.next.transport.log.count('turn/start'), 1);
    assert.equal(s.next.transport.log.count('turn/cancel'), 0);
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

import assert from 'node:assert/strict';
import { it } from 'node:test';
import { editorText, editorSubmission, RestoredEditorOrderingError } from '../src/app-server/editor.ts';
import type { UserInputBlock } from '../src/protocol/app-server.ts';
import { Editor, TUI } from '@earendil-works/pi-tui';
import { RustxTuiApp } from '../src/ui/app.ts';
import { CommandDispatcher } from '../src/commands/dispatcher.ts';
import { AppServerSession } from '../src/app-server/session.ts';
import type { AppServerClient } from '../src/app-server/client.ts';
import type { AppServerHost } from '../src/app-server/host.ts';
import { TransientFeedbackSurface, type TransientFeedback } from '../src/ui/components/transient-feedback.ts';
import { snapshot, runtimeCursor, sessionView } from './support/fixtures.ts';

it('restored drafts preserve ordered receipts and exact text through submission', () => {
  const draft: UserInputBlock[] = [
    { type: 'upload', session_id: 'destination', batch_id: 'a', token: 'one' },
    { type: 'text', text: '\n body  \n' },
    { type: 'upload', session_id: 'destination', batch_id: 'b', token: 'two' },
  ];
  assert.equal(editorText(draft), '\n body  \n');
  assert.deepEqual(editorSubmission(draft, editorText(draft)), draft);
  assert.deepEqual(editorSubmission(draft, 'edited'), [draft[0], { type: 'text', text: 'edited' }, draft[2]]);
});

for (const uploads of [1, 2]) {
  it(`refuses edited restored text across ${uploads} upload boundaries without mutating input`, () => {
    const draft = interleavedDraft(uploads);
    const original = structuredClone(draft);
    assert.deepEqual(editorSubmission(draft, editorText(draft)), original);
    assert.throws(() => editorSubmission(draft, 'edited'), RestoredEditorOrderingError);
    assert.deepEqual(draft, original, 'all text and exact receipts remain available');
  });
}

it('allows adjacent text blocks within one editable region', () => {
  const upload = interleavedDraft(1)[1]!;
  assert.deepEqual(editorSubmission([upload, { type: 'text', text: 'A' }, { type: 'text', text: 'B' }, upload], 'edited'),
    [upload, { type: 'text', text: 'edited' }, upload]);
});

function interleavedDraft(uploads: number): UserInputBlock[] {
  const draft: UserInputBlock[] = [{ type: 'text', text: '\n A ' }];
  for (let i = 0; i < uploads; i++) {
    draft.push({ type: 'upload', session_id: 'destination', batch_id: `batch-${i}`, token: `receipt-${i}` });
    draft.push({ type: 'text', text: ` B${i} \n` });
  }
  return draft;
}

it('TUI refuses an ambiguous restored edit without turn/start and retains the draft for exact recovery', async (t) => {
  const draft = interleavedDraft(2);
  const original = structuredClone(draft);
  const turns: UserInputBlock[][] = [];
  const target = { session_id: 'destination', conversation_id: "conv_01900000-0000-7000-8000-000000000002", runtime_incarnation: '1', attachment_id: 'attachment' };
  const client = {
    closed: undefined,
    onClose: () => () => {},
    call: async (method: string, params: { content: UserInputBlock[] }) => {
      if (method === 'session/attach') return { target, snapshot: snapshot(), cursor: runtimeCursor(1) };
      assert.equal(method, 'turn/start');
      turns.push(params.content);
      return { message_id: 'accepted', inbound_sequence: '1' };
    },
  } as unknown as AppServerClient;
  const session = await AppServerSession.attach(client, 'destination');
  t.mock.method(session, 'resync', async () => {});
  const host = {
    client, ownership: 'external',
    attach: async () => session,
    attachment: () => undefined,
    readSession: async () => sessionView({ id: 'destination' }),
    shutdown: async () => undefined,
  } as unknown as AppServerHost;
  t.mock.method(CommandDispatcher.prototype, 'submit', async () => ({
    kind: 'focus_session' as const, sessionId: 'destination', editorContent: draft,
  }));
  // Keep real editor input/submission mechanics; terminal I/O is irrelevant.
  t.mock.method(TUI.prototype, 'start', () => {});
  t.mock.method(TUI.prototype, 'stop', () => {});
  t.mock.method(TUI.prototype, 'requestRender', () => {});
  let editor!: Editor;
  const feedback: TransientFeedback[] = [];
  t.mock.method(TransientFeedbackSurface.prototype, 'replace', (value: TransientFeedback) => { feedback.push(value); });
  let focusedEditor: Editor | undefined;
  t.mock.method(TUI.prototype, 'setFocus', (component: unknown) => {
    if (component instanceof Editor) focusedEditor = component;
  });
  const app = new RustxTuiApp({ host, session, sessionSettings: { cwd: '/work/project' } });
  const running = app.run();
  try {
    // Drive the actual editor selected by app.run().
    editor = focusedEditor!;
    editor.setText('/fork');
    editor.handleInput('\r');
    await new Promise<void>(resolve => setImmediate(resolve));
    assert.equal(editor.getExpandedText(), editorText(original));
    const edited = '\n edited text \n';
    editor.setText(edited);
    editor.handleInput('\r');
    assert.deepEqual(turns, [], 'no turn/start or reordered payload is sent');
    assert.equal(editor.getExpandedText(), edited, 'Pi-cleared editor is restored exactly');
    assert.deepEqual(draft, original, 'all receipts and original text remain intact');
    assert.equal(feedback.at(-1)?.level, 'error');
    assert.match(feedback.at(-1)!.text, /Cannot edit restored text separated by uploads/);
    assert.match(feedback.at(-1)!.text, /Restore the original text/);
    editor.handleInput('\r');
    assert.deepEqual(turns, [], 'retry stays refused with retained receipts');
    editor.setText(editorText(original));
    editor.handleInput('\r');
    await new Promise<void>(resolve => setImmediate(resolve));
    assert.deepEqual(turns, [original], 'recovery submits the exact original ordered content');
    assert.equal(editor.getExpandedText(), '');
  } finally {
    await app.quit();
    await running;
  }
});

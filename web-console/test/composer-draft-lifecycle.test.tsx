import { act, cleanup, fireEvent, render, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { AgentComposer } from '../src/app/agent/AgentComposer';
import { ComposerDraft } from '../src/app/composer/draft';
import { App } from '../src/app/App';
import { Server, snapshot } from './fixture';
import { firstSubmissions } from '../src/app/new-conversation/first-submit';

let server: Server;
afterEach(() => { cleanup(); server?.client.disconnect(); localStorage.clear(); vi.restoreAllMocks(); });
async function fixture() {
  server = new Server();
  const origin = { type: 'fork' as const, source_session: 'B', source_node: 'source-node', source_message: 'source-message', source_surface_revision: '1', side: 'after' as const };
  const child = snapshot(); child.attempt = { attempt_id: 'active-A', phase: { type: 'running' }, turn: 1 };
  child.transcript = { inherited_through: '0', entries: [] }; server.snapshots.set('A', child);
  const source = snapshot('B'); source.transcript.entries = [{ cursor: '12', item: { type: 'message', message: { role: 'assistant', id: origin.source_message, content: [{ type: 'text', text: 'Exact historical source' }] } } }];
  server.nodeSnapshots.set(origin.source_node, source);
  server.handlers.set('session/node', request => {
    if (request.method !== 'session/node') throw new Error('wrong method');
    return { type: 'node', node: { id: request.params.node_id, ordinal: '1', conversation_id: request.params.node_id === origin.source_node ? source.conversation_id : server.snapshots.get(request.params.session_id)!.conversation_id, origin: request.params.session_id === 'A' ? origin : { type: 'new' } } };
  });
  await server.attached('A');
  localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint: server.client.getSnapshot().endpoint, openViews: ['A'] }));
  const ui = await act(async () => render(<App client={server.client} workspaceHost={server.workspaceHost}/>));
  await waitFor(() => expect((ui.getByRole('textbox', { name: 'Message' }) as HTMLTextAreaElement).disabled).toBe(false));
  const editor = () => ui.getByRole('textbox', { name: 'Message' }) as HTMLTextAreaElement;
  fireEvent.change(editor(), { target: { value: 'Unsent exact draft\nsecond line' } });
  editor().setSelectionRange(7, 12, 'backward'); fireEvent.select(editor());
  const key = JSON.stringify([server.client.getSnapshot().endpoint, server.client.getSnapshot().authorityId, '0']);
  const intake = server.client.attachmentIntakes.lookup(key)!;
  return { ui, editor, intake, key, origin, target: server.client.target('A'), text: editor().value };
}
const mutations = (from: number) => server.requests.slice(from).filter(({ request }) => ['session/attach', 'session/detach', 'session/switchNode', 'turn/start', 'turn/steer', 'session/uploadPrepare', 'session/checkpoint'].includes(request.method));
it('restored native upload blocks survive temporary unmount only under their original receipt authority', () => {
  const draftOwner = new ComposerDraft();
  const props = { draftOwner, binding: 'same-editor', disabled: false, busy: false, active: false, onSend: async () => false, onUpload: async () => [], onCancel: () => {} };
  const initialContent = [{ type: 'upload' as const, session_id: 'A', batch_id: 'original-batch', token: 'original-token' }, { type: 'text' as const, text: 'Native restored draft' }];
  let ui = render(<AgentComposer {...props} initialContent={initialContent} receiptAuthority="original-native"/>);
  fireEvent.change(ui.getByLabelText('Message'), { target: { value: 'Edited restored draft' } }); ui.unmount();
  ui = render(<AgentComposer {...props} initialContent={initialContent} receiptAuthority="original-native"/>);
  expect((ui.getByLabelText('Message') as HTMLTextAreaElement).value).toBe('Edited restored draft');
  expect(ui.getByText(/original-batch/)).toBeTruthy(); ui.unmount();
  ui = render(<AgentComposer {...props} initialContent={initialContent} receiptAuthority="replacement-native"/>);
  expect((ui.getByLabelText('Message') as HTMLTextAreaElement).value).toBe('Edited restored draft'); expect(ui.queryByText(/original-batch/)).toBeNull();
});
it('draft identity survives retirement of a completed submission while a different editor identity discards content', () => {
  const draftOwner = new ComposerDraft();
  const props = { draftOwner, draftIdentity: 'conversation-A', disabled: false, busy: false, active: false, onSend: async () => false, onUpload: async () => [], onCancel: () => {} };
  let ui = render(<AgentComposer {...props} binding="consumed-submission"/>);
  fireEvent.change(ui.getByLabelText('Message'), { target: { value: 'Unsent after completed input' } }); ui.unmount();
  ui = render(<AgentComposer {...props} binding="native-submission-retired"/>);
  expect((ui.getByLabelText('Message') as HTMLTextAreaElement).value).toBe('Unsent after completed input'); ui.unmount();
  ui = render(<AgentComposer {...props} draftIdentity="conversation-B" binding="another-route"/>);
  expect((ui.getByLabelText('Message') as HTMLTextAreaElement).value).toBe('');
});

it('late native acknowledgement from A cannot change a deliberately replaced new Conversation editor', async () => {
  const f = await fixture(); server.held.add('turn/start');
  await act(async () => fireEvent.keyDown(f.editor(), { key: 'Enter' })); const sent = await server.waitFor('turn/start', 1);
  await act(async () => fireEvent.click(f.ui.getAllByRole('button', { name: 'New Conversation' })[0]));
  fireEvent.change(f.editor(), { target: { value: 'Independent new Conversation' } });
  await act(async () => server.reply(sent));
  expect(f.editor().value).toBe('Independent new Conversation'); expect(server.requests.filter(row => row.request.method === 'turn/start')).toHaveLength(1);
});

it('a late native reply after terminal uncertain First Submission Discard cannot alter its replacement editor', async () => {
  const f = await fixture(), file = new File(['first'], 'first.txt');
  await act(async () => f.intake.add([{ file }], server.client.getSnapshot().capabilities!.upload_policy));
  server.held.add('turn/start'); await act(async () => fireEvent.keyDown(f.editor(), { key: 'Enter' }));
  const sent = await server.waitFor('turn/start', 1);
  expect(firstSubmissions(server.client).draft('0')?.phase).toBe('admitting');
  // A settled uncertain response permits Discard; the in-flight phase did not.
  expect(f.ui.queryByRole('button', { name: /Discard/i })).toBeNull();
  await act(async () => server.socket.deliver({ jsonrpc: '2.0', id: sent.id, error: { code: -32000, message: 'Committed durability unknown', data: { kind: 'committed_durability_uncertain' } } }));
  expect(firstSubmissions(server.client).draft('0')?.phase).toBe('uncertain');
  await act(async () => fireEvent.click(f.ui.getByRole('button', { name: /Discard/i })));
  const discarded = firstSubmissions(server.client).draft('0');
  fireEvent.change(f.editor(), { target: { value: 'Replacement E2' } });
  const incarnation = f.ui.container.querySelector('[data-editor-incarnation]')!.getAttribute('data-editor-incarnation');
  await act(async () => { server.reply(sent); await server.client.request({ method: 'server/info', params: {} }, 'server_info'); });
  expect(firstSubmissions(server.client).draft('0')).toBe(discarded);
  expect(f.editor().value).toBe('Replacement E2'); expect(f.intake.snapshot()).toEqual([]);
  expect(f.ui.container.querySelector('[data-editor-incarnation]')!.getAttribute('data-editor-incarnation')).toBe(incarnation);
  expect(server.requests.filter(row => row.request.method === 'turn/start')).toHaveLength(1);
  expect(server.requests.filter(row => row.request.method === 'session/uploadPrepare')).toHaveLength(1);
  expect(server.client.target('A')).toBe(f.target);
});

it('reselecting the already committed Session and unrelated publication preserve the exact editor and intake', async () => {
  const f = await fixture(), file = new File(['local'], 'local.txt');
  await act(async () => fireEvent.change(f.ui.getByLabelText('Attach files'), { target: { files: [file] } }));
  await waitFor(() => expect(f.intake.snapshot()[0].status).toBe('ready'));
  const selected = f.intake.snapshot(), before = server.requests.length;
  const card = f.editor().closest('[data-editor-incarnation]')!, incarnation = card.getAttribute('data-editor-incarnation'), revision = card.getAttribute('data-editor-revision');
  for (let click = 0; click < 2; click++) await act(async () => fireEvent.click(f.ui.getByRole('button', { name: 'Open Session A' })));
  expect(mutations(before)).toEqual([]);
  await act(async () => server.update('A', { ...server.snapshots.get('A')!, pending_interactions: [] }));
  expect(f.editor().value).toBe(f.text); expect([f.editor().selectionStart, f.editor().selectionEnd, f.editor().selectionDirection]).toEqual([7, 12, 'backward']);
  expect(f.intake.snapshot()).toBe(selected); expect(card.getAttribute('data-editor-incarnation')).toBe(incarnation); expect(card.getAttribute('data-editor-revision')).toBe(revision);
  // The fixture's explicit resync publication legitimately requests a snapshot.
  expect(mutations(before).filter(row => row.request.method !== 'session/checkpoint')).toEqual([]); expect(server.client.target('A')).toBe(f.target);
});
it.each(['reattach', 'switch'] as const)('known C1 acceptance after explicit lifecycle %s records the fact under exact canonical identity', async kind => {
  const f = await fixture(), submit = vi.spyOn(ComposerDraft.prototype, 'submit');
  server.held.add('turn/start'); await act(async () => fireEvent.keyDown(f.editor(), { key: 'Enter' }));
  const sent = await server.waitFor('turn/start', 1), reply = server.commit(sent), ticket = submit.mock.results[0].value!;
  const card = f.editor().closest('[data-editor-incarnation]')!, incarnation = card.getAttribute('data-editor-incarnation'), revision = card.getAttribute('data-editor-revision');
  await act(async () => {
    if (kind === 'switch') { server.nodeSnapshots.set('C2-node', snapshot('C2')); await server.client.switchNode('A', 'C2-node'); await server.client.attach('A', 'C2-node'); }
    else { await server.client.release('A'); await server.client.attach('A'); }
  });
  const target = server.client.target('A'), before = server.requests.length;
  expect(target).not.toEqual(f.target); expect(card.getAttribute('data-editor-incarnation')).toBe(incarnation); expect(card.getAttribute('data-editor-revision')).toBe(revision);
  await act(async () => { server.socket.deliver(reply); await server.client.request({ method: 'server/info', params: {} }, 'server_info'); });
  expect(ticket.outcome).toBe('accepted'); expect(ticket.conversation?.conversation_id).toBe(f.target.conversation_id);
  expect(f.editor().value).toBe(kind === 'switch' ? f.text : ''); expect(server.client.target('A')).toBe(target); expect(mutations(before)).toEqual([]);
  expect(server.requests.filter(row => row.request.method === 'turn/start')).toHaveLength(1);
});

it('connection authority replacement leaves a transmitted input uncertain and ignores its old acknowledgement', async () => {
  const f = await fixture(), submit = vi.spyOn(ComposerDraft.prototype, 'submit');
  server.held.add('turn/start'); await act(async () => fireEvent.keyDown(f.editor(), { key: 'Enter' }));
  const sent = await server.waitFor('turn/start', 1), oldSocket = server.socket, reply = server.commit(sent), ticket = submit.mock.results[0].value!;
  await act(async () => { await server.client.disconnect(); server.authorityId = 'replacement-native-authority'; await server.connect(); await server.client.attach('A'); });
  await act(async () => fireEvent.click(f.ui.getByRole('button', { name: 'Open Session A' })));
  fireEvent.change(f.editor(), { target: { value: 'Replacement authority draft' } });
  const target = server.client.target('A'), before = server.requests.length;
  expect(ticket.outcome).toBe('uncertain'); expect(ticket.acceptance).toBeUndefined();
  await act(async () => { oldSocket.deliver(reply); await server.client.request({ method: 'server/info', params: {} }, 'server_info'); });
  expect(ticket.outcome).toBe('uncertain'); expect(ticket.acceptance).toBeUndefined();
  expect(f.editor().value).toBe('Replacement authority draft'); expect(server.client.target('A')).toBe(target);
  expect(mutations(before)).toEqual([]); expect(server.requests.filter(row => row.request.method === 'turn/start')).toHaveLength(1);
});

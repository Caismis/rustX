import { act, cleanup, fireEvent, render, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { waitFor as waitActor } from 'xstate';
import { AgentComposer } from '../src/app/agent/AgentComposer';
import { ComposerDraft } from '../src/app/composer/draft';
import { App } from '../src/app/App';
import { Server, snapshot } from './fixture';
import { routeFixture } from './shell-navigation-fixture';

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
    return { type: 'node', node: { id: request.params.node_id, ordinal: '1', conversation_id: request.params.node_id === origin.source_node ? source.conversation_id : child.conversation_id, origin: request.params.node_id === origin.source_node ? { type: 'new' } : origin } };
  });
  await server.attached('A');
  localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint: server.client.getSnapshot().endpoint, openViews: ['A'] }));
  const ui = await act(async () => render(<App client={server.client} workspaceHost={server.workspaceHost}/>));
  await waitFor(() => expect((ui.getByRole('button', { name: 'View source' }) as HTMLButtonElement).disabled).toBe(false));
  const editor = () => ui.getByRole('textbox', { name: 'Message' }) as HTMLTextAreaElement;
  fireEvent.change(editor(), { target: { value: 'Unsent exact draft\nsecond line' } });
  editor().setSelectionRange(7, 12, 'backward'); fireEvent.select(editor());
  const key = JSON.stringify([server.client.getSnapshot().endpoint, server.client.getSnapshot().authorityId, '0']);
  const intake = server.client.attachmentIntakes.lookup(key)!;
  return { ui, editor, intake, origin, target: server.client.target('A'), text: editor().value };
}
const mutations = (from: number) => server.requests.slice(from).filter(({ request }) => ['session/attach', 'session/detach', 'session/switchNode', 'turn/start', 'turn/steer', 'session/uploadPrepare', 'session/snapshot'].includes(request.method));
it('repeated read-only history visits retain text, caret, exact intake/files/receipts and original active controls', async () => {
  const f = await fixture();
  const files = [new File(['one'], 'one.txt'), new File(['two'], 'two.txt')];
  await act(async () => fireEvent.change(f.ui.getByLabelText('Attach files'), { target: { files } }));
  await waitFor(() => expect(f.intake.snapshot().map(file => file.status)).toEqual(['ready', 'ready']));
  const selected = f.intake.snapshot(), before = server.requests.length;
  for (let visit = 0; visit < 2; visit++) {
    await act(async () => fireEvent.click(f.ui.getByRole('button', { name: 'View source' })));
    await waitFor(() => expect(f.ui.getByText('Exact historical source')).toBeTruthy());
    expect(f.ui.queryByRole('textbox', { name: 'Message' })).toBeNull();
    expect(f.intake.snapshot()).toBe(selected);
    await act(async () => fireEvent.click(f.ui.getByRole('button', { name: 'Return to conversation' })));
    expect(f.editor().value).toBe(f.text); expect([f.editor().selectionStart, f.editor().selectionEnd, f.editor().selectionDirection]).toEqual([7, 12, 'backward']);
    expect(f.intake.snapshot()).toBe(selected); expect(f.intake.snapshot().map(file => file.file)).toEqual(files); expect(server.client.attachmentIntakes.size).toBe(1);
    expect(server.client.target('A')).toBe(f.target); expect(f.editor().disabled).toBe(false);
  }
  expect(mutations(before)).toEqual([]);
  fireEvent.change(f.editor(), { target: { value: f.text + ' still editable' } }); expect(f.editor().value).toBe(f.text + ' still editable');
  await act(async () => { f.editor().focus(); fireEvent.keyDown(f.editor(), { key: 'Escape' }); fireEvent.keyDown(f.editor(), { key: 'Escape' }); });
  expect(server.requests.slice(before).filter(row => row.request.method === 'turn/cancel').map(row => row.request.params)).toEqual([{ target: f.target }]);
});
it.each(['attachment', 'node', 'connection'] as const)('native %s replacement during history preserves local content but retires receipts without replay', async replacement => {
  const f = await fixture(), file = new File(['one'], 'one.txt');
  await act(async () => fireEvent.change(f.ui.getByLabelText('Attach files'), { target: { files: [file] } }));
  await waitFor(() => expect(f.intake.snapshot()[0].status).toBe('ready'));
  const selected = f.intake.snapshot()[0];
  await act(async () => fireEvent.click(f.ui.getByRole('button', { name: 'View source' })));
  await waitFor(() => expect(f.ui.getByText('Exact historical source')).toBeTruthy());
  // Explicit native replacement, independent of read-only navigation.
  await act(async () => {
    if (replacement === 'connection') { await server.client.disconnect(); await server.connect(); await server.client.attach('A'); }
    else if (replacement === 'node') {
      const next = snapshot('replacement'); server.nodeSnapshots.set('replacement-node', next);
      await server.update('A', { ...server.snapshots.get('A')!, attempt: null });
      await server.client.switchNode('A', 'replacement-node'); await server.client.attach('A', 'replacement-node');
    } else { await server.client.release('A'); await server.client.attach('A'); }
  });
  const target = server.client.target('A'), before = server.requests.length;
  const back = f.ui.queryByRole('button', { name: 'Return to conversation' });
  if (back) await act(async () => fireEvent.click(back));
  expect(f.editor().value).toBe(f.text); expect(target).not.toEqual(f.target);
  expect(f.intake.snapshot()[0]).toMatchObject({ id: selected.id, file, status: 'draft', receipt: undefined });
  expect(f.intake.snapshot()[0].operation).not.toBe(selected.operation); expect(mutations(before)).toEqual([]);
  expect(f.editor().disabled).toBe(false);
});
it('history retains draft identity while permanently retiring the old control callback on entry and Return', async () => {
  const f = await fixture(); cleanup();
  const nav = routeFixture(server.client, server.workspaceHost), old = nav.capture(), binding = nav.actor.getSnapshot().context.binding;
  nav.actor.send({ type: 'OPEN', destination: { kind: 'history', origin: f.origin } }); await waitActor(nav.actor, state => state.matches('idle'));
  const history = nav.actor.getSnapshot().context.route; if (history.kind !== 'history') throw new Error('not history');
  const historyControl = nav.capture(); expect(old()).toBe(false); expect(nav.actor.getSnapshot().context.binding).toBe(binding);
  nav.actor.send({ type: 'RETURN', route: history });
  expect(old()).toBe(false); expect(historyControl()).toBe(false); expect(nav.capture()()).toBe(true); expect(nav.actor.getSnapshot().context.binding).toBe(binding);
  nav.stop();
});
it('an already admitted upload settles into its retained draft during history without phantom uncertainty or replay', async () => {
  const f = await fixture(); server.held.add('session/uploadPrepare');
  await act(async () => fireEvent.change(f.ui.getByLabelText('Attach files'), { target: { files: [new File(['one'], 'one.txt')] } }));
  const upload = await server.waitFor('session/uploadPrepare', 1); expect(f.intake.snapshot()[0].status).toBe('uploading');
  await act(async () => fireEvent.click(f.ui.getByRole('button', { name: 'View source' })));
  await waitFor(() => expect(f.ui.getByText('Exact historical source')).toBeTruthy());
  await act(async () => server.reply(upload));
  await waitFor(() => expect(f.intake.snapshot()[0].status).toBe('ready'));
  await act(async () => fireEvent.click(f.ui.getByRole('button', { name: 'Return to conversation' })));
  expect(f.editor().value).toBe(f.text); expect(f.ui.queryByText('Uncertain')).toBeNull();
  expect(server.requests.filter(row => row.request.method === 'session/uploadPrepare')).toHaveLength(1);
  expect(server.client.target('A')).toBe(f.target);
});
it('known native acceptance settles the submitted revision after Return without reauthorizing old controls', async () => {
  const f = await fixture(); server.held.add('turn/start');
  await act(async () => fireEvent.keyDown(f.editor(), { key: 'Enter' }));
  const sent = await server.waitFor('turn/start', 1);
  await act(async () => fireEvent.click(f.ui.getByRole('button', { name: 'View source' })));
  await waitFor(() => expect(f.ui.getByText('Exact historical source')).toBeTruthy());
  await act(async () => fireEvent.click(f.ui.getByRole('button', { name: 'Return to conversation' })));
  expect(f.editor().value).toBe(f.text);
  await act(async () => server.reply(sent));
  expect(f.editor().value).toBe(''); expect(f.editor().readOnly).toBe(false); expect(server.client.target('A')).toBe(f.target);
  expect(server.requests.filter(row => row.request.method === 'turn/start')).toHaveLength(1);
});
it('the actual View Source click and immediate scroll retire history in one stack before passive effects', async () => {
  const f = await fixture(), port = f.ui.container.querySelector<HTMLElement>('[data-conversation-scroll]')!;
  Object.defineProperties(port, { scrollHeight: { value: 2200 }, clientHeight: { value: 200 } });
  const before = server.requests.length;
  act(() => {
    fireEvent.click(f.ui.getByRole('button', { name: 'View source' }));
    port.scrollTop = 400; fireEvent.scroll(port);
  });
  await act(async () => { await server.client.request({ method: 'server/info', params: {} }, 'server_info'); });
  expect(f.ui.container.querySelector('[data-shell-route]')?.getAttribute('data-shell-route')).toBe('session');
  expect(f.ui.queryByText('Opening history…')).toBeNull(); expect(f.ui.queryByRole('alert')).toBeNull(); expect(f.ui.queryByText('Exact historical source')).toBeNull();
  expect(server.requests.slice(before).filter(row => row.request.method === 'session/history')).toEqual([]);
  expect(server.client.target('A')).toBe(f.target); expect(f.editor().value).toBe(f.text); expect(mutations(before)).toEqual([]);
});
it.each([[true, true], [false, true], [true, false]] as const)('user scrolling retires a held history successful=%s eventDelivered=%s response without publication, error or landing', async (successful, eventDelivered) => {
  const f = await fixture(); server.held.add('session/history');
  const count = server.requests.filter(row => row.request.method === 'session/history').length;
  await act(async () => fireEvent.click(f.ui.getByRole('button', { name: 'View source' })));
  const held = await server.waitFor('session/history', count + 1);
  const port = f.ui.container.querySelector<HTMLElement>('[data-conversation-scroll]')!;
  Object.defineProperties(port, { scrollHeight: { value: 2200 }, clientHeight: { value: 200 } });
  act(() => { port.scrollTop = 400; if (eventDelivered) fireEvent.scroll(port); });
  await act(async () => {
    if (successful) server.reply(held);
    else server.socket.deliver({ jsonrpc: '2.0', id: held.id, error: { code: -32000, message: 'Obsolete history error', data: { kind: 'operation_failed' } } });
    await server.client.request({ method: 'server/info', params: {} }, 'server_info');
  });
  expect(f.ui.container.querySelector('[data-shell-route]')?.getAttribute('data-shell-route')).toBe('session');
  expect(f.ui.queryByRole('alert')).toBeNull(); expect(f.ui.queryByText('Exact historical source')).toBeNull(); expect(port.scrollTop).toBe(400);
  expect(server.client.target('A')).toBe(f.target); expect(f.editor().value).toBe(f.text);
});

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

it.each([['turn/start', false], ['turn/start', true], ['turn/steer', false], ['turn/steer', true]] as const)('native %s acceptance before Return=%s settles once without replay', async (method, beforeReturn) => {
  const f = await fixture(); server.held.add(method);
  await act(async () => fireEvent.keyDown(f.editor(), { key: 'Enter', ctrlKey: method === 'turn/steer' }));
  const sent = await server.waitFor(method, 1);
  await act(async () => fireEvent.click(f.ui.getByRole('button', { name: 'View source' })));
  await waitFor(() => expect(f.ui.getByText('Exact historical source')).toBeTruthy());
  if (beforeReturn) await act(async () => server.reply(sent));
  await act(async () => fireEvent.click(f.ui.getByRole('button', { name: 'Return to conversation' })));
  if (!beforeReturn) await act(async () => server.reply(sent));
  expect(f.editor().value).toBe(''); expect(server.client.target('A')).toBe(f.target);
  expect(server.requests.filter(row => row.request.method === method)).toHaveLength(1);
  expect(server.requests.filter(row => ['session/attach', 'session/detach', 'session/switchNode'].includes(row.request.method))).toHaveLength(1);
});
it.each([false, true])('newer authored revision equalText=%s survives an older native acknowledgement', async equalText => {
  const f = await fixture(); server.held.add('turn/start');
  await act(async () => fireEvent.keyDown(f.editor(), { key: 'Enter' }));
  const sent = await server.waitFor('turn/start', 1);
  await act(async () => fireEvent.click(f.ui.getByRole('button', { name: 'View source' })));
  await waitFor(() => expect(f.ui.getByText('Exact historical source')).toBeTruthy());
  await act(async () => fireEvent.click(f.ui.getByRole('button', { name: 'Return to conversation' })));
  fireEvent.change(f.editor(), { target: { value: 'newly authored intermediate' } });
  const text = equalText ? f.text : 'Later revision R2';
  fireEvent.change(f.editor(), { target: { value: text } });
  await act(async () => server.reply(sent));
  expect(f.editor().value).toBe(text); expect(server.requests.filter(row => row.request.method === 'turn/start')).toHaveLength(1);
});
it('accepted F1 consumes only its submitted selection while a newer F2 survives', async () => {
  const f = await fixture(), first = new File(['one'], 'one.txt'), second = new File(['two'], 'two.txt');
  await act(async () => fireEvent.change(f.ui.getByLabelText('Attach files'), { target: { files: [first] } }));
  await waitFor(() => expect(f.intake.snapshot()[0].status).toBe('ready'));
  server.held.add('turn/start'); await act(async () => fireEvent.keyDown(f.editor(), { key: 'Enter' }));
  const sent = await server.waitFor('turn/start', 1);
  await act(async () => fireEvent.click(f.ui.getByRole('button', { name: 'View source' })));
  await waitFor(() => expect(f.ui.getByText('Exact historical source')).toBeTruthy());
  await act(async () => fireEvent.click(f.ui.getByRole('button', { name: 'Return to conversation' })));
  await act(async () => fireEvent.change(f.ui.getByLabelText('Attach files'), { target: { files: [second] } }));
  await waitFor(() => expect(f.intake.snapshot()[1].status).toBe('ready'));
  await act(async () => server.reply(sent));
  expect(f.intake.snapshot().map(value => value.file)).toEqual([second]); expect(f.editor().value).toBe(f.text);
  expect(server.requests.filter(row => row.request.method === 'session/uploadPrepare')).toHaveLength(2);
  expect(server.requests.filter(row => row.request.method === 'turn/start')).toHaveLength(1);
});
it.each(['failed', 'uncertain'] as const)('Discard of an actual %s First Submission establishes a stable replacement across repeated history', async outcome => {
  const f = await fixture();
  // A retired receipt becomes a local draft file and enters FirstSubmissions.
  const file = new File(['one'], 'one.txt');
  await act(async () => { f.intake.add([{ file }], server.client.getSnapshot().capabilities!.upload_policy); });
  server.handlers.set('session/uploadStatus', () => ({ type: 'upload_status', outcome: { state: outcome === 'failed' ? 'failed' : 'unresolved' } }));
  await act(async () => fireEvent.keyDown(f.editor(), { key: 'Enter' }));
  await waitFor(() => expect(f.ui.getByRole('button', { name: /Discard/i })).toBeTruthy());
  const old = f.ui.container.querySelector('[data-editor-incarnation]')!.getAttribute('data-editor-incarnation');
  await act(async () => fireEvent.click(f.ui.getByRole('button', { name: /Discard/i })));
  server.handlers.delete('session/uploadStatus');
  fireEvent.change(f.editor(), { target: { value: 'Replacement after Discard' } });
  await act(async () => fireEvent.change(f.ui.getByLabelText('Attach files'), { target: { files: [new File(['new'], 'replacement.txt')] } }));
  await waitFor(() => expect(f.intake.snapshot()[0].status).toBe('ready'));
  const incarnation = f.ui.container.querySelector('[data-editor-incarnation]')!.getAttribute('data-editor-incarnation');
  expect(incarnation).not.toBe(old);
  f.editor().setSelectionRange(3, 8, 'backward'); fireEvent.select(f.editor());
  const before = server.requests.length;
  for (let visit = 0; visit < 2; visit++) {
    await act(async () => fireEvent.click(f.ui.getByRole('button', { name: 'View source' })));
    await waitFor(() => expect(f.ui.getByText('Exact historical source')).toBeTruthy());
    await act(async () => fireEvent.click(f.ui.getByRole('button', { name: 'Return to conversation' })));
    expect(f.editor().value).toBe('Replacement after Discard');
    expect(f.ui.container.querySelector('[data-editor-incarnation]')!.getAttribute('data-editor-incarnation')).toBe(incarnation);
    expect([f.editor().selectionStart, f.editor().selectionEnd]).toEqual([3, 8]);
  }
  expect(f.intake.snapshot().map(value => value.name)).toEqual(['replacement.txt']);
  expect(mutations(before)).toEqual([]); expect(server.client.target('A')).toBe(f.target);
});

it('an uncertain dispatched input stays uncertain across history and cannot silently resubmit', async () => {
  const f = await fixture(); server.held.add('turn/start');
  await act(async () => fireEvent.keyDown(f.editor(), { key: 'Enter' })); const sent = await server.waitFor('turn/start', 1);
  await act(async () => server.socket.deliver({ jsonrpc: '2.0', id: sent.id, error: { code: -32000, message: 'Committed input durability unknown', data: { kind: 'committed_durability_uncertain' } } }));
  expect(f.ui.container.querySelector('[data-submission-outcome]')?.getAttribute('data-submission-outcome')).toBe('uncertain');
  await act(async () => fireEvent.click(f.ui.getByRole('button', { name: 'View source' })));
  await waitFor(() => expect(f.ui.getByText('Exact historical source')).toBeTruthy());
  await act(async () => fireEvent.click(f.ui.getByRole('button', { name: 'Return to conversation' })));
  expect(f.editor().value).toBe(f.text); expect(f.ui.container.querySelector('[data-submission-outcome]')?.getAttribute('data-submission-outcome')).toBe('uncertain');
  await act(async () => fireEvent.keyDown(f.editor(), { key: 'Enter' }));
  expect(server.requests.filter(row => row.request.method === 'turn/start')).toHaveLength(1); expect(server.client.target('A')).toBe(f.target);
});
it('late native acknowledgement from A cannot change a deliberately replaced new Conversation editor', async () => {
  const f = await fixture(); server.held.add('turn/start');
  await act(async () => fireEvent.keyDown(f.editor(), { key: 'Enter' })); const sent = await server.waitFor('turn/start', 1);
  await act(async () => fireEvent.click(f.ui.getAllByRole('button', { name: 'New Conversation' })[0]));
  fireEvent.change(f.editor(), { target: { value: 'Independent new Conversation' } });
  await act(async () => server.reply(sent));
  expect(f.editor().value).toBe('Independent new Conversation'); expect(server.requests.filter(row => row.request.method === 'turn/start')).toHaveLength(1);
});

it('First Submission native acceptance consumes its stable revision while its Composer is unmounted', async () => {
  const f = await fixture(), file = new File(['first'], 'first.txt');
  const native = server.snapshots.get('A')!, message = { role: 'assistant' as const, id: 'existing-message', content: [{ type: 'text' as const, text: 'Existing canonical conversation' }] };
  await act(async () => server.update('A', { ...native, messages: [message], transcript: { ...native.transcript, entries: [{ cursor: '1', item: { type: 'message', message } }] } }));
  await act(async () => f.intake.add([{ file }], server.client.getSnapshot().capabilities!.upload_policy));
  server.held.add('turn/start'); await act(async () => fireEvent.keyDown(f.editor(), { key: 'Enter' }));
  const sent = await server.waitFor('turn/start', 1);
  expect(server.client.firstSubmissions.draft('0')?.phase).toBe('admitting');
  await act(async () => fireEvent.click(f.ui.getByRole('button', { name: 'View source' })));
  await waitFor(() => expect(f.ui.getByText('Exact historical source')).toBeTruthy());
  await act(async () => server.reply(sent));
  expect(server.client.firstSubmissions.draft('0')?.phase).toBe('admitted');
  await act(async () => fireEvent.click(f.ui.getByRole('button', { name: 'Return to conversation' })));
  expect(f.editor().value).toBe(''); expect(f.intake.snapshot()).toEqual([]); expect(f.editor().readOnly).toBe(false);
  expect(server.requests.filter(row => row.request.method === 'session/uploadPrepare')).toHaveLength(1);
  expect(server.requests.filter(row => row.request.method === 'turn/start')).toHaveLength(1); expect(server.client.target('A')).toBe(f.target);
});

it('a late native reply after terminal uncertain First Submission Discard cannot alter its replacement editor', async () => {
  const f = await fixture(), file = new File(['first'], 'first.txt');
  await act(async () => f.intake.add([{ file }], server.client.getSnapshot().capabilities!.upload_policy));
  server.held.add('turn/start'); await act(async () => fireEvent.keyDown(f.editor(), { key: 'Enter' }));
  const sent = await server.waitFor('turn/start', 1);
  expect(server.client.firstSubmissions.draft('0')?.phase).toBe('admitting');
  // A settled uncertain response permits Discard; the in-flight phase did not.
  expect(f.ui.queryByRole('button', { name: /Discard/i })).toBeNull();
  await act(async () => server.socket.deliver({ jsonrpc: '2.0', id: sent.id, error: { code: -32000, message: 'Committed durability unknown', data: { kind: 'committed_durability_uncertain' } } }));
  expect(server.client.firstSubmissions.draft('0')?.phase).toBe('uncertain');
  await act(async () => fireEvent.click(f.ui.getByRole('button', { name: /Discard/i })));
  const discarded = server.client.firstSubmissions.draft('0');
  fireEvent.change(f.editor(), { target: { value: 'Replacement E2' } });
  const incarnation = f.ui.container.querySelector('[data-editor-incarnation]')!.getAttribute('data-editor-incarnation');
  await act(async () => { server.reply(sent); await server.client.request({ method: 'server/info', params: {} }, 'server_info'); });
  expect(server.client.firstSubmissions.draft('0')).toBe(discarded);
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
  expect(mutations(before).filter(row => row.request.method !== 'session/snapshot')).toEqual([]); expect(server.client.target('A')).toBe(f.target);
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

it('C1 acceptance during history cannot consume text retained for an authoritatively selected C2', async () => {
  const f = await fixture(), submit = vi.spyOn(ComposerDraft.prototype, 'submit');
  server.held.add('turn/start'); await act(async () => fireEvent.keyDown(f.editor(), { key: 'Enter' }));
  const sent = await server.waitFor('turn/start', 1), reply = server.commit(sent), ticket = submit.mock.results[0].value!;
  await act(async () => fireEvent.click(f.ui.getByRole('button', { name: 'View source' })));
  await waitFor(() => expect(f.ui.getByText('Exact historical source')).toBeTruthy());
  await act(async () => { server.nodeSnapshots.set('C2-node', snapshot('C2')); await server.client.switchNode('A', 'C2-node'); await server.client.attach('A', 'C2-node'); });
  const target = server.client.target('A'), before = server.requests.length;
  await act(async () => { server.socket.deliver(reply); await server.client.request({ method: 'server/info', params: {} }, 'server_info'); });
  expect(ticket.outcome).toBe('accepted'); expect(ticket.conversation?.conversation_id).toBe(f.target.conversation_id);
  await act(async () => fireEvent.click(f.ui.getByRole('button', { name: 'Return to conversation' })));
  expect(f.editor().value).toBe(f.text); expect(server.client.target('A')).toBe(target); expect(mutations(before)).toEqual([]);
  expect(server.requests.filter(row => row.request.method === 'turn/start')).toHaveLength(1);
});

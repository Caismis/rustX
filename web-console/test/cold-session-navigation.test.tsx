import { act, cleanup, fireEvent, render, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { App } from '../src/app/App';
import { ComposerDraft, type ComposerSubmission } from '../src/app/composer/draft';
import { firstSubmissions } from '../src/app/new-conversation/first-submit';
import { Server, snapshot } from './fixture';

let server: Server;
afterEach(() => { cleanup(); server?.client.disconnect(); localStorage.clear(); vi.restoreAllMocks(); });

async function fixture() {
  const activation = vi.spyOn(ComposerDraft.prototype, 'activate');
  server = new Server();
  server.handlers.set('workflow/list', request => {
    if (request.method !== 'workflow/list') throw new Error('Expected Workflow library read');
    return { type: 'workflow_library', library: { target: request.params.target, directory: '/workflows', absent_revision: 'empty', files: [], diagnostic: null } };
  });
  const origin = { type: 'fork' as const, source_session: 'B', source_node: 'source-node', source_message: 'source-message', source_surface_revision: '1', side: 'after' as const };
  const history = snapshot('B');
  history.transcript.entries = [{ cursor: '12', item: { type: 'message', message: { role: 'assistant', id: 'source-message', content: [{ type: 'text', text: 'Exact historical source' }] } } }];
  server.nodeSnapshots.set('source-node', history);
  server.handlers.set('session/node', request => {
    if (request.method !== 'session/node') throw new Error('Expected Node read');
    return { type: 'node', node: { id: request.params.node_id, ordinal: '1', conversation_id: request.params.node_id === 'source-node' ? history.conversation_id : server.snapshots.get(request.params.session_id)!.conversation_id, origin: request.params.session_id === 'A' ? origin : { type: 'new' } } };
  });
  const source = snapshot('A'); source.transcript = { inherited_through: '0', entries: [] }; source.attempt = { attempt_id: 'running-A', phase: { type: 'running' }, turn: 1 };
  server.snapshots.set('A', source);
  await server.attached('A');
  localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint: server.client.getSnapshot().endpoint, openViews: ['A'] }));
  const ui = await act(async () => render(<App client={server.client} workspaceHost={server.workspaceHost}/>));
  const editor = () => ui.getByRole('textbox', { name: 'Message' }) as HTMLTextAreaElement;
  await waitFor(() => expect(editor().disabled).toBe(false));
  const sourceOwner = activation.mock.contexts.at(-1) as ComposerDraft;
  const sourceKey = JSON.stringify([server.client.getSnapshot().endpoint, server.client.getSnapshot().authorityId, '0']);
  const sourceIntake = server.client.attachmentIntakes.lookup(sourceKey)!;
  return { ui, editor, sourceOwner, sourceKey, sourceIntake, target: server.client.target('A'), attempt: server.client.getSnapshot().views.A.snapshot?.attempt };
}

for (const origin of ['session', 'workflows'] as const) for (const settlement of ['upload', 'A-before-B', 'B-before-A'] as const) {
  it(`cold B accepts its own explicit Send from ${origin}; A ${settlement} remains independently owned`, async () => {
    const f = await fixture();
    fireEvent.change(f.editor(), { target: { value: 'Only Session A owns this draft' } });
    if (settlement === 'upload') server.held.add('session/uploadPrepare');
    await act(async () => fireEvent.change(f.ui.getByLabelText('Attach files'), { target: { files: [new File(['A'], 'A-owned.txt')] } }));
    const sourceUpload = await server.waitFor('session/uploadPrepare', 1);
    let sourceInput: Awaited<ReturnType<Server['waitFor']>> | undefined, ticket: ComposerSubmission | undefined;
    if (settlement !== 'upload') {
      await waitFor(() => expect(f.sourceIntake.snapshot()[0]?.status).toBe('ready'));
      server.held.add('turn/start');
      await act(async () => fireEvent.keyDown(f.editor(), { key: 'Enter' }));
      sourceInput = await server.waitFor('turn/start', 1);
      ticket = f.sourceOwner.snapshot()!.submission;
      expect(ticket?.outcome).toBe('pending');
    }
    const consumeA = vi.spyOn(f.sourceIntake, 'consume');
    const sourceFiles = f.sourceIntake.snapshot();
    if (origin === 'workflows') await act(async () => fireEvent.click(f.ui.getByRole('button', { name: 'Workflows' })));
    server.held.add('session/attach');
    await act(async () => fireEvent.click(f.ui.getByRole('button', { name: 'Open Session B' })));
    const attached = await server.waitFor('session/attach', 2);
    expect(f.ui.container.querySelector('[data-shell-route]')?.getAttribute('data-shell-route')).toBe('session-draft');
    expect(f.editor().disabled).toBe(false); expect(f.editor().value).toBe('');
    expect(f.ui.queryByText('A-owned.txt')).toBeNull();
    expect(server.client.attachmentIntakes.lookup(f.sourceKey)).toBe(f.sourceIntake);
    expect(f.sourceIntake.snapshot()).toBe(sourceFiles);
    expect(server.client.target('A')).toBe(f.target);
    expect(server.client.getSnapshot().views.A.snapshot?.attempt).toBe(f.attempt);
    expect(server.requests.filter(row => row.request.method === 'session/submissionIntent')).toHaveLength(0);
    expect(server.requests.filter(row => ['session/detach', 'turn/cancel', 'session/switchNode'].includes(row.request.method))).toEqual([]);

    fireEvent.change(f.editor(), { target: { value: 'Only Session B receives this new input' } });
    await act(async () => fireEvent.change(f.ui.getByLabelText('Attach files'), { target: { files: [new File(['B'], 'B-owned.txt')] } }));
    expect(server.requests.filter(row => row.request.method === 'session/uploadPrepare')).toHaveLength(1);
    await act(async () => fireEvent.click(f.ui.getByRole('button', { name: 'Send' })));
    const intent = await server.waitFor('session/submissionIntent', 1);
    expect(intent.params).toEqual({ session_id: 'B', node_id: 'node-B' });
    expect(server.requests.filter(row => row.request.method === 'turn/start')).toHaveLength(sourceInput ? 1 : 0);
    expect(server.requests.filter(row => row.request.method === 'session/uploadPrepare')).toHaveLength(1);
    expect(f.sourceIntake.snapshot()).toBe(sourceFiles);
    expect(f.editor().readOnly).toBe(true); // The captured first submission owns this editor revision.

    if (settlement !== 'B-before-A') {
      await act(async () => server.reply(sourceInput ?? sourceUpload));
      if (settlement === 'upload') server.held.delete('session/uploadPrepare');
      if (ticket) expect(ticket.outcome).toBe('accepted');
      else await waitFor(() => expect(f.sourceIntake.snapshot()[0]?.status).toBe('ready'));
      expect(f.editor().value).toBe('');
      expect(firstSubmissions(server.client).session('B')?.phase ?? firstSubmissions(server.client).draft('1')?.phase).toBe('attaching');
      expect(consumeA).not.toHaveBeenCalled();
    }
    await act(async () => server.reply(attached));
    const sent = await server.waitFor('turn/start', sourceInput ? 2 : 1);
    if (sent.method !== 'turn/start') throw new Error('Expected B input');
    expect(sent.params.target.session_id).toBe('B');
    expect(sent.params.content.filter(block => block.type === 'text')).toEqual([{ type: 'text', text: 'Only Session B receives this new input' }]);
    const uploaded = server.requests.filter(row => row.request.method === 'session/uploadPrepare').map(row => row.request.params);
    expect(uploaded).toMatchObject([{ target: f.target, files: [{ name: 'A-owned.txt' }] }, { target: { session_id: 'B' }, files: [{ name: 'B-owned.txt' }] }]);
    if (ticket) {
      await act(async () => server.reply(sent));
      if (settlement === 'B-before-A') {
        await waitFor(() => expect(firstSubmissions(server.client).session('B')?.phase).toBe('admitted'));
        fireEvent.change(f.editor(), { target: { value: 'Later B typing is independent of every ACK' } });
        expect(ticket.outcome).toBe('pending');
        await act(async () => server.reply(sourceInput!));
        expect(ticket.outcome).toBe('accepted');
      }
    }
    await waitFor(() => expect(firstSubmissions(server.client).session('B')?.phase).toBe('admitted'));
    if (settlement !== 'B-before-A') fireEvent.change(f.editor(), { target: { value: 'Later B typing is independent of every ACK' } });
    expect(f.editor().value).toBe('Later B typing is independent of every ACK');
    expect(consumeA).not.toHaveBeenCalled();
    expect(server.client.target('A')).toBe(f.target);
    expect(server.client.getSnapshot().views.A.snapshot?.attempt).toBe(f.attempt);
    expect(server.requests.filter(row => row.request.method === 'session/attach')).toHaveLength(2);
  });
}

it.each(['failure', 'cancel'] as const)('cold navigation %s retains B local files without retiring or transferring A intake', async outcome => {
  const f = await fixture();
  await act(async () => fireEvent.change(f.ui.getByLabelText('Attach files'), { target: { files: [new File(['A'], 'A-owned.txt')] } }));
  await waitFor(() => expect(f.sourceIntake.snapshot()[0]?.status).toBe('ready'));
  const sourceFiles = f.sourceIntake.snapshot();
  server.held.add('session/attach');
  await act(async () => fireEvent.click(f.ui.getByRole('button', { name: 'Open Session B' })));
  const attached = await server.waitFor('session/attach', 2);
  fireEvent.change(f.editor(), { target: { value: 'B local draft' } });
  await act(async () => fireEvent.change(f.ui.getByLabelText('Attach files'), { target: { files: [new File(['B'], 'B-local.txt')] } }));
  if (outcome === 'failure') await act(async () => server.socket.deliver({ jsonrpc: '2.0', id: attached.id, error: { code: -32000, message: 'B preparation failed', data: { kind: 'operation_failed' } } }));
  else await act(async () => fireEvent.click(f.ui.getByRole('button', { name: 'Settings' })));
  if (outcome === 'cancel') await act(async () => fireEvent.click(f.ui.getByRole('button', { name: 'Close Settings' })));
  expect(server.client.attachmentIntakes.lookup(f.sourceKey)).toBe(f.sourceIntake);
  expect(f.sourceIntake.snapshot()).toBe(sourceFiles);
  expect(f.editor().value).toBe('B local draft');
  expect(f.ui.getByText('B-local.txt')).toBeTruthy();
  expect(server.client.target('A')).toBe(f.target);
  expect(server.requests.filter(row => ['session/submissionIntent', 'turn/start', 'turn/steer'].includes(row.request.method))).toEqual([]);
  // Returning to an already attached A is an explicit normal route replacement.
  // Local B files neither enter A nor remain in the selected intake registry.
  await act(async () => fireEvent.click(f.ui.getByRole('button', { name: 'Open Session A' })));
  expect(f.editor().value).toBe(''); expect(f.ui.queryByText('B-local.txt')).toBeNull();
  expect(server.client.attachmentIntakes.size).toBe(1);
  if (outcome === 'cancel') {
    fireEvent.change(f.editor(), { target: { value: 'A explicit replacement editor' } });
    await act(async () => { server.reply(attached); await server.client.waitForAttachment('B', () => true); });
    expect(f.editor().value).toBe('A explicit replacement editor');
    expect(server.client.target('A')).toBe(f.target);
    expect(f.ui.queryByText('B-local.txt')).toBeNull();
    expect(server.requests.filter(row => ['session/submissionIntent', 'turn/start', 'turn/steer'].includes(row.request.method))).toEqual([]);
  }
});

it('cold B still requires fresh Host authorization for its explicit submission intent', async () => {
  const f = await fixture();
  server.held.add('session/attach');
  await act(async () => fireEvent.click(f.ui.getByRole('button', { name: 'Open Session B' })));
  await server.waitFor('session/attach', 2);
  const classify = vi.spyOn(server.workspaceHost, 'classifyLocations').mockResolvedValue([{ authorized: false, reason: 'denied' }]);
  fireEvent.change(f.editor(), { target: { value: 'Must remain an unsent B draft' } });
  await act(async () => fireEvent.click(f.ui.getByRole('button', { name: 'Send' })));
  await waitFor(() => expect(firstSubmissions(server.client).draft('1')?.phase).toBe('failed'));
  expect(classify).toHaveBeenCalled();
  expect(server.requests.filter(row => ['session/submissionIntent', 'turn/start', 'turn/steer', 'session/uploadPrepare'].includes(row.request.method))).toEqual([]);
  expect(server.client.target('A')).toBe(f.target);
  expect(f.editor().value).toBe('Must remain an unsent B draft');
});

it.each(['immediate reconnect', 'settled disconnect and reopened Settings'] as const)('same-authority reconnect renews cold B publication after held metadata without sending input (%s)', async recovery => {
  const f = await fixture();
  fireEvent.change(f.editor(), { target: { value: 'A keeps its own unsent draft' } });
  await act(async () => fireEvent.change(f.ui.getByLabelText('Attach files'), { target: { files: [new File(['A'], 'A-owned.txt')] } }));
  await waitFor(() => expect(f.sourceIntake.snapshot()[0]?.status).toBe('ready'));
  const sourceFiles = f.sourceIntake.snapshot();
  const reads = server.requests.filter(row => row.request.method === 'session/read').length;
  server.held.add('session/read');
  await act(async () => fireEvent.click(f.ui.getByRole('button', { name: 'Open Session B' })));
  const metadata = await server.waitFor('session/read', reads + 1);
  expect(metadata.params).toEqual({ session_id: 'B' });
  expect(server.requests.filter(row => row.request.method === 'session/attach')).toHaveLength(1);
  expect(f.ui.container.querySelector('[data-shell-route]')?.getAttribute('data-shell-route')).toBe('session-draft');
  expect(f.editor().disabled).toBe(false);
  fireEvent.change(f.editor(), { target: { value: 'B draft survives its connection generation' } });
  await act(async () => fireEvent.change(f.ui.getByLabelText('Attach files'), { target: { files: [new File(['B'], 'B-local.txt')] } }));
  expect(server.client.attachmentIntakes.lookup(f.sourceKey)).toBe(f.sourceIntake);
  expect(f.sourceIntake.snapshot()).toBe(sourceFiles);
  const draft = f.sourceOwner.snapshot()!, intake = server.client.attachmentIntakes.lookup(draft.key)!;
  const oldSocket = server.socket, lateMetadata = server.commit(metadata, oldSocket);
  const generation = server.client.getSnapshot().generation, authority = server.client.getSnapshot().authorityRevision;
  if (recovery === 'immediate reconnect') await act(async () => { oldSocket.close(); server.held.delete('session/read'); await server.connect(); });
  else {
    await act(async () => oldSocket.close());
    await act(async () => fireEvent.click(f.ui.getByRole('button', { name: 'Settings' })));
    await act(async () => fireEvent.click(f.ui.getByRole('button', { name: 'Close Settings' })));
    server.held.delete('session/read');
    await act(async () => server.connect());
  }
  await waitFor(() => {
    for (const id of ['A', 'B']) expect(server.client.isAttachmentControlCurrent(id, server.client.getSnapshot().views[id]?.attachmentObservation)).toBe(true);
  });
  expect(server.client.getSnapshot().generation).toBeGreaterThan(generation);
  expect(server.client.getSnapshot().authorityRevision).toBe(authority);
  expect(f.sourceOwner.snapshot()?.incarnation).toBe(draft.incarnation);
  expect(server.client.attachmentIntakes.lookup(draft.key)).toBe(intake);
  expect(f.editor().value).toBe('B draft survives its connection generation');
  expect(f.ui.queryByText('A-owned.txt')).toBeNull();
  expect(f.ui.getByText('B-local.txt')).toBeTruthy();
  expect(server.requests.filter(row => ['session/submissionIntent', 'turn/start', 'turn/steer', 'turn/cancel', 'session/detach'].includes(row.request.method))).toEqual([]);
  await waitFor(() => expect(f.editor().disabled).toBe(false));
  expect(f.ui.container.querySelector('[data-shell-route]')?.getAttribute('data-shell-route')).toBe('session');
  await act(async () => oldSocket.deliver(lateMetadata));
  expect(f.editor().disabled).toBe(false);
  expect(f.editor().value).toBe('B draft survives its connection generation');
  expect(server.requests.filter(row => row.request.method === 'session/attach').map(row => row.request.params)).toEqual([
    { session_id: 'A', node_id: 'node-A' }, { session_id: 'A', node_id: 'node-A' }, { session_id: 'B', node_id: 'node-B' },
  ]);
});

it.each(['lost Attach ACK', 'known Attach refusal', 'released before Attach'] as const)('same-authority navigation observes lifecycle recovery without reopening %s', async outcome => {
  const f = await fixture();
  const method = outcome === 'released before Attach' ? 'session/read' : 'session/attach';
  const before = server.requests.filter(row => row.request.method === method).length;
  server.held.add(method);
  await act(async () => fireEvent.click(f.ui.getByRole('button', { name: 'Open Session B' })));
  const pending = await server.waitFor(method, before + 1);
  fireEvent.change(f.editor(), { target: { value: 'B remains an unsent local draft' } });
  await act(async () => fireEvent.change(f.ui.getByLabelText('Attach files'), { target: { files: [new File(['B'], 'B-uncommitted.txt')] } }));
  const oldSocket = server.socket;
  const late = outcome === 'lost Attach ACK' ? server.commit(pending, oldSocket) : undefined;
  if (outcome === 'known Attach refusal') {
    await act(async () => oldSocket.deliver({ jsonrpc: '2.0', id: pending.id, error: { code: -32000, message: 'B Attach was refused', data: { kind: 'operation_failed' } } }));
    await waitFor(() => expect(server.client.getSnapshot().views.B.attachment).toBe('error'));
  }
  let released: Promise<void> | undefined;
  await act(async () => {
    if (outcome === 'released before Attach') released = server.client.release('B');
    oldSocket.close();
    server.held.delete(method);
    await server.connect();
    await released;
  });
  await waitFor(() => expect(server.client.isAttachmentControlCurrent('A', server.client.getSnapshot().views.A.attachmentObservation)).toBe(true));
  const expectUnopened = () => {
    expect(server.client.isAttachmentControlCurrent('B', server.client.getSnapshot().views.B.attachmentObservation)).toBe(false);
    expect(server.client.getSnapshot().views.B.target).toBeUndefined();
    expect(server.claims().some(target => target.session_id === 'B')).toBe(false);
    expect(server.requests.filter(row => row.request.method === 'session/attach' && row.request.params.session_id === 'B')).toHaveLength(outcome === 'released before Attach' ? 0 : 1);
    expect(f.ui.container.querySelector('[data-shell-route]')?.getAttribute('data-shell-route')).toBe('session-draft');
    expect(f.editor().value).toBe('B remains an unsent local draft');
    expect(f.ui.getByText('B-uncommitted.txt')).toBeTruthy();
    expect(server.requests.filter(row => ['session/submissionIntent', 'turn/start', 'turn/steer', 'turn/cancel', 'session/detach'].includes(row.request.method))).toEqual([]);
  };
  expectUnopened();
  if (late) { await act(async () => oldSocket.deliver(late)); expectUnopened(); }
  // A deliberate later selection still owns the existing explicit Open path;
  // automatic publication must neither invoke it nor disable it globally.
  await act(async () => fireEvent.click(f.ui.getByRole('button', { name: 'Open Session B' })));
  await waitFor(() => expect(server.client.isAttachmentControlCurrent('B', server.client.getSnapshot().views.B.attachmentObservation)).toBe(true));
  await waitFor(() => expect(f.ui.container.querySelector('[data-shell-route]')?.getAttribute('data-shell-route')).toBe('session'));
  expect(f.editor().value).toBe('B remains an unsent local draft');
  expect(server.requests.filter(row => row.request.method === 'session/attach' && row.request.params.session_id === 'B')).toHaveLength(outcome === 'released before Attach' ? 1 : 2);
  expect(server.requests.filter(row => ['session/submissionIntent', 'turn/start', 'turn/steer'].includes(row.request.method))).toEqual([]);
});

it('admitted cold recovery renews B input interest before its Attach ACK without manufacturing another Open', async () => {
  const f = await fixture();
  const reads = server.requests.filter(row => row.request.method === 'session/read').length;
  server.held.add('session/read');
  await act(async () => fireEvent.click(f.ui.getByRole('button', { name: 'Open Session B' })));
  const metadata = await server.waitFor('session/read', reads + 1);
  fireEvent.change(f.editor(), { target: { value: 'Explicit B input while recovery remains cold' } });
  await act(async () => fireEvent.change(f.ui.getByLabelText('Attach files'), { target: { files: [new File(['B'], 'B-recovered.txt')] } }));
  const incarnation = f.sourceOwner.snapshot()!.incarnation;
  const oldSocket = server.socket, lateMetadata = server.commit(metadata, oldSocket);
  await act(async () => { oldSocket.close(); server.held.delete('session/read'); server.held.add('session/attach'); await server.connect(); });
  await server.waitFor('session/attach', 3);
  const recovered = server.requests.filter(row => row.socket === server.socket && row.request.method === 'session/attach');
  expect(recovered.map(row => row.request.params)).toEqual([{ session_id: 'A', node_id: 'node-A' }, { session_id: 'B', node_id: 'node-B' }]);
  await act(async () => server.reply(recovered[0].request));
  await waitFor(() => expect(server.client.isAttachmentControlCurrent('A', server.client.getSnapshot().views.A.attachmentObservation)).toBe(true));
  expect(server.client.isAttachmentPreparing('B')).toBe(true);
  expect(server.client.isAttachmentControlCurrent('B', server.client.getSnapshot().views.B.attachmentObservation)).toBe(false);
  await waitFor(() => expect(f.editor().disabled).toBe(false));
  expect(f.editor().value).toBe('Explicit B input while recovery remains cold');
  expect(f.sourceOwner.snapshot()!.incarnation).toBe(incarnation);
  expect(f.ui.getByText('B-recovered.txt')).toBeTruthy();
  expect(server.requests.filter(row => ['session/submissionIntent', 'turn/start', 'turn/steer'].includes(row.request.method))).toEqual([]);
  await act(async () => oldSocket.deliver(lateMetadata));
  await act(async () => fireEvent.click(f.ui.getByRole('button', { name: 'Send' })));
  const intent = await server.waitFor('session/submissionIntent', 1);
  expect(intent.params).toEqual({ session_id: 'B', node_id: 'node-B' });
  expect(server.requests.filter(row => row.request.method === 'turn/start' || row.request.method === 'session/uploadPrepare')).toEqual([]);
  expect(server.requests.filter(row => row.request.method === 'session/attach')).toHaveLength(3);
  await act(async () => server.reply(recovered[1].request));
  const input = await server.waitFor('turn/start', 1);
  if (input.method !== 'turn/start') throw new Error('Expected B input');
  expect(input.params.target).toMatchObject({ session_id: 'B', conversation_id: 'conversation-B' });
  expect(input.params.content).toEqual([
    { type: 'upload', session_id: 'B', batch_id: expect.any(String), token: 'file-0' },
    { type: 'text', text: 'Explicit B input while recovery remains cold' },
  ]);
  await waitFor(() => expect(firstSubmissions(server.client).session('B')?.phase).toBe('admitted'));
  await waitFor(() => expect(f.ui.container.querySelector('[data-shell-route]')?.getAttribute('data-shell-route')).toBe('session'));
  expect(f.editor().value).toBe('');
  expect(server.requests.filter(row => row.request.method === 'turn/start')).toHaveLength(1);
  expect(server.requests.filter(row => row.request.method === 'session/submissionIntent')).toHaveLength(1);
  expect(server.requests.filter(row => row.request.method === 'session/attach')).toHaveLength(3);
  expect(server.requests.filter(row => row.request.method === 'session/uploadPrepare')).toHaveLength(1);
  expect(server.requests.filter(row => ['session/detach', 'turn/cancel', 'turn/steer'].includes(row.request.method))).toEqual([]);
});

it.each(['same target', 'newer Session'] as const)('Refresh observation restores only its still-selected cold draft (%s)', async destination => {
  const f = await fixture();
  await act(async () => fireEvent.change(f.ui.getByLabelText('Attach files'), { target: { files: [new File(['A'], 'A-owned.txt')] } }));
  await waitFor(() => expect(f.sourceIntake.snapshot()[0]?.status).toBe('ready'));
  const sourceFiles = f.sourceIntake.snapshot(), generation = server.client.getSnapshot().generation;
  const checkpoints = server.requests.filter(row => row.request.method === 'session/checkpoint').length;
  server.held.add('session/checkpoint'); server.held.add('session/attach');
  await act(async () => fireEvent.click(f.ui.getByRole('button', { name: 'Open Session B' })));
  const attached = await server.waitFor('session/attach', 2);
  expect(f.editor().disabled).toBe(false);
  fireEvent.change(f.editor(), { target: { value: 'B text survives its failed observation' } });
  await act(async () => fireEvent.change(f.ui.getByLabelText('Attach files'), { target: { files: [new File(['B'], 'B-refresh.txt')] } }));
  const draft = f.sourceOwner.snapshot()!, intake = server.client.attachmentIntakes.lookup(draft.key)!;
  expect(f.ui.getByText('B-refresh.txt')).toBeTruthy();
  await act(async () => server.reply(attached));
  const failed = await server.waitFor('session/checkpoint', checkpoints + 1);
  await act(async () => server.socket.deliver({ jsonrpc: '2.0', id: failed.id, error: { code: -32000, message: 'B checkpoint unavailable', data: { kind: 'operation_failed' } } }));
  await waitFor(() => expect(f.ui.getByRole('button', { name: 'Refresh observation' })).toBeTruthy());
  await waitFor(() => expect(f.editor().disabled).toBe(true));
  expect(server.client.isAttachmentControlCurrent('A', server.client.getSnapshot().views.A.attachmentObservation)).toBe(true);
  expect(f.ui.container.querySelector('[data-shell-route]')?.getAttribute('data-shell-route')).toBe('session-draft');
  expect(f.editor().value).toBe('B text survives its failed observation');
  expect(f.ui.getByText('B-refresh.txt')).toBeTruthy();
  await act(async () => fireEvent.click(f.ui.getByRole('button', { name: 'Refresh observation' })));
  const refreshed = await server.waitFor('session/checkpoint', checkpoints + 2);
  if (refreshed.method !== 'session/checkpoint') throw new Error('Expected explicit observation refresh');
  expect(refreshed.params.target).toEqual(server.client.getSnapshot().views.B.target);
  expect(server.client.attachmentIntakes.lookup(f.sourceKey)).toBe(f.sourceIntake);
  expect(f.sourceIntake.snapshot()).toBe(sourceFiles);
  if (destination === 'newer Session') {
    await act(async () => fireEvent.click(f.ui.getByRole('button', { name: 'Open Session A' })));
    fireEvent.change(f.editor(), { target: { value: 'A remains selected after the B refresh' } });
  }
  await act(async () => server.reply(refreshed));
  await waitFor(() => expect(server.client.isAttachmentControlCurrent('B', server.client.getSnapshot().views.B.attachmentObservation)).toBe(true));
  await waitFor(() => expect(f.ui.container.querySelector('[data-shell-route]')?.getAttribute('data-shell-route')).toBe('session'));
  await waitFor(() => expect(f.editor().disabled).toBe(false));
  expect(server.client.getSnapshot().generation).toBe(generation);
  if (destination === 'same target') {
    expect(f.sourceOwner.snapshot()?.incarnation).toBe(draft.incarnation);
    expect(server.client.attachmentIntakes.lookup(draft.key)).toBe(intake);
    expect(f.editor().value).toBe('B text survives its failed observation');
    expect(f.ui.queryByRole('button', { name: 'Refresh observation' })).toBeNull();
    expect(f.ui.getByText('B-refresh.txt')).toBeTruthy();
    expect(f.ui.queryByText('A-owned.txt')).toBeNull();
  } else {
    expect(f.editor().value).toBe('A remains selected after the B refresh');
    expect(f.ui.queryByText('B-refresh.txt')).toBeNull();
  }
  expect(server.client.target('A')).toBe(f.target);
  expect(server.client.getSnapshot().views.A.snapshot?.attempt).toBe(f.attempt);
  expect(server.requests.filter(row => row.request.method === 'session/attach').map(row => row.request.params)).toEqual([{ session_id: 'A', node_id: 'node-A' }, { session_id: 'B', node_id: 'node-B' }]);
  expect(server.requests.filter(row => row.request.method === 'session/uploadPrepare')).toHaveLength(1);
  expect(server.requests.filter(row => ['session/submissionIntent', 'turn/start', 'turn/steer', 'turn/cancel', 'session/detach', 'session/switchNode'].includes(row.request.method))).toEqual([]);
});

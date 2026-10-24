import { act, cleanup, fireEvent, render, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { AttachmentTarget, RuntimeClientTranscriptEntry, SessionNodeObservation } from '../../protocol/app-server/v57';
import { turnPresentation } from '../src/bindings/turn-presentation';
import { AgentTranscript } from '../src/app/agent/AgentTranscript';
import { ConversationLive } from '../src/app/agent/ConversationLive';
import { useForkPoint } from '../src/app/agent/ForkPoint';
import { App } from '../src/app/App';
import { Server, snapshot } from './fixture';
import { firstSubmissions } from '../src/app/new-conversation/first-submit';

let server: Server | undefined;
afterEach(() => { cleanup(); server?.client.disconnect(); localStorage.clear(); });
const origin = { type: 'fork' as const, source_session: 'B', source_node: 'source-node', source_surface_revision: '8', source_message: 'source-message', side: 'after' as const };
const entry = (n: number): RuntimeClientTranscriptEntry => ({ cursor: String(n), item: { type: 'message', message: { role: 'assistant', id: `m${n}`, content: [{ type: 'text', text: `Message ${n}` }] } } });
const kinds = (numbers: number[], through: string, next?: string) => turnPresentation(numbers.map(entry), { inherited_through: through, next_cursor: next }).map(node => node.kind === 'entry' ? node.entry.cursor : node.kind);

it('seats the fork at the native message boundary, including a cut inside a turn', () => {
  expect(kinds([1, 2, 3, 4], '2')).toEqual(['1', '2', 'fork-point', '3', '4']);
  expect(kinds([1, 2], '2')).toEqual(['1', '2', 'fork-point']);
  expect(kinds([3, 4], '2', '3')).toEqual(['fork-point', '3', '4']);
  expect(kinds([80, 81], '2', '80')).toEqual(['80', '81']);
  expect(kinds([1, 2], '20', '1')).toEqual(['1', '2']);
  expect(kinds([], '0')).toEqual(['fork-point']);
});

it('keeps inherited completion controls above the fork and starts an empty prefix before new content', () => {
  const inherited = entry(1);
  inherited.completed_response = {closing_locator:{type:'by_message_id',message_id:'m1'},deferred_fields:[], closing_message_id: 'm1', origin: {attempt_id_source:null, conversation_id: 'source', attempt_id: 'turn', closing_message_id: 'source-m1' }, surface_revision: '1', completed_at: '2026-10-07T00:00:00Z', models: [] };
  expect(turnPresentation([inherited, entry(2)], { inherited_through: '1' }).map(node => node.kind)).toEqual(['entry', 'tail', 'fork-point', 'entry']);
  expect(kinds([1, 2], '0')).toEqual(['fork-point', '1', '2']);
});

it('renders one source action with the exact origin and retains compaction cards', () => {
  const s = snapshot(); s.transcript = { inherited_through: '1', entries: [entry(1), entry(2)] };
  const open = vi.fn();
  const ui = render(<AgentTranscript snapshot={s} forkPoint={{ origin, through: '1' }} onOpenSource={open}/>);
  expect(ui.getByRole('group', { name: 'Fork point' }).textContent).toContain('Forked from “B”');
  const rows = [...ui.container.querySelectorAll('[data-chat-anchor-key]')].map(row => row.getAttribute('data-chat-anchor-key'));
  expect(rows.indexOf('fork-point')).toBeGreaterThan(rows.indexOf('message:m1'));
  expect(rows.indexOf('fork-point')).toBeLessThan(rows.indexOf('message:m2'));
  fireEvent.click(ui.getByRole('button', { name: 'View source' }));
  expect(open).toHaveBeenCalledExactlyOnceWith(origin);
  ui.rerender(<AgentTranscript snapshot={s}/>);
  expect(ui.queryByRole('group', { name: 'Fork point' })).toBeNull();
});

it.each([33, 129, 4096])('reads the exact attached node at ordinal %s in one request, ignoring the mutable default and other origins', async ordinal => {
  server = new Server();
  server.handlers.set('session/read', () => ({ type: 'session', session: { id: 'A', active_node: 'attached-node', active_conversation_id: 'conversation-A', node_count: 33, created_at: '2026-10-07T00:00:00Z', updated_at: '2026-10-07T00:00:00Z' } }));
  const s = snapshot(); s.transcript = { inherited_through: '1', entries: [entry(1), entry(2)] };
  server.snapshots.set('A', s);
  const node: SessionNodeObservation = { ordinal: '33', id: 'attached-node', conversation_id: 'conversation-A', origin };
  server.handlers.set('session/node', request => {
    expect(request).toMatchObject({ params: { session_id: 'A', node_id: 'attached-node' } });
    return { type: 'node', node: { ...node, ordinal: String(ordinal) } };
  });
  await server.attached('A');
  const initialNodeReads = server.requests.filter(row => row.request.method === 'session/node').length;
  const ui = await act(async () => render(<ConversationLive client={server!.client} sessionId="A" mode="chat" disabled={false} onHistorical={() => {}} onOpenSource={() => {}}/>));
  await waitFor(() => expect(ui.getByRole('group', { name: 'Fork point' })).toBeTruthy());
  expect(ui.getByText(/Forked from/).title).toContain('2026');
  expect(server.client.getSnapshot().views.A.nodeId).toBe('attached-node');
  expect(server.requests.filter(row => row.request.method === 'session/node')).toHaveLength(initialNodeReads + 1);
  expect(server.requests.filter(row => row.request.method === 'session/tree')).toHaveLength(0);
  node.origin = { type: 'clone', source_session: 'B', source_node: 'source-node', source_surface_revision: '8' };
  cleanup();
  const original = await act(async () => render(<ConversationLive client={server!.client} sessionId="A" mode="chat" disabled={false} onHistorical={() => {}}/>));
  await waitFor(() => expect(server!.requests.filter(row => row.request.method === 'session/node')).toHaveLength(initialNodeReads + 2));
  expect(original.queryByRole('group', { name: 'Fork point' })).toBeNull();
});

for (const pendingInput of [false, true]) it.each(['B', 'A'])(`the source action reads exact Node and Message in Session %s without executable effects, pending input=${pendingInput}`, async sourceSession => {
  server = new Server();
  server.handlers.set('session/read', () => ({ type: 'session', session: { id: 'A', active_node: 'child-node', active_conversation_id: 'conversation-A', node_count: 1, created_at: '2026-10-07T00:00:00Z', updated_at: '2026-10-07T00:00:00Z' } }));
  const selectedOrigin = { ...origin, source_session: sourceSession };
  const child = snapshot(); child.attempt = { attempt_id: 'running-origin', phase: { type: 'running' }, turn: 1 }; child.transcript = { inherited_through: '1', entries: [entry(1), entry(2)] };
  const source = snapshot(sourceSession); source.conversation_id = `source-conversation-${sourceSession}`;
  source.transcript.entries = [{ cursor: '12', item: { type: 'message', message: { role: 'assistant', id: origin.source_message, content: [{ type: 'text', text: 'Original source response [local.ts](./src/local.ts)' }] } } }];
  if (sourceSession !== 'A') server.snapshots.set(sourceSession, source);
  server.snapshots.set('A', child);
  server.nodeSnapshots.set(origin.source_node, source);
  server.handlers.set('session/node', request => {
    if (request.method !== 'session/node') throw new Error('wrong method');
    return { type: 'node', node: request.params.node_id === 'child-node'
      ? { ordinal: '129', id: 'child-node', conversation_id: child.conversation_id, origin: selectedOrigin }
      : { ordinal: '4096', id: origin.source_node, conversation_id: source.conversation_id, origin: { type: 'new' } } };
  });
  await server.attached('A');
  let input: Promise<boolean> | undefined;
  if (pendingInput) {
    server.held.add('turn/start');
    const { firstSubmitPort } = await import('../src/app/new-conversation/port');
    input = firstSubmissions(server.client).submit('native-input', { workspaceId: '', text: 'Already admitted input', files: [] }, firstSubmitPort(server.client, server.workspaceHost, () => true, () => () => {}, true), undefined, undefined, 'A');
    await server.waitFor('turn/start', 1);
    expect(firstSubmissions(server.client).session('A')?.phase).toBe('admitting');
  }
  localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint: 'ws://127.0.0.1:8080/', openViews: ['A'] }));
  const before = server.requests.length, target = server.client.target('A');
  const ui = await act(async () => render(<App client={server!.client} workspaceHost={server!.workspaceHost}/>));
  await waitFor(() => expect((ui.getByRole('button', { name: 'View source' }) as HTMLButtonElement).disabled).toBe(false));
  expect((ui.getByRole('button', { name: /^Stop$/ }) as HTMLButtonElement).disabled).toBe(false);
  await act(async () => fireEvent.click(ui.getByRole('button', { name: 'View source' })));
  await waitFor(() => expect(ui.container.querySelector('[data-history-session]')?.getAttribute('data-history-session')).toBe(sourceSession));
  expect(ui.container.querySelector('[data-history-node]')?.getAttribute('data-history-node')).toBe(origin.source_node);
  expect(ui.container.querySelector(`[data-chat-anchor-key="message:${origin.source_message}"]`)).toBeTruthy();
  await waitFor(() => expect(ui.getByText('Original source response local.ts')).toBeTruthy());
  expect(ui.queryByRole('button', { name: 'local.ts' })).toBeNull();
  const requests = server.requests.slice(before).map(row => row.request);
  expect(requests.filter(request => ['session/attach', 'session/detach', 'session/switchNode', 'turn/start'].includes(request.method))).toEqual([]);
  expect(requests.filter(request => request.method === 'session/history' && request.params.at.type === 'message').map(request => request.params)).toEqual([{ session_id: sourceSession, node_id: origin.source_node, at: { type: 'message', id: origin.source_message }, limit: 32 }]);
  expect(server.client.target('A')).toBe(target); expect(ui.queryByRole('textbox', { name: 'Message' })).toBeNull();
  await act(async () => fireEvent.click(ui.getByRole('button', { name: 'Return to conversation' })));
  expect(server.client.target('A')).toBe(target);
  expect(server.client.getSnapshot().views.A.snapshot?.attempt?.attempt_id).toBe('running-origin');
  expect((ui.getByRole('button', { name: /^Stop$/ }) as HTMLButtonElement).disabled).toBe(false);
  if (input) { server.reply(await server.waitFor('turn/start', 1)); expect(await input).toBe(true); }

});


function ForkProbe({ target }: { target: AttachmentTarget }) {
  const reading = useForkPoint(server!.client, target, '1');
  return <div data-testid="point">{reading.error ?? reading.point?.origin.source_session ?? 'none'}</div>;
}
it.each(['attachment', 'connection'])('retires a held exact node read after replacing %s', async replacement => {
  server = new Server();
  const child = snapshot(); child.transcript.inherited_through = '1'; server.snapshots.set('A', child);
  await server.attached('A');
  server.handlers.set('session/node', () => ({ type: 'node', node: { id: 'node-A', ordinal: '4096', conversation_id: child.conversation_id, origin } }));
  server.held.add('session/node');
  const ui = await act(async () => render(<ForkProbe target={server!.client.target('A')}/>));
  const held = await server.waitFor('session/node', server.requests.filter(row => row.request.method === 'session/node').length);
  const response = server.commit(held);
  server.held.delete('session/node'); server.handlers.delete('session/node');
  await act(async () => {
    if (replacement === 'connection') { server!.client.disconnect(); await server!.connect(); }
    else await server!.client.release('A');
    await server!.client.attach('A');
    ui.rerender(<ForkProbe target={server!.client.target('A')}/>);
  });
  await waitFor(() => expect(ui.getByTestId('point').textContent).toBe('none'));
  await act(async () => server!.socket.deliver(response));
  expect(ui.getByTestId('point').textContent).toBe('none');
});

it('keeps the immutable Fork when optional source summary is unavailable', async () => {
  server = new Server(); await server.attached('A');
  server.handlers.set('session/node', () => ({ type: 'node', node: { id: 'node-A', ordinal: '129', conversation_id: 'conversation-A', origin: { ...origin, source_session: 'deleted-source' } } }));
  const ui = await act(async () => render(<ForkProbe target={server!.client.target('A')}/>));
  await waitFor(() => expect(ui.getByTestId('point').textContent).toBe('deleted-source'));
  expect(server.requests.some(row => row.request.method === 'session/tree')).toBe(false);
});

it('does not attach or publish an obsolete source Node failure after a newer route gesture', async () => {
  server = new Server();
  const child = snapshot(); child.transcript = { entries: [entry(1)], inherited_through: '1' };
  server.snapshots.set('A', child);
  server.handlers.set('session/node', () => ({ type: 'node', node: { id: 'node-A', ordinal: '129', conversation_id: child.conversation_id, origin } }));
  await server.attached('A');
  localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint: 'ws://127.0.0.1:8080/', openViews: ['A'] }));
  const ui = await act(async () => render(<App client={server!.client} workspaceHost={server!.workspaceHost}/>));
  await waitFor(() => expect(ui.getByRole('button', { name: 'View source' })).toBeTruthy());
  server.held.add('session/node');
  await act(async () => fireEvent.click(ui.getByRole('button', { name: 'View source' })));
  const held = await server.waitFor('session/node', server.requests.filter(row => row.request.method === 'session/node').length);
  expect(held).toMatchObject({ params: { session_id: 'B', node_id: origin.source_node } });
  await act(async () => fireEvent.click(ui.getAllByRole('button', { name: 'New Conversation' })[0]));
  await act(async () => server!.socket.deliver({ jsonrpc: '2.0', id: held.id, error: { code: -32000, message: 'Late source Node missing', data: { kind: 'unknown_node', session_id: 'B', node_id: origin.source_node } } }));
  expect(ui.queryByText(/Late source Node missing/)).toBeNull();
  expect(server.requests.filter(row => row.request.method === 'session/attach' && row.request.params.session_id === 'B')).toHaveLength(0);
});

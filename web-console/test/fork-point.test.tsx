import { act, cleanup, fireEvent, render, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { RuntimeClientTranscriptEntry, SessionNode } from '../../protocol/app-server/v36';
import { turnPresentation } from '../src/bindings/turn-presentation';
import { AgentTranscript } from '../src/app/agent/AgentTranscript';
import { ConversationLive } from '../src/app/agent/ConversationLive';
import { App } from '../src/app/App';
import { Server, snapshot } from './fixture';

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
  inherited.completed_response = { closing_message_id: 'm1', origin: { conversation_id: 'source', attempt_id: 'turn', closing_message_id: 'source-m1' }, surface_revision: '1', completed_at: '2026-10-07T00:00:00Z', models: [] };
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

it('reads the attached node across tree pages, ignoring the mutable default and other origins', async () => {
  server = new Server();
  server.handlers.set('session/read', () => ({ type: 'session', session: { id: 'A', active_node: 'attached-node', active_conversation_id: 'conversation-A', node_count: 33, created_at: '2026-10-07T00:00:00Z', updated_at: '2026-10-07T00:00:00Z' } }));
  const s = snapshot(); s.transcript = { inherited_through: '1', entries: [entry(1), entry(2)] };
  server.snapshots.set('A', s); await server.attached('A');
  const node: SessionNode = { ordinal: '33', id: 'attached-node', conversation_id: 'conversation-A', origin };
  server.handlers.set('session/tree', request => ({ type: 'tree', nodes: request.method === 'session/tree' && request.params.offset === 32 ? [node] : [{ ...node, id: 'default-node', conversation_id: 'other', origin: { type: 'new' } }], next_offset: request.method === 'session/tree' && request.params.offset === 32 ? null : 32 }));
  const ui = await act(async () => render(<ConversationLive client={server!.client} sessionId="A" mode="chat" disabled={false} onHistorical={() => {}} onOpenSource={() => {}}/>));
  await waitFor(() => expect(ui.getByRole('group', { name: 'Fork point' })).toBeTruthy());
  expect(ui.getByText(/Forked from/).title).toContain('2026');
  expect(server.client.getSnapshot().views.A.nodeId).toBe('attached-node');
  node.origin = { type: 'clone', source_session: 'B', source_node: 'source-node', source_surface_revision: '8' };
  cleanup();
  const original = await act(async () => render(<ConversationLive client={server!.client} sessionId="A" mode="chat" disabled={false} onHistorical={() => {}}/>));
  await waitFor(() => expect(server!.requests.filter(row => row.request.method === 'session/tree')).toHaveLength(4));
  expect(original.queryByRole('group', { name: 'Fork point' })).toBeNull();
});

it.each(['B', 'A'])('the source action opens the recorded node in Session %s and locates its original message', async sourceSession => {
  server = new Server();
  server.handlers.set('session/read', () => ({ type: 'session', session: { id: 'A', active_node: 'child-node', active_conversation_id: 'conversation-A', node_count: 1, created_at: '2026-10-07T00:00:00Z', updated_at: '2026-10-07T00:00:00Z' } }));
  const selectedOrigin = { ...origin, source_session: sourceSession };
  const child = snapshot(); child.transcript = { inherited_through: '1', entries: [entry(1), entry(2)] };
  const source = snapshot(sourceSession); source.conversation_id = `source-conversation-${sourceSession}`;
  source.transcript.entries = [{ cursor: '12', item: { type: 'message', message: { role: 'assistant', id: origin.source_message, content: [{ type: 'text', text: 'Original source response' }] } } }];
  if (sourceSession !== 'A') server.snapshots.set(sourceSession, source);
  server.snapshots.set('A', child);
  server.nodeSnapshots.set(origin.source_node, source);
  server.handlers.set('session/tree', request => ({ type: 'tree', nodes: [
    ...(request.method === 'session/tree' && request.params.session_id === 'A' ? [{ ordinal: '2', id: 'child-node', conversation_id: child.conversation_id, origin: selectedOrigin }] : []),
    { ordinal: '1', id: origin.source_node, conversation_id: source.conversation_id, origin: { type: 'new' } },
  ], next_offset: null }));
  await server.attached('A');
  localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint: 'ws://127.0.0.1:8080/', openViews: ['A'] }));
  const locate = vi.spyOn(server.client, 'navigateMessage');
  const ui = await act(async () => render(<App client={server!.client} workspaceHost={server!.workspaceHost}/>));
  await waitFor(() => expect(ui.getByRole('button', { name: 'View source' })).toBeTruthy());
  await act(async () => fireEvent.click(ui.getByRole('button', { name: 'View source' })));
  await waitFor(() => expect(locate).toHaveBeenCalledWith(sourceSession, origin.source_message, expect.any(Function)));
  expect(server.requests.find(row => row.request.method === 'session/attach' && row.request.params.node_id === origin.source_node)?.request).toMatchObject({ params: { node_id: origin.source_node, session_id: sourceSession } });
  expect(ui.getByLabelText('Canonical conversation').textContent).toContain('Original source response');
  expect(server.requests.some(row => row.request.method === 'session/switchNode')).toBe(sourceSession === 'A');
});

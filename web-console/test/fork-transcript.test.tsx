import { act, cleanup, render, waitFor } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import type { RuntimeClientTranscriptEntry } from '../../protocol/app-server/v62';
import { AgentTranscript } from '../src/app/agent/AgentTranscript';
import { ConversationLive } from '../src/app/agent/ConversationLive';
import { Server, snapshot } from './fixture';

let server: Server | undefined;
afterEach(() => { cleanup(); server?.client.disconnect(); });
const entry = (cursor: string, id: string, text: string): RuntimeClientTranscriptEntry => ({ cursor,
  item: { type: 'message', message: { id, role: 'assistant', content: [{ type: 'text', text }] } } });
function forkSnapshot() {
  const state = snapshot(); state.messages = []; state.attempt = null;
  const inherited = entry('1', 'inherited', 'Inherited answer');
  inherited.completed_response = { closing_locator: { type: 'by_message_id', message_id: 'inherited' }, deferred_fields: [], closing_message_id: 'inherited',
    origin: { conversation_id: 'source', attempt_id: 'source-attempt', closing_message_id: 'source-answer' }, surface_revision: '1', completed_at: '2026-10-07T00:00:00Z', models: [] };
  state.transcript = { inherited_through: '1', entries: [inherited, entry('2', 'branch', 'New branch answer')] };
  return state;
}

it('continues inherited messages and completion controls directly into the branch', () => {
  const ui = render(<AgentTranscript snapshot={forkSnapshot()}/>);
  const inherited = ui.container.querySelector('[data-chat-anchor-key="message:inherited"]')!;
  const tail = ui.container.querySelector('[data-turn-tail]')!;
  const branch = ui.container.querySelector('[data-chat-anchor-key="message:branch"]')!;
  expect(inherited.compareDocumentPosition(tail) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  expect(tail.compareDocumentPosition(branch) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  expect(ui.queryByRole('group', { name: 'Fork point' })).toBeNull();
  expect(ui.queryByRole('button', { name: 'View source' })).toBeNull();
});

it('renders a fork without supplementary source metadata reads', async () => {
  server = new Server(); server.snapshots.set('A', forkSnapshot());
  server.handlers.set('session/node', () => ({ type: 'node', node: { id: 'node-A', ordinal: '1', conversation_id: 'conversation-A',
    origin: { type: 'fork', source_session: 'B', source_node: 'node-B', source_surface_revision: '1', source_message: 'source-answer', side: 'after' } } }));
  await server.attached('A');
  const before = server.requests.length;
  const ui = await act(async () => render(<ConversationLive client={server!.client} sessionId="A" mode="chat" disabled={false} onHistorical={() => {}}/>));
  await waitFor(() => expect(ui.getByText('New branch answer')).toBeTruthy());
  expect(ui.getByText('Inherited answer')).toBeTruthy();
  expect(ui.queryByRole('group', { name: 'Fork point' })).toBeNull();
  expect(server.requests.slice(before).filter(({ request }) => request.method === 'session/node'
    || request.method === 'session/summary' && request.params.session_id === 'B'
    || request.method === 'session/read' && request.params.session_id === 'B')).toEqual([]);
});

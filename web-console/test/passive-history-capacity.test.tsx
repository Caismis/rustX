import { act, cleanup, render, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { waitFor as waitIdle } from 'xstate';
import type { RuntimeClientTranscriptEntry } from '../../protocol/app-server/v80';
import { RpcFailure } from '../src/client/errors';
import { deriveSessionProductState } from '../src/bindings/session-product';
import { translator } from '../src/locale/translation';
import { ConversationLive } from '../src/app/agent/ConversationLive';
import { HISTORY_PAGE_SIZE } from '../src/client/transcript';
import { Server, snapshot } from './fixture';
import { routeFixture } from './shell-navigation-fixture';

afterEach(() => { cleanup(); vi.restoreAllMocks(); });

const refusal = (kind: 'residency_capacity' | 'attachment_capacity' | 'operation_failed' | 'request_capacity') =>
  new RpcFailure({ code: -32000, message: 'Operation rejected', data: { kind } });
const message = (cursor: string, text: string, role: 'user' | 'assistant' = 'assistant'): RuntimeClientTranscriptEntry => role === 'user'
  ? { cursor, item: { type: 'message', message: { id: `message-${cursor}`, role, source: 'human', content: [{ type: 'text', text }] } } }
  : { cursor, item: { type: 'message', message: { id: `message-${cursor}`, role, content: [{ type: 'text', text }] } }, turn_process: {
    conversation_id: 'conversation-A', attempt_id: Number(cursor) < 40 ? 'attempt-early' : 'attempt-late', control_cursor: cursor, outcome: 'completed',
    message_count: 1, tool_call_count: 0, final_message_locator: null, final_message_id_deferred: false, final_message_id_source: null,
  } };
const historyOf = (entries: RuntimeClientTranscriptEntry[], next: string | null) => ({
  type: 'session_history' as const, conversation_id: 'conversation-A',
  window: { cut: { conversation_id: 'conversation-A', journal: '1000', transcript: '1000', mutation_revision: '0', ledger_through: '0', surface_revision: '0', transcript_total: '0', pending_through: '0', pending_total: '0' },
    page: { entries, next_cursor: next } },
});
const mutations = (server: Server) => server.requests.filter(row => ['thread/resume', 'thread/start', 'session/detach', 'session/switchNode', 'inbound/queue', 'turn/interrupt', 'session/checkpoint', 'session/subscribe'].includes(row.request.method)).map(row => row.request.method);

it.each(['residency_capacity', 'attachment_capacity'] as const)('%s leaves the selected session readable and pages durable history without activation or retry', async kind => {
  const server = new Server(); await server.connect();
  const older = [message('1', 'Fork source', 'user')], newer = [message('2', 'Completed fork answer.')];
  server.handlers.set('thread/resume', () => { throw refusal(kind); });
  server.handlers.set('session/history', request => {
    if (request.method !== 'session/history') throw Error('history');
    return request.params.at.type === 'latest' ? historyOf(newer, '2') : historyOf(older, null);
  });
  await expect(server.client.attach('A')).rejects.toThrow(/Operation rejected/);
  await vi.waitFor(() => expect(server.client.getSnapshot().views.A.preview?.history.page.entries?.map(entry => entry.cursor)).toEqual(['1', '2']));
  const view = server.client.getSnapshot().views.A;
  expect(view).toMatchObject({ passiveHistory: true, attachment: 'detached', attachmentIntent: 'wanted', error: undefined });
  expect(view.target).toBeUndefined();
  expect(view.snapshot).toBeUndefined();
  expect(mutations(server)).toEqual(['thread/resume']);
  expect(server.requests.filter(row => row.request.method === 'session/history').every(row => row.request.method === 'session/history' && row.request.params.node_id === 'node-A')).toBe(true);
  expect(deriveSessionProductState(translator('en'), server.client.getSnapshot(), view).status).toBe('idle');
  expect(deriveSessionProductState(translator('en'), server.client.getSnapshot(), view).label).toBe('Read-only history');

  server.socket.close();
  await server.connect();
  expect(server.requests.filter(row => row.request.method === 'thread/resume')).toHaveLength(1);
  expect(server.client.getSnapshot().views.A.passiveHistory).toBe(true);
  expect(server.client.getSnapshot().views.A.target).toBeUndefined();
  server.client.disconnect();
});

it('a window that is already useful still pages one older durable page, and a released read cannot land', async () => {
  const server = new Server(); await server.connect();
  const ready = Array.from({ length: 50 }, (_, index) => ({ ...message(String(index + 51), `Ready ${index + 51}`), turn_process: { ...message(String(index + 51), '').turn_process!, attempt_id: index < 25 ? 'attempt-early' : 'attempt-late' } }));
  const hidden = message('50', 'Hidden earlier answer');
  server.handlers.set('thread/resume', () => { throw refusal('residency_capacity'); });
  server.handlers.set('session/history', request => {
    if (request.method !== 'session/history') throw Error('history');
    if (request.params.at.type === 'latest') return historyOf(ready, '51');
    expect(request.params.at).toMatchObject({ type: 'older', before: '51' });
    expect(request.params.limit).toBe(HISTORY_PAGE_SIZE);
    return historyOf([hidden], null);
  });
  await expect(server.client.attach('A')).rejects.toThrow(/Operation rejected/);
  await vi.waitFor(() => expect(server.client.getSnapshot().views.A.preview?.history.loading).toBe(false));
  expect(server.client.getSnapshot().views.A.preview?.history.page.entries).toHaveLength(50);
  expect(server.client.getSnapshot().views.A.preview?.history.page.next_cursor).toBe('51');

  server.held.add('session/history');
  const paging = server.client.loadEarlierPreview('A');
  const page = await server.waitFor('session/history', server.requests.filter(row => row.request.method === 'session/history').length);
  await server.client.release('A');
  server.reply(page);
  await paging;
  await Promise.resolve();
  expect(server.client.getSnapshot().views.A.passiveHistory).toBeUndefined();
  expect(server.client.getSnapshot().views.A.preview?.history.page.entries?.some(entry => entry.cursor === '50')).toBeFalsy();
  expect(server.requests.filter(row => row.request.method === 'thread/resume')).toHaveLength(1);
  server.client.disconnect();
});

it.each(['operation_failed', 'request_capacity'] as const)('%s still fails activation and does not install passive history', async kind => {
  const server = new Server(); await server.connect();
  server.held.add('session/history');
  server.handlers.set('thread/resume', () => { throw refusal(kind); });
  const opening = server.client.attach('A');
  const history = await server.waitFor('session/history', 1);
  await expect(opening).rejects.toThrow(/Operation rejected/);
  server.reply(history);
  await Promise.resolve(); await Promise.resolve();
  const view = server.client.getSnapshot().views.A;
  expect(view.passiveHistory).toBeUndefined();
  expect(view.preview).toBeUndefined();
  expect(view.error).toContain('Operation rejected');
  expect(view.target).toBeUndefined();
  expect(server.requests.filter(row => row.request.method === 'thread/resume')).toHaveLength(1);
  server.client.disconnect();
});

it('selecting a capacity-refused session shows durable history and does not surface the refusal', async () => {
  const server = new Server(); await server.connect();
  server.snapshots.set('B', { ...snapshot('B'), transcript: { entries: [message('1', 'Fork source', 'user'), message('2', 'Completed fork answer.')] } });
  server.handlers.set('thread/resume', request => {
    if (request.method === 'thread/resume' && request.params.threadId === 'conversation-B') throw refusal('attachment_capacity');
    throw Error('unexpected resume');
  });
  const navigation = routeFixture(server.client, server.workspaceHost);
  navigation.actor.send({ type: 'OPEN', destination: { kind: 'session', sessionId: 'B', sidebar: false } });
  await waitIdle(navigation.actor, snapshot => snapshot.matches('idle'));
  expect(navigation.actor.getSnapshot().context.error).toBeUndefined();
  expect(navigation.actor.getSnapshot().context.route).toEqual({ kind: 'session-draft', sessionId: 'B' });
  await vi.waitFor(() => expect(server.client.getSnapshot().views.B.preview?.history.page.entries?.map(entry => entry.cursor)).toEqual(['1', '2']));
  expect(server.client.getSnapshot().views.B.target).toBeUndefined();
  expect(mutations(server)).toEqual(['thread/resume']);

  const ui = await act(async () => render(<ConversationLive client={server.client} sessionId="B" mode="chat" disabled={false} onHistorical={() => {}}/>));
  await waitFor(() => expect(ui.getByLabelText('Canonical conversation').textContent).toContain('Fork source'));
  expect(ui.getByLabelText('Canonical conversation').textContent).toContain('Completed fork answer.');
  expect(ui.queryByText(/Operation rejected/)).toBeNull();
  expect(server.requests.filter(row => row.request.method === 'thread/resume')).toHaveLength(1);
  server.client.disconnect();
});

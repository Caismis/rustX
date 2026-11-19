import { act, cleanup, fireEvent, render, waitFor, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { waitFor as waitIdle } from 'xstate';
import type { RuntimeClientTranscriptEntry, TraceRecord } from '../../protocol/app-server/v80';
import { isDurableColdRead } from '../src/client/cold-reads';
import { RpcFailure } from '../src/client/errors';
import { deriveSessionProductState } from '../src/bindings/session-product';
import { translator } from '../src/locale/translation';
import { ConversationLive } from '../src/app/agent/ConversationLive';
import { TRACE_PAGE_SIZE } from '../src/client/trace';
import { HISTORY_PAGE_SIZE } from '../src/client/transcript';
import { checkpoint, Server, snapshot, traceReceipt } from './fixture';
import { routeFixture } from './shell-navigation-fixture';
import { requestDetail, traceRecord } from './trace-fixture';

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

const traceCut = (id = 'A') => checkpoint(snapshot(id)).read_cut;
const traceHistoryOf = (records: TraceRecord[], next: string | null, id = 'A') => ({
  type: 'session_trace_history' as const,
  conversation_id: `conversation-${id}`,
  page: { read_cut: traceCut(id), records, next_cursor: next },
});

it.each(['residency_capacity', 'attachment_capacity'] as const)('%s supports passive Trajectory initial load, pagination, selection, details and release fencing without retrying thread/resume', async kind => {
  const server = new Server(); await server.connect();
  const ready: TraceRecord[] = [
    ...Array.from({ length: 50 }, (_, index) => traceRecord(index + 52, {
      kind: index % 2 === 0 ? 'user' : 'assistant',
      request: null,
      location: { attempt_id: index < 25 ? 'attempt-early' : 'attempt-late', step_id: null },
      preview: { text: `Trace message ${index + 52}`, truncated: false },
    })),
    traceRecord(102, { location: { attempt_id: 'attempt-late', step_id: '1' } }),
  ];
  const earlier = traceRecord(51, { location: { attempt_id: 'attempt-early', step_id: '1' } });
  server.held.add('session/traceHistory');
  server.handlers.set('thread/resume', () => { throw refusal(kind); });
  server.handlers.set('session/traceHistory', request => {
    if (request.method !== 'session/traceHistory') throw Error('trace history');
    if (request.params.before == null) return traceHistoryOf(ready, 'trace:52');
    expect(request.params.before).toBe('trace:52');
    expect(request.params.limit).toBe(TRACE_PAGE_SIZE);
    return traceHistoryOf([earlier], null);
  });
  server.handlers.set('session/traceHistoryDetail', request => {
    if (request.method !== 'session/traceHistoryDetail') throw Error('trace detail');
    return { type: 'session_trace_history_detail', conversation_id: 'conversation-A', page: traceReceipt(requestDetail(Number(request.params.record_id.slice(6)))).page };
  });

  const opening = server.client.attach('A');
  const attachingTrace = await server.waitFor('session/traceHistory', 1);
  await expect(opening).rejects.toThrow(/Operation rejected/);
  // The attaching prefetch's open admission retired on refusal; its reply cannot land.
  server.reply(attachingTrace);
  await Promise.resolve();
  expect(isDurableColdRead(server.client.getSnapshot().views.A)).toBe(true);
  expect(server.client.getSnapshot().views.A.tracePreview).toBeUndefined();

  server.held.delete('session/traceHistory');
  await server.client.loadInitialTrace('A');
  expect(server.client.getSnapshot().views.A.tracePreview?.cache.page.records).toHaveLength(51);
  expect(server.client.getSnapshot().views.A.tracePreview?.cache.page.next_cursor).toBe('trace:52');

  await server.client.loadEarlierTrace('A');
  expect(server.client.getSnapshot().views.A.tracePreview?.cache.page.records[0]?.id).toBe('trace:51');
  expect(server.client.getSnapshot().views.A.tracePreview?.cache.page.next_cursor).toBeNull();

  server.client.selectTrace('A', 'trace:51');
  expect(server.client.getSnapshot().views.A.tracePreview?.cache.selection?.id).toBe('trace:51');

  await server.client.loadTraceDetail('A', 'trace:51');
  expect(server.client.getSnapshot().views.A.tracePreview?.cache.details['trace:51']?.detail).toEqual(requestDetail(51));
  expect(server.requests.filter(row => row.request.method === 'session/traceHistory' || row.request.method === 'session/traceHistoryDetail')
    .every(row => 'node_id' in row.request.params && row.request.params.node_id === 'node-A')).toBe(true);
  expect(mutations(server)).toEqual(['thread/resume']);

  // Reconnecting preserves passive history and allows further detail reads without thread/resume.
  server.socket.close();
  await server.connect();
  await server.client.loadInitialTrace('A');
  await server.client.loadTraceDetail('A', 'trace:102');
  expect(server.client.getSnapshot().views.A.tracePreview?.cache.details['trace:102']?.detail).toEqual(requestDetail(102));
  expect(server.requests.filter(row => row.request.method === 'thread/resume')).toHaveLength(1);

  // Releasing retires an in-flight passive detail read so it cannot land.
  server.held.add('session/traceHistoryDetail');
  const pending = server.client.loadTraceDetail('A', 'trace:52');
  const inFlight = await server.waitFor('session/traceHistoryDetail', server.requests.filter(row => row.request.method === 'session/traceHistoryDetail').length);
  await server.client.release('A');
  server.reply(inFlight);
  await pending;
  expect(isDurableColdRead(server.client.getSnapshot().views.A)).toBe(false);
  expect(server.client.getSnapshot().views.A.tracePreview?.cache.details['trace:52']?.detail).toBeUndefined();
  expect(server.requests.filter(row => row.request.method === 'thread/resume')).toHaveLength(1);
  server.client.disconnect();
});

it('passive Trajectory UI loads initial trace, paginates, selects and opens details without repeated thread/resume', async () => {
  vi.spyOn(HTMLElement.prototype, 'offsetHeight', 'get').mockReturnValue(360);
  vi.spyOn(HTMLElement.prototype, 'offsetWidth', 'get').mockReturnValue(800);
  vi.spyOn(HTMLElement.prototype, 'clientHeight', 'get').mockReturnValue(360);
  const server = new Server(); await server.connect();
  const ready: TraceRecord[] = [
    ...Array.from({ length: 50 }, (_, index) => traceRecord(index + 20, {
      kind: index % 2 === 0 ? 'user' : 'assistant',
      request: null,
      location: { attempt_id: index < 25 ? 'attempt-early' : 'attempt-late', step_id: null },
      preview: { text: `Passive row ${index + 20}`, truncated: false },
    })),
    traceRecord(70, { location: { attempt_id: 'attempt-late', step_id: '1' } }),
  ];
  const earlier = traceRecord(19, { location: { attempt_id: 'attempt-early', step_id: '1' } });
  server.snapshots.set('B', { ...snapshot('B'), transcript: { entries: [message('1', 'Fork source', 'user'), message('2', 'Completed fork answer.')] } });
  server.held.add('session/traceHistory');
  server.handlers.set('thread/resume', request => {
    if (request.method === 'thread/resume' && request.params.threadId === 'conversation-B') throw refusal('attachment_capacity');
    throw Error('unexpected resume');
  });
  server.handlers.set('session/traceHistory', request => {
    if (request.method !== 'session/traceHistory') throw Error('trace history');
    return request.params.before == null ? traceHistoryOf(ready, 'trace:20', 'B') : traceHistoryOf([earlier], null, 'B');
  });
  server.handlers.set('session/traceHistoryDetail', request => {
    if (request.method !== 'session/traceHistoryDetail') throw Error('trace detail');
    return { type: 'session_trace_history_detail', conversation_id: 'conversation-B', page: traceReceipt(requestDetail(Number(request.params.record_id.slice(6)))).page };
  });

  const navigation = routeFixture(server.client, server.workspaceHost);
  navigation.actor.send({ type: 'OPEN', destination: { kind: 'session', sessionId: 'B', sidebar: false } });
  await waitIdle(navigation.actor, snapshot => snapshot.matches('idle'));
  server.reply(await server.waitFor('session/traceHistory', 1));
  await vi.waitFor(() => expect(server.client.getSnapshot().views.B.preview?.history.page.entries?.map(entry => entry.cursor)).toEqual(['1', '2']));
  expect(server.client.getSnapshot().views.B.tracePreview).toBeUndefined();

  const ui = await act(async () => render(<ConversationLive client={server.client} sessionId="B" mode="chat" disabled={false} onHistorical={() => {}}/>));
  await waitFor(() => expect(ui.getByLabelText('Canonical conversation').textContent).toContain('Completed fork answer.'));

  await act(async () => ui.rerender(<ConversationLive client={server.client} sessionId="B" mode="trajectory" disabled={false} onHistorical={() => {}}/>));
  const initialTrace = await server.waitFor('session/traceHistory', 2);
  expect(ui.queryByLabelText('Canonical conversation')).toBeNull();
  server.held.delete('session/traceHistory');
  await act(async () => { server.reply(initialTrace); });
  await waitFor(() => expect(ui.getByRole('table', { name: 'Trace ledger' })).toBeTruthy());
  expect(ui.queryByLabelText('Canonical conversation')).toBeNull();
  expect(ui.queryByText(/Operation rejected/)).toBeNull();
  await waitFor(() => expect(server.client.getSnapshot().views.B.tracePreview?.cache.page.records).toHaveLength(51));

  // Page older history directly through the Trajectory timeline control.
  await act(async () => { fireEvent.click(within(ui.getByRole('region', { name: 'Trajectory timeline' })).getByRole('button', { name: 'Load earlier history' })); });
  await waitFor(() => expect(server.client.getSnapshot().views.B.tracePreview?.cache.page.records[0]?.id).toBe('trace:19'));

  // Select a request row and inspect its durable detail.
  await act(async () => { fireEvent.click(ui.container.querySelector('[data-request-owner="trace:70"]')!); });
  await waitFor(() => expect(ui.getByRole('complementary', { name: 'Event details' }).textContent).toContain('openai_chat_completions'));
  expect(server.client.getSnapshot().views.B.tracePreview?.cache.selection?.id).toBe('trace:70');
  expect(server.client.getSnapshot().views.B.tracePreview?.cache.details['trace:70']?.detail).toEqual(requestDetail(70));
  expect(server.requests.filter(row => row.request.method === 'thread/resume')).toHaveLength(1);
  server.client.disconnect();
});

import { afterEach, expect, it } from 'vitest';
import { forkLatestSession, increasedForkTitle } from '../src/app/commands/fork-session';
import { Server, snapshot } from './fixture';

let server: Server;
afterEach(() => server?.client.disconnect());
async function setup() {
  server = new Server(); await server.connect();
  server.snapshots.get('A')!.transcript.entries = [{ cursor: '2', item: { type: 'message', message: { id: 'answer', role: 'assistant', content: [{ type: 'text', text: 'Native completed reply' }] } },
    completed_response: { origin: { conversation_id: 'conversation-A', attempt_id: 'done' }, closing_locator: { type: 'by_message_id', message_id: 'answer' }, surface_revision: '7', deferred_fields: [], completed_at: '2026-10-20T00:00:00Z' } }];
  server.handlers.set('session/fork', request => {
    if (request.method !== 'session/fork') throw Error('wrong method');
    expect(request.params).toEqual({ session_id: 'A', node_id: 'node-A', boundary: { type: 'by_message_id', message_id: 'answer' }, surface_revision: '7', side: 'after' });
    server.snapshots.set('child', snapshot('child'));
    return { type: 'session_transition', session: { id: 'child', active_node: 'node-child', active_conversation_id: 'conversation-child', node_count: 1, created_at: '0', updated_at: '0' } };
  });
}
it('forks a cold Session at its last completion without attaching, sending or stopping work', async () => {
  await setup();
  expect(await forkLatestSession(server.client, 'A', () => true)).toBe('child');
  expect(server.requests.filter(row => row.request.method === 'session/attach')).toEqual([]);
  expect(server.requests.filter(row => ['turn/start', 'turn/cancel', 'session/switchNode', 'session/unload'].includes(row.request.method))).toEqual([]);
});
it('uses older pages at the retained native read cut when an unfinished turn fills the latest page', async () => {
  await setup();
  const native = await server.client.request({ method: 'session/history', params: { session_id: 'A', node_id: 'node-A', at: { type: 'latest' }, limit: 32 } }, 'session_history');
  let pages = 0;
  server.handlers.set('session/history', request => {
    if (request.method !== 'session/history') throw Error('wrong method');
    pages++;
    if (request.params.at.type === 'latest') return { ...native, window: { ...native.window, page: { entries: [], next_cursor: '40' } } };
    expect(request.params.at).toEqual({ type: 'older', before: '40', cut: native.window.cut });
    return native;
  });
  expect(await forkLatestSession(server.client, 'A', () => true)).toBe('child'); expect(pages).toBe(2);
});
it('uses the newest native completion descriptor when its large message body is deferred', async () => {
  await setup();
  const native = await server.client.request({ method: 'session/history', params: { session_id: 'A', node_id: 'node-A', at: { type: 'latest' }, limit: 32 } }, 'session_history');
  server.handlers.set('session/history', () => ({ ...native, window: { ...native.window, page: { ...native.window.page,
    deferred_entries: [{ offset: '4', identity: 'large-answer', content: [], deferred: true, value: {
      identity: 'large-answer', locator: { type: 'by_message_id', message_id: 'large-answer' }, role: 'assistant', kind: 'assistant',
      block_count: '1', tool_call_count: '0', tool_calls: [], content: [], collections: [],
      completed_response: { closing_locator: { type: 'by_message_id', message_id: 'large-answer' }, surface_revision: '9', deferred_fields: [],
        origin: { conversation_id: 'conversation-A', attempt_id: 'large-done' }, completed_at: '2026-10-20T01:00:00Z' },
    } }],
  } } }));
  server.handlers.set('session/fork', request => {
    if (request.method !== 'session/fork') throw Error('wrong method');
    expect(request.params).toMatchObject({ boundary: { type: 'by_message_id', message_id: 'large-answer' }, surface_revision: '9', side: 'after' });
    server.snapshots.set('child', snapshot('child'));
    return { type: 'session_transition', session: { id: 'child', active_node: 'node-child', active_conversation_id: 'conversation-child', node_count: 1, created_at: '0', updated_at: '0' } };
  });
  expect(await forkLatestSession(server.client, 'A', () => true)).toBe('child');
  expect(server.requests.some(row => row.request.method === 'session/attach')).toBe(false);
});
it('an empty Session has no invented completion or replay input', async () => {
  await setup(); server.snapshots.get('A')!.transcript.entries = [];
  await expect(forkLatestSession(server.client, 'A', () => true)).rejects.toThrow('no completed response');
  expect(server.requests.some(row => row.request.method === 'session/fork')).toBe(false);
});

it.each([['Roadmap','Roadmap (1)'],['Roadmap (1)','Roadmap (2)'],['计划（1）','计划（2）'],['计划 （9）','计划 （10）']])('fork numbering follows DSH: %s', (source, child) => { expect(increasedForkTitle(source)).toBe(child); });

import { afterEach, expect, it, vi } from 'vitest';
import { forkLatestSession, increasedForkTitle } from '../src/app/commands/fork-session';
import { routeFixture } from './shell-navigation-fixture';
import { Server, snapshot } from './fixture';

let server: Server;
afterEach(() => { server?.client.disconnect(); vi.restoreAllMocks(); });
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
  expect(await forkLatestSession(server.client, 'A', () => true)).toEqual({ sessionId: 'child' });
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
  expect(await forkLatestSession(server.client, 'A', () => true)).toEqual({ sessionId: 'child' }); expect(pages).toBe(2);
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
  expect(await forkLatestSession(server.client, 'A', () => true)).toEqual({ sessionId: 'child' });
  expect(server.requests.some(row => row.request.method === 'session/attach')).toBe(false);
});
it('an empty Session has no invented completion or replay input', async () => {
  await setup(); server.snapshots.get('A')!.transcript.entries = [];
  await expect(forkLatestSession(server.client, 'A', () => true)).rejects.toThrow('no completed response');
  expect(server.requests.some(row => row.request.method === 'session/fork')).toBe(false);
});

it.each([['Roadmap','Roadmap (1)'],['Roadmap (1)','Roadmap (2)'],['计划（1）','计划（2）'],['计划 （9）','计划 （10）']])('fork numbering follows DSH: %s', (source, child) => { expect(increasedForkTitle(source)).toBe(child); });

it.each([
  { name: '计划（9）', preview: 'First message', expected: '计划（10）' },
  { name: undefined, preview: '首条消息', expected: '首条消息 (1)' },
  { name: undefined, preview: undefined, expected: undefined },
])('uses the native title without inventing a placeholder: $expected', async ({ name, preview, expected }) => {
  await setup();
  server.summaries.set('A', { name, preview });
  await server.client.listSessions();
  const rename = vi.spyOn(server.client, 'renameSession');
  await forkLatestSession(server.client, 'A', () => true);
  if (expected) expect(rename).toHaveBeenCalledExactlyOnceWith('child', expected);
  else expect(rename).not.toHaveBeenCalled();
});

it('sidebar Fork increments the generated title before the opening preview', async () => {
  await setup();
  server.summaries.set('A', { name: undefined, preview: 'Original question', generated_title: { text: 'Generated title', provenance: { type: 'inherited', source_session: 'origin' } } });
  await server.client.listSessions();
  await forkLatestSession(server.client, 'A', () => true);
  expect(server.summary('child').name).toBe('Generated title (1)');
});

it('settles naming after the sidebar gesture retires once creation has committed', async () => {
  await setup(); server.summaries.set('A', { name: 'Roadmap (1)' });
  server.held.add('session/fork');
  let current = true;
  const work = forkLatestSession(server.client, 'A', () => current);
  const request = await server.waitFor('session/fork', 1);
  const response = server.commit(request);
  current = false; server.socket.deliver(response);
  expect(await work).toEqual({ sessionId: 'child' });
  expect(server.summary('child').name).toBe('Roadmap (2)');
  expect(server.requests.filter(row => row.request.method === 'session/fork')).toHaveLength(1);
});

it.each(['水'.repeat(120), '🙂'.repeat(120), 'x'.repeat(116) + ' (9)'])('reserves native scalar space for a legal fork title: %s', async name => {
  await setup();
  server.handlers.set('session/read', () => {
    return { type: 'session', session: { id: 'A', name, active_node: 'node-A', active_conversation_id: 'conversation-A', node_count: 1, created_at: '0', updated_at: '0' } };
  });
  const rename = vi.spyOn(server.client, 'renameSession').mockImplementation(async (_id, title) => {
    expect(Array.from(title).length).toBeLessThanOrEqual(120);
  });
  expect(await forkLatestSession(server.client, 'A', () => true)).toEqual({ sessionId: 'child' });
  expect(rename).toHaveBeenCalledOnce();
});
it('returns the committed Session identity when automatic naming fails, without repeating creation', async () => {
  await setup();
  server.handlers.set('session/read', () => ({ type: 'session', session: { id: 'A', name: 'Source', active_node: 'node-A', active_conversation_id: 'conversation-A', node_count: 1, created_at: '0', updated_at: '0' } }));
  vi.spyOn(server.client, 'renameSession').mockRejectedValue(new Error('Naming refused'));
  expect(await forkLatestSession(server.client, 'A', () => true)).toEqual({ sessionId: 'child', warning: expect.stringContaining('child was created, but automatic naming failed') });
  expect(server.requests.filter(row => row.request.method === 'session/fork')).toHaveLength(1);
});

it('publishes a post-commit naming warning through the sidebar navigation owner', async () => {
  await setup(); await server.client.attach('A');
  server.handlers.set('session/read', () => ({ type: 'session', session: { id: 'A', name: 'Source', active_node: 'node-A', active_conversation_id: 'conversation-A', node_count: 1, created_at: '0', updated_at: '0' } }));
  vi.spyOn(server.client, 'renameSession').mockRejectedValue(new Error('Naming refused'));
  const navigation = routeFixture(server.client);
  navigation.actor.send({ type: 'OPEN', destination: { kind: 'fork-session', sessionId: 'A' } });
  await vi.waitFor(() => expect(navigation.actor.getSnapshot().context.error).toContain('child was created, but automatic naming failed'));
  expect(navigation.actor.getSnapshot().context.route).toEqual({ kind: 'session', sessionId: 'A' });
  expect(server.requests.filter(row => row.request.method === 'session/fork')).toHaveLength(1);
});

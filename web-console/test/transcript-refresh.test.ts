import { afterEach, expect, it } from 'vitest';
import type { RuntimeClientSnapshot, RuntimeClientTranscriptEntry } from '../../protocol/app-server/v57';
import { AgentReading } from '../src/client/agent-reading';
import { repairTranscript, replaceTranscript } from '../src/client/transcript';
import { Server, snapshot, readingWindow } from './fixture';

const row = (n: number): RuntimeClientTranscriptEntry => ({ cursor: String(n), item: { type: 'message', message: { id: `reply-${n}`, role: 'assistant', content: [{ type: 'text', text: `Answer ${n}` }] } } });
function pending() {
  const entry = row(1);
  entry.response_pending = true;
  entry.tool_calls = [{ committed_result_deferred: false, occurrence: { locator: { type: 'by_message_id', message_id: 'reply-1' }, block_index: 0 }, assembly_complete: true, message_id: 'reply-1', block_index: 0, call_id: 'call-1', tool_id: 'tool-bash', name: 'bash', state: { type: 'assembled', arguments: '{}' } }];
  return entry;
}
function settled() {
  const entry = pending();
  entry.response_pending = false;
  entry.tool_calls![0].state = { type: 'settled', arguments: '{}', result: { status: { type: 'success' }, duration_ms: '30' } };
  return entry;
}
const state = (entries: RuntimeClientTranscriptEntry[]): RuntimeClientSnapshot => ({ ...snapshot(), transcript: { entries, next_cursor: null } });
const result = (entry = settled(), conversation = 'conversation-A') => ({ type: 'transcript_window' as const, window: { ...readingWindow({ entries: [entry] }), cut: { ...readingWindow({}).cut, conversation_id: conversation }, target_cursor: '1' } });
let server: Server, reader: AgentReading | undefined;
afterEach(() => { reader?.dispose(); reader = undefined; server?.client.disconnect(); });
async function setup() {
  server = new Server(); server.snapshots.set('A', state([pending(), row(2)]));
  await server.attached('A'); server.held.add('session/transcript');
}
it('silently rereads an old Tool/response by identity, preserving loaded rows and the live cursor', async () => {
  await setup();
  const epoch = server.client.getSnapshot().views.A.history!.epoch;
  await server.update('A', state([row(2), row(3)]));
  const request = await server.waitFor('session/transcript', 1);
  expect(request.params).toMatchObject({ at: { type: 'message', id: 'reply-1', cut: null }, limit: 1 });
  const during = server.client.getSnapshot().views.A;
  expect(during.history!.page.entries!.map(entry => entry.cursor)).toEqual(['1', '2', '3']);
  expect(during.history!.loading).not.toBe(true); expect(during.history!.error).toBeUndefined();
  server.socket.success(request, result());
  await expect.poll(() => server.client.getSnapshot().views.A.history!.page.entries![0].response_pending).toBe(false);
  const after = server.client.getSnapshot().views.A;
  expect(after.history!.epoch).toBe(epoch); expect(after.history!.page.next_cursor).toBeNull();
  expect(after.history!.page.entries![0].tool_calls![0].state.type).toBe('settled');
  expect(after.history!.page.entries).toHaveLength(3); expect(after.cursor).toBe(during.cursor);
  expect(after.history!.error).toBeUndefined();
});
it('coalesces observations racing a pending reread into one fresh follow-up', async () => {
  await setup(); await server.update('A', state([row(2), row(3)]));
  const first = await server.waitFor('session/transcript', 1);
  await server.update('A', state([row(3), row(4)]));
  await server.update('A', state([row(4), row(5)]));
  expect(server.requests.filter(row => row.request.method === 'session/transcript')).toHaveLength(1);
  server.socket.success(first, result(pending()));
  const second = await server.waitFor('session/transcript', 2);
  server.socket.success(second, result());
  await expect.poll(() => server.client.getSnapshot().views.A.history!.page.entries![0].response_pending).toBe(false);
  expect(server.requests.filter(row => row.request.method === 'session/transcript')).toHaveLength(2);
  expect(server.client.getSnapshot().views.A.history!.page.entries).toHaveLength(5);
});
it('a late repair cannot overwrite a newer navigation', async () => {
  await setup(); await server.update('A', state([row(2), row(3)]));
  const request = await server.waitFor('session/transcript', 1);
  server.client.invalidateReading('A'); const history = server.client.getSnapshot().views.A.history;
  server.socket.success(request, result()); await Promise.resolve(); await Promise.resolve();
  expect(server.client.getSnapshot().views.A.history).toBe(history);
  expect(history!.page.entries!.map(entry => entry.cursor)).toEqual(['1', '2', '3']);
  expect(history!.page.entries![0].response_pending).toBe(true);
});
it('child history uses the same silent identity reread and stops after disposal', async () => {
  server = new Server(); await server.attached('A');
  server.handlers.set('agent/turns', () => ({ type: 'conversation_turns', page: { cut: { ...readingWindow({}).cut, conversation_id: 'child' }, offset: 0, total: '0', turns: [] } }));
  reader = new AgentReading(server.client, server.target('A'), 'worker', 'child', () => true);
  reader.observe({ ...state([pending(), row(2)]), conversation_id: 'child' });
  server.held.add('agent/transcript');
  reader.observe({ ...state([row(2), row(3)]), conversation_id: 'child' });
  const first = await server.waitFor('agent/transcript', 1);
  expect(first.params).toMatchObject({ agent_id: 'worker', at: { type: 'message', id: 'reply-1' }, limit: 1 });
  expect(reader.getSnapshot().history!.page.entries).toHaveLength(3);
  server.socket.success(first, result(settled(), 'child'));
  await expect.poll(() => reader!.getSnapshot().history!.page.entries![0].response_pending).toBe(false);
  expect(reader.getSnapshot().history!.error).toBeUndefined();
  const history = reader.getSnapshot().history; reader.dispose();
  reader.observe({ ...state([row(3), row(4)]), conversation_id: 'child' });
  expect(reader.getSnapshot().history).toBe(history);
});
it('rejects an identity-mismatched native repair without changing cached presentation', () => {
  const cache = replaceTranscript({ entries: [pending(), row(2)] });
  expect(() => repairTranscript(cache, result(row(2)).window, pending())).toThrow('different message');
  expect(cache.page.entries![0].response_pending).toBe(true);
});

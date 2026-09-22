import { afterEach, expect, it } from 'vitest';
import type { RuntimeClientTranscriptEntry } from '../../protocol/app-server/v18';
import { Server, snapshot } from './fixture';
import { prependTranscript, refreshTranscript, replaceTranscript, HISTORY_LIMIT } from '../src/client/transcript';
const entry = (n: number): RuntimeClientTranscriptEntry => ({ cursor: String(n), item: { type: 'message', message: { id: `m${n}`, role: 'assistant', content: [{ type: 'text', text: `Message ${n}` }] } } });
let server: Server;
afterEach(() => server?.client.disconnect());
it('prepends overlapping pages exactly once and retains native numeric order', () => {
  const first = replaceTranscript({ entries: [entry(10), entry(11)], next_cursor: '10' });
  const next = prependTranscript(first, { entries: [entry(9), entry(10)], next_cursor: '9' });
  expect(next.page.entries?.map(item => item.cursor)).toEqual(['9', '10', '11']);
  expect(prependTranscript(next, { entries: [entry(9), entry(10)], next_cursor: '9' })).toEqual(next);
  expect(prependTranscript(next, { entries: [] }).page.next_cursor).toBeUndefined();
});
it('ordinary live refresh retains provable overlap; gaps discard the cache', () => {
  const first = replaceTranscript({ entries: [entry(8), entry(9)], next_cursor: '8' });
  const live = refreshTranscript(first, { entries: [entry(9), entry(10)], next_cursor: '9' });
  expect(live.epoch).toBe(first.epoch);
  expect(live.page.entries?.map(item => item.cursor)).toEqual(['8', '9', '10']);
  const gap = refreshTranscript(live, { entries: [entry(20)], next_cursor: '20' });
  expect(gap.epoch).toBeGreaterThan(live.epoch);
  expect(gap.page.entries).toEqual([entry(20)]);
});
it('retention is finite', () => {
  const full = replaceTranscript({ entries: Array.from({ length: HISTORY_LIMIT }, (_, i) => entry(i + 1)), next_cursor: '1' });
  expect(() => prependTranscript(full, { entries: [entry(0)] })).toThrow('full');
});
it('a live message arriving during older read survives and advances only the live cursor', async () => {
  server = new Server(); server.snapshots.set('A', { ...snapshot(), transcript: { entries: [entry(10)], next_cursor: '10' } });
  await server.attached('A'); server.held.add('session/transcript');
  const initialCursor = server.client.getSnapshot().views.A.cursor;
  const older = server.client.loadEarlier('A'); const request = await server.waitFor('session/transcript', 1);
  expect(request.params).toMatchObject({ before: '10' });
  await server.update('A', { ...snapshot(), transcript: { entries: [entry(10), entry(11)], next_cursor: '10' } });
  const live = server.client.getSnapshot().views.A.cursor;
  server.socket.success(request, { type: 'transcript', page: { entries: [entry(8), entry(9)] } }); await older;
  const view = server.client.getSnapshot().views.A;
  expect(view.cursor).toBe(live);
  expect(live).not.toBe(initialCursor);
  expect(view.history?.page.entries?.map(entry => entry.cursor)).toEqual(['8', '9', '10', '11']);
  await server.client.loadEarlier('A');
  expect(server.requests.filter(item => item.request.method === 'session/transcript')).toHaveLength(1);
});
it('resync fences an older read even when snapshot refresh installs the same content', async () => {
  server = new Server(); server.snapshots.set('A', { ...snapshot(), transcript: { entries: [entry(10)], next_cursor: '10' } });
  await server.attached('A'); server.held.add('session/transcript');
  const older = server.client.loadEarlier('A'); const request = await server.waitFor('session/transcript', 1);
  server.socket.deliver({ jsonrpc: '2.0', method: 'session/resyncRequired', params: { target: server.target('A'), after_cursor: '0', earliest_serviceable: '1' } });
  await server.client.refresh('A');
  server.socket.success(request, { type: 'transcript', page: { entries: [entry(9)] } }); await older;
  expect(server.client.getSnapshot().views.A.history?.page.entries).toEqual([entry(10)]);
});
it('reconnect replaces historical read caches without replay', async () => {
  server = new Server(); server.snapshots.set('A', { ...snapshot(), transcript: { entries: [entry(10)], next_cursor: '10' } });
  await server.attached('A'); server.held.add('session/transcript');
  const older = server.client.loadEarlier('A'); const rejected = expect(older).rejects.toThrow();
  const request = await server.waitFor('session/transcript', 1); const old = server.socket;
  await server.connect(); await rejected;
  old.success(request, { type: 'transcript', page: { entries: [entry(9)] } });
  expect(server.client.getSnapshot().views.A.history?.page.entries).toEqual([entry(10)]);
});
it('reattachment in the same connection rejects the old page and duplicate load gestures coalesce', async () => {
  server = new Server(); server.snapshots.set('A', { ...snapshot(), transcript: { entries: [entry(10)], next_cursor: '10' } });
  await server.attached('A'); server.held.add('session/transcript');
  const older = server.client.loadEarlier('A'); await server.client.loadEarlier('A');
  const request = await server.waitFor('session/transcript', 1);
  await server.client.release('A'); await server.client.attach('A');
  server.socket.success(request, { type: 'transcript', page: { entries: [entry(9)] } }); await older;
  expect(server.client.getSnapshot().views.A.history?.page.entries).toEqual([entry(10)]);
  expect(server.requests.filter(item => item.request.method === 'session/transcript')).toHaveLength(1);
});

it('fresh native Tool projections replace overlaps; unresolved old projections force an authoritative window rebase', () => {
  const call = entry(8);
  call.tool_calls = [{ message_id: 'm8', block_index: 0, call_id: 'native-call', tool_id: 'tool-bash', name: 'bash', state: { type: 'assembled', arguments: '{}' } }];
  const first = replaceTranscript({ entries: [call, entry(9)], next_cursor: '8' });
  const rebased = refreshTranscript(first, { entries: [entry(9), entry(10)], next_cursor: '9' });
  expect(rebased.page.entries).toEqual([entry(9), entry(10)]);
  expect(rebased.epoch).toBeGreaterThan(first.epoch);
  expect(rebased.error).toContain('reread unresolved native responses or Tools');
  const settled = structuredClone(call);
  settled.tool_calls![0].state = { type: 'settled', arguments: '{}', result: { status: { type: 'success' }, duration_ms: 1 } };
  const repaired = refreshTranscript(first, { entries: [settled, entry(9)], next_cursor: '8' });
  expect(repaired.page.entries![0].tool_calls![0].state.type).toBe('settled');
});

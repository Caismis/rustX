import { afterEach, expect, it } from 'vitest';
import { Server, snapshot } from './fixture';
import { prependTrace, refreshTrace, replaceTrace, TRACE_LIMIT } from '../src/client/trace';
import { traceEntry as entry } from './trace-fixture';
let server: Server;
afterEach(() => server?.client.disconnect());
it('prepends overlapping pages exactly once and retains native numeric order', () => {
  const first = replaceTrace({ entries: [entry(10), entry(11)], next_cursor: 'trace:10' });
  const next = prependTrace(first, { entries: [entry(9), entry(10)], next_cursor: 'trace:9' });
  expect(next.page.entries?.map(item => item.id)).toEqual(['trace:9', 'trace:10', 'trace:11']);
  expect(prependTrace(next, { entries: [entry(9), entry(10)], next_cursor: 'trace:9' })).toEqual(next);
  expect(prependTrace(next, { entries: [] }).page.next_cursor).toBeUndefined();
});
it('ordinary live refresh retains provable overlap; gaps discard the cache', () => {
  const first = replaceTrace({ entries: [entry(8), entry(9)], next_cursor: 'trace:8' });
  const live = refreshTrace(first, { entries: [entry(9), entry(10)], next_cursor: 'trace:9' });
  expect(live.epoch).toBe(first.epoch);
  expect(live.page.entries?.map(item => item.id)).toEqual(['trace:8', 'trace:9', 'trace:10']);
  const gap = refreshTrace(live, { entries: [entry(20)], next_cursor: 'trace:20' });
  expect(gap.epoch).toBeGreaterThan(live.epoch);
  expect(gap.page.entries).toEqual([entry(20)]);
});
it('retention is finite', () => {
  const full = replaceTrace({ entries: Array.from({ length: TRACE_LIMIT }, (_, i) => entry(i + 1)), next_cursor: 'trace:1' });
  expect(() => prependTrace(full, { entries: [entry(0)] })).toThrow('full');
});
it('a live message arriving during older read survives and advances only the live cursor', async () => {
  server = new Server(); server.snapshots.set('A', { ...snapshot(), trace: { entries: [entry(10)], next_cursor: 'trace:10' } });
  await server.attached('A'); server.held.add('session/trace');
  const initialCursor = server.client.getSnapshot().views.A.cursor;
  const older = server.client.loadEarlierTrace('A'); const request = await server.waitFor('session/trace', 1);
  expect(request.params).toMatchObject({ before: 'trace:10' });
  await server.update('A', { ...snapshot(), trace: { entries: [entry(10), entry(11)], next_cursor: 'trace:10' } });
  const live = server.client.getSnapshot().views.A.cursor;
  server.socket.success(request, { type: 'trace', page: { entries: [entry(8), entry(9)] } }); await older;
  const view = server.client.getSnapshot().views.A;
  expect(view.cursor).toBe(live);
  expect(live).not.toBe(initialCursor);
  expect(view.trace?.page.entries?.map(entry => entry.id)).toEqual(['trace:8', 'trace:9', 'trace:10', 'trace:11']);
  await server.client.loadEarlierTrace('A');
  expect(server.requests.filter(item => item.request.method === 'session/trace')).toHaveLength(1);
});
it('resync fences an older read even when snapshot refresh installs the same content', async () => {
  server = new Server(); server.snapshots.set('A', { ...snapshot(), trace: { entries: [entry(10)], next_cursor: 'trace:10' } });
  await server.attached('A'); server.held.add('session/trace');
  const older = server.client.loadEarlierTrace('A'); const request = await server.waitFor('session/trace', 1);
  server.socket.deliver({ jsonrpc: '2.0', method: 'session/resyncRequired', params: { target: server.target('A'), after_cursor: '0', earliest_serviceable: '1' } });
  await server.client.refresh('A');
  server.socket.success(request, { type: 'trace', page: { entries: [entry(9)] } }); await older;
  expect(server.client.getSnapshot().views.A.trace?.page.entries).toEqual([entry(10)]);
});
it('reconnect replaces historical read caches without replay', async () => {
  server = new Server(); server.snapshots.set('A', { ...snapshot(), trace: { entries: [entry(10)], next_cursor: 'trace:10' } });
  await server.attached('A'); server.held.add('session/trace');
  const older = server.client.loadEarlierTrace('A'); const rejected = expect(older).rejects.toThrow();
  const request = await server.waitFor('session/trace', 1); const old = server.socket;
  await server.connect(); await rejected;
  old.success(request, { type: 'trace', page: { entries: [entry(9)] } });
  expect(server.client.getSnapshot().views.A.trace?.page.entries).toEqual([entry(10)]);
});
it('reattachment in the same connection rejects the old page and duplicate load gestures coalesce', async () => {
  server = new Server(); server.snapshots.set('A', { ...snapshot(), trace: { entries: [entry(10)], next_cursor: 'trace:10' } });
  await server.attached('A'); server.held.add('session/trace');
  const older = server.client.loadEarlierTrace('A'); await server.client.loadEarlierTrace('A');
  const request = await server.waitFor('session/trace', 1);
  await server.client.release('A', false); await server.client.attach('A');
  server.socket.success(request, { type: 'trace', page: { entries: [entry(9)] } }); await older;
  expect(server.client.getSnapshot().views.A.trace?.page.entries).toEqual([entry(10)]);
  expect(server.requests.filter(item => item.request.method === 'session/trace')).toHaveLength(1);
});

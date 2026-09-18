import { afterEach, expect, it } from 'vitest';
import { Server, snapshot } from './fixture';
import { prependTrace, refreshTrace, replaceTrace, selectTrace, traceInterests, TRACE_LIMIT } from '../src/client/trace';
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
it('overlapping refresh preserves history; a disconnected tail establishes a new interval', () => {
  const first = replaceTrace({ entries: [entry(8), entry(9)], next_cursor: 'trace:8' });
  const live = refreshTrace(first, { entries: [entry(9), entry(10)], next_cursor: 'trace:9' });
  expect(live.epoch).toBe(first.epoch);
  expect(live.page.entries?.map(item => item.id)).toEqual(['trace:8', 'trace:9', 'trace:10']);
  const gap = refreshTrace(live, { entries: [entry(20)], next_cursor: 'trace:20' });
  expect(gap.epoch).toBeGreaterThan(live.epoch);
  expect(gap.page.next_cursor).toBe('trace:20');
  expect(gap.page.entries.map(item => item.id)).toEqual(['trace:20']);
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
  await server.client.release('A'); await server.client.attach('A');
  server.socket.success(request, { type: 'trace', page: { entries: [entry(9)] } }); await older;
  expect(server.client.getSnapshot().views.A.trace?.page.entries).toEqual([entry(10)]);
  expect(server.requests.filter(item => item.request.method === 'session/trace')).toHaveLength(1);
});

it('native lifecycle patches repair a selected old record separately from rebased history', () => {
  const old = { ...entry(1), state: 'running' as const };
  const first = selectTrace(replaceTrace({ entries: [old, entry(2)], next_cursor: 'trace:1' }), old.id);
  const updated = refreshTrace(first, { entries: [entry(100)] }, [
    { id: old.id, state: 'completed', artifacts: [], truncated: false, timing: { ...old.timing, duration_ms: '123' } },
  ]);
  expect(updated.epoch).toBeGreaterThan(first.epoch);
  expect(updated.page.next_cursor).toBeUndefined();
  expect(updated.page.entries.map(item => item.id)).toEqual(['trace:100']);
  expect(updated.selection).toMatchObject({ state: 'completed', timing: { ...old.timing, duration_ms: '123' } });
});

it('paging completion repairs newly loaded interests after a terminal notification raced the page', async () => {
  server = new Server();
  server.snapshots.set('A', { ...snapshot(), trace: { entries: [entry(100)], next_cursor: 'trace:100' } });
  await server.attached('A'); server.held.add('session/trace');
  const older = server.client.loadEarlierTrace('A');
  const request = await server.waitFor('session/trace', 1);
  const old = entry(1, { state: 'running' });
  await server.update('A', { ...snapshot(), trace: { entries: [entry(100), entry(101)], next_cursor: 'trace:100' }, trace_updates: [{
    id: old.id, state: 'completed', timing: old.timing, artifacts: [], truncated: false,
  }] });
  server.socket.success(request, { type: 'trace', page: { entries: [old] } });
  await older;
  expect(server.client.getSnapshot().views.A.trace?.page.entries[0]).toMatchObject({ id: old.id, state: 'completed' });
  const snapshots = server.requests.filter(item => item.request.method === 'session/snapshot');
  expect(snapshots.at(-1)?.request.params).toMatchObject({ trace_records: ['trace:1', 'trace:100', 'trace:101'] });
});

it('newly revealed prefix rows keep server order around stable overlap anchors', () => {
  const cache = replaceTrace({ entries: [entry(1), entry(2), entry(100)], next_cursor: 'trace:1' });
  const refreshed = refreshTrace(cache, { entries: [entry(98), entry(99), entry(100), entry(101)] });
  expect(refreshed.page.entries.map(row => row.id)).toEqual(['trace:1', 'trace:2', 'trace:98', 'trace:99', 'trace:100', 'trace:101']);
  expect(refreshed.epoch).toBe(cache.epoch);
});

it('the first repaired tail restores older paging in a new resync epoch', () => {
  const old = replaceTrace({ entries: [entry(1)], next_cursor: 'trace:1' });
  const empty = replaceTrace({ entries: [], next_cursor: null }, old);
  const repaired = refreshTrace(empty, { entries: [entry(100)], next_cursor: 'trace:100' });
  expect(repaired.epoch).toBe(empty.epoch);
  expect(repaired.epoch).toBeGreaterThan(old.epoch);
  expect(repaired.page.next_cursor).toBe('trace:100');
});

it('forty new anchors rebase history, retain one exact selection, and bound interests', () => {
  const old = entry(4, { state: 'running' });
  const cache = selectTrace(replaceTrace({ entries: [old, entry(8), entry(9), entry(10)], next_cursor: 'before-four' }), old.id);
  const committed = Array.from({ length: 40 }, (_, index) => entry(11 + index));
  const tail = { entries: committed.slice(-32), next_cursor: 'before-nineteen' };
  const next = refreshTrace(cache, tail);
  expect(next.page).toEqual(tail);
  expect(next.epoch).toBeGreaterThan(cache.epoch);
  expect(next.selection).toEqual(old);
  expect(traceInterests(next)[0]).toBe(old.position);
  const settled = refreshTrace(next, tail, [{ id: old.id, state: 'completed', timing: old.timing, artifacts: [], truncated: false }]);
  expect(settled.selection).toMatchObject({ id: old.id, state: 'completed' });
  expect(settled.page.entries).toEqual(tail.entries);
  expect(settled.page.entries.some(row => row.id === old.id)).toBe(false);
  expect(traceInterests({ ...settled, page: { entries: Array.from({ length: TRACE_LIMIT }, (_, i) => entry(i + 100)) } })).toHaveLength(TRACE_LIMIT);
});
it('non-overlap rebase fences a pending older page and uses the new interval cursor', async () => {
  server = new Server(); server.snapshots.set('A', { ...snapshot(), trace: { entries: [entry(8), entry(9), entry(10)], next_cursor: 'before-eight' } });
  await server.attached('A'); server.held.add('session/trace');
  server.client.selectTrace('A', 'trace:8');
  const older = server.client.loadEarlierTrace('A'); const request = await server.waitFor('session/trace', 1);
  await server.update('A', { ...snapshot(), trace: { entries: [entry(20), entry(21), entry(22)], next_cursor: 'before-twenty' } });
  server.socket.success(request, { type: 'trace', page: { entries: [entry(7)] } }); await older;
  expect(server.client.getSnapshot().views.A.trace?.page.entries.map(row => row.id)).toEqual(['trace:20', 'trace:21', 'trace:22']);
  expect(server.client.getSnapshot().views.A.trace?.selection?.id).toBe('trace:8');
  const next = server.client.loadEarlierTrace('A'); const nextRequest = await server.waitFor('session/trace', 2);
  expect(nextRequest.params).toMatchObject({ before: 'before-twenty' });
  server.socket.success(nextRequest, { type: 'trace', page: { entries: [entry(19)] } }); await next;
  expect(server.client.getSnapshot().views.A.trace?.page.entries.map(row => row.id)).toEqual(['trace:19', 'trace:20', 'trace:21', 'trace:22']);
});

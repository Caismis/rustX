import { afterEach, expect, it } from 'vitest';
import { Server, snapshot } from './fixture';
import { beginTraceDetail, completeTraceDetail, prependTrace, refreshTrace, replaceTrace, selectTrace, traceInterests, TRACE_DETAIL_LIMIT, TRACE_LIMIT } from '../src/client/trace';
import { requestDetail as detail, traceRecord as entry } from './trace-fixture';
let server: Server;
afterEach(() => server?.client.disconnect());
it('prepends overlapping pages exactly once and retains native numeric order', () => {
  const first = replaceTrace({ records: [entry(10), entry(11)], next_cursor: 'trace:10' });
  const next = prependTrace(first, { records: [entry(9), entry(10)], next_cursor: 'trace:9' });
  expect(next.page.records?.map(item => item.id)).toEqual(['trace:9', 'trace:10', 'trace:11']);
  expect(prependTrace(next, { records: [entry(9), entry(10)], next_cursor: 'trace:9' })).toEqual(next);
  expect(prependTrace(next, { records: [] }).page.next_cursor).toBeUndefined();
});
it('overlapping refresh preserves history; a disconnected tail establishes a new interval', () => {
  const first = replaceTrace({ records: [entry(8), entry(9)], next_cursor: 'trace:8' });
  const live = refreshTrace(first, { records: [entry(9), entry(10)], next_cursor: 'trace:9' });
  expect(live.epoch).toBe(first.epoch);
  expect(live.page.records?.map(item => item.id)).toEqual(['trace:8', 'trace:9', 'trace:10']);
  const gap = refreshTrace(live, { records: [entry(20)], next_cursor: 'trace:20' });
  expect(gap.epoch).toBeGreaterThan(live.epoch);
  expect(gap.page.next_cursor).toBe('trace:20');
  expect(gap.page.records.map(item => item.id)).toEqual(['trace:20']);
});
it('retention is finite', () => {
  const full = replaceTrace({ records: Array.from({ length: TRACE_LIMIT }, (_, i) => entry(i + 1)), next_cursor: 'trace:1' });
  expect(() => prependTrace(full, { records: [entry(0)] })).toThrow('full');
});
it('a live message arriving during older read survives and advances only the live cursor', async () => {
  server = new Server(); server.snapshots.set('A', { ...snapshot(), trace: { records: [entry(10)], next_cursor: 'trace:10' } });
  await server.attached('A'); server.held.add('session/trace');
  const initialCursor = server.client.getSnapshot().views.A.cursor;
  const older = server.client.loadEarlierTrace('A'); const request = await server.waitFor('session/trace', 1);
  expect(request.params).toMatchObject({ before: 'trace:10' });
  await server.update('A', { ...snapshot(), trace: { records: [entry(10), entry(11)], next_cursor: 'trace:10' } });
  const live = server.client.getSnapshot().views.A.cursor;
  server.socket.success(request, { type: 'trace', page: { records: [entry(8), entry(9)] } }); await older;
  const view = server.client.getSnapshot().views.A;
  expect(view.cursor).toBe(live);
  expect(live).not.toBe(initialCursor);
  expect(view.trace?.page.records?.map(entry => entry.id)).toEqual(['trace:8', 'trace:9', 'trace:10', 'trace:11']);
  await server.client.loadEarlierTrace('A');
  expect(server.requests.filter(item => item.request.method === 'session/trace')).toHaveLength(1);
});
it('resync fences an older read even when snapshot refresh installs the same content', async () => {
  server = new Server(); server.snapshots.set('A', { ...snapshot(), trace: { records: [entry(10)], next_cursor: 'trace:10' } });
  await server.attached('A'); server.held.add('session/trace');
  const older = server.client.loadEarlierTrace('A'); const request = await server.waitFor('session/trace', 1);
  server.socket.deliver({ jsonrpc: '2.0', method: 'session/resyncRequired', params: { target: server.target('A'), after_cursor: '0', earliest_serviceable: '1' } });
  await server.client.refresh('A');
  server.socket.success(request, { type: 'trace', page: { records: [entry(9)] } }); await older;
  expect(server.client.getSnapshot().views.A.trace?.page.records).toEqual([entry(10)]);
});
it('reconnect replaces historical read caches without replay', async () => {
  server = new Server(); server.snapshots.set('A', { ...snapshot(), trace: { records: [entry(10)], next_cursor: 'trace:10' } });
  await server.attached('A'); server.held.add('session/trace');
  const older = server.client.loadEarlierTrace('A'); const rejected = expect(older).rejects.toThrow();
  const request = await server.waitFor('session/trace', 1); const old = server.socket;
  await server.connect(); await rejected;
  old.success(request, { type: 'trace', page: { records: [entry(9)] } });
  expect(server.client.getSnapshot().views.A.trace?.page.records).toEqual([entry(10)]);
});
it('reattachment in the same connection rejects the old page and duplicate load gestures coalesce', async () => {
  server = new Server(); server.snapshots.set('A', { ...snapshot(), trace: { records: [entry(10)], next_cursor: 'trace:10' } });
  await server.attached('A'); server.held.add('session/trace');
  const older = server.client.loadEarlierTrace('A'); await server.client.loadEarlierTrace('A');
  const request = await server.waitFor('session/trace', 1);
  await server.client.release('A'); await server.client.attach('A');
  server.socket.success(request, { type: 'trace', page: { records: [entry(9)] } }); await older;
  expect(server.client.getSnapshot().views.A.trace?.page.records).toEqual([entry(10)]);
  expect(server.requests.filter(item => item.request.method === 'session/trace')).toHaveLength(1);
});

it('native lifecycle patches repair a selected old record separately from rebased history', () => {
  const old = { ...entry(1), state: 'running' as const };
  const first = selectTrace(replaceTrace({ records: [old, entry(2)], next_cursor: 'trace:1' }), old.id);
  const updated = refreshTrace(first, { records: [entry(100)] }, [
    { id: old.id, state: 'completed', attachments: [], truncated: false, timing: { ...old.timing, duration_ms: '123' } },
  ]);
  expect(updated.epoch).toBeGreaterThan(first.epoch);
  expect(updated.page.next_cursor).toBeUndefined();
  expect(updated.page.records.map(item => item.id)).toEqual(['trace:100']);
  expect(updated.selection).toMatchObject({ state: 'completed', timing: { ...old.timing, duration_ms: '123' } });
});

it('paging completion repairs newly loaded interests after a terminal notification raced the page', async () => {
  server = new Server();
  server.snapshots.set('A', { ...snapshot(), trace: { records: [entry(100)], next_cursor: 'trace:100' } });
  await server.attached('A'); server.held.add('session/trace');
  const older = server.client.loadEarlierTrace('A');
  const request = await server.waitFor('session/trace', 1);
  const old = entry(1, { state: 'running' });
  await server.update('A', { ...snapshot(), trace: { records: [entry(100), entry(101)], next_cursor: 'trace:100' }, trace_updates: [{
    id: old.id, state: 'completed', timing: old.timing, attachments: [], truncated: false,
  }] });
  server.socket.success(request, { type: 'trace', page: { records: [old] } });
  await older;
  expect(server.client.getSnapshot().views.A.trace?.page.records[0]).toMatchObject({ id: old.id, state: 'completed' });
  const snapshots = server.requests.filter(item => item.request.method === 'session/snapshot');
  expect(snapshots.at(-1)?.request.params).toMatchObject({ trace_records: ['trace:1', 'trace:100', 'trace:101'] });
});

it('newly revealed prefix rows keep server order around stable overlap anchors', () => {
  const cache = replaceTrace({ records: [entry(1), entry(2), entry(100)], next_cursor: 'trace:1' });
  const refreshed = refreshTrace(cache, { records: [entry(98), entry(99), entry(100), entry(101)] });
  expect(refreshed.page.records.map(row => row.id)).toEqual(['trace:1', 'trace:2', 'trace:98', 'trace:99', 'trace:100', 'trace:101']);
  expect(refreshed.epoch).toBe(cache.epoch);
});

it('the first repaired tail restores older paging in a new resync epoch', () => {
  const old = replaceTrace({ records: [entry(1)], next_cursor: 'trace:1' });
  const empty = replaceTrace({ records: [], next_cursor: null }, old);
  const repaired = refreshTrace(empty, { records: [entry(100)], next_cursor: 'trace:100' });
  expect(repaired.epoch).toBe(empty.epoch);
  expect(repaired.epoch).toBeGreaterThan(old.epoch);
  expect(repaired.page.next_cursor).toBe('trace:100');
});

it('forty new anchors rebase history, retain one exact selection, and bound interests', () => {
  const old = entry(4, { state: 'running' });
  const cache = selectTrace(replaceTrace({ records: [old, entry(8), entry(9), entry(10)], next_cursor: 'before-four' }), old.id);
  const committed = Array.from({ length: 40 }, (_, index) => entry(11 + index));
  const tail = { records: committed.slice(-32), next_cursor: 'before-nineteen' };
  const next = refreshTrace(cache, tail);
  expect(next.page).toEqual(tail);
  expect(next.epoch).toBeGreaterThan(cache.epoch);
  expect(next.selection).toEqual(old);
  expect(traceInterests(next)[0]).toBe(old.position);
  const settled = refreshTrace(next, tail, [{ id: old.id, state: 'completed', timing: old.timing, attachments: [], truncated: false }]);
  expect(settled.selection).toMatchObject({ id: old.id, state: 'completed' });
  expect(settled.page.records).toEqual(tail.records);
  expect(settled.page.records.some(row => row.id === old.id)).toBe(false);
  expect(traceInterests({ ...settled, page: { records: Array.from({ length: TRACE_LIMIT }, (_, i) => entry(i + 100)) } })).toHaveLength(TRACE_LIMIT);
});
it('non-overlap rebase fences a pending older page and uses the new interval cursor', async () => {
  server = new Server(); server.snapshots.set('A', { ...snapshot(), trace: { records: [entry(8), entry(9), entry(10)], next_cursor: 'before-eight' } });
  await server.attached('A'); server.held.add('session/trace');
  server.client.selectTrace('A', 'trace:8');
  const older = server.client.loadEarlierTrace('A'); const request = await server.waitFor('session/trace', 1);
  await server.update('A', { ...snapshot(), trace: { records: [entry(20), entry(21), entry(22)], next_cursor: 'before-twenty' } });
  server.socket.success(request, { type: 'trace', page: { records: [entry(7)] } }); await older;
  expect(server.client.getSnapshot().views.A.trace?.page.records.map(row => row.id)).toEqual(['trace:20', 'trace:21', 'trace:22']);
  expect(server.client.getSnapshot().views.A.trace?.selection?.id).toBe('trace:8');
  const next = server.client.loadEarlierTrace('A'); const nextRequest = await server.waitFor('session/trace', 2);
  expect(nextRequest.params).toMatchObject({ before: 'before-twenty' });
  server.socket.success(nextRequest, { type: 'trace', page: { records: [entry(19)] } }); await next;
  expect(server.client.getSnapshot().views.A.trace?.page.records.map(row => row.id)).toEqual(['trace:19', 'trace:20', 'trace:21', 'trace:22']);
});

it('a detail reply is fenced by the epoch it was requested in', () => {
  const cache = selectTrace(replaceTrace({ records: [entry(1)], next_cursor: null }), 'trace:1');
  const requested = beginTraceDetail(cache, 'trace:1');
  expect(requested.details['trace:1']).toMatchObject({ epoch: cache.epoch, loading: true });
  // A rebase onto a disconnected tail starts a new interval, and therefore a
  // new epoch. The in-flight detail reply describes the interval that is gone.
  const rebased = refreshTrace(requested, { records: [entry(50)], next_cursor: null });
  expect(rebased.epoch).toBeGreaterThan(cache.epoch);
  const stale = completeTraceDetail(rebased, 'trace:1', cache.epoch, detail(1));
  expect(stale).toBe(rebased);
  const current = completeTraceDetail(rebased, 'trace:1', rebased.epoch, detail(1));
  expect(current.details['trace:1']?.detail).toEqual(detail(1));
});

it('detail retention is finite and never evicts the selected record', () => {
  let cache = replaceTrace({
    records: Array.from({ length: 20 }, (_, index) => entry(index)),
    next_cursor: null,
  });
  cache = selectTrace(cache, 'trace:0');
  for (let index = 0; index < 20; index += 1) {
    cache = completeTraceDetail(beginTraceDetail(cache, `trace:${index}`), `trace:${index}`, cache.epoch, detail(index));
  }
  expect(Object.keys(cache.details).length).toBeLessThanOrEqual(TRACE_DETAIL_LIMIT);
  expect(cache.details['trace:0']).toBeDefined();
});

it('a detail read is issued once per record and fetches on demand only', async () => {
  server = new Server();
  server.snapshots.set('A', { ...snapshot(), trace: { records: [entry(10)], next_cursor: null } });
  await server.attached('A');
  server.traceDetails.set('trace:10', detail(10));
  expect(server.requests.filter(item => item.request.method === 'session/traceDetail')).toHaveLength(0);
  await server.client.loadTraceDetail('A', 'trace:10');
  await server.client.loadTraceDetail('A', 'trace:10');
  expect(server.requests.filter(item => item.request.method === 'session/traceDetail')).toHaveLength(1);
  expect(server.client.getSnapshot().views.A.trace?.details['trace:10']?.detail).toBeDefined();
});

it('a detail reply cannot attach to a different attachment target', async () => {
  server = new Server();
  server.snapshots.set('A', { ...snapshot(), trace: { records: [entry(10)], next_cursor: null } });
  await server.attached('A');
  server.held.add('session/traceDetail');
  const pending = server.client.loadTraceDetail('A', 'trace:10');
  const request = await server.waitFor('session/traceDetail', 1);
  await server.client.release('A');
  await server.client.attach('A');
  server.socket.success(request, { type: 'trace_detail', detail: detail(10) });
  await pending;
  expect(server.client.getSnapshot().views.A.trace?.details['trace:10']?.detail).toBeUndefined();
});

it('a lifecycle repair invalidates cached detail and fences a pending older read', async () => {
  server = new Server();
  const running = entry(1, { state: 'running' });
  server.snapshots.set('A', { ...snapshot(), trace: { records: [running] } });
  await server.attached('A');
  server.held.add('session/traceDetail');
  const pending = server.client.loadTraceDetail('A', running.id);
  const request = await server.waitFor('session/traceDetail', 1);
  await server.update('A', { ...snapshot(), trace: { records: [entry(1)] } });
  server.socket.success(request, { type: 'trace_detail', detail: detail(1) });
  await pending;
  expect(server.client.getSnapshot().views.A.trace?.details[running.id]).toBeUndefined();
  const cached = completeTraceDetail(replaceTrace({ records: [running] }), running.id, 1, detail(1));
  expect(refreshTrace(cached, { records: [entry(1)] }).details[running.id]).toBeUndefined();
});

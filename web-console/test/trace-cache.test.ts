import{traceReceipt}from'./fixture';
import { afterEach, expect, it } from 'vitest';
import { Server, snapshot,checkpoint } from './fixture';
import { beginTraceDetail, completeTraceDetail, fillTraceGap, prependTrace, refreshTrace, replaceTrace, selectTrace,traceInterestBatches, traceInterests, TRACE_DETAIL_LIMIT, TRACE_DETAIL_MAX_BYTES } from '../src/client/trace';
import { requestDetail as detail, traceRecord as entry } from './trace-fixture';
const rootCut=()=>checkpoint(snapshot()).read_cut;
let server: Server;
afterEach(() => server?.client.disconnect());
it('prepends overlapping pages exactly once and retains native numeric order', () => {
  const first = replaceTrace({ records: [entry(10), entry(11)], next_cursor: 'trace:10' });
  const next = prependTrace(first, { records: [entry(9), entry(10)], next_cursor: 'trace:9' });
  expect(next.page.records?.map(item => item.id)).toEqual(['trace:9', 'trace:10', 'trace:11']);
  expect(prependTrace(next, { records: [entry(9), entry(10)], next_cursor: 'trace:9' })).toEqual(next);
  expect(prependTrace(next, { records: [] }).page.next_cursor).toBeUndefined();
});
it('live refresh preserves loaded history and represents disjoint tails with native paging boundaries', () => {
  const first = replaceTrace({ records: [entry(8), entry(9)], next_cursor: 'trace:8' });
  const live = refreshTrace(first, { records: [entry(9), entry(10)], next_cursor: 'trace:9' });
  expect(live.epoch).toBe(first.epoch);
  expect(live.page.records?.map(item => item.id)).toEqual(['trace:8', 'trace:9', 'trace:10']);
  const gap = refreshTrace(live, { records: [entry(20)], next_cursor: 'trace:20' });
  expect(gap.epoch).toBe(live.epoch);
  expect(gap.page.next_cursor).toBe('trace:8');
  expect(gap.page.records.map(item => item.id)).toEqual(['trace:8', 'trace:9', 'trace:10', 'trace:20']);
  expect(gap.gaps).toEqual([{ after: 'trace:10', before: 'trace:20', cursor: 'trace:20' }]);
});
it('retains previously loaded history beyond one transport window', () => {
  const full = replaceTrace({ records: Array.from({ length: 1024 }, (_, i) => entry(i + 1)), next_cursor: 'trace:1' });
  const next = prependTrace(full, { records: [entry(0)] });
  expect(next.page.records).toHaveLength(1025);
  expect(next.epoch).toBe(full.epoch);
  expect(refreshTrace(next, { records: [entry(1024), entry(1025)] }).page.records).toHaveLength(1026);
});
it('rejects an opaque native continuation cycle without discarding the loaded ledger', async () => {
  server = new Server();
  server.snapshots.set('A', { ...snapshot(), trace: { records: [entry(100)], next_cursor: 'before-a' } });
  await server.attached('A');
  const previous = server.client.getSnapshot().views.A.trace!.page;
  server.handlers.set('session/trace', request => {
    if (request.method !== 'session/trace') throw Error('Expected Trace page');
    return { type: 'trace', page: { read_cut: rootCut(), records: [entry(request.params.before === 'before-a' ? 90 : 80)], next_cursor: request.params.before === 'before-a' ? 'before-b' : 'before-a' } };
  });
  await expect(server.client.loadEarlierTrace('A')).rejects.toThrow('did not advance');
  expect(server.client.getSnapshot().views.A.trace!.page).toBe(previous);
  expect(server.requests.filter(row => row.request.method === 'session/trace')).toHaveLength(2);
});
it('a live message arriving during older read survives and advances only the live cursor', async () => {
  server = new Server(); server.snapshots.set('A', { ...snapshot(), trace: { records: [entry(10)], next_cursor: 'trace:10' } });
  await server.attached('A'); server.held.add('session/trace');
  const initialCursor = server.client.getSnapshot().views.A.cursor;
  const older = server.client.loadEarlierTrace('A'); const request = await server.waitFor('session/trace', 1);
  expect(request.params).toMatchObject({ before: 'trace:10' });
  server.socket.deliver({ jsonrpc: '2.0', method: 'session/event', params: { target: server.target('A'), cursor: '1', event: { type: 'message_committed', attempt_id: 'attempt', message: { id: 'live-message', role: 'user', source: 'human', content: [] }, transcript_cursor: '1' } } });
  const live = server.client.getSnapshot().views.A.cursor;
  server.socket.success(request, { type: 'trace', page: {read_cut:rootCut(), records: [entry(8), entry(9)] } });
  const repair = await server.waitFor('session/trace', 2);
  server.socket.success(repair, { type: 'trace', page: {read_cut:rootCut(), records: [entry(10), entry(11)], next_cursor: 'trace:10' } }); await older;
  const view = server.client.getSnapshot().views.A;
  expect(view.cursor).toBe(live);
  expect(view.snapshot?.messages[0]?.id).toBe('live-message');
  expect(live).not.toBe(initialCursor);
  expect(view.trace?.page.records?.map(entry => entry.id)).toEqual(['trace:8', 'trace:9', 'trace:10', 'trace:11']);
  await server.client.loadEarlierTrace('A');
  expect(server.requests.filter(item => item.request.method === 'session/trace')).toHaveLength(2);
});
it('resync fences an older read even when snapshot refresh installs the same content', async () => {
  server = new Server(); server.snapshots.set('A', { ...snapshot(), trace: { records: [entry(10)], next_cursor: 'trace:10' } });
  await server.attached('A'); server.held.add('session/trace');
  const older = server.client.loadEarlierTrace('A'); const request = await server.waitFor('session/trace', 1);
  server.socket.deliver({ jsonrpc: '2.0', method: 'session/resyncRequired', params: { target: server.target('A'), after_cursor: '0', earliest_serviceable: '1' } });
  await server.client.refresh('A');
  server.socket.success(request, { type: 'trace', page: {read_cut:rootCut(), records: [entry(9)] } }); await older;
  expect(server.client.getSnapshot().views.A.trace?.page.records).toEqual([entry(10)]);
});
it('reconnect replaces historical read caches without replay', async () => {
  server = new Server(); server.snapshots.set('A', { ...snapshot(), trace: { records: [entry(10)], next_cursor: 'trace:10' } });
  await server.attached('A'); server.held.add('session/trace');
  const older = server.client.loadEarlierTrace('A'); const rejected = expect(older).rejects.toThrow();
  const request = await server.waitFor('session/trace', 1); const old = server.socket;
  await server.connect(); await rejected;
  old.success(request, { type: 'trace', page: {read_cut:rootCut(), records: [entry(9)] } });
  expect(server.client.getSnapshot().views.A.trace?.page.records).toEqual([entry(10)]);
});
it('reattachment in the same connection rejects the old page and duplicate load gestures coalesce', async () => {
  server = new Server(); server.snapshots.set('A', { ...snapshot(), trace: { records: [entry(10)], next_cursor: 'trace:10' } });
  await server.attached('A'); server.held.add('session/trace');
  const older = server.client.loadEarlierTrace('A'); await server.client.loadEarlierTrace('A');
  const request = await server.waitFor('session/trace', 1);
  await server.client.release('A'); await server.client.attach('A');
  server.socket.success(request, { type: 'trace', page: {read_cut:rootCut(), records: [entry(9)] } }); await older;
  expect(server.client.getSnapshot().views.A.trace?.page.records).toEqual([entry(10)]);
  expect(server.requests.filter(item => item.request.method === 'session/trace')).toHaveLength(1);
});

it('native lifecycle patches repair selected history alongside a disjoint live tail', () => {
  const old = { ...entry(1), state: 'running' as const };
  const first = selectTrace(replaceTrace({ records: [old, entry(2)], next_cursor: 'trace:1' }), old.id);
  const updated = refreshTrace(first, { records: [entry(100)] }, [
    { fields:old.fields,collections:old.collections,id: old.id, location: old.location, state: 'completed', attachments: [], truncated: false, timing: { ...old.timing, duration_ms: '123' } },
  ]);
  expect(updated.epoch).toBe(first.epoch);
  expect(updated.page.next_cursor).toBe('trace:1');
  expect(updated.page.records.map(item => item.id)).toEqual(['trace:1', 'trace:2', 'trace:100']);
  expect(updated.selection).toMatchObject({ state: 'completed', timing: { ...old.timing, duration_ms: '123' } });
});

it('paging completion repairs newly loaded interests after a terminal notification raced the page', async () => {
  server = new Server();
  server.snapshots.set('A', { ...snapshot(), trace: { records: [entry(100)], next_cursor: 'trace:100' } });
  await server.attached('A'); server.held.add('session/trace');
  const older = server.client.loadEarlierTrace('A');
  const request = await server.waitFor('session/trace', 1);
  const old = entry(1, { state: 'running' });
  server.socket.deliver({ jsonrpc: '2.0', method: 'session/event', params: { target: server.target('A'), cursor: '1', event: { type: 'trace_changed' } } });
  const live = await server.waitFor('session/trace', 2);
  const liveWork = server.client['traces']['traceReads'].get('A')!.work;
  server.socket.success(live, { type: 'trace', page: {read_cut:rootCut(), records: [entry(100), entry(101)], next_cursor: 'trace:100', updates: [{
    fields:old.fields,collections:old.collections,id: old.id, location: old.location, state: 'completed', timing: old.timing, attachments: [], truncated: false,
  }] } });
  await liveWork;
  server.socket.success(request, { type: 'trace', page: {read_cut:rootCut(), records: [old] } });
  const repair = await server.waitFor('session/trace', 3);
  expect(repair.params).toMatchObject({ records: ['trace:1', 'trace:100', 'trace:101'] });
  server.socket.success(repair, { type: 'trace', page: {read_cut:rootCut(), records: [entry(100), entry(101)], updates: [{ fields:old.fields,collections:old.collections,id: old.id, location: old.location, state: 'completed', timing: old.timing, attachments: [], truncated: false }] } });
  await older;
  expect(server.client.getSnapshot().views.A.trace?.page.records[0]).toMatchObject({ id: old.id, state: 'completed' });
  expect(server.requests.filter(item => item.request.method === 'session/checkpoint')).toHaveLength(1);

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

it('forty new anchors retain history, exact selection, and bounded interest batches', () => {
  const old = entry(4, { state: 'running' });
  const cache = selectTrace(replaceTrace({ records: [old, entry(8), entry(9), entry(10)], next_cursor: 'before-four' }), old.id);
  const committed = Array.from({ length: 40 }, (_, index) => entry(11 + index));
  const tail = { records: committed.slice(-32), next_cursor: 'before-nineteen' };
  const next = refreshTrace(cache, tail);
  expect(next.page.records).toEqual([...cache.page.records, ...tail.records]);
  expect(next.gaps).toEqual([{ after: 'trace:10', before: 'trace:19', cursor: 'before-nineteen' }]);
  expect(next.epoch).toBe(cache.epoch);
  expect(next.selection).toEqual(old);
  expect(traceInterests(next)[0]).toBe(old.position);
  const settled = refreshTrace(next, tail, [{ fields:old.fields,collections:old.collections,id: old.id, location: old.location, state: 'completed', timing: old.timing, attachments: [], truncated: false }]);
  expect(settled.selection).toMatchObject({ id: old.id, state: 'completed' });
  expect(settled.page.records.slice(-32)).toEqual(tail.records);
  expect(settled.page.records[0]).toMatchObject({id:old.id,state:'completed'});
  const batches=traceInterestBatches({ ...settled, page: { records: Array.from({ length: 1024 }, (_, i) => entry(i + 100)) } });
  expect(batches.flat()).toHaveLength(1025);
  expect(batches.every(batch=>batch.length<=8)).toBe(true);
});
it('disjoint appends preserve an in-flight older read and its native continuation', async () => {
  server = new Server(); server.snapshots.set('A', { ...snapshot(), trace: { records: [entry(8), entry(9), entry(10)], next_cursor: 'before-eight' } });
  await server.attached('A'); server.held.add('session/trace');
  server.client.selectTrace('A', 'trace:8');
  const older = server.client.loadEarlierTrace('A'); const request = await server.waitFor('session/trace', 1);
  await server.update('A', { ...snapshot(), trace: { records: [entry(20), entry(21), entry(22)], next_cursor: 'before-twenty' } });
  server.socket.success(request, { type: 'trace', page: {read_cut:rootCut(), records: [entry(7)] } });
  const repair = await server.waitFor('session/trace', 2);
  server.socket.success(repair, { type: 'trace', page: {read_cut:rootCut(), records: [entry(20), entry(21), entry(22)], next_cursor: 'before-twenty' } }); await older;
  const trace = server.client.getSnapshot().views.A.trace!;
  expect(trace.page.records.map(row => row.id)).toEqual(['trace:7', 'trace:8', 'trace:9', 'trace:10', 'trace:20', 'trace:21', 'trace:22']);
  expect(trace.selection?.id).toBe('trace:8');
  expect(trace.page.next_cursor).toBeUndefined();
  expect(trace.gaps).toEqual([{ after:'trace:10', before:'trace:20', cursor:'before-twenty' }]);
});

it('a detail reply is fenced by the epoch it was requested in', () => {
  const cache = selectTrace(replaceTrace({ records: [entry(1)], next_cursor: null }), 'trace:1');
  const requested = beginTraceDetail(cache, 'trace:1');
  expect(requested.details['trace:1']).toMatchObject({ epoch: cache.epoch, loading: true });
  // Explicit navigation replaces the read domain and retires its pending detail.
  const rebased = replaceTrace({ records: [entry(50)], next_cursor: null }, requested);
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
  server.socket.success(request, traceReceipt(detail(10)));
  await pending;
  expect(server.client.getSnapshot().views.A.trace?.details['trace:10']?.detail).toBeUndefined();
});

it('a lifecycle-only repair preserves cached detail and its pending original read', async () => {
  server = new Server();
  const running = entry(1, { state: 'running' });
  server.snapshots.set('A', { ...snapshot(), trace: { records: [running] } });
  await server.attached('A');
  server.held.add('session/traceDetail');
  const pending = server.client.loadTraceDetail('A', running.id);
  const request = await server.waitFor('session/traceDetail', 1);
  await server.update('A', { ...snapshot(), trace: { records: [entry(1)] } });
  server.socket.success(request, traceReceipt(detail(1)));
  await pending;
  expect(server.client.getSnapshot().views.A.trace?.details[running.id]?.detail).toEqual(detail(1));
  const cached = completeTraceDetail(replaceTrace({ records: [running] }), running.id, 1, detail(1));
  expect(refreshTrace(cached, { records: [entry(1)] }).details[running.id]).toBe(cached.details[running.id]);
});

it('a body update retains readable detail and coalesces changes while the replacement is in flight', () => {
  const original = entry(1), first = { ...original, preview: { text: 'New content', truncated: false } };
  const cached = completeTraceDetail(replaceTrace({ records: [original], read_cut: rootCut() }), original.id, 1, detail(1));
  const updated = refreshTrace(cached, { records: [first], read_cut: { ...rootCut(), revision: '2' } });
  expect(updated.details[original.id]).toMatchObject({ stale: true, detail: detail(1) });
  const reading = beginTraceDetail(updated, original.id);
  expect(reading.details[original.id]).toMatchObject({ loading: true, stale: false, detail: detail(1) });
  const newer = refreshTrace(reading, { records: [{ ...first, preview: { text: 'Newest content', truncated: false } }] });
  expect(newer.details[original.id].read).toBe(reading.details[original.id].read);
  const completed = completeTraceDetail(newer, original.id, newer.epoch, detail(1, { truncated: true }));
  expect(completed.details[original.id]).toMatchObject({ stale: true, detail: { truncated: true } });
  const failed = completeTraceDetail(beginTraceDetail(completed, original.id), original.id, newer.epoch, undefined, 'Read failed');
  expect(failed.details[original.id]).toMatchObject({ loading: false, stale: false, error: 'Read failed', detail: { truncated: true } });
});

it('native TraceChanged coalesces burst reads independently of durable Session updates', async () => {
  server = new Server(); await server.attached('A');
  server.held.add('session/trace');
  let cursor = 0;
  const emit = (event: import('../../protocol/app-server/v72').RuntimeClientEvent) => server.socket.deliver({ jsonrpc: '2.0', method: 'session/event', params: { target: server.target('A'), cursor: String(++cursor), event } });
  emit({ type: 'read_domains_updated', transcript: { entries: [] }, trace: {records:[],next_cursor:null} });
  expect(server.requests.filter(row => row.request.method === 'session/trace')).toHaveLength(0);
  emit({ type: 'trace_changed' });
  const first = await server.waitFor('session/trace', 1);
  for (let i = 0; i < 100; i++) emit({ type: 'trace_changed' });
  expect(server.requests.filter(row => row.request.method === 'session/trace')).toHaveLength(1);
  server.socket.success(first, { type: 'trace', page: {read_cut:rootCut(), records: [entry(1)] } });
  const second = await server.waitFor('session/trace', 2);
  const observed = new Promise<void>(resolve => { const stop = server.client.subscribe(() => {
    if (server.client.getSnapshot().views.A.trace?.page.records.some(row => row.id === 'trace:2')) { stop(); resolve(); }
  }); });
  server.socket.success(second, { type: 'trace', page: {read_cut:rootCut(), records: [entry(1), entry(2)] } });
  await observed;
  expect(server.requests.filter(row => row.request.method === 'session/trace')).toHaveLength(2);
  expect(server.requests.filter(row => row.request.method === 'session/checkpoint')).toHaveLength(1);
});

it('a delayed summary cannot roll back a newer native head while retained detail stays readable', async () => {
  server = new Server();
  const running = entry(1, { state: 'running' });
  server.snapshots.set('A', { ...snapshot(), trace: { records: [running], next_cursor: null } });
  await server.attached('A'); server.held.add('session/trace');
  server.socket.deliver({ jsonrpc: '2.0', method: 'session/event', params: { target: server.target('A'), cursor: String(++server.cursor), event: { type: 'trace_changed' } } });
  const stale = await server.waitFor('session/trace', 1);
  await server.update('A', { ...snapshot(), trace: { records: [entry(1)], next_cursor: null } });
  server.socket.success(stale, { type: 'trace', page: { read_cut: rootCut(), records: [running], next_cursor: null } });
  await server.waitFor('session/trace', 2);
  expect(server.client.getSnapshot().views.A.trace!.page.records[0].state).toBe('completed');
});

it('overlapping snapshot supersedes paging and unchanged pending detail without stranding loading', async () => {
  server = new Server();
  server.snapshots.set('A', { ...snapshot(), trace: { records: [entry(10)], next_cursor: 'trace:10' } });
  await server.attached('A');
  const epoch = server.client.getSnapshot().views.A.trace!.epoch;
  server.held.add('session/trace'); server.held.add('session/traceDetail');
  const older = server.client.loadEarlierTrace('A');
  const heavy = server.client.loadTraceDetail('A', 'trace:10');
  const page = await server.waitFor('session/trace', 1);
  const pending = await server.waitFor('session/traceDetail', 1);
  await server.client.refresh('A');
  const authoritative = server.client.getSnapshot().views.A.trace!;
  expect(authoritative.epoch).toBe(epoch);
  expect(authoritative.loading).toBe(false);
  expect(authoritative.details['trace:10']).toBeUndefined();
  server.socket.success(page, { type: 'trace', page: {read_cut:rootCut(), records: [entry(9)] } });
  server.socket.success(pending, traceReceipt(detail(10)));
  await Promise.all([older, heavy]);
  expect(server.client.getSnapshot().views.A.trace).toBe(authoritative);
  expect(server.requests.filter(row => row.request.method === 'session/trace')).toHaveLength(1);
  server.traceDetails.set('trace:10', detail(10));
  server.held.delete('session/traceDetail');
  await server.client.loadTraceDetail('A', 'trace:10');
  expect(server.client.getSnapshot().views.A.trace!.details['trace:10'].detail).toEqual(detail(10));
});

it('421: Web consumes the generated bounded Tool vocabulary through the actual client', async () => {
  const { fixtures } = await import('../../protocol/app-server/fixtures');
  const fixture = fixtures.find(value => 'id' in value && value.id === 'trace-tool-summary');
  if (!fixture || !('result' in fixture) || !fixture.result || fixture.result.type !== 'trace') throw new Error('missing native Tool fixture');
  const page = JSON.parse(JSON.stringify(fixture.result.page));
  server = new Server(); server.snapshots.set('A', { ...snapshot(), trace: page });
  await server.attached('A');
  const tool = server.client.getSnapshot().views.A.trace!.page.records[0]!.tool!;
  expect(tool.arguments).toEqual({ text: '{"command":"bounded input', truncated: true });
  expect(server.requests.filter(item => item.request.method === 'session/traceDetail')).toHaveLength(0);
});

it('cold trace supports paging and details without runtime attachment, and retires late reads', async () => {
  server = new Server(); await server.connect(); server.held.add('session/attach');
  server.handlers.set('session/traceHistory', request => ({ type: 'session_trace_history', conversation_id: 'conversation-A', page: request.method === 'session/traceHistory' && request.params.before ? {read_cut:rootCut(), records: [entry(9)], next_cursor: null } : {read_cut:rootCut(), records: [entry(10)], next_cursor: 'trace:10' } }));
  server.handlers.set('session/traceHistoryDetail', request => {if(request.method!=='session/traceHistoryDetail')throw Error('method');return {type:'session_trace_history_detail',conversation_id:'conversation-A',page:traceReceipt(detail(Number(request.params.record_id.slice(6)))).page};});
  const attach = server.client.attach('A');
  const opening = await server.waitFor('session/attach', 1);
  await new Promise(resolve => setTimeout(resolve, 0));
  expect(server.client.getSnapshot().views.A.tracePreview?.cache.page.records).toEqual([entry(10)]);
  expect(server.client.getSnapshot().views.A.target).toBeUndefined();
  await server.client.loadEarlierTrace('A');
  expect(server.client.getSnapshot().views.A.tracePreview?.cache.page.records).toEqual([entry(9), entry(10)]);
  await server.client.loadTraceDetail('A', 'trace:9');
  expect(server.client.getSnapshot().views.A.tracePreview?.cache.details['trace:9']?.detail).toEqual(detail(9));
  server.client.selectTrace('A', 'trace:10');
  server.held.add('session/traceHistoryDetail');
  const read = server.client.loadTraceDetail('A', 'trace:10');
  const request = await server.waitFor('session/traceHistoryDetail', 2);
  server.reply(opening); await attach;
  server.reply(request); await read;
  expect(server.client.getSnapshot().views.A.tracePreview).toBeUndefined();
  expect(server.client.getSnapshot().views.A.trace?.details['trace:10']).toBeUndefined();
});

// T20: count alone cannot bound a cache containing a few enormous details.
it('bounds encoded detail bytes while preserving selection and native interval', () => {
  let cache=selectTrace(replaceTrace({records:[entry(1),entry(2),entry(3)]}),'trace:1');
  const huge='🙂'.repeat(1_800_000);
  for(let id=1;id<=3;id++){
    const value=detail(id);
    value.request!.effective_system_prompt={text:huge,truncated:false};
    cache=completeTraceDetail(cache,`trace:${id}`,cache.epoch,value);
  }
  expect(new TextEncoder().encode(JSON.stringify(cache.details)).length).toBeLessThanOrEqual(TRACE_DETAIL_MAX_BYTES);
  expect(cache.details['trace:1']?.detail).toBeDefined();
  expect(cache.selection?.id).toBe('trace:1');
  expect(cache.page.records.map(row=>row.id)).toEqual(['trace:1','trace:2','trace:3']);
});

it('fills disjoint history in bounded pages while retaining newer events and row cuts', () => {
  const old = entry(1), second = entry(2), oldCut = { ...rootCut(), revision:'1' };
  let cache = selectTrace(replaceTrace({ records:[old,second], read_cut:oldCut, next_cursor:'oldest' }), old.id);
  cache = completeTraceDetail(beginTraceDetail(cache, old.id), old.id, cache.epoch, detail(1));
  const evidence = cache.details[old.id];
  cache = refreshTrace(cache, { records:[entry(8),entry(9)], next_cursor:'before-eight', read_cut:rootCut() });
  cache = fillTraceGap(cache, cache.gaps![0]!, { records:[entry(6),entry(7)], next_cursor:'before-six', read_cut:rootCut() });
  expect(cache.gaps).toEqual([{ after:'trace:2', before:'trace:6', cursor:'before-six' }]);
  cache = refreshTrace(cache, { records:[entry(9),entry(10)], next_cursor:'before-nine', read_cut:rootCut() });
  cache = fillTraceGap(cache, cache.gaps![0]!, { records:[entry(2,{preview:{text:'Older paging body',truncated:false}}),entry(3),entry(4),entry(5)], next_cursor:'before-two', read_cut:rootCut() });
  expect(cache.page.records.map(row=>row.id)).toEqual(Array.from({length:10},(_,i)=>`trace:${i+1}`));
  expect(cache.gaps).toEqual([]); expect(cache.page.next_cursor).toBe('oldest');
  expect(cache.details[old.id]).toBe(evidence); expect(cache.readCuts![old.id]).toBe(oldCut);
  expect(cache.page.records[1]).toBe(second); expect(cache.readCuts![second.id]).toBe(oldCut);
});
it('native overlap closes gaps and empty live pages preserve the reading window', () => {
  const old = replaceTrace({records:[entry(1),entry(2)],next_cursor:'oldest'});
  const live = refreshTrace(old,{records:[entry(5)],next_cursor:'before-five'});
  const bridged = refreshTrace(live,{records:[entry(2),entry(3),entry(4),entry(5),entry(6)],next_cursor:'before-two'});
  expect(bridged.gaps).toEqual([]);
  expect(bridged.page.records.map(row=>row.id)).toEqual(Array.from({length:6},(_,i)=>`trace:${i+1}`));
  expect(refreshTrace(bridged,{records:[]}).page.records).toEqual(bridged.page.records);
});
it('a disjoint append keeps an in-flight detail read valid', () => {
  const pending = beginTraceDetail(selectTrace(replaceTrace({records:[entry(1)]}),'trace:1'),'trace:1');
  const live = refreshTrace(pending,{records:[entry(50)],next_cursor:'before-fifty'});
  expect(completeTraceDetail(live,'trace:1',pending.epoch,detail(1)).details['trace:1']?.detail).toEqual(detail(1));
});

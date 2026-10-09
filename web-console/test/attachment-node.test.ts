import { afterEach, expect, it, vi } from 'vitest';
import type { MethodResult } from '../../protocol/app-server/v38';
import { OutcomeUncertain, RpcFailure } from '../src/client/app-server';
import { Server, snapshot } from './fixture';
import { traceRecord, requestDetail } from './trace-fixture';
const servers: Server[] = [];
afterEach(() => { servers.forEach(s => s.client.disconnect()); vi.restoreAllMocks(); });
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(done => { resolve = done; }); return { promise, resolve }; }
async function setup() {
  const s = new Server(); servers.push(s);
  for (const node of ['left', 'right']) s.nodeSnapshots.set(node, { ...snapshot(), conversation_id: `conversation-${node}`, trace: { records: [traceRecord(1)] } });
  await s.connect(); return s;
}
const requests = (s: Server) => s.requests.map(row => row.request).filter(row => ['session/attach', 'session/detach', 'session/switchNode'].includes(row.method));
function published(s: Server, predicate: () => boolean) {
  if (predicate()) return Promise.resolve();
  return new Promise<void>(resolve => { const stop = s.client.subscribe(() => { if (predicate()) { stop(); resolve(); } }); });
}
function readBarrier(s: Server, method: string) {
  const entered = deferred<Promise<unknown>>(), request = s.client.request.bind(s.client);
  vi.spyOn(s.client, 'request').mockImplementation(((...args: Parameters<typeof request>) => {
    const work = request(...args); if (args[0].method === method) entered.resolve(work); return work;
  }) as typeof request);
  return entered.promise;
}

it.each(['host', 'validation', 'ack'] as const)('conflicting Node Open is rejected without mutating or acquiring: %s', async phase => {
  const s = await setup(), entered = deferred<void>(), gate = deferred<void>();
  if (phase !== 'ack') s.client.setAttachmentAdmission(async () => {
    if (phase === 'host') { entered.resolve(); await gate.promise; }
    return { current: () => true, validate: async () => { if (phase === 'validation') { entered.resolve(); await gate.promise; } return true; } };
  });
  s.held.add('session/attach'); const first = s.client.attach('A', 'left'); void first.catch(() => {});
  if (phase === 'ack') await s.waitFor('session/attach', 1); else await entered.promise;
  const conflict = s.client.attach('A', 'right').then(() => 'accepted', error => String(error));
  expect(s.client.getSnapshot().views.A.nodeId).toBe('left');
  expect(await conflict).toContain('branch switching');
  gate.resolve(); const attach = await s.waitFor('session/attach', 1);
  expect(attach.params).toEqual({ session_id: 'A', node_id: 'left' });
  s.reply(attach); await first;
  const view = s.client.getSnapshot().views.A;
  expect(view.snapshot?.conversation_id).toBe('conversation-left');
  expect(view.attachmentNodeId).toBe('left'); expect(s.client.isAttachmentObservationCurrent('A', view.attachmentObservation)).toBe(true);
  expect(requests(s)).toHaveLength(1); expect(s.claims()).toEqual([view.target]); expect(s.maxClaims).toBe(1);
});

it('Release then Open another Node retains the obsolete claim only until its exact detach ACK', async () => {
  const s = await setup(); s.held.add('session/attach'); s.held.add('session/detach');
  const left = s.client.attach('A', 'left'), a = await s.waitFor('session/attach', 1);
  const release = s.client.release('A'), right = s.client.attach('A', 'right');
  s.reply(a); await left; const target = s.target('A'), detach = await s.waitFor('session/detach', 1);
  expect(target.conversation_id).toBe('conversation-left'); expect(detach.params).toEqual({ target });
  expect(s.client.getSnapshot().views.A.attachmentObservation).toBeUndefined();
  expect(s.client.getSnapshot().views.A.snapshot).toBeUndefined();
  expect(requests(s).map(row => row.method)).toEqual(['session/attach', 'session/detach']); expect(s.claims()).toEqual([target]);
  s.reply(detach); await release; const b = await s.waitFor('session/attach', 2);
  expect(b.params).toEqual({ session_id: 'A', node_id: 'right' }); s.reply(b); await right;
  const view = s.client.getSnapshot().views.A; expect(view.snapshot?.conversation_id).toBe('conversation-right');
  expect(s.client.isAttachmentObservationCurrent('A', view.attachmentObservation)).toBe(true);
  s.reply(a); expect(s.client.getSnapshot().views.A.target).toEqual(view.target);
  expect(s.claims()).toEqual([view.target]); expect(s.maxClaims).toBe(1);
});

it('an unsuccessful detach cannot satisfy a queued different-Node Open by refreshing the retained target', async () => {
  const s = await setup(); await s.client.attach('A', 'left'); const target = s.target('A'); s.held.add('session/detach');
  s.handlers.set('session/detach', () => { throw new RpcFailure({ code: -32000, message: 'still owned', data: { kind: 'invalid_state' } }); });
  const release = s.client.release('A'), rejected = expect(release).rejects.toThrow('still owned');
  const next = s.client.attach('A', 'right'), refused = expect(next).rejects.toThrow('previous Node');
  s.reply(await s.waitFor('session/detach', 1)); await Promise.all([rejected, refused]);
  expect(s.client.getSnapshot().views.A.target).toEqual(target); expect(s.client.getSnapshot().views.A.attachmentObservation).toBeUndefined();
  expect(() => s.client.target('A')).toThrow('not authoritatively attached');
  expect(s.claims()).toEqual([target]); expect(requests(s)).toHaveLength(2);
  s.handlers.delete('session/detach'); s.held.delete('session/detach'); await s.client.release('A'); await s.client.attach('A', 'right');
  expect(s.client.target('A').conversation_id).toBe('conversation-right'); expect(s.claims()).toHaveLength(1);
  expect(requests(s).map(row => row.method)).toEqual(['session/attach', 'session/detach', 'session/detach', 'session/attach']);
});

it('unknown transmitted Node acquisition stays uncertain through explicit different-Node recovery', async () => {
  const s = await setup(); s.held.add('session/attach');
  const left = s.client.attach('A', 'left'), uncertain = expect(left).rejects.toBeInstanceOf(OutcomeUncertain);
  const a = await s.waitFor('session/attach', 1); s.commit(a); const release = s.client.release('A'); s.socket.close();
  await Promise.all([uncertain, release]); expect(s.client.getSnapshot().views.A.attachment).toBe('stale');
  await s.connect(); expect(requests(s)).toHaveLength(1); s.held.delete('session/attach'); await s.client.attach('A', 'right');
  expect(s.client.target('A').conversation_id).toBe('conversation-right'); expect(s.client.getSnapshot().uncertain).toHaveLength(1);
  expect(requests(s).map(row => row.params)).toEqual([{ session_id: 'A', node_id: 'left' }, { session_id: 'A', node_id: 'right' }]);
  expect(s.claims()).toHaveLength(1);
});

it('equivalent queued Opens share one exact Node claim', async () => {
  const s = await setup(); s.held.add('session/attach'); const first = s.client.attach('A', 'left');
  const request = await s.waitFor('session/attach', 1), repeats = Array.from({ length: 6 }, () => s.client.attach('A', 'left'));
  s.reply(request); await Promise.all([first, ...repeats]);
  expect(requests(s)).toHaveLength(1); expect(s.claims()).toHaveLength(1); expect(s.client.getSnapshot().views.A.nodeId).toBe('left');
});

it('a mismatched native Conversation cannot become Node observation authority but its claim remains releasable', async () => {
  const s = await setup(); s.held.add('session/attach'); const open = s.client.attach('A', 'left'), rejected = expect(open).rejects.toThrow('Mismatched');
  const request = await s.waitFor('session/attach', 1);
  // Native fault: target and snapshot agree with each other but not the admitted Node.
  s.nodeSnapshots.set('left', s.nodeSnapshots.get('right')!); const response = s.commit(request);
  s.socket.deliver(response); await rejected;
  expect(s.claims()).toHaveLength(1); expect(s.client.getSnapshot().views.A.attachmentObservation).toBeUndefined();
  await s.client.release('A'); expect(s.claims()).toHaveLength(0); expect(requests(s)).toHaveLength(2);
});

it.each(['session/statistics', 'session/history', 'session/traceHistory'] as const)('cold scope rejects another Conversation from %s', async method => {
  const s = await setup(); s.held.add('session/attach'); s.held.add(method); const finished = readBarrier(s, method);
  const open = s.client.attach('A', 'left'); const request = await s.waitFor(method, 1);
  expect(request.params).toMatchObject({ session_id: 'A', node_id: 'left' });
  const result: MethodResult = method === 'session/statistics' ? { type: 'session_statistics', conversation_id: 'wrong', statistics: { turns: '99', steps: '0', completed_responses: '0', model_requests: '0', requests_with_usage: '0' } }
    : method === 'session/traceHistory' ? { type: 'session_trace_history', conversation_id: 'wrong', page: { records: [traceRecord(99)] } }
    : { type: 'session_history', conversation_id: 'wrong', window: { cut: { conversation_id: 'wrong', journal: '1', transcript: '1', mutation_revision: '0' }, page: { entries: [] } } };
  s.socket.success(request, result); await finished;
  const view = s.client.getSnapshot().views.A;
  if (method === 'session/statistics') expect(view.statisticsPreview).toBeUndefined();
  if (method === 'session/history') expect(view.preview).toBeUndefined();
  if (method === 'session/traceHistory') expect(view.tracePreview?.cache.page.records).toEqual([]);
  expect(s.claims()).toHaveLength(0); s.reply(await s.waitFor('session/attach', 1)); await open;
});

it('late cold A statistics and Trace detail cannot overwrite B evidence', async () => {
  const s = await setup(); s.held.add('session/attach'); s.held.add('session/statistics'); s.held.add('session/traceHistoryDetail');
  const finished = readBarrier(s, 'session/statistics'), a = s.client.attach('A', 'left');
  const oldStatistics = await s.waitFor('session/statistics', 1);
  await published(s, () => !!s.client.getSnapshot().views.A.tracePreview);
  const detail = s.client.loadTraceDetail('A', 'trace:1'), oldDetail = await s.waitFor('session/traceHistoryDetail', 1);
  const release = s.client.release('A'), b = s.client.attach('A', 'right');
  s.reply(await s.waitFor('session/attach', 1)); await Promise.all([a, release]);
  const newStatistics = await s.waitFor('session/statistics', 2); s.reply(newStatistics);
  await published(s, () => s.client.getSnapshot().views.A.statisticsPreview?.conversationId === 'conversation-right');
  const expected = s.client.getSnapshot().views.A.statisticsPreview;
  s.reply(oldStatistics); await finished;
  s.socket.success(oldDetail, { type: 'session_trace_history_detail', conversation_id: 'conversation-left', detail: requestDetail(1) }); await detail;
  expect(s.client.getSnapshot().views.A.statisticsPreview).toBe(expected);
  expect(s.client.getSnapshot().views.A.tracePreview?.cache.details['trace:1']?.detail).toBeUndefined();
  s.reply(await s.waitFor('session/attach', 2)); await b; expect(s.claims()).toHaveLength(1); expect(requests(s)).toHaveLength(3);
});

it('Release during final validation retires A before send and admits only the explicitly queued B', async () => {
  const s = await setup(), entered = deferred<void>(), gate = deferred<boolean>(); let first = true;
  s.client.setAttachmentAdmission(async () => ({ current: () => true, validate: async () => { if (first) { first = false; entered.resolve(); return gate.promise; } return true; } }));
  const a = s.client.attach('A', 'left'), rejected = expect(a).rejects.toThrow('before dispatch'); await entered.promise;
  const release = s.client.release('A'), b = s.client.attach('A', 'right'); gate.resolve(true);
  await Promise.all([rejected, release, b]);
  expect(requests(s).map(row => row.params)).toEqual([{ session_id: 'A', node_id: 'right' }]);
  expect(s.client.getSnapshot().views.A.snapshot?.conversation_id).toBe('conversation-right');
  expect(s.claims()).toEqual([s.client.target('A')]);
});

it.each(['wrong-conversation', 'released'] as const)('cold History pagination cannot publish across its scope: %s', async invalidation => {
  const s = await setup(); s.held.add('session/attach');
  const first = s.nodeSnapshots.get('left')!; first.transcript = { entries: [], next_cursor: '5' };
  const opening = s.client.attach('A', 'left');
  await published(s, () => !!s.client.getSnapshot().views.A.preview);
  s.held.add('session/history'); const older = s.client.loadEarlierPreview('A'), page = await s.waitFor('session/history', 2);
  expect(page.params).toMatchObject({ session_id: 'A', node_id: 'left', at: { type: 'older', before: '5' } });
  const release = invalidation === 'released' ? s.client.release('A') : undefined;
  s.socket.success(page, { type: 'session_history', conversation_id: invalidation === 'released' ? 'conversation-left' : 'wrong', window: {
    cut: { conversation_id: 'conversation-left', journal: '5', transcript: '5', mutation_revision: '0' }, page: { entries: [{ cursor: '1', item: { type: 'message', message: { id: 'obsolete', role: 'user', source: 'human', content: [] } } }], next_cursor: null },
  } }); await older;
  expect(s.client.getSnapshot().views.A.preview?.history.page.entries).toEqual([]);
  if (invalidation === 'wrong-conversation') expect(s.client.getSnapshot().views.A.preview?.history.error).toContain('Invalid history');
  expect(requests(s)).toHaveLength(1); expect(s.claims()).toHaveLength(0);
  s.reply(await s.waitFor('session/attach', 1)); await opening; await release;
  expect(requests(s)).toHaveLength(invalidation === 'released' ? 2 : 1);
  expect(s.requests.some(({ request }) => ['turn/start', 'turn/steer', 'session/setModel'].includes(request.method))).toBe(false);
});


it('a transmitted switch settles its claim without replacing the Node of a later Release and Open', async () => {
  const s = await setup(); await s.client.attach('A', 'left');
  s.held.add('session/switchNode'); const switching = s.client.switchNode('A', 'right');
  const request = await s.waitFor('session/switchNode', 1);
  expect(s.client.getSnapshot().views.A.attachmentObservation).toBeUndefined();
  await expect(s.client.attach('A', 'left')).rejects.toThrow('current Node switch');
  expect(s.client.getSnapshot().views.A.nodeId).toBe('left');
  const release = s.client.release('A'), reopen = s.client.attach('A', 'left');
  s.reply(request); await Promise.all([switching, release, reopen]);
  const view = s.client.getSnapshot().views.A;
  expect(requests(s).map(row => row.method)).toEqual(['session/attach', 'session/switchNode', 'session/attach']);
  expect(view.nodeId).toBe('left'); expect(view.snapshot?.conversation_id).toBe('conversation-left');
  expect(s.client.isAttachmentObservationCurrent('A', view.attachmentObservation)).toBe(true);
  expect(s.claims()).toEqual([view.target]); expect(s.maxClaims).toBe(1);
});

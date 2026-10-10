import { afterEach, expect, it, vi } from 'vitest';
import type { MethodResult } from '../../protocol/app-server/v39';
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
  const refused = expect(right).rejects.toThrow('already resident');
  s.reply(a); await left; const target = s.target('A'), detach = await s.waitFor('session/detach', 1);
  expect(target.conversation_id).toBe('conversation-left'); expect(detach.params).toEqual({ target });
  expect(s.client.getSnapshot().views.A.attachmentObservation).toBeUndefined();
  expect(s.client.getSnapshot().views.A.snapshot).toBeUndefined();
  expect(requests(s).map(row => row.method)).toEqual(['session/attach', 'session/detach']); expect(s.claims()).toEqual([target]);
  s.reply(detach); await release; const b = await s.waitFor('session/attach', 2);
  expect(b.params).toEqual({ session_id: 'A', node_id: 'right' }); s.reply(b); await refused;
  expect(s.claims()).toHaveLength(0); expect(s.residentConversations.get('A')).toBe('conversation-left');
  s.held.delete('session/attach'); await s.client.attach('A', 'left'); await s.client.switchNode('A', 'right'); await s.client.attach('A', 'right');
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
  s.handlers.delete('session/detach'); s.held.delete('session/detach'); await s.client.release('A'); await s.client.attach('A', 'left'); await s.client.switchNode('A', 'right'); await s.client.attach('A', 'right');
  expect(s.client.target('A').conversation_id).toBe('conversation-right'); expect(s.claims()).toHaveLength(1);
  expect(requests(s).map(row => row.method)).toEqual(['session/attach', 'session/detach', 'session/detach', 'session/attach', 'session/switchNode', 'session/attach']);
});

it('unknown transmitted Node acquisition cannot bypass existing native residency', async () => {
  const s = await setup(); s.held.add('session/attach');
  const left = s.client.attach('A', 'left'), uncertain = expect(left).rejects.toBeInstanceOf(OutcomeUncertain);
  const a = await s.waitFor('session/attach', 1); s.commit(a); const release = s.client.release('A'); s.socket.close();
  await Promise.all([uncertain, release]); expect(s.client.getSnapshot().views.A.attachment).toBe('stale');
  await s.connect(); expect(requests(s)).toHaveLength(1); s.held.delete('session/attach'); await expect(s.client.attach('A', 'right')).rejects.toThrow('already resident');
  await s.client.attach('A', 'left'); await s.client.switchNode('A', 'right'); await s.client.attach('A', 'right');
  expect(s.client.target('A').conversation_id).toBe('conversation-right'); expect(s.client.getSnapshot().uncertain).toHaveLength(1);
  expect(requests(s).map(row => row.method)).toEqual(['session/attach', 'session/attach', 'session/attach', 'session/switchNode', 'session/attach']);
  expect(s.claims()).toHaveLength(1);
  await s.client.release('A'); await s.client.release('A');
  expect(s.client.getSnapshot().views.A.attachment).toBe('detached');
  expect(s.client.getSnapshot().uncertain).toHaveLength(1); // Historical uncertainty is not current claim ownership.
  expect(s.claims()).toHaveLength(0);
  expect(requests(s).filter(row => row.method === 'session/detach')).toHaveLength(1);
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
  const refused = expect(b).rejects.toThrow('already resident');
  s.reply(await s.waitFor('session/attach', 1)); await Promise.all([a, release]);
  const newStatistics = await s.waitFor('session/statistics', 2); s.reply(newStatistics);
  await published(s, () => s.client.getSnapshot().views.A.statisticsPreview?.conversationId === 'conversation-right');
  const expected = s.client.getSnapshot().views.A.statisticsPreview;
  s.reply(oldStatistics); await finished;
  s.socket.success(oldDetail, { type: 'session_trace_history_detail', conversation_id: 'conversation-left', detail: requestDetail(1) }); await detail;
  expect(s.client.getSnapshot().views.A.statisticsPreview).toBe(expected);
  expect(s.client.getSnapshot().views.A.tracePreview?.cache.details['trace:1']?.detail).toBeUndefined();
  s.reply(await s.waitFor('session/attach', 2)); await refused; expect(s.claims()).toHaveLength(0); expect(requests(s)).toHaveLength(3);
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


it('a transmitted switch excludes Opens through queued Release and settles native B selection', async () => {
  const s = await setup(); await s.client.attach('A', 'left');
  s.held.add('session/switchNode'); const switching = s.client.switchNode('A', 'right');
  const request = await s.waitFor('session/switchNode', 1);
  expect(s.client.getSnapshot().views.A.attachmentObservation).toBeUndefined();
  await expect(s.client.attach('A', 'left')).rejects.toThrow('current Node switch');
  const release = s.client.release('A');
  await expect(s.client.attach('A', 'left')).rejects.toThrow('current Node switch');
  expect(s.client.getSnapshot().views.A.nodeId).toBe('left');
  s.socket.deliver({ jsonrpc: '2.0', method: 'session/closed', params: { target: s.target('A') } });
  s.reply(request); await Promise.all([switching, release]);
  const view = s.client.getSnapshot().views.A;
  expect(requests(s).map(row => row.method)).toEqual(['session/attach', 'session/switchNode']);
  expect(view.nodeId).toBe('right'); expect(view.attachmentIntent).toBe('released');
  expect(view.attachment).toBe('detached'); expect(view.target).toBeUndefined();
  expect(s.claims()).toHaveLength(0);
  await s.client.attach('A');
  expect(s.client.target('A').conversation_id).toBe('conversation-right');
  expect(s.claims()).toHaveLength(1); expect(s.maxClaims).toBe(1);
});

it.each(['release', 'switch'] as const)('%s synchronously revokes controls but retains the cleanup claim', async transition => {
  const s = await setup(); await s.client.attach('A', 'left'); const target = s.client.target('A');
  const method = transition === 'release' ? 'session/detach' : 'session/switchNode'; s.held.add(method);
  const work = transition === 'release' ? s.client.release('A') : s.client.switchNode('A', 'right');
  await expect(s.client.send('A', 'must not execute')).rejects.toThrow('not authoritatively attached');
  await expect(s.client.request({ method: 'turn/start', params: { target, content: [] } }, 'inbound_accepted')).rejects.toThrow('authority was revoked');
  expect(s.client.getSnapshot().views.A.target).toEqual(target); expect(s.claims()).toEqual([target]);
  expect(s.requests.filter(row => row.request.method === 'turn/start')).toHaveLength(0);
  s.reply(await s.waitFor(method, 1)); await work; expect(s.claims()).toHaveLength(0);
});

it.each(['capacity', 'validation'] as const)('a revoked unsent control cannot cross %s admission or revive after Open', async phase => {
  const s = await setup(); await s.client.attach('A', 'left'); const target = s.client.target('A');
  const gate = deferred<boolean>(), entered = deferred<void>(); let signal: AbortSignal | undefined;
  s.held.add('session/read');
  const reads = phase === 'capacity' ? Array.from({ length: 8 }, () => s.client.request({ method: 'session/read', params: { session_id: 'A' } }, 'session')) : [];
  if (reads.length) await s.waitFor('session/read', 8);
  const control = s.client.request({ method: 'turn/start', params: { target, content: [] } }, 'inbound_accepted', undefined,
    phase === 'validation' ? { current: () => true, validate: async controller => { signal = controller; entered.resolve(); return gate.promise; } } : undefined);
  const rejected = expect(control).rejects.toThrow('before dispatch');
  if (phase === 'validation') await entered.promise;
  const release = s.client.release('A'), reopen = s.client.attach('A', 'left');
  s.requests.filter(row => row.request.method === 'session/read').forEach(row => s.reply(row.request));
  gate.resolve(true); await Promise.all([rejected, release, reopen, ...reads]);
  expect(s.requests.filter(row => row.request.method === 'turn/start')).toHaveLength(0);
  expect(s.claims()).toEqual([s.client.target('A')]); expect(s.client.target('A')).not.toEqual(target);
  if (signal) expect(signal.aborted).toBe(true);
  await s.client.send('A', 'fresh authority');
  expect(s.requests.filter(row => row.request.method === 'turn/start')).toHaveLength(1);
  const before = s.requests.filter(row => row.request.method === 'session/read').length;
  const reused = Array.from({ length: 8 }, () => s.client.request({ method: 'session/read', params: { session_id: 'A' } }, 'session'));
  const sentReads = s.requests.filter(row => row.request.method === 'session/read').slice(before);
  expect(sentReads).toHaveLength(8); // No leaked or double-held reservation.
  sentReads.forEach(row => s.reply(row.request)); await Promise.all(reused);
});

it('a transmitted control keeps its correlated acknowledgement after Release', async () => {
  const s = await setup(); await s.client.attach('A', 'left'); s.held.add('turn/start'); s.held.add('session/detach');
  const sent = s.client.send('A', 'already sent'), request = await s.waitFor('turn/start', 1);
  s.commit(request); const release = s.client.release('A');
  s.reply(request); await expect(sent).resolves.toMatchObject({ type: 'inbound_accepted' });
  s.reply(await s.waitFor('session/detach', 1)); await release;
  expect(s.claims()).toHaveLength(0); expect(s.client.getSnapshot().uncertain).toHaveLength(0);
});

it.each(['before-transition', 'after-unload'] as const)('switch failure preserves cleanup evidence and requires explicit recovery: %s', async phase => {
  const s = await setup(); s.summaries.set('A', { active_node: 'left' }); await s.client.attach('A', 'left'); const target = s.client.target('A');
  s.handlers.set('session/switchNode', request => {
    if (phase === 'after-unload') {
      s.handlers.delete('session/switchNode'); s.commit(request); s.loaded.delete('A');
    }
    throw new RpcFailure({ code: -32000, message: 'switch failed', data: { kind: 'operation_failed' } });
  });
  const revision = await deletionPreview(s); s.held.add('session/switchNode');
  const switching = s.client.switchNode('A', 'right'), failed = expect(switching).rejects.toThrow('switch failed');
  const switchRequest = await s.waitFor('session/switchNode', 1);
  await expectSwitchExcludesDeletion(s, revision);
  s.reply(switchRequest); await failed;
  const view = s.client.getSnapshot().views.A;
  expect(view.attachment).toBe('stale'); expect(view.attachmentObservation).toBeUndefined(); expect(view.nodeId).toBeUndefined();
  expect(view.target).toEqual(target); expect(s.claims()).toHaveLength(phase === 'after-unload' ? 0 : 1);
  await expect(s.client.send('A', 'forbidden')).rejects.toThrow('not authoritatively attached');
  expect(requests(s)).toHaveLength(2);
  const freshRevision = await deletionPreview(s); await s.client.deleteSession('A', freshRevision);
  expect(s.requests.filter(row => row.request.method === 'session/delete')).toHaveLength(1);
  await s.client.release('A'); expect(s.claims()).toHaveLength(0);
  await s.client.attach('A');
  expect(s.client.target('A').conversation_id).toBe(phase === 'after-unload' ? 'conversation-right' : 'conversation-left');
  expect(requests(s).map(row => row.method)).toEqual(['session/attach', 'session/switchNode', 'session/detach', 'session/attach']);
  expect(s.claims()).toHaveLength(1); expect(s.maxClaims).toBe(1);
});

it('a lost switch response never replays or restores old authority on reconnect', async () => {
  const s = await setup(); await s.client.attach('A', 'left'); s.held.add('session/switchNode');
  const work = s.client.switchNode('A', 'right'), unknown = expect(work).rejects.toBeInstanceOf(OutcomeUncertain);
  const switchRequest = await s.waitFor('session/switchNode', 1);
  await expectSwitchExcludesDeletion(s, await deletionPreview(s));
  s.commit(switchRequest); s.socket.close(); await unknown;
  await s.connect();
  expect(requests(s).map(row => row.method)).toEqual(['session/attach', 'session/switchNode']);
  expect(s.client.getSnapshot().views.A.attachment).toBe('stale'); expect(s.client.getSnapshot().views.A.nodeId).toBeUndefined();
  expect(s.client.getSnapshot().uncertain.map(row => row.method)).toEqual(['session/switchNode']);
  expect(s.claims()).toHaveLength(0);
  const revision = await deletionPreview(s); expect(revision).toBe('revision-right');
  await s.client.deleteSession('A', revision);
  expect(s.requests.filter(row => row.request.method === 'session/delete').map(row => row.request.params)).toEqual([{ session_id: 'A', expected_target_revision: revision }]);
  await s.client.attach('A');
  expect(s.client.target('A').conversation_id).toBe('conversation-right');
  expect(requests(s).map(row => row.method)).toEqual(['session/attach', 'session/switchNode', 'session/attach']);
  expect(s.claims()).toHaveLength(1);
});

it('Release retires a switch blocked by RPC capacity before any native transition', async () => {
  const s = await setup(); await s.client.attach('A', 'left'); const target = s.client.target('A');
  s.held.add('session/read');
  const reads = Array.from({ length: 8 }, () => s.client.request({ method: 'session/read', params: { session_id: 'A' } }, 'session'));
  await s.waitFor('session/read', 8);
  const switchWork = s.client.switchNode('A', 'right'), refused = expect(switchWork).rejects.toThrow('before dispatch');
  const release = s.client.release('A');
  await expect(s.client.attach('A', 'left')).rejects.toThrow('current Node switch');
  expect(s.claims()).toEqual([target]);
  s.requests.filter(row => row.request.method === 'session/read').forEach(row => s.reply(row.request));
  await Promise.all([refused, release, ...reads]);
  expect(requests(s).map(row => row.method)).toEqual(['session/attach', 'session/detach']);
  expect(s.client.getSnapshot().views.A).toMatchObject({ attachment: 'detached', attachmentIntent: 'released', nodeId: 'left' });
  expect(s.claims()).toHaveLength(0); expect(s.residentConversations.get('A')).toBe('conversation-left');
});

it('deletion rejection cannot restore an older control proof', async () => {
  const s = await setup(); await s.client.attach('A', 'left'); const admission = s.client.getSnapshot().views.A.attachmentObservation;
  s.held.add('session/delete'); s.handlers.set('session/delete', () => ({ type: 'deletion', result: { status: 'stale', session_id: 'A' } }));
  const deleting = s.client.deleteSession('A', 'old-revision');
  expect(s.client.isAttachmentObservationCurrent('A', admission)).toBe(false);
  s.reply(await s.waitFor('session/delete', 1)); await deleting;
  expect(s.client.getSnapshot().views.A.deleting).toBe(false);
  expect(s.client.isAttachmentObservationCurrent('A', admission)).toBe(false);
  await expect(s.client.send('A', 'old proof')).rejects.toThrow('not authoritatively attached');
  expect(s.claims()).toHaveLength(1);
  await s.client.release('A'); await s.client.attach('A', 'left');
  await s.client.send('A', 'fresh proof');
  expect(s.requests.filter(row => row.request.method === 'turn/start')).toHaveLength(1);
});

it('disconnect between switch ACK decoding and lifecycle continuation cannot reopen the old Node', async () => {
  const s = await setup(); await s.client.attach('A', 'left'); s.held.add('session/switchNode');
  const work = s.client.switchNode('A', 'right'), retired = expect(work).rejects.toThrow('Obsolete connection response');
  const request = await s.waitFor('session/switchNode', 1);
  s.reply(request); s.socket.close(); await retired;
  await s.connect();
  expect(requests(s).map(row => row.method)).toEqual(['session/attach', 'session/switchNode']);
  expect(s.client.getSnapshot().uncertain).toHaveLength(0); // ACK was actually decoded.
  expect(s.client.getSnapshot().views.A.attachmentIntent).toBe('released');
  await s.client.attach('A'); expect(s.client.target('A').conversation_id).toBe('conversation-right');
  expect(s.claims()).toHaveLength(1);
});

async function deletionPreview(s: Server) {
  s.handlers.set('session/deletePreview', () => ({ type: 'deletion', result: { status: 'preview', preview: {
    session_id: 'A', target_revision: `revision-${s.summary('A').active_node}`, owned_node_count: 2, owned_conversation_count: 2, owned_child_count: 0,
  } } }));
  const response = await s.client.request({ method: 'session/deletePreview', params: { session_id: 'A' } }, 'deletion');
  if (response.result.status !== 'preview') throw new Error('Expected deletion preview');
  return response.result.preview.target_revision;
}
async function expectSwitchExcludesDeletion(s: Server, revision: string) {
  const before = s.client.getSnapshot(), count = s.requests.length;
  // A definitive fixture rejection makes the old implementation fail an
  // assertion, rather than hang waiting for an unimplemented deletion reply.
  s.handlers.set('session/delete', () => ({ type: 'deletion', result: { status: 'stale', session_id: 'A' } }));
  const outcome = s.client.deleteSession('A', revision).then(() => 'admitted', error => String(error));
  const immediatelyAfter = s.client.getSnapshot();
  expect(await outcome).toContain('Node switch');
  expect(immediatelyAfter).toBe(before);
  expect(s.client.getSnapshot()).toBe(before);
  expect(s.requests).toHaveLength(count);
}

it.each(['ack', 'release', 'closed', 'decoded'] as const)('unsettled switch excludes deletion without side effects: %s', async phase => {
  const s = await setup(); await s.client.attach('A', 'left'); const revision = await deletionPreview(s);
  s.held.add('session/switchNode'); const switching = s.client.switchNode('A', 'right'); void switching.catch(() => {});
  const request = await s.waitFor('session/switchNode', 1), response = s.commit(request);
  expect(s.summary('A').active_node).toBe('right'); expect(s.residentConversations.get('A')).toBe('conversation-right');
  const release = phase === 'release' ? s.client.release('A') : undefined;
  if (phase === 'closed') s.socket.deliver({ jsonrpc: '2.0', method: 'session/closed', params: { target: s.client.getSnapshot().views.A.target! } });
  if (phase === 'decoded') {
    // No await: the ACK is decoded but the lifecycle continuation has not run.
    s.socket.deliver(response);
    s.handlers.set('session/delete', () => ({ type: 'deletion', result: { status: 'stale', session_id: 'A' } }));
    const before = s.client.getSnapshot();
    const rejected = s.client.deleteSession('A', revision).then(() => 'admitted', error => String(error));
    expect(s.client.getSnapshot()).toBe(before);
    expect(await rejected).toContain('Node switch');
  } else {
    await expectSwitchExcludesDeletion(s, revision);
    s.socket.deliver(response);
  }
  await Promise.all([switching, release]);
  expect(s.client.getSnapshot().views.A).toMatchObject({ nodeId: 'right', nodeConversationId: 'conversation-right', attachment: 'detached', attachmentIntent: phase === 'release' ? 'released' : 'wanted' });
  expect(s.client.getSnapshot().views.A.attachmentObservation).toBeUndefined();
  expect(s.requests.filter(row => row.request.method === 'session/delete')).toHaveLength(0);
  expect(requests(s).map(row => row.method)).toEqual(['session/attach', 'session/switchNode']);
  expect(s.claims()).toHaveLength(0);
  await s.client.attach('A');
  expect(s.client.target('A').conversation_id).toBe('conversation-right'); expect(s.claims()).toHaveLength(1); expect(s.maxClaims).toBe(1);
  const freshRevision = await deletionPreview(s);
  expect(freshRevision).toBe('revision-right');
  s.handlers.set('session/delete', request => {
    expect(request.params).toEqual({ session_id: 'A', expected_target_revision: freshRevision });
    return { type: 'deletion', result: { status: 'stale', session_id: 'A' } };
  });
  await s.client.deleteSession('A', freshRevision);
  expect(s.requests.filter(row => row.request.method === 'session/delete')).toHaveLength(1);
  expect(s.client.getSnapshot().views.A.nodeId).toBe('right');
  expect(() => s.client.target('A')).toThrow('not authoritatively attached');
});

it('deletion admitted first excludes switch and cannot restore its revoked control proof', async () => {
  const s = await setup(); await s.client.attach('A', 'left'); const revision = await deletionPreview(s);
  s.held.add('session/delete'); s.handlers.set('session/delete', () => ({ type: 'deletion', result: { status: 'stale', session_id: 'A' } }));
  const deleting = s.client.deleteSession('A', revision), request = await s.waitFor('session/delete', 1), before = s.client.getSnapshot();
  expect(() => s.client.switchNode('A', 'right')).toThrow('current attachment operation');
  expect(s.client.getSnapshot()).toBe(before);
  expect(s.requests.filter(row => row.request.method === 'session/switchNode')).toHaveLength(0);
  s.reply(request); await deleting;
  expect(s.client.getSnapshot().views.A.deleting).toBe(false);
  expect(() => s.client.target('A')).toThrow('not authoritatively attached');
  await expect(s.client.attach('A')).rejects.toThrow('Release the retained attachment');
  await s.client.release('A'); await s.client.attach('A');
  expect(s.client.target('A').conversation_id).toBe('conversation-left'); expect(s.claims()).toHaveLength(1);
});

it.each(['proceed', 'release'] as const)('switch owns deletion exclusion behind RPC capacity: %s', async action => {
  const s = await setup(); await s.client.attach('A', 'left'); const revision = await deletionPreview(s);
  s.held.add('session/read');
  const reads = Array.from({ length: 8 }, () => s.client.request({ method: 'session/read', params: { session_id: 'A' } }, 'session'));
  await s.waitFor('session/read', 8);
  const switching = s.client.switchNode('A', 'right');
  const outcome = switching.then(() => 'switched', error => String(error));
  const release = action === 'release' ? s.client.release('A') : undefined;
  await expectSwitchExcludesDeletion(s, revision);
  expect(s.requests.filter(row => row.request.method === 'session/switchNode')).toHaveLength(0);
  s.requests.filter(row => row.request.method === 'session/read').forEach(row => s.reply(row.request));
  await Promise.all([outcome, release, ...reads]);
  expect(await outcome).toContain(action === 'release' ? 'before dispatch' : 'switched');
  expect(requests(s).map(row => row.method)).toEqual(['session/attach', action === 'release' ? 'session/detach' : 'session/switchNode']);
  expect(s.client.getSnapshot().views.A.nodeId).toBe(action === 'release' ? 'left' : 'right');
  expect(s.claims()).toHaveLength(0);
  const before = s.requests.filter(row => row.request.method === 'session/read').length;
  const reused = Array.from({ length: 8 }, () => s.client.request({ method: 'session/read', params: { session_id: 'A' } }, 'session'));
  const sent = s.requests.filter(row => row.request.method === 'session/read').slice(before); expect(sent).toHaveLength(8);
  sent.forEach(row => s.reply(row.request)); await Promise.all(reused);
});

it.each(['proceed', 'release'] as const)('switch retains deletion exclusion through final validation: %s', async action => {
  const s = await setup(); await s.client.attach('A', 'left'); const revision = await deletionPreview(s);
  const request = s.client.request.bind(s.client), entered = deferred<void>(), gate = deferred<boolean>(); let signal: AbortSignal | undefined;
  vi.spyOn(s.client, 'request').mockImplementation(((...args: Parameters<typeof request>) => {
    if (args[0].method === 'session/switchNode') {
      const proof = args[3], current = typeof proof === 'function' ? proof : proof?.current;
      if (!current) throw new Error('Expected switch dispatch proof');
      if (typeof proof !== 'object') throw new Error('Expected lifecycle port admission');
      proof.validate = async controller => { signal = controller; entered.resolve(); return gate.promise; };
    }
    return request(...args);
  }) as typeof request);
  const switching = s.client.switchNode('A', 'right'), outcome = switching.then(() => 'switched', error => String(error));
  await entered.promise;
  const release = action === 'release' ? s.client.release('A') : undefined;
  await expectSwitchExcludesDeletion(s, revision);
  expect(s.requests.filter(row => row.request.method === 'session/switchNode')).toHaveLength(0);
  gate.resolve(true); await Promise.all([outcome, release]);
  expect(await outcome).toContain(action === 'release' ? 'before dispatch' : 'switched');
  expect(signal?.aborted).toBe(action === 'release');
  expect(requests(s).map(row => row.method)).toEqual(['session/attach', action === 'release' ? 'session/detach' : 'session/switchNode']);
  expect(s.client.getSnapshot().views.A.nodeId).toBe(action === 'release' ? 'left' : 'right');
  expect(s.claims()).toHaveLength(0);
});

it('switch deletion exclusion is Session scoped', async () => {
  const s = await setup(); await s.client.attach('A', 'left'); s.held.add('session/switchNode');
  const switching = s.client.switchNode('A', 'right'), request = await s.waitFor('session/switchNode', 1);
  const view = s.client.getSnapshot().views.A;
  s.handlers.set('session/delete', request => {
    expect(request.params).toEqual({ session_id: 'B', expected_target_revision: 'B-preview' });
    return { type: 'deletion', result: { status: 'stale', session_id: 'B' } };
  });
  await s.client.deleteSession('B', 'B-preview');
  expect(s.client.getSnapshot().views.A).toStrictEqual(view);
  expect(s.requests.filter(row => row.request.method === 'session/delete')).toHaveLength(1);
  s.reply(request); await switching;
  expect(s.client.getSnapshot().views.A.nodeId).toBe('right'); expect(s.claims()).toHaveLength(0);
});

it('generic request cannot bypass the actor to acquire, switch, detach or delete a Session', async () => {
  const s = await setup(); await s.client.attach('A', 'left'); const target = s.client.target('A'), before = s.client.getSnapshot(), count = s.requests.length;
  const operations = [
    { method: 'session/attach', params: { session_id: 'A', node_id: 'right' } },
    { method: 'session/switchNode', params: { target, node_id: 'right' } },
    { method: 'session/detach', params: { target } },
    { method: 'session/delete', params: { session_id: 'A', expected_target_revision: 'preview' } },
    { method: 'session/recoverDeletion', params: { session_id: 'A' } },
  ] as const;
  for (const operation of operations) await expect(s.client.request(operation, 'session')).rejects.toThrow('actor admission');
  expect(s.client.getSnapshot()).toBe(before); expect(s.requests).toHaveLength(count); expect(s.claims()).toEqual([target]);
});


it('equivalent Open joins observation resynchronization without reviving revoked control', async () => {
  const s = await setup(); await s.client.attach('A', 'left');
  const proof = s.client.getSnapshot().views.A.attachmentObservation, target = s.target('A');
  s.held.add('session/snapshot');
  s.socket.deliver({ jsonrpc: '2.0', method: 'session/resyncRequired', params: { target, after_cursor: '0', earliest_serviceable: '1' } });
  const read = await s.waitFor('session/snapshot', 1);
  expect(() => s.client.target('A')).toThrow();
  const opening = s.client.attach('A', 'left');
  s.held.delete('session/snapshot'); s.reply(read); await opening;
  expect(s.client.isAttachmentObservationCurrent('A', proof)).toBe(true);
  expect(requests(s).map(row => row.method)).toEqual(['session/attach']);
  expect(s.claims()).toHaveLength(1);
  await s.client.release('A');
  expect(s.client.isAttachmentObservationCurrent('A', proof)).toBe(false);
});


it('equivalent Open after navigation replacement observes the original exact native result without a second claim', async () => {
  const s = await setup(); s.held.add('session/attach');
  let navigation = true;
  const obsoleteNavigation = vi.fn(), latestNavigation = vi.fn();
  const first = s.client.attach('A', 'left', () => navigation, obsoleteNavigation);
  const attach = await s.waitFor('session/attach', 1);
  navigation = false;
  const equivalent = s.client.attach('A', 'left', () => true, latestNavigation);
  s.reply(attach); await Promise.all([first, equivalent]);
  const view = s.client.getSnapshot().views.A;
  expect(obsoleteNavigation).not.toHaveBeenCalled(); expect(latestNavigation).toHaveBeenCalledOnce();
  expect(s.client.isAttachmentObservationCurrent('A', view.attachmentObservation)).toBe(true);
  expect(s.client.target('A').conversation_id).toBe('conversation-left');
  expect(requests(s).map(row => row.method)).toEqual(['session/attach']);
  expect(s.claims()).toEqual([view.target]);
});

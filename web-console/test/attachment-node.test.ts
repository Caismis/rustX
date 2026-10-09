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
  await expect(s.client.switchNode('A', 'right')).rejects.toThrow('switch failed');
  const view = s.client.getSnapshot().views.A;
  expect(view.attachment).toBe('stale'); expect(view.attachmentObservation).toBeUndefined(); expect(view.nodeId).toBeUndefined();
  expect(view.target).toEqual(target); expect(s.claims()).toHaveLength(phase === 'after-unload' ? 0 : 1);
  await expect(s.client.send('A', 'forbidden')).rejects.toThrow('not authoritatively attached');
  expect(requests(s)).toHaveLength(2);
  await s.client.release('A'); expect(s.claims()).toHaveLength(0);
  await s.client.attach('A');
  expect(s.client.target('A').conversation_id).toBe(phase === 'after-unload' ? 'conversation-right' : 'conversation-left');
  expect(requests(s).map(row => row.method)).toEqual(['session/attach', 'session/switchNode', 'session/detach', 'session/attach']);
  expect(s.claims()).toHaveLength(1); expect(s.maxClaims).toBe(1);
});

it('a lost switch response never replays or restores old authority on reconnect', async () => {
  const s = await setup(); await s.client.attach('A', 'left'); s.held.add('session/switchNode');
  const work = s.client.switchNode('A', 'right'), unknown = expect(work).rejects.toBeInstanceOf(OutcomeUncertain);
  s.commit(await s.waitFor('session/switchNode', 1)); s.socket.close(); await unknown;
  await s.connect();
  expect(requests(s).map(row => row.method)).toEqual(['session/attach', 'session/switchNode']);
  expect(s.client.getSnapshot().views.A.attachment).toBe('stale'); expect(s.client.getSnapshot().views.A.nodeId).toBeUndefined();
  expect(s.client.getSnapshot().uncertain.map(row => row.method)).toEqual(['session/switchNode']);
  expect(s.claims()).toHaveLength(0);
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

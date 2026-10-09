import { afterEach, expect, it, vi } from 'vitest';
import type { Request, RuntimeClientTranscriptEntry } from '../../protocol/app-server/v38';
import { Server, snapshot } from './fixture';
import { traceRecord, requestDetail } from './trace-fixture';
const servers: Server[] = [];
afterEach(() => { servers.forEach(s => s.client.disconnect()); vi.restoreAllMocks(); });
const view = (s: Server) => s.client.getSnapshot().views.A;
const count = (s: Server, method: Request['method']) => s.requests.filter(r => r.request.method === method).length;
const entry = (n: number): RuntimeClientTranscriptEntry => ({ cursor: String(n), item: { type: 'message', message: { id: `m${n}`, role: 'assistant', content: [{ type: 'text', text: String(n) }] } } });
async function setup() {
  const s = new Server(); servers.push(s);
  s.snapshots.set('A', { ...snapshot(), transcript: { entries: [entry(10)], next_cursor: '10' }, trace: { records: [traceRecord(10)], next_cursor: 'trace:10' } });
  s.nodeSnapshots.set('right', { ...snapshot(), conversation_id: 'conversation-right' });
  await s.attached('A'); return s;
}
async function revoke(s: Server, kind: 'release' | 'switch') {
  const method = kind === 'release' ? 'session/detach' : 'session/switchNode';
  s.held.add(method);
  const work = kind === 'release' ? s.client.release('A') : s.client.switchNode('A', 'right');
  void work.catch(() => {});
  const request = await s.waitFor(method, 1);
  expect(view(s).attachmentObservation).toBeUndefined();
  expect(s.claims()).toHaveLength(1);
  return { request, work };
}
function event(s: Server, type: 'message' | 'trace' = 'message') {
  s.socket.deliver({ jsonrpc: '2.0', method: 'session/event', params: { target: s.target('A'), cursor: '1', event: type === 'trace' ? { type: 'trace_changed' } : { type: 'message_committed', attempt_id: 'attempt', transcript_cursor: '11', message: { id: 'obsolete', role: 'user', source: 'human', content: [] } } } });
}
it.each(['release', 'switch'] as const)('%s revokes Runtime event publication before native cleanup settles', async kind => {
  const s = await setup(), before = view(s), proof = before.attachmentObservation;
  const reconcile = vi.spyOn(s.client as unknown as { reconcileInteractions(id: string): void }, 'reconcileInteractions');
  const submissions = vi.spyOn(s.client as unknown as { settleSubmissions(id: string): void }, 'settleSubmissions');
  const closing = await revoke(s, kind); reconcile.mockClear(); submissions.mockClear();
  event(s); event(s, 'trace');
  s.socket.deliver({ jsonrpc: '2.0', method: 'session/resyncRequired', params: { target: s.target('A'), after_cursor: '0', earliest_serviceable: '1' } });
  expect(view(s).snapshot).toBe(before.snapshot); expect(view(s).cursor).toBe(before.cursor);
  expect(reconcile).not.toHaveBeenCalled(); expect(submissions).not.toHaveBeenCalled();
  expect(count(s, 'session/snapshot')).toBe(0); expect(count(s, 'session/trace')).toBe(0);
  expect(view(s).target).toBe(before.target); expect(s.client.isAttachmentObservationCurrent('A', proof)).toBe(false);
  s.reply(closing.request); await closing.work;
  expect(s.claims()).toHaveLength(0); expect(count(s, 'session/attach')).toBe(1);
  expect(count(s, 'session/detach')).toBe(kind === 'release' ? 1 : 0); expect(count(s, 'session/switchNode')).toBe(kind === 'switch' ? 1 : 0);
  expect(count(s, 'turn/start')).toBe(0);
  if (kind === 'switch') { expect(view(s).nodeId).toBe('right'); expect(s.residentConversations.get('A')).toBe('conversation-right'); expect(view(s).snapshot).toBeUndefined(); }
});
it('a held Snapshot cannot publish or reconcile after Release, and cleanup retains the exact claim', async () => {
  const s = await setup(), before = view(s); s.held.add('session/snapshot');
  const refresh = s.client.refresh('A'), read = await s.waitFor('session/snapshot', 1);
  const closing = await revoke(s, 'release');
  const reconcile = vi.spyOn(s.client as unknown as { reconcileInteractions(id: string): void }, 'reconcileInteractions');
  s.socket.success(read, { type: 'snapshot', snapshot: { ...snapshot(), messages: [{ id: 'obsolete', role: 'assistant', content: [] }], transcript: { entries: [entry(99)] }, trace: { records: [traceRecord(99)] } }, cursor: '99' });
  await refresh;
  expect(view(s).snapshot).toBe(before.snapshot); expect(view(s).cursor).toBe(before.cursor);
  expect(view(s).history?.page).toEqual(before.history?.page); expect(view(s).trace?.page).toEqual(before.trace?.page);
  expect(reconcile).not.toHaveBeenCalled(); expect(view(s).target).toBe(before.target);
  expect(count(s, 'session/snapshot')).toBe(1); expect(count(s, 'session/subscribe')).toBe(0);
  s.reply(closing.request); await closing.work; expect(s.claims()).toHaveLength(0);
});
it.each(['latest', 'earlier', 'detail'] as const)('held Trace %s cannot publish after Release', async kind => {
  const s = await setup(), before = view(s).trace!;
  const method = kind === 'detail' ? 'session/traceDetail' : 'session/trace'; s.held.add(method);
  const work = kind === 'detail' ? s.client.loadTraceDetail('A', 'trace:10') : kind === 'earlier' ? s.client.loadEarlierTrace('A')
    : (s.client as unknown as { refreshTraceDomain(id: string): Promise<void> }).refreshTraceDomain('A');
  const read = await s.waitFor(method, 1), closing = await revoke(s, 'release');
  s.handlers.set('session/trace', () => ({ type: 'trace', page: { records: [traceRecord(9)] } }));
  s.held.delete(method);
  s.socket.success(read, kind === 'detail' ? { type: 'trace_detail', detail: requestDetail(10) } : { type: 'trace', page: { records: [traceRecord(9)], next_cursor: null } });
  await work;
  expect(view(s).trace?.page).toEqual(before.page); expect(view(s).trace?.details['trace:10']).toBeUndefined(); expect(view(s).trace?.loading).not.toBe(true);
  expect(count(s, method)).toBe(1); expect(view(s).target).toEqual(s.target('A'));
  s.reply(closing.request); await closing.work; expect(s.claims()).toHaveLength(0);
});
function published(s: Server, predicate: () => boolean) {
  if (predicate()) return Promise.resolve();
  return new Promise<void>(resolve => { const stop = s.client.subscribe(() => { if (predicate()) { stop(); resolve(); } }); });
}
it('resync keeps observation proof while controls wait for Snapshot and subscription ACK', async () => {
  const s = await setup(), proof = view(s).attachmentObservation, target = s.target('A');
  s.held.add('session/snapshot'); s.held.add('session/subscribe');
  s.socket.deliver({ jsonrpc: '2.0', method: 'session/resyncRequired', params: { target, after_cursor: '0', earliest_serviceable: '1' } });
  const read = await s.waitFor('session/snapshot', 1);
  expect(view(s).attachment).toBe('resynchronizing'); expect(view(s).attachmentObservation).toBe(proof);
  expect(s.client.isAttachmentObservationCurrent('A', proof)).toBe(true);
  expect(s.client.isAttachmentControlCurrent('A', proof)).toBe(false); expect(() => s.client.target('A')).toThrow();
  s.socket.success(read, { type: 'snapshot', snapshot: { ...snapshot(), transcript: { entries: [entry(20)] } }, cursor: '20' });
  const subscription = await s.waitFor('session/subscribe', 1);
  expect(subscription.params).toEqual({ target, after_cursor: '20' });
  expect(view(s).cursor).toBe('20'); expect(s.client.isAttachmentControlCurrent('A', proof)).toBe(false);
  // Contiguous replacement registration replay may precede its subscribe ACK.
  s.socket.deliver({ jsonrpc: '2.0', method: 'session/event', params: { target, cursor: '21', event: { type: 'message_committed', attempt_id: 'attempt', transcript_cursor: '21', message: { id: 'current', role: 'user', source: 'human', content: [] } } } });
  expect(view(s).cursor).toBe('21'); expect(view(s).snapshot?.messages[0].id).toBe('current');
  const ready = published(s, () => view(s).attachment === 'attached'); s.reply(subscription); await ready;
  expect(s.client.isAttachmentControlCurrent('A', proof)).toBe(true); expect(s.client.target('A')).toEqual(target);
  expect(count(s, 'session/snapshot')).toBe(1); expect(count(s, 'session/subscribe')).toBe(1);
  expect(count(s, 'session/attach')).toBe(1); expect(count(s, 'session/detach')).toBe(0); expect(s.claims()).toHaveLength(1);
});
it.each(['release', 'switch', 'delete'] as const)('held Turn outline and History navigation retire on %s', async kind => {
  const s = await setup();
  const cut = { conversation_id: 'conversation-A', journal: '10', transcript: '10', mutation_revision: '0' };
  const turn = { id: { conversation_id: 'conversation-A', attempt_id: 'old-attempt' }, ordinal: 1, cursor: '1', prompt: '', response: '' };
  s.handlers.set('session/turns', () => ({ type: 'conversation_turns', page: { cut, offset: 0, total: 1, turns: [turn] } }));
  await s.client.readTurns('A');
  s.held.add('session/turns'); s.held.add('session/transcript');
  const outline = s.client.readTurns('A'), outlineRead = await s.waitFor('session/turns', 2);
  const older = s.client.loadEarlier('A'), pageRead = await s.waitFor('session/transcript', 1);
  const navigation = s.client.navigateTurn('A', turn), navRead = await s.waitFor('session/transcript', 2);
  let finish: () => Promise<unknown>;
  if (kind === 'delete') {
    s.held.add('session/delete'); const deleting = s.client.deleteSession('A', 'reviewed'); void deleting.catch(() => {});
    const request = await s.waitFor('session/delete', 1);
    finish = async () => { s.socket.success(request, { type: 'deletion', result: { status: 'stale', session_id: 'A' } }); return deleting; };
  } else { const closing = await revoke(s, kind); finish = async () => { s.reply(closing.request); return closing.work; }; }
  const before = view(s);
  expect(before.history?.loading).not.toBe(true); expect(before.turnOutline?.loading).not.toBe(true); expect(before.turnNavigation?.pending).toBeUndefined();
  s.reply(outlineRead);
  for (const request of [pageRead, navRead]) s.socket.success(request, { type: 'transcript_window', window: { cut, page: { entries: [entry(1)] }, target: turn.id, target_cursor: '1' } });
  await outline; await older; expect(await navigation).toBe(false);
  expect(view(s).history).toBe(before.history); expect(view(s).turnOutline).toBe(before.turnOutline); expect(view(s).turnNavigation).toBe(before.turnNavigation);
  s.client.returnToLatest('A'); expect(view(s).history).toBe(before.history);
  expect(count(s, 'session/transcript')).toBe(2); expect(count(s, 'session/turns')).toBe(2);
  await finish(); expect(view(s).attachmentObservation).toBeUndefined();
});
it('old Snapshot completion cannot remove a fresh Attachment refresh worker', async () => {
  const s = await setup(), oldProof = view(s).attachmentObservation, oldTarget = s.target('A');
  s.held.add('session/snapshot'); const old = s.client.refresh('A'), first = await s.waitFor('session/snapshot', 1);
  await s.client.release('A'); await s.client.attach('A');
  const proof = view(s).attachmentObservation, target = s.target('A'); expect(proof).not.toBe(oldProof); expect(target.attachment_id).not.toBe(oldTarget.attachment_id);
  const fresh = s.client.refresh('A'), second = await s.waitFor('session/snapshot', 2);
  s.socket.success(first, { type: 'snapshot', snapshot: { ...snapshot(), messages: [{ id: 'old', role: 'assistant', content: [] }] }, cursor: '90' }); await old;
  expect(view(s).snapshot?.messages).toEqual([]); expect(view(s).cursor).toBe('0');
  // Coalesced new demand must join the fresh worker, then discharge one later read.
  const joined = s.client.refresh('A'); expect(joined).toBe(fresh);
  s.socket.success(second, { type: 'snapshot', snapshot: snapshot(), cursor: '2' });
  const third = await s.waitFor('session/snapshot', 3); s.socket.success(third, { type: 'snapshot', snapshot: { ...snapshot(), messages: [{ id: 'new', role: 'assistant', content: [] }] }, cursor: '3' });
  await fresh; expect(view(s).cursor).toBe('3'); expect(view(s).snapshot?.messages[0].id).toBe('new');
  expect(s.client.isAttachmentObservationCurrent('A', oldProof)).toBe(false); expect(s.client.isAttachmentObservationCurrent('A', proof)).toBe(true);
  expect(count(s, 'session/snapshot')).toBe(3); expect(count(s, 'session/attach')).toBe(2); expect(count(s, 'session/detach')).toBe(1); expect(s.claims()).toHaveLength(1); expect(s.maxClaims).toBe(1);
});
it.each([false, true])('late Snapshot cannot enter restored generation (authority replacement=%s)', async replacement => {
  const s = await setup(), oldProof = view(s).attachmentObservation, socket = s.socket;
  s.held.add('session/snapshot'); const old = s.client.refresh('A'); const failed = old.catch(error => error);
  const read = await s.waitFor('session/snapshot', 1);
  await s.client.disconnect(); await failed; if (replacement) s.authorityId = 'replacement';
  s.held.delete('session/snapshot'); await s.connect(); if (!view(s)?.target) await s.client.attach('A');
  const current = view(s); socket.success(read, { type: 'snapshot', snapshot: snapshot(), cursor: '900' });
  expect(view(s)).toBe(current); expect(s.client.isAttachmentObservationCurrent('A', oldProof)).toBe(false); expect(s.claims()).toHaveLength(1);
  expect(count(s, 'session/attach')).toBe(2); expect(count(s, 'session/snapshot')).toBe(1);
});
it('RPC capacity cannot dispatch a Snapshot after its observation proof is revoked', async () => {
  const s = await setup(); s.held.add('session/read');
  const blockers = Array.from({ length: 8 }, () => s.client.request({ method: 'session/read', params: { session_id: 'A' } }, 'session'));
  const reads = s.requests.filter(r => r.request.method === 'session/read').slice(-8);
  const refresh = s.client.refresh('A'); const refused = refresh.catch(error => error);
  expect(count(s, 'session/snapshot')).toBe(0);
  s.held.add('session/detach'); const release = s.client.release('A'); void release.catch(() => {});
  // Free the real dispatch lane; the queued Snapshot must fail its captured proof.
  reads.forEach(({ request }) => s.reply(request)); await Promise.all(blockers);
  const detach = await s.waitFor('session/detach', 1);
  expect(String(await refused)).toContain('No operation was sent'); expect(count(s, 'session/snapshot')).toBe(0);
  s.reply(detach); await release;
  expect(view(s).attachment).toBe('detached'); expect(s.claims()).toHaveLength(0);
  s.held.delete('session/read'); await s.client.attach('A'); await s.client.refresh('A');
  expect(count(s, 'session/attach')).toBe(2); expect(count(s, 'session/snapshot')).toBe(1); expect(s.claims()).toHaveLength(1);
});
it('failed detach retains only cleanup ownership, and old notifications cannot trigger observation repair', async () => {
  const s = await setup(), before = view(s), closing = await revoke(s, 'release');
  s.socket.deliver({ jsonrpc: '2.0', id: closing.request.id, error: { code: -32000, message: 'busy', data: { kind: 'operation_failed' } } });
  await expect(closing.work).rejects.toThrow('busy');
  event(s); event(s, 'trace');
  s.socket.deliver({ jsonrpc: '2.0', method: 'session/resyncRequired', params: { target: s.target('A'), after_cursor: '0', earliest_serviceable: '1' } });
  await s.client.refresh('A'); await s.client.loadTraceDetail('A', 'trace:10'); await s.client.readTurns('A');
  expect(view(s).snapshot).toBe(before.snapshot); expect(view(s).cursor).toBe(before.cursor); expect(view(s).target).toBe(before.target);
  expect(() => s.client.target('A')).toThrow(); expect(s.claims()).toHaveLength(1);
  expect(count(s, 'session/snapshot')).toBe(0); expect(count(s, 'session/trace')).toBe(0); expect(count(s, 'session/traceDetail')).toBe(0); expect(count(s, 'session/turns')).toBe(0);
  expect(count(s, 'session/attach')).toBe(1); expect(count(s, 'session/detach')).toBe(1);
  const release = s.client.release('A'); s.reply(await s.waitFor('session/detach', 2)); await release; expect(s.claims()).toHaveLength(0);
});
it('final async validation cannot dispatch a revoked Snapshot or retain its reservation', async () => {
  const s = await setup(); let entered!: () => void, resume!: () => void, signal!: AbortSignal;
  const ready = new Promise<void>(resolve => { entered = resolve; }), gate = new Promise<void>(resolve => { resume = resolve; });
  const request = s.client.request.bind(s.client);
  vi.spyOn(s.client, 'request').mockImplementation(((...args: Parameters<typeof request>) => {
    if (args[0].method === 'session/snapshot') {
      const admission = args[3];
      args[3] = { current: () => typeof admission === 'function' ? admission() : !!admission?.current(), validate: async value => { signal = value; entered(); await gate; return true; } };
    }
    return request(...args);
  }) as typeof request);
  const refresh = s.client.refresh('A'), refused = refresh.catch(error => error); await ready;
  expect(count(s, 'session/snapshot')).toBe(0);
  const closing = await revoke(s, 'release'); resume();
  expect(String(await refused)).toContain('No operation was sent'); expect(signal.aborted).toBe(true);
  expect(count(s, 'session/snapshot')).toBe(0); s.reply(closing.request); await closing.work;
  expect(view(s).attachment).toBe('detached'); expect(s.claims()).toHaveLength(0);
  vi.restoreAllMocks(); await s.client.attach('A'); await s.client.refresh('A');
  expect(count(s, 'session/snapshot')).toBe(1); expect(s.claims()).toHaveLength(1);
});

it.each([true, false])('pending inbound invalidation rechecks after synchronous publication (Release=%s)', async releaseDuringInvalidation => {
  const s = await setup(), cut = { conversation_id: 'conversation-A', journal: '10', transcript: '10', mutation_revision: '0' };
  s.handlers.set('session/turns', () => ({ type: 'conversation_turns', page: { cut, offset: 0, total: 0, turns: [] } }));
  await s.client.readTurns('A'); s.held.add('session/transcript'); s.held.add('session/detach');
  const page = s.client.loadEarlier('A'), read = await s.waitFor('session/transcript', 1);
  const before = view(s); let released: ReturnType<typeof view> | undefined, release: Promise<void> | undefined;
  const stop = s.client.subscribe(() => {
    if (releaseDuringInvalidation && !released && view(s).history?.loading === false) {
      released = view(s); release = s.client.release('A'); void release.catch(() => {}); released = view(s);
    }
  });
  const reconcile = vi.spyOn(s.client as unknown as { reconcileInteractions(id: string): void }, 'reconcileInteractions');
  const submissions = vi.spyOn(s.client as unknown as { settleSubmissions(id: string): void }, 'settleSubmissions');
  s.socket.deliver({ jsonrpc: '2.0', method: 'session/event', params: { target: s.target('A'), cursor: '1', event: { type: 'pending_inbound_changed', pending: [] } } }); stop();
  if (releaseDuringInvalidation) {
    expect(released).toBeDefined(); expect(view(s)).toBe(released);
    expect(view(s).snapshot).toBe(before.snapshot); expect(view(s).cursor).toBe(before.cursor);
    expect(view(s).history?.page).toBe(before.history?.page); expect(view(s).turnOutline?.page).toBe(before.turnOutline?.page);
    expect(reconcile).not.toHaveBeenCalled(); expect(submissions).not.toHaveBeenCalled(); expect(view(s).target).toBe(before.target);
  } else {
    expect(view(s).cursor).toBe('1'); expect(view(s).snapshot?.inbound.pending).toEqual([]);
    expect(view(s).turnOutline?.error).toContain('History changed'); expect(view(s).history?.loading).toBe(false);
    expect(reconcile).toHaveBeenCalledTimes(1); expect(submissions).toHaveBeenCalledTimes(1);
  }
  s.socket.success(read, { type: 'transcript_window', window: { cut, page: { entries: [entry(1)] } } }); await page;
  expect(count(s, 'session/snapshot')).toBe(0); expect(count(s, 'turn/start')).toBe(0);
  if (release) { s.reply(await s.waitFor('session/detach', 1)); await release; expect(s.claims()).toHaveLength(0); }
  else expect(s.claims()).toHaveLength(1);
});
it('pending inbound replay during resync observes without restoring control before subscribe ACK', async () => {
  const s = await setup(), proof = view(s).attachmentObservation, target = s.target('A');
  s.held.add('session/snapshot'); s.held.add('session/subscribe');
  s.socket.deliver({ jsonrpc: '2.0', method: 'session/resyncRequired', params: { target, after_cursor: '0', earliest_serviceable: '1' } });
  s.socket.success(await s.waitFor('session/snapshot', 1), { type: 'snapshot', snapshot: snapshot(), cursor: '20' });
  const subscription = await s.waitFor('session/subscribe', 1);
  s.socket.deliver({ jsonrpc: '2.0', method: 'session/event', params: { target, cursor: '21', event: { type: 'pending_inbound_changed', pending: [] } } });
  expect(view(s).cursor).toBe('21'); expect(view(s).snapshot?.inbound.pending).toEqual([]); expect(view(s).turnOutline?.error).toContain('History changed');
  expect(s.client.isAttachmentObservationCurrent('A', proof)).toBe(true); expect(s.client.isAttachmentControlCurrent('A', proof)).toBe(false);
  const ready = published(s, () => view(s).attachment === 'attached'); s.reply(subscription); await ready;
  expect(s.client.isAttachmentControlCurrent('A', proof)).toBe(true); expect(count(s, 'session/snapshot')).toBe(1); expect(count(s, 'session/subscribe')).toBe(1); expect(s.claims()).toHaveLength(1);
});

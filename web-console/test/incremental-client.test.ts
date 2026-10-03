import { incrementalAgent } from './incremental-agent-fixture';
import { afterEach, expect, it } from 'vitest';
import { Server } from './fixture';
import { traceRecord } from './trace-fixture';
import type { RuntimeClientEvent } from '../../protocol/app-server/v34';
const servers: Server[] = [];
const create = async () => { const s = new Server(); servers.push(s); await s.attached('A'); return s; };
afterEach(() => { for (const s of servers) s.client.disconnect(); servers.length = 0; });
function emit(s: Server, cursor: string, event: RuntimeClientEvent) {
  s.socket.deliver({ jsonrpc: '2.0', method: 'session/event', params: { target: s.target('A'), cursor, event } });
}
const started: RuntimeClientEvent = { type: 'attempt_started', attempt_id: 'attempt' };
const open: RuntimeClientEvent = { type: 'assistant_message_started', attempt_id: 'attempt', message_id: 'message' };
const delta: RuntimeClientEvent = { type: 'assistant_text_delta', attempt_id: 'attempt', message_id: 'message', block_index: 0, delta: 'x' };
it('production client consumes 100 contiguous deltas once with zero snapshot RPCs', async () => {
  const s = await create(); emit(s, '1', started); emit(s, '2', open);
  for (let cursor = 3; cursor <= 102; cursor++) { emit(s, String(cursor), delta); emit(s, String(cursor), delta); }
  expect(s.client.getSnapshot().views.A.snapshot?.attempt?.in_flight?.blocks).toEqual([{ type: 'text', block_index: 0, text: 'x'.repeat(100) }]);
  expect(s.requests.filter(row => row.request.method === 'session/snapshot')).toHaveLength(0);
});
it('gap retires continuation and repeated resync coalesces; snapshot cursor joins replay', async () => {
  const s = await create(); emit(s, '1', started); emit(s, '2', open);
  s.held.add('session/snapshot'); emit(s, '4', delta);
  const request = await s.waitFor('session/snapshot', 1);
  for (let i = 0; i < 10; i++) s.socket.deliver({ jsonrpc: '2.0', method: 'session/resyncRequired', params: { target: s.target('A'), after_cursor: '2', earliest_serviceable: '4' } });
  emit(s, '5', delta);
  expect(s.client.getSnapshot().views.A.snapshot?.attempt?.in_flight?.blocks).toEqual([]);
  expect(s.requests.filter(row => row.request.method === 'session/snapshot')).toHaveLength(1);
  const authoritative = { ...s.snapshots.get('A')!, attempt: { attempt_id: 'attempt', turn: 0, phase: { type: 'running' as const }, in_flight: { message_id: 'message', blocks: [{ type: 'text' as const, block_index: 0, text: 'native' }] } } };
  s.socket.success(request, { type: 'snapshot', snapshot: authoritative, cursor: '5' });
  await s.waitFor('session/subscribe', 1);
  await new Promise<void>(resolve => { if (s.client.getSnapshot().views.A.attachment === 'attached') return resolve(); const stop = s.client.subscribe(() => { if (s.client.getSnapshot().views.A.attachment === 'attached') { stop(); resolve(); } }); });
  emit(s, '5', delta); emit(s, '6', delta);
  expect(s.client.getSnapshot().views.A.snapshot?.attempt?.in_flight?.blocks).toEqual([{ type: 'text', block_index: 0, text: 'nativex' }]);
});
it('an explicit snapshot overlapping events installs its cut then replays only later cursors', async () => {
  const s = await create(); emit(s, '1', started); emit(s, '2', open);
  const before = s.client.getSnapshot().views.A.snapshot!;
  s.held.add('session/snapshot'); const read = s.client.refresh('A');
  const request = await s.waitFor('session/snapshot', 1);
  emit(s, '3', delta); emit(s, '4', delta);
  s.socket.success(request, { type: 'snapshot', snapshot: before, cursor: '2' });
  await read;
  emit(s, '3', delta); emit(s, '4', delta); emit(s, '4', delta);
  expect(s.client.getSnapshot().views.A.snapshot?.attempt?.in_flight?.blocks?.[0]).toMatchObject({ text: 'xx' });
  expect(s.requests.filter(row => row.request.method === 'session/snapshot')).toHaveLength(1);
});
it('canonical commitment retires streaming and late suffixes cannot reopen it', async () => {
  const s = await create(); emit(s, '1', started); emit(s, '2', open); emit(s, '3', delta);
  const message = { role: 'assistant' as const, id: 'message', content: [{ type: 'text' as const, text: 'canonical differs' }] };
  emit(s, '4', { type: 'message_committed', attempt_id: 'attempt', message, transcript_cursor: '1' });
  emit(s, '5', delta); emit(s, '6', open);
  const snapshot = s.client.getSnapshot().views.A.snapshot!;
  expect(snapshot.attempt?.in_flight).toBeUndefined(); expect(snapshot.messages).toEqual([message]);
  expect(snapshot.transcript.entries).toHaveLength(1);
});
it('failed snapshot remains stale until explicit recovery and never replays a mutation', async () => {
  const s = await create(); emit(s, '1', started);
  s.held.add('session/snapshot'); emit(s, '3', open);
  const request = await s.waitFor('session/snapshot', 1);
  const failed = s.client.refresh('A').catch(error => error);
  s.socket.deliver({ jsonrpc: '2.0', id: request.id, error: { code: -32000, message: 'read failed', data: { kind: 'invalid_state' } } });
  expect(await failed).toBeInstanceOf(Error);
  expect(s.client.getSnapshot().views.A.attachment).toBe('stale');
  expect(s.client.getSnapshot().views.A.cursor).toBe('1');
  s.held.delete('session/snapshot'); s.cursor = 4n;
  await s.client.refresh('A');
  expect(s.client.getSnapshot().views.A.cursor).toBe('4');
  expect(s.requests.filter(row => row.request.method === 'session/snapshot')).toHaveLength(2);
  expect(s.requests.some(row => ['turn/start', 'turn/steer', 'session/uploadPrepare', 'session/create'].includes(row.request.method))).toBe(false);
});
it('same Session replacement fences the old snapshot and old events by exact attachment', async () => {
  const s = await create(); const oldTarget = s.target('A');
  s.held.add('session/snapshot'); const oldRead = s.client.refresh('A');
  const request = await s.waitFor('session/snapshot', 1);
  await s.client.release('A'); await s.client.attach('A');
  const current = s.client.getSnapshot().views.A.snapshot;
  expect(s.target('A').attachment_id).not.toBe(oldTarget.attachment_id);
  s.socket.success(request, { type: 'snapshot', snapshot: { ...current!, shutting_down: true }, cursor: '99' }); await oldRead;
  s.socket.deliver({ jsonrpc: '2.0', method: 'session/event', params: { target: oldTarget, cursor: '100', event: { type: 'runtime_shutdown' } } });
  expect(s.client.getSnapshot().views.A.snapshot).toBe(current);
  expect(s.client.getSnapshot().views.A.cursor).toBe('0');
});
it('notifications preceding the attach response join the returned cursor through native replay', async () => {
  const s = new Server(); servers.push(s); await s.connect(); s.held.add('session/attach');
  const attaching = s.client.attach('A'); const request = await s.waitFor('session/attach', 1);
  const ack = s.commit(request);
  emit(s, '1', started); emit(s, '2', open);
  s.socket.deliver(ack); await attaching;
  const subscribe = await s.waitFor('session/subscribe', 1);
  expect(subscribe.params).toMatchObject({ after_cursor: '0' });
  emit(s, '1', started); emit(s, '2', open); emit(s, '3', delta);
  expect(s.client.getSnapshot().views.A.snapshot?.attempt?.in_flight?.blocks?.[0]).toMatchObject({ text: 'x' });
  expect(s.requests.filter(row => row.request.method === 'session/snapshot')).toHaveLength(0);
});

it('resync during an explicit snapshot survives acquisition and installs an exact subscription handoff', async () => {
  const s = await create(); emit(s, '1', started); emit(s, '2', open);
  const target = s.target('A');
  const authoritative = s.client.getSnapshot().views.A.snapshot!;
  s.held.add('session/snapshot'); s.held.add('session/subscribe');
  const refresh = s.client.refresh('A');
  const request = await s.waitFor('session/snapshot', 1);
  s.socket.deliver({ jsonrpc: '2.0', method: 'session/resyncRequired', params: { target, after_cursor: '2', earliest_serviceable: '5' } });
  expect(s.client.getSnapshot().views.A.attachment).toBe('resynchronizing');
  expect(s.client.getSnapshot().views.A.cursor).toBe('2');
  s.socket.success(request, { type: 'snapshot', snapshot: authoritative, cursor: '7' });
  const subscribe = await s.waitFor('session/subscribe', 1);
  expect(subscribe.params).toEqual({ target, after_cursor: '7' });
  expect(s.client.getSnapshot().views.A.attachment).toBe('resynchronizing');
  emit(s, '3', delta); // An in-flight old-registration event precedes the acquired cut.
  expect(s.client.getSnapshot().views.A.cursor).toBe('7');
  s.socket.success(subscribe, { type: 'subscribed', after_cursor: '7' });
  await refresh;
  expect(s.client.getSnapshot().views.A).toMatchObject({ target, cursor: '7', attachment: 'attached' });
  expect(s.client.getSnapshot().views.A.snapshot?.attempt?.in_flight?.blocks).toEqual([]);
  expect(s.requests.filter(row => row.request.method === 'session/snapshot')).toHaveLength(1);
  expect(s.requests.filter(row => row.request.method === 'session/subscribe')).toHaveLength(1);
  expect(s.requests.some(row => ['turn/start', 'turn/steer', 'session/uploadPrepare', 'session/create'].includes(row.request.method))).toBe(false);
  emit(s, '8', delta); // Now the registered continuation advances normally.
  expect(s.client.getSnapshot().views.A.cursor).toBe('8');
});

it.each([2, null])('acquisition-time resync uses bounded authoritative replay repair (success at %s)', async successAt => {
  const s = await create();
  s.held.add('session/snapshot'); s.held.add('session/subscribe');
  const refresh = s.client.refresh('A').catch(error => error);
  await s.waitFor('session/snapshot', 1);
  s.socket.deliver({ jsonrpc: '2.0', method: 'session/resyncRequired', params: { target: s.target('A'), after_cursor: '0', earliest_serviceable: '7' } });
  const attempts = successAt ?? 3;
  for (let attempt = 1; attempt <= attempts; attempt++) {
    const snapshot = await s.waitFor('session/snapshot', attempt);
    const cursor = String(6 + attempt);
    s.socket.success(snapshot, { type: 'snapshot', snapshot: s.snapshots.get('A')!, cursor });
    const subscribe = await s.waitFor('session/subscribe', attempt);
    expect(subscribe.params).toEqual({ target: s.target('A'), after_cursor: cursor });
    expect(s.client.getSnapshot().views.A.attachment).toBe('resynchronizing');
    if (attempt === successAt) s.socket.success(subscribe, { type: 'subscribed', after_cursor: cursor });
    else s.socket.deliver({ jsonrpc: '2.0', id: subscribe.id, error: { code: -32000, message: 'replay expired', data: { kind: 'resync_required' } } });
  }
  const result = await refresh;
  if (!successAt) expect(result).toBeInstanceOf(Error);
  expect(s.client.getSnapshot().views.A).toMatchObject({ cursor: String(6 + attempts), attachment: successAt ? 'attached' : 'stale' });
  expect(s.requests.filter(row => row.request.method === 'session/snapshot')).toHaveLength(attempts);
  expect(s.requests.filter(row => row.request.method === 'session/subscribe')).toHaveLength(attempts);
  expect(s.requests.some(row => ['turn/start', 'turn/steer', 'session/uploadPrepare', 'session/create'].includes(row.request.method))).toBe(false);
});

it('registered native replay preceding subscribe ACK advances the cut without enabling controls early', async () => {
  const s = await create(); emit(s, '1', started); emit(s, '2', open);
  const authoritative = s.client.getSnapshot().views.A.snapshot!;
  s.held.add('session/snapshot'); s.held.add('session/subscribe');
  const refresh = s.client.refresh('A');
  const snapshot = await s.waitFor('session/snapshot', 1);
  s.socket.deliver({ jsonrpc: '2.0', method: 'session/resyncRequired', params: { target: s.target('A'), after_cursor: '2', earliest_serviceable: '5' } });
  s.socket.success(snapshot, { type: 'snapshot', snapshot: authoritative, cursor: '7' });
  const subscribe = await s.waitFor('session/subscribe', 1);
  const acknowledgement = s.commit(subscribe); // Native registration precedes its response delivery.
  emit(s, '3', delta); emit(s, '8', delta); emit(s, '8', delta);
  expect(s.client.getSnapshot().views.A).toMatchObject({ cursor: '8', attachment: 'resynchronizing' });
  expect(s.client.getSnapshot().views.A.snapshot?.attempt?.in_flight?.blocks?.[0]).toMatchObject({ text: 'x' });
  s.socket.deliver(acknowledgement); await refresh;
  expect(s.client.getSnapshot().views.A).toMatchObject({ cursor: '8', attachment: 'attached' });
  expect(s.requests.filter(row => row.request.method === 'session/snapshot')).toHaveLength(1);
  expect(s.requests.filter(row => row.request.method === 'session/subscribe')).toHaveLength(1);
});

// Await the actual coalesced owner so rejection is observed without sleeps or
// a subsequent invalidation that could conceal a stale installation.
it.each(['explicit refresh', 'resync'] as const)('snapshot Trace authority supersedes a held older read during %s', async mode => {
  const s = new Server(); servers.push(s);
  const running = traceRecord(1, { state: 'running', timing: { started_at: '2026-09-15T00:00:00Z' }, message_id: null });
  const completed = traceRecord(1, { message_id: 'terminal-message' });
  s.snapshots.set('A', { ...s.snapshots.get('A')!, trace: { records: [running] } });
  await s.attached('A');
  const target = s.target('A'), epoch = s.client.getSnapshot().views.A.trace!.epoch;
  s.held.add('session/trace'); s.held.add('session/snapshot');
  emit(s, '1', { type: 'trace_changed' });
  const old = await s.waitFor('session/trace', 1);
  const oldWork = s.client['traceReads'].get('A')!.work;
  if (mode === 'resync') s.socket.deliver({ jsonrpc: '2.0', method: 'session/resyncRequired', params: { target, after_cursor: '1', earliest_serviceable: '7' } });
  const refreshed = mode === 'resync' ? s.client['refreshes'].get('A')! : s.client.refresh('A');
  const request = await s.waitFor('session/snapshot', 1);
  s.socket.success(request, { type: 'snapshot', cursor: '7', snapshot: { ...s.snapshots.get('A')!, trace: { records: [completed] } } });
  await refreshed;
  const authoritative = s.client.getSnapshot().views.A;
  if (mode === 'explicit refresh') expect(authoritative.trace!.epoch).toBe(epoch);
  expect(authoritative.trace!.page.records[0]).toEqual(completed);
  s.socket.success(old, { type: 'trace', page: { records: [running] } });
  await oldWork;
  expect(s.client.getSnapshot().views.A.trace!.page.records[0].state).toBe('completed');
  expect(s.client.getSnapshot().views.A.trace!.page.records[0].timing).toEqual(completed.timing);
  expect(s.client.getSnapshot().views.A).toBe(authoritative);
  expect(authoritative).toMatchObject({ target, cursor: '7', attachment: 'attached' });
  expect(s.requests.filter(row => row.request.method === 'session/trace')).toHaveLength(1);

  emit(s, '8', { type: 'trace_changed' });
  const fresh = await s.waitFor('session/trace', 2);
  const freshWork = s.client['traceReads'].get('A')!.work;
  s.socket.success(fresh, { type: 'trace', page: { records: [completed, traceRecord(2)] } });
  await freshWork;
  expect(s.client.getSnapshot().views.A.trace!.page.records).toEqual([completed, traceRecord(2)]);
  expect(s.client.getSnapshot().views.A).toMatchObject({ target, cursor: '8', attachment: 'attached' });
  expect(s.requests.filter(row => row.request.method === 'session/snapshot')).toHaveLength(1);
});

it('same Session attachment replacement rejects a held Trace response', async () => {
  const s = await create(); const target = s.target('A');
  s.held.add('session/trace'); emit(s, '1', { type: 'trace_changed' });
  const old = await s.waitFor('session/trace', 1);
  const oldWork = s.client['traceReads'].get('A')!.work;
  await s.client.release('A'); await s.client.attach('A');
  const replacement = s.client.getSnapshot().views.A;
  expect(replacement.target).not.toEqual(target);
  s.socket.success(old, { type: 'trace', page: { records: [traceRecord(1)] } });
  await oldWork;
  expect(s.client.getSnapshot().views.A).toBe(replacement);
});

it.each(['snapshot', 'latest'] as const)('%s supersession keeps a burst bounded to one active tail read and one follow-up', async owner => {
  const s = await create(); s.held.add('session/trace');
  emit(s, '1', { type: 'trace_changed' });
  const old = await s.waitFor('session/trace', 1);
  const work = s.client['traceReads'].get('A')!.work;
  if (owner === 'snapshot') { s.cursor = 1n; await s.client.refresh('A'); }
  else s.client.latestTrace('A');
  const trace = s.client.getSnapshot().views.A.trace;
  for (let cursor = 2; cursor <= 101; cursor++) emit(s, String(cursor), { type: 'trace_changed' });
  expect(s.requests.filter(row => row.request.method === 'session/trace')).toHaveLength(1);
  s.socket.success(old, { type: 'trace', page: { records: [traceRecord(1, { state: 'running' })] } });
  const fresh = await s.waitFor('session/trace', 2);
  expect(s.client.getSnapshot().views.A.trace).toBe(trace);
  s.socket.success(fresh, { type: 'trace', page: { records: [traceRecord(1)] } });
  await work;
  expect(s.client.getSnapshot().views.A.trace!.page.records).toEqual([traceRecord(1)]);
  expect(s.requests.filter(row => row.request.method === 'session/trace')).toHaveLength(2);
  expect(s.requests.filter(row => row.request.method === 'session/snapshot')).toHaveLength(owner === 'snapshot' ? 1 : 0);
});

it.each([
  { owner: 'snapshot', invalidations: 1 },
  { owner: 'latest', invalidations: 0 },
  { owner: 'existing authority', invalidations: 100 },
])('failed Trace iteration preserves the owed read for $owner ($invalidations invalidations)', async ({ owner, invalidations }) => {
  const s = await create(); s.held.add('session/trace');
  emit(s, '1', { type: 'trace_changed' });
  const old = await s.waitFor('session/trace', 1);
  const work = s.client['traceReads'].get('A')!.work;
  const authority = s.client['traceAuthorities'].get('A');
  if (owner === 'snapshot') {
    s.cursor = 1n;
    s.snapshots.set('A', { ...s.snapshots.get('A')!, trace: { records: [traceRecord(1)] } });
    await s.client.refresh('A');
  } else if (owner === 'latest') s.client.latestTrace('A');
  if (owner !== 'existing authority') expect(s.client['traceAuthorities'].get('A')).not.toBe(authority);
  if (owner === 'latest') expect(s.client.getSnapshot().views.A.trace!.page.records).toEqual([]);
  for (let cursor = 2; cursor <= invalidations + 1; cursor++) emit(s, String(cursor), { type: 'trace_changed' });
  const beforeFailure = s.client.getSnapshot().views.A;
  expect(s.requests.filter(row => row.request.method === 'session/trace')).toHaveLength(1);
  // Fail immediately if the owner exits instead of dispatching the owed read;
  // neither a timeout nor another notification establishes the ordering.
  const next = Promise.race([
    s.waitFor('session/trace', 2),
    work.then(() => { throw new Error('Trace owner exited with a read still owed'); }),
  ]);
  s.socket.deliver({ jsonrpc: '2.0', id: old.id, error: { code: -32000, message: 'older Trace read failed', data: { kind: 'invalid_state' } } });
  const fresh = await next;
  expect(s.client.getSnapshot().views.A).toBe(beforeFailure);
  expect(s.requests.filter(row => row.request.method === 'session/trace')).toHaveLength(2);
  s.socket.success(fresh, { type: 'trace', page: { records: [traceRecord(1), traceRecord(2)] } });
  await work;
  const view = s.client.getSnapshot().views.A;
  expect(view.trace!.page.records).toEqual([traceRecord(1), traceRecord(2)]);
  expect(view.trace!.error).toBeUndefined();
  expect(view).toMatchObject({ target: beforeFailure.target, cursor: beforeFailure.cursor, attachment: 'attached' });
  expect(s.client['traceReads'].has('A')).toBe(false);
  expect(s.requests.filter(row => row.request.method === 'session/trace')).toHaveLength(2);
  expect(s.requests.filter(row => row.request.method === 'session/snapshot')).toHaveLength(owner === 'snapshot' ? 1 : 0);
});

it('failed Trace iteration without a newer obligation propagates the error and never retries', async () => {
  const s = await create(); s.held.add('session/trace');
  s.client.latestTrace('A');
  const request = await s.waitFor('session/trace', 1);
  const work = s.client['traceReads'].get('A')!.work;
  const rejected = expect(work).rejects.toThrow('current Trace read failed');
  s.socket.deliver({ jsonrpc: '2.0', id: request.id, error: { code: -32000, message: 'current Trace read failed', data: { kind: 'invalid_state' } } });
  await rejected;
  expect(s.client.getSnapshot().views.A.trace!.error).toContain('current Trace read failed');
  expect(s.client['traceReads'].has('A')).toBe(false);
  expect(s.requests.filter(row => row.request.method === 'session/trace')).toHaveLength(1);
  expect(s.requests.filter(row => row.request.method === 'session/snapshot')).toHaveLength(0);
});

it('a failed owed follow-up stops without further invalidation and reports only its own error', async () => {
  const s = await create(); s.held.add('session/trace');
  emit(s, '1', { type: 'trace_changed' });
  const old = await s.waitFor('session/trace', 1);
  s.client.latestTrace('A');
  const work = s.client['traceReads'].get('A')!.work;
  const rejected = expect(work).rejects.toThrow('latest read failed');
  s.socket.deliver({ jsonrpc: '2.0', id: old.id, error: { code: -32000, message: 'superseded read failed' } });
  const fresh = await s.waitFor('session/trace', 2);
  expect(s.client.getSnapshot().views.A.trace!.error).toBeUndefined();
  s.socket.deliver({ jsonrpc: '2.0', id: fresh.id, error: { code: -32000, message: 'latest read failed' } });
  await rejected;
  expect(s.client.getSnapshot().views.A.trace!.error).toContain('latest read failed');
  expect(s.client.getSnapshot().views.A.trace!.error).not.toContain('superseded');
  expect(s.client['traceReads'].has('A')).toBe(false);
  expect(s.requests.filter(row => row.request.method === 'session/trace')).toHaveLength(2);
});

it.each(['attachment replacement', 'disconnect'] as const)('failed dirty Trace owner cannot continue after %s', async retirement => {
  const s = await create(); s.held.add('session/trace');
  emit(s, '1', { type: 'trace_changed' });
  const old = await s.waitFor('session/trace', 1);
  const work = s.client['traceReads'].get('A')!.work;
  const rejected = expect(work).rejects.toThrow();
  emit(s, '2', { type: 'trace_changed' });
  if (retirement === 'disconnect') await s.client.disconnect();
  else { await s.client.release('A'); await s.client.attach('A'); }
  const view = s.client.getSnapshot().views.A;
  s.socket.deliver({ jsonrpc: '2.0', id: old.id, error: { code: -32000, message: 'retired read failed' } });
  await rejected;
  expect(s.client.getSnapshot().views.A).toBe(view);
  expect(s.client['traceReads'].has('A')).toBe(false);
  expect(s.requests.filter(row => row.request.method === 'session/trace')).toHaveLength(1);
});

it('Agent and Job domains join cursor replay after resync without snapshot polling or Trace invalidation', async () => {
  const s = await create();
  emit(s, '1', { type: 'agent_updated', agent: incrementalAgent() });
  emit(s, '2', { type: 'job_updated', job: { job_id: 'job-1', tool_id: 'bash', tool_name: 'bash', state: 'running' } });
  const captured = s.client.getSnapshot().views.A.snapshot!;
  s.held.add('agent/wait');
  const waiting = s.client.request({ method: 'agent/wait', params: { target: s.target('A'), agent_id: 'durable-child' } }, 'agent_wait');
  const waitRequest = await s.waitFor('agent/wait', 1);
  expect(s.requests.filter(row => ['session/snapshot', 'session/trace'].includes(row.request.method))).toHaveLength(0);
  s.held.add('session/snapshot');
  emit(s, '4', { type: 'agent_updated', agent: incrementalAgent('activation-2') });
  const read = await s.waitFor('session/snapshot', 1);
  s.socket.success(read, { type: 'snapshot', snapshot: captured, cursor: '2' });
  await s.waitFor('session/subscribe', 1);
  await new Promise<void>(resolve => { const ready = () => s.client.getSnapshot().views.A.attachment === 'attached'; if (ready()) return resolve(); const stop = s.client.subscribe(() => { if (ready()) { stop(); resolve(); } }); });
  const installed = s.client.getSnapshot().views.A.snapshot!;
  emit(s, '3', { type: 'job_updated', job: { job_id: 'job-1', tool_id: 'bash', tool_name: 'bash', state: 'succeeded' } });
  emit(s, '4', { type: 'agent_updated', agent: incrementalAgent('activation-2') });
  emit(s, '4', { type: 'agent_updated', agent: incrementalAgent() }); // duplicate cannot retarget
  s.socket.success(waitRequest, { type: 'agent_wait', agent_id: 'durable-child', activation_id: 'activation-1', outcome: 'succeeded', agent: incrementalAgent() });
  expect((await waiting).activation_id).toBe('activation-1');
  expect(s.requests.filter(row => row.request.method === 'agent/wait')).toHaveLength(1);
  const current = s.client.getSnapshot().views.A.snapshot!;
  expect(current.agents![0].activation_id).toBe('activation-2');
  expect(captured.agents![0].activation_id).toBe('activation-1');
  expect(current.jobs![0].state).toBe('succeeded');
  expect(current.messages).toBe(installed.messages); expect(current.transcript).toBe(installed.transcript);
  expect(s.client.getSnapshot().views.A.cursor).toBe('4');
  emit(s, '5', { type: 'read_domains_updated', transcript: current.transcript, todos: current.todos });
  expect(s.client.getSnapshot().views.A.snapshot?.agents).toBe(current.agents);
  expect(s.client.getSnapshot().views.A.snapshot?.jobs).toBe(current.jobs);
  expect(s.requests.filter(row => row.request.method === 'session/snapshot')).toHaveLength(1);
  expect(s.requests.filter(row => row.request.method === 'session/trace')).toHaveLength(0);
});

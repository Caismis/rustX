import { afterEach, expect, it } from 'vitest';
import { Server } from './fixture';
import type { RuntimeClientEvent } from '../../protocol/app-server/v25';
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
  expect(s.requests.some(row => ['turn/start', 'turn/steer', 'session/upload', 'session/create'].includes(row.request.method))).toBe(false);
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

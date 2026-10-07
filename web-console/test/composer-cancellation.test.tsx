import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { RuntimeClientSnapshot } from '../../protocol/app-server/v36';
import { composerPreferences } from '../src/app/composer/preferences';
import { App } from '../src/app/App';
import { isOutcomeUncertain } from '../src/client/app-server';
import { Server, snapshot, endpoint } from './fixture';
let server: Server;
beforeEach(() => { localStorage.clear(); server = new Server(); });
afterEach(() => { cleanup(); server.client.disconnect(); vi.useRealTimers(); });
const running = (id = 'attempt-A'): RuntimeClientSnapshot => ({ ...snapshot(), attempt: { attempt_id: id, phase: { type: 'running' as const }, turn: 1 } });
const cancels = () => server.requests.filter(row => row.request.method === 'turn/cancel').map(row => row.request);
const input = () => screen.getByRole('textbox', { name: 'Message' }) as HTMLTextAreaElement;
const escape = () => fireEvent.keyDown(input(), { key: 'Escape' });
async function mount() {
  server.snapshots.set('A', running()); await server.attached('A');
  localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint, openViews: ['A'] }));
  await act(async () => { render(<App client={server.client} workspaceHost={server.workspaceHost}/>); });
  act(() => input().focus()); vi.useFakeTimers();
}
it('eligible double Escape sends exactly one native target; a gated reply and settlement preserve drafts/receipts', async () => {
  await mount(); const receipt = { session_id: 'A', batch_id: 'batch', token: 'token' };
  server.handlers.set('session/uploadStatus', () => ({ type: 'upload_status', outcome: { state: 'ready', files: [{ receipt, file: { batch_id: 'batch', name: 'kept.txt' }, path: '/kept.txt' }] } }));
  await act(async () => fireEvent.change(screen.getByLabelText('Attach files'), { target: { files: [new File(['kept'], 'kept.txt')] } }));
  fireEvent.change(input(), { target: { value: 'keep draft' } }); act(() => input().focus()); input().setSelectionRange(2, 5);
  server.held.add('turn/cancel'); escape(); expect(cancels()).toHaveLength(0);
  await act(async () => escape()); const request = await server.waitFor('turn/cancel', 1);
  expect(request).toMatchObject({ method: 'turn/cancel', params: { target: server.target('A') } }); expect(cancels()).toHaveLength(1);
  expect(server.client.getSnapshot().views.A.cancellation).toEqual({ attemptId: 'attempt-A', status: 'in-flight' });
  expect(screen.getByLabelText('Session status').textContent).toContain('Stopping');
  for (let i = 0; i < 4; i++) escape(); expect(cancels()).toHaveLength(1);
  expect(input().value).toBe('keep draft'); expect([input().selectionStart, input().selectionEnd]).toEqual([2, 5]);
  expect(screen.getByRole('button', { name: 'Remove kept.txt' })).toBeTruthy();
  await act(async () => { server.reply(request); });
  expect(server.client.getSnapshot().views.A.cancellation?.status).toBe('acknowledged');
  expect(server.client.getSnapshot().views.A.snapshot?.attempt?.phase.type).toBe('running');
  fireEvent.change(input(), { target: { value: 'replacement draft' } });
  const settled = running(); settled.attempt!.phase = { type: 'settled', outcome: { type: 'cancelled', reason: 'user_requested' } };
  await act(async () => { server.snapshots.set('A', settled); await server.client.refresh('A'); });
  expect(server.client.getSnapshot().views.A.cancellation).toBeUndefined(); expect(input().value).toBe('replacement draft');
  expect(screen.getByRole('button', { name: 'Remove kept.txt' })).toBeTruthy(); expect(cancels()).toHaveLength(1);
  // Cancellation never consumes the receipt. The existing Send operation still owns it.
  await act(async () => fireEvent.keyDown(input(), { key: 'Enter' }));
  expect(server.requests.filter(row => row.request.method === 'turn/start').map(row => row.request)).toMatchObject([{ params: { content: [{ type: 'upload', ...receipt }, { type: 'text', text: 'replacement draft' }] } }]);
});
it('exact gated Attempt replacement between presses cannot cancel the successor', async () => {
  await mount(); escape(); server.held.add('session/snapshot');
  server.snapshots.set('A', { ...snapshot(), attempt: { attempt_id: 'attempt-A', turn: 1, phase: { type: 'settled', outcome: { type: 'cancelled', reason: 'user_requested' } } } });
  const nextRead = () => server.requests.filter(row => row.request.method === 'session/snapshot').length + 1;
  const settledGate = server.waitFor('session/snapshot', nextRead());
  const settlement = server.client.refresh('A'); const settledRead = await settledGate;
  expect(server.client.getSnapshot().views.A.snapshot?.attempt?.phase.type).toBe('running');
  await act(async () => { server.reply(settledRead); await settlement; });
  expect(server.client.getSnapshot().views.A.snapshot?.attempt?.phase.type).toBe('settled');
  server.snapshots.set('A', running('attempt-B')); const replacementGate = server.waitFor('session/snapshot', nextRead());
  const replacement = server.client.refresh('A'); const replacementRead = await replacementGate;
  await act(async () => { server.reply(replacementRead); await replacement; });
  expect(server.client.getSnapshot().views.A.snapshot?.attempt?.attempt_id).toBe('attempt-B');
  escape(); expect(cancels()).toHaveLength(0);
  server.held.add('turn/cancel'); await act(async () => escape());
  expect(cancels()).toHaveLength(1); expect(server.client.getSnapshot().views.A.cancellation?.attemptId).toBe('attempt-B');
});
it('lost cancellation response stays uncertain and reconnect repairs without replay', async () => {
  await mount(); fireEvent.change(input(), { target: { value: 'preserved' } }); server.held.add('turn/cancel');
  await act(async () => { escape(); escape(); }); await server.waitFor('turn/cancel', 1);
  await act(async () => server.socket.close());
  expect(server.client.getSnapshot().views.A.cancellation?.status).toBe('uncertain');
  expect(server.client.getSnapshot().uncertain.filter(item => item.method === 'turn/cancel')).toHaveLength(1);
  escape(); escape(); expect(cancels()).toHaveLength(1); expect(input().value).toBe('preserved');
  server.snapshots.set('A', { ...snapshot(), attempt: { attempt_id: 'attempt-A', turn: 1, phase: { type: 'settled', outcome: { type: 'cancelled', reason: 'user_requested' } } } });
  vi.useRealTimers(); await act(async () => server.connect());
  expect(server.client.getSnapshot().views.A.snapshot?.attempt?.phase.type).toBe('settled');
  expect(server.client.getSnapshot().views.A.cancellation).toBeUndefined(); expect(cancels()).toHaveLength(1); expect(input().value).toBe('preserved');
});
it('a stale expected Attempt or attachment is refused at the client owner', async () => {
  server.snapshots.set('A', running()); await server.attached('A'); const expected = server.client.cancellationTarget('A')!;
  server.snapshots.set('A', running('attempt-B')); await server.client.refresh('A'); await server.client.cancelTurn(expected); expect(cancels()).toHaveLength(0);
  const before = server.client.cancellationTarget('A')!; await server.client.release('A'); await server.client.attach('A');
  await server.client.cancelTurn(before); expect(cancels()).toHaveLength(0);
});
it('ordinary RPC saturation cannot block exact lifecycle cancellation or its acknowledgement', async () => {
  server.snapshots.set('A', running()); await server.attached('A');
  const expected = server.client.cancellationTarget('A')!;
  server.held.add('server/info'); server.held.add('turn/cancel');
  const occupied = Array.from({ length: 8 }, () => server.client.request({ method: 'server/info', params: {} }, 'server_info'));
  await server.waitFor('server/info', 8);
  const cancellation = server.client.cancelTurn(expected);
  // No reply or capacity release occurs before this synchronous wire assertion.
  expect(cancels()).toHaveLength(1);
  expect(cancels()[0]).toMatchObject({ method: 'turn/cancel', params: { target: expected.target } });
  expect(server.client.getSnapshot().views.A.cancellation).toEqual({ attemptId: expected.attemptId, status: 'in-flight' });
  await server.client.cancelTurn(expected); expect(cancels()).toHaveLength(1);
  const repairRead = server.waitFor('session/snapshot', server.requests.filter(row => row.request.method === 'session/snapshot').length + 1);
  const acknowledged = new Promise<void>(resolve => {
    const unsubscribe = server.client.subscribe(() => {
      if (server.client.getSnapshot().views.A.cancellation?.status === 'acknowledged') { unsubscribe(); resolve(); }
    });
  });
  server.reply(cancels()[0]); await acknowledged;
  expect(server.client.getSnapshot().views.A.cancellation?.status).toBe('acknowledged');
  // Acknowledgement is published even though its observation refresh needs an RPC slot.
  for (const { request } of server.requests.filter(row => row.request.method === 'server/info')) server.reply(request);
  await repairRead; await Promise.all([...occupied, cancellation]);
  expect(server.client.getSnapshot().views.A.cancellation).toEqual({ attemptId: expected.attemptId, status: 'acknowledged' });
  expect(server.client.getSnapshot().views.A.snapshot?.attempt?.phase.type).toBe('running');
  const settled = running(); settled.attempt!.phase = { type: 'settled', outcome: { type: 'cancelled', reason: 'user_requested' } };
  server.snapshots.set('A', settled); await server.client.refresh('A');
  expect(server.client.getSnapshot().views.A.cancellation).toBeUndefined(); expect(cancels()).toHaveLength(1);
});

function occupyControls() {
  server.held.add('job/cancel');
  server.handlers.set('job/cancel', () => ({ type: 'job', job: { job_id: 'job-a', tool_id: 'bash', tool_name: 'bash', state: 'cancelled' } }));
  return Array.from({ length: 2 }, () => server.client.request({ method: 'job/cancel', params: { target: server.target('A'), job_id: 'job-a' } }, 'job'));
}
function releaseControls() {
  for (const { request } of server.requests.filter(row => row.request.method === 'job/cancel')) server.reply(request);
}
it('full bounded control capacity refuses cancellation locally and releases only its marker for a deliberate retry', async () => {
  server.snapshots.set('A', running()); await server.attached('A');
  const occupied = occupyControls(); await server.waitFor('job/cancel', 2);
  const expected = server.client.cancellationTarget('A')!;
  const cancelled = server.client.cancelTurn(expected).catch(error => error);
  expect(server.client.getSnapshot().views.A.cancellation).toEqual({ attemptId: 'attempt-A', status: 'in-flight' });
  expect(cancels()).toHaveLength(0);
  const failure = await cancelled;
  expect(failure).toBeInstanceOf(Error); expect(failure.message).toContain('control capacity');
  expect(isOutcomeUncertain(failure)).toBe(false);
  expect(server.client.getSnapshot().views.A.cancellation).toBeUndefined();
  expect(server.client.getSnapshot().uncertain).toEqual([]); expect(cancels()).toHaveLength(0);
  releaseControls(); await Promise.all(occupied);
  const available = server.client.cancellationTarget('A')!; expect(available).toEqual(expected);
  expect(server.client.getSnapshot().views.A.snapshot?.attempt?.phase.type).toBe('running');
  expect(cancels()).toHaveLength(0); // Capacity release never replays a refused gesture.
  server.held.add('turn/cancel'); const accepted = server.client.cancelTurn(available);
  expect(cancels()).toHaveLength(1); expect(cancels()[0].params).toEqual({ target: available.target });
  await server.client.cancelTurn(available); expect(cancels()).toHaveLength(1);
  server.reply(cancels()[0]); await accepted;
  expect(server.client.getSnapshot().views.A.cancellation).toEqual({ attemptId: 'attempt-A', status: 'acknowledged' });
});

it('a locally refused control continuation cannot clear a successor cancellation operation', async () => {
  server.snapshots.set('A', running()); await server.attached('A');
  const occupied = occupyControls(); await server.waitFor('job/cancel', 2);
  server.held.add('turn/cancel');
  const older = server.client.cancelTurn(server.client.cancellationTarget('A')!).catch(error => error);
  expect(server.client.getSnapshot().views.A.cancellation).toEqual({ attemptId: 'attempt-A', status: 'in-flight' });
  expect(cancels()).toHaveLength(0);
  // Replace A and admit B before the already-refused A's async cleanup resumes.
  server.snapshots.set('A', running('attempt-B'));
  server.socket.deliver({ jsonrpc: '2.0', method: 'session/event', params: { target: server.target('A'), cursor: String(++server.cursor), event: { type: 'attempt_started', attempt_id: 'attempt-B' } } });
  releaseControls();
  const expectedB = server.client.cancellationTarget('A')!;
  const successor = server.client.cancelTurn(expectedB);
  const operationB = server.client.getSnapshot().views.A.cancellation;
  expect(operationB).toEqual({ attemptId: 'attempt-B', status: 'in-flight' });
  const failure = await older;
  expect(failure.message).toContain('control capacity'); expect(isOutcomeUncertain(failure)).toBe(false);
  expect(server.client.getSnapshot().views.A.cancellation).toBe(operationB);
  expect(server.client.getSnapshot().uncertain).toEqual([]);
  expect(cancels()).toHaveLength(1); expect(cancels()[0].params).toEqual({ target: expectedB.target });
  await Promise.all(occupied); server.reply(cancels()[0]); await successor;
  expect(server.client.getSnapshot().views.A.cancellation).toEqual({ attemptId: 'attempt-B', status: 'acknowledged' });
});

it.each(['queue', 'steer'] as const)('resident ContextSeat preserves %s submission, exact cancellation and compact draft ownership', async preference => {
  composerPreferences().setBusyEnter(preference);
  await mount(); vi.useRealTimers();
  expect(document.querySelector('[data-context-seat]')).toBeNull();
  const expected = server.client.cancellationTarget('A')!;
  fireEvent.change(input(), { target: { value: 'ordinary draft' } });
  await act(async () => fireEvent.keyDown(input(), { key: 'Enter' }));
  const method = preference === 'queue' ? 'turn/start' : 'turn/steer';
  expect(server.requests.filter(row => row.request.method === method)).toHaveLength(1);
  expect(server.client.cancellationTarget('A')).toEqual(expected);
  server.held.add('turn/cancel'); act(() => input().focus());
  await act(async () => { escape(); escape(); });
  expect(cancels()).toHaveLength(1);
  expect(cancels()[0]).toMatchObject({ params: { target: expected.target } });
  expect(server.client.getSnapshot().views.A.cancellation?.attemptId).toBe(expected.attemptId);
  await act(async () => server.reply(cancels()[0]));
  await act(async () => server.update('A', { ...snapshot(), context: { compaction_count: 0, compaction_in_progress: false } }));
  server.held.add('context/compact');
  fireEvent.change(input(), { target: { value: '/compact' } });
  await act(async () => fireEvent.keyDown(input(), { key: 'Enter' }));
  const compact = await server.waitFor('context/compact', 1);
  expect(input().value).toBe(''); expect(screen.queryByRole('dialog')).toBeNull();
  fireEvent.change(input(), { target: { value: 'later draft' } });
  const requestId = server.client.getSnapshot().views.A.compactionRequest!.requestId;
  await act(async () => server.update('A', { ...snapshot(), context: { compaction_count: 1, compaction_in_progress: false, manual_compaction: { request_id: requestId, released: true, error: null } } }));
  expect(screen.getByText('Context compacted')).toBeTruthy();
  expect(input().value).toBe('later draft');
  expect(server.requests.filter(row => ['turn/start', 'turn/steer'].includes(row.request.method))).toHaveLength(1);
  server.handlers.set('context/compact', () => ({ type: 'context', context: server.snapshots.get('A')!.context! }));
  await act(async () => server.reply(compact));
  composerPreferences().setBusyEnter('queue');
});

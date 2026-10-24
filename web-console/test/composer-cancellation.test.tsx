import { compactReceipt } from './fixture';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { RuntimeClientSnapshot } from '../../protocol/app-server/v57';
import { composerPreferences } from '../src/app/composer/preferences';
import { App } from '../src/app/App';
import { isOutcomeUncertain } from '../src/client/errors';
import { Server, snapshot, endpoint, interaction } from './fixture';
let server: Server;
beforeEach(() => { localStorage.clear(); server = new Server(); server.handlers.set('turn/cancel', request => ({type:'cancellation_accepted',attempt_id:request.method==='turn/cancel' ? server.snapshots.get(request.params.target.session_id)?.attempt?.attempt_id ?? 'attempt-A' : 'attempt-A'})); });
afterEach(() => { cleanup(); server.client.disconnect(); vi.useRealTimers(); });
const running = (id = 'attempt-A'): RuntimeClientSnapshot => ({ ...snapshot(), attempt: { attempt_id: id, phase: { type: 'running' as const }, turn: 1 } });
const cancels = () => server.requests.filter(row => row.request.method === 'turn/cancel').map(row => row.request);
const input = () => screen.getByRole('textbox', { name: 'Message' }) as HTMLTextAreaElement;
const escape = () => fireEvent.keyDown(input(), { key: 'Escape' });
async function mount(initial = running()) {
  server.snapshots.set('A', initial); await server.attached('A');
  localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint, openViews: ['A'] }));
  await act(async () => { render(<App client={server.client} workspaceHost={server.workspaceHost}/>); });
  const editor = screen.queryByRole('textbox', { name: 'Message' });
  if (editor) act(() => editor.focus());
  vi.useFakeTimers();
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
  await mount(); escape(); server.held.add('session/checkpoint');
  server.snapshots.set('A', { ...snapshot(), attempt: { attempt_id: 'attempt-A', turn: 1, phase: { type: 'settled', outcome: { type: 'cancelled', reason: 'user_requested' } } } });
  const nextRead = () => server.requests.filter(row => row.request.method === 'session/checkpoint').length + 1;
  const settledGate = server.waitFor('session/checkpoint', nextRead());
  const settlement = server.client.refresh('A'); const settledRead = await settledGate;
  expect(server.client.getSnapshot().views.A.snapshot?.attempt?.phase.type).toBe('running');
  await act(async () => { server.reply(settledRead); await settlement; });
  expect(server.client.getSnapshot().views.A.snapshot?.attempt?.phase.type).toBe('settled');
  server.snapshots.set('A', running('attempt-B')); const replacementGate = server.waitFor('session/checkpoint', nextRead());
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
it('current-Attempt commands follow native authority while a retired attachment is refused', async () => {
  server.snapshots.set('A', running()); await server.attached('A'); const expected = server.client.cancellationTarget('A')!;
  server.snapshots.set('A', running('attempt-B')); await server.client.refresh('A');
  await server.client.cancelTurn(expected); expect(cancels()).toHaveLength(1);
  expect(server.client.getSnapshot().views.A.cancellation).toEqual({attemptId:'attempt-B',status:'acknowledged'});
  await server.client.release('A'); await server.client.attach('A');
  await server.client.cancelTurn(expected); expect(cancels()).toHaveLength(1);
});
it('unavailable stale A forwards cancellation and retains native B receipt without a repair read', async () => {
  server.snapshots.set('A', running()); await server.attached('A');
  const expected = server.client.cancellationTarget('A')!;
  const target = server.target('A'), registration = server.registrations.get(JSON.stringify(target))!;
  server.socket.deliver({jsonrpc:'2.0',method:'session/observationUnavailable',params:{target,registration_id:registration,message:'held source'}});
  const before = server.client.getSnapshot().views.A;
  const reads = server.requests.filter(row => row.request.method === 'session/checkpoint').length;
  server.handlers.set('turn/cancel', () => ({type:'cancellation_accepted',attempt_id:'native-B'}));
  server.held.add('turn/cancel');
  const cancelling = server.client.cancelTurn(expected);
  expect(cancels()).toHaveLength(1);
  server.reply(cancels()[0]); await cancelling;
  const accepted = server.client.getSnapshot().views.A;
  expect(accepted.cancellation).toEqual({attemptId:'native-B',status:'acknowledged'});
  expect(accepted.snapshot).toBe(before.snapshot); expect(accepted.cursor).toBe(before.cursor);
  expect(accepted.observationState).toBe('unavailable');
  expect(server.requests.filter(row => row.request.method === 'session/checkpoint')).toHaveLength(reads);
});
it.each(['button', 'Escape'] as const)('%s captures cancellation authority after Source becomes unavailable without enabling Send or draft edits', async gesture => {
  await mount();
  fireEvent.change(input(), { target: { value: 'retained draft' } });
  const target = server.target('A'), registration_id = server.registrations.get(JSON.stringify(target))!;
  await act(async () => server.socket.deliver({ jsonrpc: '2.0', method: 'session/observationUnavailable', params: { target, registration_id, message: 'held source' } }));
  const before = server.client.getSnapshot().views.A;
  expect(before.observationState).toBe('unavailable');
  expect(server.client.isAttachmentControlCurrent('A', before.attachmentObservation)).toBe(false);
  // Capture happens after failure, as the rendered gesture does, not before it.
  expect(server.client.cancellationTarget('A')?.target).toEqual(target);
  expect(input().disabled).toBe(false); expect(input().readOnly).toBe(true);
  act(() => input().focus()); expect(document.activeElement).toBe(input());
  fireEvent.change(input(), { target: { value: 'must not replace draft' } });
  fireEvent.keyDown(input(), { key: 'Enter' });
  expect(input().value).toBe('retained draft');
  expect(server.requests.filter(row => ['turn/start', 'turn/steer'].includes(row.request.method))).toHaveLength(0);
  const reads = server.requests.filter(row => row.request.method === 'session/checkpoint').length;
  server.handlers.set('turn/cancel', () => ({ type: 'cancellation_accepted', attempt_id: 'native-current-B' }));
  server.held.add('turn/cancel');
  if (gesture === 'button') {
    const button = screen.getByRole('button', { name: 'Stop' }) as HTMLButtonElement;
    expect(button.disabled).toBe(false); await act(async () => fireEvent.click(button));
  } else await act(async () => { escape(); escape(); });
  expect(cancels()).toHaveLength(1); expect(cancels()[0].params).toEqual({ target });
  await act(async () => server.reply(cancels()[0]));
  const after = server.client.getSnapshot().views.A;
  expect(after.cancellation).toEqual({ attemptId: 'native-current-B', status: 'acknowledged' });
  expect(after.snapshot).toBe(before.snapshot); expect(after.cursor).toBe(before.cursor);
  expect(after.observationState).toBe('unavailable'); expect(input().value).toBe('retained draft');
  expect(server.requests.filter(row => row.request.method === 'session/checkpoint')).toHaveLength(reads);
});

const unavailableSnapshots: readonly (readonly [string, RuntimeClientSnapshot])[] = [
  ['settled', { ...running(), attempt: { ...running().attempt!, phase: { type: 'settled' as const, outcome: { type: 'cancelled' as const, reason: 'user_requested' } } } }],
  ['absent Attempt', snapshot()],
  ['shutdown and durability failure', { ...snapshot(), shutting_down: true, durability_failure: { operation: 'retained', diagnostic: 'retained failure' } }],
  ['pending interaction', { ...snapshot(), pending_interactions: [interaction('questionnaire')] }],
];
it.each(unavailableSnapshots.flatMap(([name, initial]) => (['button', 'Escape'] as const).map(gesture => ({ name, initial, gesture }))))('$gesture cancels with unavailable retained $name using the native receipt', async ({ initial, gesture }) => {
  await mount(initial);
  const target = server.target('A'), registration_id = server.registrations.get(JSON.stringify(target))!;
  await act(async () => server.socket.deliver({ jsonrpc: '2.0', method: 'session/observationUnavailable', params: { target, registration_id, message: 'held source' } }));
  const before = server.client.getSnapshot().views.A;
  const reads = server.requests.filter(row => row.request.method === 'session/checkpoint').length;
  expect(server.client.cancellationTarget('A')).toBeDefined();
  expect(input().readOnly).toBe(true); act(() => input().focus());
  server.handlers.set('turn/cancel', () => ({ type: 'cancellation_accepted', attempt_id: 'native-current-B' }));
  server.held.add('turn/cancel');
  if (gesture === 'button') await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Stop' })));
  else await act(async () => { escape(); escape(); });
  expect(cancels()).toHaveLength(1); expect(cancels()[0].params).toEqual({ target });
  expect(server.client.getSnapshot().views.A.cancellation?.attemptId).toBe(initial.attempt?.attempt_id ?? undefined);
  await act(async () => server.reply(cancels()[0]));
  const after = server.client.getSnapshot().views.A;
  expect(after.cancellation).toEqual({ attemptId: 'native-current-B', status: 'acknowledged' });
  expect(after.snapshot).toBe(before.snapshot); expect(after.cursor).toBe(before.cursor);
  expect(after.observationState).toBe('unavailable');
  expect(server.requests.filter(row => row.request.method === 'session/checkpoint')).toHaveLength(reads);
});
it('an acknowledged native A under unavailable A permits an explicit successor cancellation without replay', async () => {
  await mount();
  const target = server.target('A'), registration_id = server.registrations.get(JSON.stringify(target))!;
  await act(async () => server.socket.deliver({ jsonrpc: '2.0', method: 'session/observationUnavailable', params: { target, registration_id, message: 'held source' } }));
  const before = server.client.getSnapshot().views.A;
  await act(async () => { escape(); escape(); });
  expect(cancels()).toHaveLength(1);
  expect(server.client.getSnapshot().views.A.cancellation).toEqual({ attemptId: 'attempt-A', status: 'acknowledged' });
  expect(server.client.cancellationTarget('A')).toBeDefined(); expect(cancels()).toHaveLength(1);
  server.handlers.set('turn/cancel', () => ({ type: 'cancellation_accepted', attempt_id: 'native-successor-B' }));
  await act(async () => { escape(); escape(); });
  expect(cancels()).toHaveLength(2);
  const after = server.client.getSnapshot().views.A;
  expect(after.cancellation).toEqual({ attemptId: 'native-successor-B', status: 'acknowledged' });
  expect(after.snapshot).toBe(before.snapshot); expect(after.cursor).toBe(before.cursor);
});
it('Escape cancels through the exact attachment while explicit observation recovery is held', async () => {
  await mount(snapshot());
  const target = server.target('A');
  server.held.add('session/checkpoint');
  const repairing = server.client.refresh('A');
  const read = server.requests.filter(row => row.request.method === 'session/checkpoint').at(-1)!.request;
  await act(async () => {});
  expect(server.client.getSnapshot().views.A.observationState).toBe('checkpointLoading');
  expect(server.client.cancellationTarget('A')).toBeDefined();
  server.handlers.set('turn/cancel', () => ({ type: 'cancellation_accepted', attempt_id: 'native-current-B' }));
  act(() => input().focus()); await act(async () => { escape(); escape(); });
  expect(cancels()).toHaveLength(1); expect(cancels()[0].params).toEqual({ target });
  expect(server.client.getSnapshot().views.A.cancellation).toEqual({ attemptId: 'native-current-B', status: 'acknowledged' });
  await act(async () => { server.reply(read); await repairing; });
  expect(cancels()).toHaveLength(1);
});
it('unavailable cancellation rejection clears only its operation and never replays', async () => {
  await mount(snapshot());
  const target = server.target('A'), registration_id = server.registrations.get(JSON.stringify(target))!;
  await act(async () => server.socket.deliver({ jsonrpc: '2.0', method: 'session/observationUnavailable', params: { target, registration_id, message: 'held source' } }));
  server.held.add('turn/cancel');
  await act(async () => { escape(); escape(); });
  const request = cancels()[0]; expect(request).toBeDefined();
  await act(async () => server.socket.deliver({ jsonrpc: '2.0', id: request.id, error: { code: -32000, message: 'No active Attempt' } }));
  expect(server.client.getSnapshot().views.A.cancellation).toBeUndefined();
  expect(server.client.cancellationTarget('A')).toBeDefined(); expect(cancels()).toHaveLength(1);
});

it.each(['attachment', 'connection'] as const)('an unavailable Escape arm cannot cross a replaced %s', async replacement => {
  await mount();
  const target = server.target('A'), registration_id = server.registrations.get(JSON.stringify(target))!;
  await act(async () => server.socket.deliver({ jsonrpc: '2.0', method: 'session/observationUnavailable', params: { target, registration_id, message: 'held source' } }));
  const retired = server.client.cancellationTarget('A')!; expect(retired).toBeDefined();
  act(() => input().focus()); escape();
  vi.useRealTimers();
  if (replacement === 'attachment') {
    await act(async () => { await server.client.release('A'); await server.client.attach('A'); });
  } else {
    await act(async () => server.socket.close());
    await server.client.cancelTurn(retired); expect(cancels()).toHaveLength(0);
    await act(async () => server.connect());
  }
  await server.client.cancelTurn(retired); expect(cancels()).toHaveLength(0);
  const next = server.client.cancellationTarget('A')!;
  expect(next).toBeDefined(); expect(next.target).not.toEqual(target);
  act(() => input().focus()); await act(async () => escape()); expect(cancels()).toHaveLength(0);
  await act(async () => escape()); expect(cancels()).toHaveLength(1);
  expect(cancels()[0].params).toEqual({ target: next.target });
});
it('native B acknowledgement survives stale A folds and permits a genuine observed successor', async () => {
  server.snapshots.set('A', running()); await server.attached('A');
  const expected = server.client.cancellationTarget('A')!;
  server.handlers.set('turn/cancel', () => ({type:'cancellation_accepted',attempt_id:'native-B'}));
  await server.client.cancelTurn(expected);
  await server.update('A', running());
  expect(server.client.getSnapshot().views.A.cancellation).toEqual({attemptId:'native-B',status:'acknowledged'});
  await server.update('A', running('native-B'));
  expect(server.client.getSnapshot().views.A.cancellation?.attemptId).toBe('native-B');
  const settled = running('native-B'); settled.attempt!.phase = {type:'settled',outcome:{type:'cancelled',reason:'user_requested'}};
  await server.update('A', settled);
  expect(server.client.getSnapshot().views.A.cancellation).toBeUndefined();
  await server.update('A', running('native-C'));
  const successor = server.client.cancellationTarget('A')!;
  expect(successor).toBeDefined();
  server.handlers.set('turn/cancel', () => ({type:'cancellation_accepted',attempt_id:'native-C'}));
  await server.client.cancelTurn(successor);
  expect(cancels()).toHaveLength(2);
  expect(server.client.getSnapshot().views.A.cancellation).toEqual({attemptId:'native-C',status:'acknowledged'});
});
const unrelatedCancellationSnapshots = [
  ['absent Attempt', snapshot()],
  ['different Attempt', running('unrelated-C')],
  ['display Attempt settled', { ...running(), attempt: { ...running().attempt!, phase: { type: 'settled' as const, outcome: { type: 'cancelled' as const, reason: 'user_requested' } } } }],
] as const;
it.each(unrelatedCancellationSnapshots)('an in-flight cancellation survives a checkpoint with %s', async (_name, next) => {
  server.snapshots.set('A', running()); await server.attached('A');
  const expected = server.client.cancellationTarget('A')!;
  server.held.add('turn/cancel');
  const cancelling = server.client.cancelTurn(expected), request = cancels()[0];
  const operation = server.client.getSnapshot().views.A.cancellation;
  server.snapshots.set('A', next); await server.client.refresh('A');
  expect(server.client.getSnapshot().views.A.cancellation).toBe(operation);
  expect(server.client.cancellationTarget('A')).toBeUndefined();
  await server.client.cancelTurn(expected); expect(cancels()).toHaveLength(1);
  server.handlers.set('turn/cancel', () => ({ type: 'cancellation_accepted', attempt_id: 'actual-native-B' }));
  server.reply(request); await cancelling;
  expect(server.client.getSnapshot().views.A.cancellation).toEqual({ attemptId: 'actual-native-B', status: 'acknowledged' });
});
it.each(unrelatedCancellationSnapshots)('uncertain cancellation survives same-attachment repair with %s without replay', async (_name, next) => {
  server.snapshots.set('A', running()); await server.attached('A');
  const expected = server.client.cancellationTarget('A')!;
  server.held.add('turn/cancel');
  const cancelling = server.client.cancelTurn(expected).catch(error => error), request = cancels()[0];
  server.socket.deliver({ jsonrpc: '2.0', id: request.id, error: { code: -32000, message: 'Native outcome unknown', data: { kind: 'outcome_unknown' } } });
  expect(isOutcomeUncertain(await cancelling)).toBe(true);
  const operation = server.client.getSnapshot().views.A.cancellation;
  expect(operation).toEqual({ attemptId: 'attempt-A', status: 'uncertain' });
  const target = server.target('A'), registration_id = server.registrations.get(JSON.stringify(target))!;
  server.socket.deliver({ jsonrpc: '2.0', method: 'session/observationUnavailable', params: { target, registration_id, message: 'held source' } });
  server.snapshots.set('A', next); await server.client.refresh('A');
  expect(server.client.getSnapshot().views.A.target).toEqual(target);
  expect(server.client.getSnapshot().views.A.cancellation).toBe(operation);
  expect(server.client.cancellationTarget('A')).toBeUndefined();
  await server.client.cancelTurn(expected); expect(cancels()).toHaveLength(1);
});

it('ordinary RPC saturation cannot block exact lifecycle cancellation or its acknowledgement', async () => {
  server.snapshots.set('A', running()); await server.attached('A');
  const expected = server.client.cancellationTarget('A')!;
  server.held.add('server/info'); server.held.add('turn/cancel');
  const occupied = Array.from({ length: 6 }, () => server.client.request({ method: 'server/info', params: {} }, 'server_info'));
  await server.waitFor('server/info', 6);
  const cancellation = server.client.cancelTurn(expected);
  // No reply or capacity release occurs before this synchronous wire assertion.
  expect(cancels()).toHaveLength(1);
  expect(cancels()[0]).toMatchObject({ method: 'turn/cancel', params: { target: expected.target } });
  expect(server.client.getSnapshot().views.A.cancellation).toEqual({ attemptId: expected.attemptId, status: 'in-flight' });
  await server.client.cancelTurn(expected); expect(cancels()).toHaveLength(1);
  const reads = server.requests.filter(row => row.request.method === 'session/checkpoint').length;
  const acknowledged = new Promise<void>(resolve => {
    const unsubscribe = server.client.subscribe(() => {
      if (server.client.getSnapshot().views.A.cancellation?.status === 'acknowledged') { unsubscribe(); resolve(); }
    });
  });
  server.reply(cancels()[0]); await acknowledged;
  expect(server.client.getSnapshot().views.A.cancellation?.status).toBe('acknowledged');
  await cancellation;
  expect(server.requests.filter(row => row.request.method === 'session/checkpoint')).toHaveLength(reads);
  // The receipt needs no ordinary RPC slot or observation refresh.
  for (const { request } of server.requests.filter(row => row.request.method === 'server/info')) server.reply(request);
  await Promise.all(occupied);
  expect(server.client.getSnapshot().views.A.cancellation).toEqual({ attemptId: expected.attemptId, status: 'acknowledged' });
  expect(server.client.getSnapshot().views.A.snapshot?.attempt?.phase.type).toBe('running');
  const settled = running(); settled.attempt!.phase = { type: 'settled', outcome: { type: 'cancelled', reason: 'user_requested' } };
  server.snapshots.set('A', settled); await server.client.refresh('A');
  expect(server.client.getSnapshot().views.A.cancellation).toBeUndefined(); expect(cancels()).toHaveLength(1);
});

function occupyControls() {
  server.held.add('delivery/cancel');
  server.handlers.set('delivery/cancel',()=>({type:'delivery_cancel',accepted:false}));
  return Array.from({length:2},(_,index)=>server.client.request({method:'delivery/cancel',params:{request_id:`reserved-control-${index}`}},'delivery_cancel'));
}
function releaseControls() {
  for (const { request } of server.requests.filter(row => row.request.method === 'delivery/cancel')) server.reply(request);
}
it('full bounded control capacity refuses cancellation locally and releases only its marker for a deliberate retry', async () => {
  server.snapshots.set('A', running()); await server.attached('A');
  const occupied = occupyControls(); await server.waitFor('delivery/cancel', 2);
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

it('a retired attachment response cannot clear a successor cancellation operation', async () => {
  server.snapshots.set('A', running()); await server.attached('A');
  server.held.add('turn/cancel');
  const expectedA = server.client.cancellationTarget('A')!;
  const older = server.client.cancelTurn(expectedA);
  const requestA = cancels()[0];
  server.commit(requestA); // Commit A's receipt before retiring its attachment; hold only delivery.
  expect(server.client.getSnapshot().views.A.cancellation).toEqual({ attemptId: 'attempt-A', status: 'in-flight' });
  await server.client.release('A');
  expect(server.client.getSnapshot().views.A.cancellation).toBeUndefined();
  server.snapshots.set('A', running('attempt-B')); await server.client.attach('A');
  const expectedB = server.client.cancellationTarget('A')!;
  const successor = server.client.cancelTurn(expectedB), requestB = cancels()[1];
  const operationB = server.client.getSnapshot().views.A.cancellation;
  expect(expectedB.target).not.toEqual(expectedA.target);
  expect(operationB).toEqual({ attemptId: 'attempt-B', status: 'in-flight' });
  server.reply(requestA); await older;
  expect(server.client.getSnapshot().views.A.cancellation).toBe(operationB);
  expect(server.client.getSnapshot().uncertain).toEqual([]);
  expect(cancels()).toHaveLength(2); expect(requestB.params).toEqual({ target: expectedB.target });
  server.reply(requestB); await successor;
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
  server.handlers.set('context/compact', () => (compactReceipt(server.snapshots.get('A')!.context!)));
  await act(async () => server.reply(compact));
  composerPreferences().setBusyEnter('queue');
});

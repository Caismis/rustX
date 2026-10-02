import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { RuntimeClientSnapshot } from '../../protocol/app-server/v32';
import { App } from '../src/app/App';
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
  server.handlers.set('session/upload', () => ({ type: 'session_uploaded', files: [{ receipt, file: { batch_id: 'batch', name: 'kept.txt' }, path: '/kept.txt' }] }));
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
it('RPC capacity deferral revalidates the Attempt at actual native request admission', async () => {
  server.snapshots.set('A', running()); await server.attached('A'); const expected = server.client.cancellationTarget('A')!;
  server.held.add('server/info'); const occupied = Array.from({ length: 8 }, () => server.client.request({ method: 'server/info', params: {} }, 'server_info'));
  await server.waitFor('server/info', 8);
  const cancellation = server.client.cancelTurn(expected); const refused = expect(cancellation).rejects.toThrow('Authority changed before dispatch');
  expect(cancels()).toHaveLength(0); expect(server.client.getSnapshot().views.A.cancellation?.status).toBe('in-flight');
  // Exact authoritative event publication while every RPC slot is occupied.
  server.socket.deliver({ jsonrpc: '2.0', method: 'session/event', params: { target: server.target('A'), cursor: String(++server.cursor), event: { type: 'attempt_started', attempt_id: 'attempt-B' } } });
  expect(server.client.getSnapshot().views.A.snapshot?.attempt?.attempt_id).toBe('attempt-B');
  for (const { request } of server.requests.filter(row => row.request.method === 'server/info')) server.reply(request);
  await Promise.all(occupied); await refused; expect(cancels()).toHaveLength(0);
});

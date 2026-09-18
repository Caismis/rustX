import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { AppServerClient } from '../src/client/app-server';
import { ConnectionController } from '../src/connection/controller';
import { App } from '../src/app/App';
import { ConnectionSettings } from '../src/app/settings/ConnectionSettings';
import { Server, TOKEN, endpoint, interaction } from './fixture';

const remote = 'wss://remote.example/';
function pair() {
  const a = new Server(), b = new Server();
  const client = new AppServerClient((url, protocols) => (url === endpoint ? a : b).socketFactory(url, protocols));
  client.setAttachmentAdmission(async () => true);
  const owner = new ConnectionController(client, async () => new Response(JSON.stringify({ connectionMode: 'local', appServerEndpoint: endpoint, appServerTransportToken: TOKEN }), { headers: { 'content-type': 'application/json' } }));
  return { a, b, client, owner };
}
afterEach(() => { cleanup(); localStorage.clear(); vi.useRealTimers(); });
it('two authorities containing Session A never inherit wanted attachment intent in either direction; same authority restores it', async () => {
  const { a, b, client, owner } = pair();
  await owner.start(); await client.attach('A');
  await owner.disconnect(); await owner.reconnect();
  expect(a.requests.filter(row => row.request.method === 'session/attach')).toHaveLength(2);
  await owner.select('remote'); await owner.connectRemote(remote, TOKEN);
  expect(client.getSnapshot().sessions.some(row => row.id === 'A')).toBe(true);
  expect(client.getSnapshot().views).toEqual({});
  expect(b.requests.map(row => row.request.method)).toEqual(['initialize', 'session/list']);
  await client.attach('A');
  await owner.select('local');
  expect(client.getSnapshot().views).toEqual({});
  expect(a.requests.filter(row => row.request.method === 'session/attach')).toHaveLength(2);
  await owner.disconnect();
});
it('replacement fences immediately and waits for exactly one close before retiring state and creating the new socket', async () => {
  const { a, b, client, owner } = pair(); await owner.start(); await client.attach('A');
  const oldGeneration = client.getSnapshot().generation;
  const close = vi.spyOn(a.socket, 'close').mockImplementation(() => {});
  const switching = client.connect(remote, TOKEN, 'replace-authority');
  expect(client.getSnapshot().generation).toBeGreaterThan(oldGeneration);
  expect(client.getSnapshot().views.A.target).toBeUndefined();
  await expect(client.request({ method: 'session/list', params: { offset: 0, limit: 32 } }, 'sessions')).rejects.toThrow('Connect and initialize');
  expect(b.sockets).toHaveLength(0); expect(close).toHaveBeenCalledTimes(1);
  a.socket.onclose?.(new CloseEvent('close')); await switching;
  expect(b.sockets).toHaveLength(1); expect(client.getSnapshot().views).toEqual({});
  await owner.disconnect();
});
it('old mutation uncertainty stays inspectable and inert despite the same Session ID on the replacement server', async () => {
  const { a, b, client, owner } = pair(); await owner.start(); await client.attach('A');
  a.held.add('turn/start');
  const sending = client.send('A', 'do not replay').catch(error => error);
  await a.waitFor('turn/start', 1);
  await owner.select('remote'); await sending; await owner.connectRemote(remote, TOKEN);
  expect(client.getSnapshot().uncertain).toEqual([]);
  expect(client.getSnapshot().interactionOperations).toEqual({});
  expect(client.getSnapshot().detached).toEqual([expect.objectContaining({ authority: endpoint, operations: [expect.objectContaining({ method: 'turn/start', sessionId: 'A' })] })]);
  expect(b.requests.some(row => row.request.method === 'turn/start')).toBe(false);
  await client.attach('A'); await client.send('A', 'new explicit B operation');
  expect(b.requests.filter(row => row.request.method === 'turn/start')).toHaveLength(1);
  render(<ConnectionSettings connection={owner} client={client} />);
  expect(screen.getByText('Detached authority diagnostics')).toBeTruthy();
  expect(screen.getByRole('heading', { name: endpoint })).toBeTruthy();
  await act(async () => owner.disconnect());
});
it.each(['committed_cleanup_pending', 'committed_durability_uncertain'] as const)('replacement refuses unresolved %s without dropping recovery', async status => {
  const { a, b, client, owner } = pair(); await owner.start(); await client.attach('A');
  a.handlers.set('session/delete', () => ({ type: 'deletion', result: { status, session_id: 'A' } }));
  await client.deleteSession('A', 'revision');
  await owner.select('remote'); await owner.connectRemote(remote, TOKEN);
  expect(owner.getSnapshot().error).toContain('Resolve pending Session deletion');
  expect(client.getSnapshot().views.A.deletionRecovery).toBe(status);
  expect(b.sockets).toHaveLength(0); expect(client.getSnapshot().endpoint).toBe(endpoint);
  await owner.disconnect();
});
it('authority replacement clears open/focused browser Session A before B can reuse its ID', async () => {
  const { a, b, client, owner } = pair(); await owner.start();
  await act(async () => { render(<App client={client} connection={owner} workspaceHost={a.workspaceHost} />); });
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Session A' })));
  expect(screen.getByLabelText('Session title').textContent).toBe('Session A');
  fireEvent.click(screen.getByRole('button', { name: 'Session actions for Session A' }));
  fireEvent.click(screen.getByRole('menuitem', { name: 'Rename' }));
  expect(screen.getByRole('dialog', { name: 'Rename session' })).toBeTruthy();
  await act(async () => { await owner.select('remote'); await owner.connectRemote(remote, TOKEN); });
  expect(screen.queryByLabelText('Session title')).toBeNull();
  expect(screen.queryByRole('dialog', { name: 'Rename session' })).toBeNull();
  expect(screen.getByText('What would you like to work on?')).toBeTruthy();
  expect(JSON.parse(localStorage.getItem('rustx-console-view-v2')!)).toEqual({ endpoint: remote, openViews: [] });
  expect(b.requests.some(row => row.request.method === 'session/attach')).toBe(false);
  await act(async () => owner.disconnect());
});
it('normal Settings opens Overview, while recovery Show details opens Connection', async () => {
  const { a, client, owner } = pair();
  render(<App client={client} connection={owner} workspaceHost={a.workspaceHost} />);
  fireEvent.click(screen.getByRole('button', { name: 'Settings' }));
  expect(screen.getByRole('button', { name: 'Overview' }).getAttribute('aria-current')).toBe('page');
  fireEvent.click(screen.getByRole('button', { name: 'Close Settings' }));
  fireEvent.click(screen.getByRole('button', { name: 'Show details' }));
  expect(screen.getByRole('button', { name: 'Connection' }).getAttribute('aria-current')).toBe('page');
});
it('interaction uncertainty cannot lock the same Session/interaction on another authority', async () => {
  const { a, b, client, owner } = pair();
  const pending = interaction('approval');
  a.snapshots.get('A')!.pending_interactions = [pending]; b.snapshots.get('A')!.pending_interactions = [pending];
  await owner.start(); await client.attach('A');
  a.held.add('interaction/cancel');
  const request = client.request({ method: 'interaction/cancel', params: { target: client.target('A'), interaction: pending.interaction } }, 'interaction_settled').catch(error => error);
  await a.waitFor('interaction/cancel', 1);
  await owner.select('remote'); await request; await owner.connectRemote(remote, TOKEN); await client.attach('A');
  expect(client.getSnapshot().interactionOperations).toEqual({});
  expect(client.getSnapshot().detached?.[0].operations[0].interactionKey).toBeDefined();
  expect(b.requests.some(row => row.request.method === 'interaction/cancel')).toBe(false);
  await owner.disconnect();
});
it('close timeout fails closed without retiring unresolved evidence or creating a replacement socket', async () => {
  const { a, b, client, owner } = pair(); await owner.start();
  vi.useFakeTimers(); vi.spyOn(a.socket, 'close').mockImplementation(() => {});
  const switching = client.connect(remote, TOKEN, 'replace-authority');
  const failure = expect(switching).rejects.toThrow('Previous WebSocket did not close');
  await vi.advanceTimersByTimeAsync(30_000); await failure;
  expect(b.sockets).toHaveLength(0); expect(client.getSnapshot().endpoint).toBe(endpoint);
  expect(client.getSnapshot().connection).toBe('disconnected');
});
it('saved view hints from a different endpoint cannot open the colliding Session ID', async () => {
  const { a, client, owner } = pair();
  localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint: remote, openViews: ['A'] }));
  render(<App client={client} connection={owner} workspaceHost={a.workspaceHost} />);
  await act(async () => owner.start());
  expect(screen.queryByLabelText('Session title')).toBeNull();
  expect(a.requests.some(row => row.request.method === 'session/attach')).toBe(false);
  await act(async () => owner.disconnect());
});
it('detached evidence capacity refuses replacement rather than silently evicting unresolved operations', async () => {
  const { a, b, client, owner } = pair(); await owner.start();
  a.held.add('session/name'); b.held.add('session/name');
  for (let index = 0; index < 8; index++) {
    const lost = client.request({ method: 'session/name', params: { session_id: 'A', name: 'Uncertain name' } }, 'session').catch(error => error);
    await client.connect(index % 2 === 0 ? remote : endpoint, TOKEN, 'replace-authority'); await lost;
  }
  const evidence = client.getSnapshot().detached;
  expect(evidence).toHaveLength(8);
  await expect(client.connect(remote, TOKEN, 'replace-authority')).rejects.toThrow('Review and acknowledge');
  expect(client.getSnapshot().detached).toBe(evidence);
  expect(client.getSnapshot().endpoint).toBe(endpoint);
  expect(a.requests.concat(b.requests).filter(row => row.request.method === 'session/name')).toHaveLength(8);
  client.acknowledgeDetached(0);
  await client.connect(remote, TOKEN, 'replace-authority');
  expect(client.getSnapshot().detached).toHaveLength(7);
  await owner.disconnect();
});

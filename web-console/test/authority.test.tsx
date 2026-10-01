import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { localeController } from '../src/locale/controller';
import { translator } from '../src/locale/translation';
import { AppServerClient } from '../src/client/app-server';
import { ConnectionController } from '../src/connection/controller';
import { App } from '../src/app/App';
import { ConnectionSettings } from '../src/app/settings/ConnectionSettings';
import { Server, TOKEN, endpoint, interaction } from './fixture';

const remote = 'wss://remote.example/';
function pair() {
  const a = new Server(), b = new Server();
  const client = new AppServerClient((url, protocols) => (url === endpoint ? a : b).socketFactory(url, protocols));
  client.setAttachmentAdmission(async () => ({ current: () => true, validate: async () => true }));
  const owner = new ConnectionController(client, async () => new Response(JSON.stringify({ connectionMode: 'local', appServerEndpoint: endpoint, appServerTransportToken: TOKEN }), { headers: { 'content-type': 'application/json' } }));
  return { a, b, client, owner };
}
afterEach(() => { cleanup(); localStorage.clear(); localeController.setLocale('en'); vi.useRealTimers(); });
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
  await owner.disconnect(); await owner.reconnect();
  expect(b.requests.filter(row => row.request.method === 'session/attach')).toHaveLength(2);
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
  await owner.select('remote'); await owner.connectRemote(remote, TOKEN); await sending;
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
it.each(['committed_cleanup_pending', 'committed_durability_uncertain'] as const)('Remote %s is retained outside Local active views', async status => {
  const { a, b, client, owner } = pair(); await owner.select('remote'); await owner.connectRemote(remote, TOKEN);
  b.handlers.set('session/delete', () => ({ type: 'deletion', result: { status, session_id: 'A' } }));
  await client.deleteSession('A', 'revision'); await owner.select('local');
  expect(client.getSnapshot().connection).toBe('connected');
  expect(client.getSnapshot().views).toEqual({});
  expect(client.getSnapshot().detached?.[0].sessions).toEqual([expect.objectContaining({ id: 'A', deletion: status })]);
  expect(a.requests.map(row => row.request.method)).toEqual(['initialize', 'session/list']);
  await owner.disconnect();
});
it.each(['uncertain', 'committed_cleanup_pending', 'committed_durability_uncertain'] as const)('same endpoint replacement retains %s deletion without replay', async status => {
  const { a, client, owner } = pair(); await owner.start(); await client.attach('A');
  const oldSocket = a.socket;
  a.handlers.set('session/delete', () => ({ type: 'deletion', result: { status: 'committed_cleanup_pending', session_id: 'A' } }));
  if (status === 'uncertain') a.held.add('session/delete');
  else a.handlers.set('session/delete', () => ({ type: 'deletion', result: { status, session_id: 'A' } }));
  const deleting = client.deleteSession('A', 'old-revision').catch(error => error);
  const request = await a.waitFor('session/delete', 1);
  if (status !== 'uncertain') await deleting;
  await owner.disconnect(); await deleting;
  a.authorityId = 'replacement-native';
  await owner.reconnect();
  expect(client.getSnapshot().connection).toBe('connected');
  expect(client.getSnapshot().authorityId).toBe('replacement-native');
  expect(client.getSnapshot().sessions.some(row => row.id === 'A')).toBe(true);
  expect(client.getSnapshot().views).toEqual({});
  const evidence = client.getSnapshot().detached;
  expect(evidence?.[0]).toMatchObject({ authority: endpoint, authorityId: 'fixture-app-server-authority', sessions: [{ id: 'A', deletion: status }] });
  expect(a.socket.requests.map(row => row.method)).toEqual(['initialize', 'session/list']);
  oldSocket.success(request, { type: 'deletion', result: { status: 'deleted', session_id: 'A' } });
  oldSocket.deliver({ jsonrpc: '2.0', id: request.id, error: { code: -32000, message: 'late failure' } });
  expect(client.getSnapshot().detached).toBe(evidence);
  expect(client.getSnapshot().views).toEqual({});
  expect(client.getSnapshot().sessions.some(row => row.id === 'A')).toBe(true);
  expect(a.requests.filter(row => row.request.method === 'session/delete')).toHaveLength(1);
  expect(a.requests.filter(row => ['session/deletePreview', 'session/recoverDeletion'].includes(row.request.method))).toHaveLength(0);
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
  expect(screen.getByText('What would you like to build?')).toBeTruthy();
  expect(JSON.parse(localStorage.getItem('rustx-console-view-v2')!)).toEqual({ endpoint: remote, openViews: [] });
  expect(b.requests.some(row => row.request.method === 'session/attach')).toBe(false);
  await act(async () => owner.disconnect());
});
it('normal Settings opens General, while recovery Show details opens the Advanced Connection sub-surface', async () => {
  const { a, client, owner } = pair();
  render(<App client={client} connection={owner} workspaceHost={a.workspaceHost} />);
  fireEvent.click(screen.getByRole('button', { name: 'Settings' }));
  expect(screen.getByRole('tab', { name: 'General' }).getAttribute('aria-selected')).toBe('true');
  fireEvent.click(screen.getByRole('button', { name: 'Close Settings' }));
  fireEvent.click(screen.getByRole('button', { name: 'Show details' }));
  expect(screen.getByRole('tab', { name: 'Advanced' }).getAttribute('aria-selected')).toBe('true');
  expect(screen.getByRole('region', { name: 'Connection Settings' })).toBeTruthy();
});
it('interaction uncertainty cannot lock the same Session/interaction on another authority', async () => {
  const { a, b, client, owner } = pair();
  const pending = interaction('approval');
  a.snapshots.get('A')!.pending_interactions = [pending]; b.snapshots.get('A')!.pending_interactions = [pending];
  await owner.start(); await client.attach('A');
  a.held.add('interaction/cancel');
  const request = client.request({ method: 'interaction/cancel', params: { target: client.target('A'), interaction: pending.interaction } }, 'interaction_settled').catch(error => error);
  await a.waitFor('interaction/cancel', 1);
  await owner.select('remote'); await owner.connectRemote(remote, TOKEN); await request; await client.attach('A');
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
it('detached evidence capacity refusal preserves Remote and its material until explicit acknowledgement', async () => {
  const { a, b, client, owner } = pair(); await owner.select('remote'); await owner.connectRemote(remote, TOKEN);
  a.held.add('session/name'); b.held.add('session/name');
  for (let index = 0; index < 8; index++) {
    const lost = client.request({ method: 'session/name', params: { session_id: 'A', name: 'Uncertain name' } }, 'session').catch(error => error);
    await owner.select(index % 2 === 0 ? 'local' : 'remote'); await lost;
  }
  const before = client.getSnapshot(), close = vi.spyOn(b.socket, 'close'), localCount = a.sockets.length;
  expect(before.detached).toHaveLength(8);
  await owner.select('local');
  expect(owner.getSnapshot().error).toContain('Review and acknowledge');
  expect(owner.getSnapshot().mode).toBe('remote'); expect(client.getSnapshot()).toBe(before);
  expect(close).not.toHaveBeenCalled(); expect(a.sockets).toHaveLength(localCount);
  await client.listSessions();
  client.acknowledgeDetached(0);
  await owner.select('local');
  expect(close).toHaveBeenCalledTimes(1); expect(a.sockets).toHaveLength(localCount + 1);
  expect(client.getSnapshot().detached).toHaveLength(7);
  await owner.select('remote'); expect(owner.getSnapshot().mode).toBe('remote');
  await owner.disconnect();
});
it('Session diagnostic capacity refuses before closing the current Remote socket', async () => {
  const { a, client, owner, b } = pair(); await owner.select('remote'); await owner.connectRemote(remote, TOKEN);
  client.setAttachmentAdmission(async () => { throw new Error('Review this Session'); });
  for (let i = 0; i < 65; i++) await client.attach(`diagnostic-${i}`).catch(() => {});
  const before = client.getSnapshot(), close = vi.spyOn(b.socket, 'close');
  await owner.select('local');
  expect(owner.getSnapshot().error).toContain('Too many unresolved Session diagnostics');
  expect(owner.getSnapshot().mode).toBe('remote'); expect(client.getSnapshot()).toBe(before);
  expect(close).not.toHaveBeenCalled(); expect(a.sockets).toHaveLength(0);
  await client.listSessions();
  // A new explicit inspection clears one diagnostic without adding another.
  client.setAttachmentAdmission(async () => false);
  await client.attach('diagnostic-0');
  await owner.select('local');
  expect(close).toHaveBeenCalledTimes(1); expect(a.sockets).toHaveLength(1);
  expect(client.getSnapshot().detached?.[0].sessions).toHaveLength(64);
  await owner.select('remote'); expect(owner.getSnapshot().mode).toBe('remote');
  await owner.disconnect();
});
it('admission reserves the last detached batch for eight transmitted mutations and discards queued work', async () => {
  const { a, b, client, owner } = pair(); await owner.select('remote'); await owner.connectRemote(remote, TOKEN);
  a.held.add('session/name'); b.held.add('session/name');
  for (let index = 0; index < 7; index++) {
    const lost = client.request({ method: 'session/name', params: { session_id: 'A', name: 'Earlier' } }, 'session').catch(error => error);
    await owner.select(index % 2 === 0 ? 'local' : 'remote'); await lost;
  }
  const old = a.socket, close = vi.spyOn(old, 'close').mockImplementation(() => {});
  const pending = Array.from({ length: 64 }, (_, i) => client.request({ method: 'session/name', params: { session_id: 'A', name: `Pending ${i}` } }, 'session').catch(error => error));
  expect(old.requests.filter(row => row.method === 'session/name')).toHaveLength(8);
  const replacement = owner.select('remote');
  expect(close).toHaveBeenCalledTimes(1);
  expect(client.getSnapshot().uncertain).toHaveLength(8);
  const outcomes = await Promise.all(pending);
  expect(outcomes.filter(error => String(error).includes('Unsent operations were discarded'))).toHaveLength(56);
  old.onclose?.(new CloseEvent('close')); await replacement;
  expect(client.getSnapshot().detached).toHaveLength(8);
  const operations = client.getSnapshot().detached![7].operations;
  expect(operations).toHaveLength(8); expect(new Set(operations.map(item => item.id)).size).toBe(8);
  expect(client.getSnapshot().uncertain).toEqual([]); expect(owner.getSnapshot().mode).toBe('remote');
  await owner.disconnect();
});
it('failed Local transport after ownership commit has no Remote fallback but permits explicit return', async () => {
  const { a, b, client, owner } = pair(); await owner.select('remote'); await owner.connectRemote(remote, TOKEN);
  a.version = 7;
  await owner.select('local');
  expect(owner.getSnapshot().mode).toBe('local'); expect(owner.getSnapshot().error).toContain('Incompatible');
  expect(client.getSnapshot().endpoint).toBe(endpoint); expect(b.sockets).toHaveLength(1);
  await owner.select('remote');
  expect(b.sockets).toHaveLength(2); expect(client.getSnapshot().connection).toBe('connected');
  await owner.disconnect();
});

it.each(['en', 'zh'] as const)('replacement deletion evidence is reachable and acknowledged without native settlement in %s', async locale => {
  const { a, client, owner } = pair(); await owner.start();
  a.held.add('session/delete');
  const pending = client.deleteSession('A', 'old-revision').catch(error => error);
  await a.waitFor('session/delete', 1); await owner.disconnect(); await pending;
  a.authorityId = 'replacement'; await owner.reconnect();
  a.held.delete('session/delete');
  for (const status of ['committed_cleanup_pending', 'committed_durability_uncertain'] as const) {
    a.handlers.set('session/delete', () => ({ type: 'deletion', result: { status, session_id: 'A' } }));
    await client.deleteSession('A', 'fresh-revision'); await owner.disconnect();
    a.authorityId = status; await owner.reconnect();
  }
  await act(async () => { render(<App client={client} connection={owner} workspaceHost={a.workspaceHost} />); });
  fireEvent.click(screen.getByRole('button', { name: 'Settings' }));
  fireEvent.click(screen.getByRole('tab', { name: 'Advanced' }));
  fireEvent.click(screen.getByRole('button', { name: 'Connection' }));
  act(() => localeController.setLocale(locale)); const tx = translator(locale);
  fireEvent.click(screen.getByText(tx('settings:connection-settings.detached-authority-diagnostics')));
  for (const key of ['settings:connection-settings.deletion-uncertain', 'settings:connection-settings.deletion-cleanup', 'settings:connection-settings.deletion-durability'] as const) {
    expect(screen.getByText(`A: ${tx(key)}`)).toBeTruthy();
  }
  expect(screen.getByText(tx('settings:connection-settings.historical-evidence-only-these-operations-are-never-replayed-and'))).toBeTruthy();
  const before = a.requests.length;
  for (let count = 0; count < 3; count++) {
    await act(async () => fireEvent.click(screen.getAllByRole('button', { name: tx('settings:connection-settings.i-have-reviewed-this-historical-evidence') })[0]));
  }
  expect(client.getSnapshot().detached).toEqual([]);
  expect(a.requests).toHaveLength(before);
  expect(client.getSnapshot().sessions.some(row => row.id === 'A')).toBe(true);
  await act(async () => owner.disconnect()); act(() => localeController.setLocale('en'));
});
it('same-endpoint diagnostic capacity refusal has a disconnected UI acknowledgement path', async () => {
  const { a, client, owner } = pair(); await owner.start(); a.held.add('session/delete');
  for (let index = 0; index < 8; index++) {
    const pending = client.deleteSession('A', 'old').catch(error => error);
    await a.waitFor('session/delete', index + 1); await owner.disconnect(); await pending;
    a.authorityId = `replacement-${index}`; await owner.reconnect();
  }
  expect(client.getSnapshot().detached).toHaveLength(8);
  await owner.disconnect(); a.authorityId = 'capacity-replacement'; await owner.reconnect();
  expect(client.getSnapshot().connection).toBe('error');
  expect(a.socket.requests.map(row => row.method)).toEqual(['initialize']);
  render(<ConnectionSettings connection={owner} client={client} />);
  fireEvent.click(screen.getByText('Detached authority diagnostics'));
  const count = a.requests.length;
  fireEvent.click(screen.getAllByRole('button', { name: 'I have reviewed this historical evidence' })[0]);
  expect(a.requests).toHaveLength(count);
  await act(async () => owner.reconnect());
  expect(client.getSnapshot().connection).toBe('connected');
  expect(a.socket.requests.map(row => row.method)).toEqual(['initialize', 'session/list']);
  await act(async () => owner.disconnect());
});
it('lost recovery preserves the earlier committed fact when the native authority is replaced', async () => {
  const { a, client, owner } = pair(); await owner.start();
  a.handlers.set('session/delete', () => ({ type: 'deletion', result: { status: 'committed_durability_uncertain', session_id: 'A' } }));
  await client.deleteSession('A', 'old'); a.held.add('session/recoverDeletion');
  const recovery = client.recoverSessionDeletion('A').catch(error => error);
  await a.waitFor('session/recoverDeletion', 1); await owner.disconnect(); await recovery;
  expect(client.getSnapshot().views.A.deletionRecovery).toBeUndefined();
  a.authorityId = 'replacement'; await owner.reconnect();
  expect(client.getSnapshot().detached?.[0].sessions[0].deletion).toBe('committed_durability_uncertain');
  expect(a.socket.requests.map(row => row.method)).toEqual(['initialize', 'session/list']);
  await owner.disconnect();
});

it.each(['en', 'zh'] as const)('disconnected current diagnostic acknowledgement frees capacity without settling a deletion in %s', async locale => {
  const { a, client, owner } = pair(); await owner.start();
  a.handlers.set('session/delete', () => ({ type: 'deletion', result: { status: 'committed_cleanup_pending', session_id: 'A' } }));
  await client.deleteSession('A', 'old');
  client.setAttachmentAdmission(async () => { throw new Error('diagnostic'); });
  for (let index = 0; index < 64; index++) await client.attach(`diagnostic-${index}`).catch(() => {});
  await owner.disconnect(); a.authorityId = 'replacement'; await owner.reconnect();
  expect(client.getSnapshot().connection).toBe('error');
  expect(client.getSnapshot().views.A.deletionCommitted).toBe('committed_cleanup_pending');
  render(<ConnectionSettings connection={owner} client={client} />);
  act(() => localeController.setLocale(locale)); const tx = translator(locale);
  fireEvent.click(screen.getByText(tx('settings:connection-settings.current-evidence')));
  const count = a.requests.length, deleted = vi.fn(); client.subscribeSessionDeletion(deleted);
  fireEvent.click(screen.getAllByRole('button', { name: tx('settings:connection-settings.i-have-reviewed-this-historical-evidence') })[0]);
  expect(a.requests).toHaveLength(count); expect(deleted).not.toHaveBeenCalled();
  expect(client.getSnapshot().views.A).toBeUndefined();
  await act(async () => owner.reconnect());
  expect(client.getSnapshot().connection).toBe('connected');
  expect(client.getSnapshot().detached?.[0].sessions).toHaveLength(64);
  expect(a.socket.requests.map(row => row.method)).toEqual(['initialize', 'session/list']);
  await act(async () => owner.disconnect());
});

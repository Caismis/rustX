import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { App } from '../src/app/App';
import { NavigationEpoch } from '../src/app/commands/native';
import { createWorkspaceSession } from '../src/workspaces/navigation';
import { sessionObservation } from '../src/workspaces/WorkspaceNavigation';
import type { ProductHostWorkspaces, WorkspaceCatalog } from '../src/workspaces/host';
import { Server, endpoint, snapshot } from './fixture';
let server: Server;
beforeEach(() => {
  server = new Server(); localStorage.clear();
  HTMLDialogElement.prototype.showModal = function () { this.setAttribute('open', ''); };
  HTMLDialogElement.prototype.close = function () { this.removeAttribute('open'); };
});
afterEach(() => { cleanup(); server.client.disconnect(); });
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(done => { resolve = done; }); return { promise, resolve }; }
function hostFixture(picker = true) {
  let rows = ['A', 'B'].map(id => ({ id: `w${id}`, displayName: `Workspace ${id}`, location: `root-${id}`, displayPath: `/workspace/${id}` }));
  const host: ProductHostWorkspaces = {
    listWorkspaces: vi.fn(async (): Promise<WorkspaceCatalog> => ({ endpoint, workspaces: rows, picker: picker ? { kind: 'configured', locations: [{ id: 'root-C', displayName: 'Workspace C' }] } : { kind: 'unavailable', reason: 'Directory picker unavailable' } })),
    groupSessions: vi.fn(async (cwds: readonly string[]) => cwds.map(cwd => rows.find(row => row.displayPath === cwd)?.id ?? null)),
    resolveWorkspace: vi.fn(async id => { const row = rows.find(row => row.id === id); if (!row) throw new Error('Unauthorized'); return { cwd: row.displayPath }; }),
    renameWorkspace: vi.fn(async (id, displayName) => { rows = rows.map(row => row.id === id ? { ...row, displayName } : row); }),
    reorderWorkspace: vi.fn(async () => {}), removeWorkspace: vi.fn(async id => { rows = rows.filter(row => row.id !== id); }),
    adoptWorkspace: vi.fn(async () => {}),
  };
  return host;
}
async function mount(host = hostFixture()) {
  await server.connect();
  await act(async () => { render(<App client={server.client} workspaceHost={host} />); });
  return host;
}
const methods = () => server.requests.map(row => row.request.method);
it('cold grouping and Workspace selection issue no attach, cancel, unload, settings write, or trust operation', async () => {
  const host = await mount(hostFixture(false));
  expect(screen.queryByLabelText('Session cwd')).toBeNull();
  expect(screen.queryByRole('button', { name: 'Add Workspace' })).toBeNull();
  expect(screen.getByText('Directory picker unavailable')).toBeTruthy();
  expect(screen.getAllByText('Durable · unloaded (list observation)')).toHaveLength(2);
  await act(async () => fireEvent.change(screen.getByLabelText('Choose Workspace'), { target: { value: 'wA' } }));
  expect((screen.getByRole('button', { name: 'Workspace settings' }) as HTMLButtonElement).disabled).toBe(true);
  expect(methods()).toEqual(['initialize', 'session/list']);
  expect(host.resolveWorkspace).not.toHaveBeenCalled(); expect(server.loaded.size).toBe(0);
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Session A' })));
  expect(server.coldLoads.get('A')).toBe(1);
  await act(async () => fireEvent.change(screen.getByLabelText('Choose Workspace'), { target: { value: 'wB' } }));
  expect(server.loaded.has('A')).toBe(true);
  expect(methods()).not.toContain('session/unload'); expect(methods()).not.toContain('turn/cancel');
});
it('native untrusted and unknown fail closed independently of Host registration; names and unregister stay Host-owned', async () => {
  server.handlers.set('settings/read', () => ({ type: 'settings', revision: '0', settings: { cwd: '/workspace/A' }, project_trusted: false }));
  const host = await mount();
  await act(async () => fireEvent.change(screen.getByLabelText('Choose Workspace'), { target: { value: 'wA' } }));
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Session A' })));
  expect(screen.getByText(/Untrusted project source/)).toBeTruthy();
  expect((screen.getByRole('button', { name: 'Workspace settings' }) as HTMLButtonElement).disabled).toBe(true);
  await act(async () => fireEvent.click(screen.getAllByText('Rename Workspace')[0]));
  fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'Renamed A' } });
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Save name' })));
  expect(host.renameWorkspace).toHaveBeenCalledWith('wA', 'Renamed A');
  await act(async () => fireEvent.click(screen.getAllByText('Unregister Workspace')[0]));
  await act(async () => fireEvent.click(screen.getByRole('button', { name: /^Unregister$/ })));
  expect(host.removeWorkspace).toHaveBeenCalledWith('wA'); expect(server.snapshots.has('A')).toBe(true);
  expect(server.loaded.has('A')).toBe(true); expect(methods()).not.toContain('settings/replace'); expect(methods()).not.toContain('session/delete');
});
it('picker capability exposes only authorized choices and Session rename uses the native typed operation', async () => {
  const host = await mount();
  server.handlers.set('session/name', request => {
    if (request.method !== 'session/name') throw new Error('wrong request');
    return { type: 'session', session: { id: request.params.session_id, name: request.params.name, active_node: 'node-A', active_conversation_id: 'conversation-A', node_count: 1, created_at: '0', updated_at: '0' } };
  });
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Add Workspace' })));
  const dialog = screen.getByRole('dialog', { name: 'Add Workspace' });
  expect(within(dialog).queryByRole('textbox')).toBeNull();
  await act(async () => fireEvent.click(within(dialog).getByRole('button', { name: 'Workspace C' })));
  expect(host.adoptWorkspace).toHaveBeenCalledWith('root-C');
  await act(async () => fireEvent.click(screen.getAllByText('Rename Session')[0]));
  fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'Native name' } });
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Save name' })));
  expect(server.requests.find(row => row.request.method === 'session/name')?.request.params).toEqual({ session_id: 'A', name: 'Native name' });
  expect(host.renameWorkspace).not.toHaveBeenCalled();
});
it('Host resolution supplies exact cwd, rejects unauthorized identifiers, and a late resolution cannot create or reopen', async () => {
  await server.connect(); const host = hostFixture(), navigation = new NavigationEpoch();
  const gate = deferred<{ cwd: string }>(); vi.mocked(host.resolveWorkspace).mockReturnValueOnce(gate.promise);
  const pending = createWorkspaceSession(host, 'wA', endpoint, server.client, navigation.capture());
  navigation.invalidate(); gate.resolve({ cwd: '/workspace/A' }); expect(await pending).toBeUndefined();
  expect(methods()).not.toContain('session/create');
  await expect(createWorkspaceSession(host, '/arbitrary/path', endpoint, server.client, navigation.capture())).rejects.toThrow('Unauthorized');
  server.handlers.set('session/create', request => {
    if (request.method !== 'session/create') throw new Error('wrong request');
    expect(request.params.settings.cwd).toBe('/workspace/A');
    server.snapshots.set('created', snapshot('created'));
    return { type: 'session_transition', session: { id: 'created', active_node: 'node-created', active_conversation_id: 'conversation-created', node_count: 1, created_at: '0', updated_at: '0' } };
  });
  expect((await createWorkspaceSession(host, 'wA', endpoint, server.client, navigation.capture()))?.session.id).toBe('created');
});
it('late metadata searches cannot replace newer results or results after Workspace navigation', async () => {
  await mount(); server.held.add('session/list');
  server.handlers.set('session/list', request => {
    if (request.method !== 'session/list') throw new Error('wrong request');
    const id = request.params.query!;
    return { type: 'sessions', residencies: { [id]: 'Unloaded' }, sessions: [{ id, cwd: '/workspace/A', active_node: `node-${id}`, name: id, updated_at: '0' }] };
  });
  fireEvent.change(screen.getByLabelText('Search Session metadata'), { target: { value: 'old' } });
  const old = await server.waitFor('session/list', 2);
  fireEvent.change(screen.getByLabelText('Search Session metadata'), { target: { value: 'new' } });
  const fresh = await server.waitFor('session/list', 3);
  await act(async () => server.reply(fresh)); await act(async () => server.reply(old));
  expect(server.client.getSnapshot().sessions[0].id).toBe('new');
  fireEvent.change(screen.getByLabelText('Search Session metadata'), { target: { value: 'obsolete' } });
  const obsolete = await server.waitFor('session/list', 4);
  fireEvent.change(screen.getByLabelText('Choose Workspace'), { target: { value: 'wB' } });
  await act(async () => server.reply(obsolete));
  expect(server.client.getSnapshot().sessions[0].id).toBe('new');
});
it('late cold open cannot restore focus after Workspace navigation', async () => {
  await mount(); server.held.add('session/attach');
  fireEvent.click(screen.getByRole('button', { name: 'Open Session A' }));
  const request = await server.waitFor('session/attach', 1);
  fireEvent.change(screen.getByLabelText('Choose Workspace'), { target: { value: 'wB' } });
  await act(async () => { server.reply(request); });
  expect(document.querySelector('.session-toolbar')).toBeNull();
  expect((screen.getByLabelText('Choose Workspace') as HTMLSelectElement).value).toBe('wB');
  expect(methods()).not.toContain('session/unload'); expect(methods()).not.toContain('turn/cancel');
});
it('stale or unloaded snapshots cannot claim running work', () => {
  const view = { id: 'A', attachmentIntent: 'wanted' as const, attachment: 'unloaded' as const, snapshot: snapshot('A') };
  expect(sessionObservation(view, true)).toBe('Durable · unloaded');
  expect(sessionObservation(view, false)).toBe('Observation stale / disconnected');
});
it('sidebar Fork uses the exact native boundary and late completion cannot undo Workspace focus', async () => {
  const boundary = { surface_revision: '37', message: { id: 'user-cut', kind: 'message' as const, source: 'human' as const, content: [{ type: 'text' as const, text: 'Fork this native boundary' }] } };
  server.handlers.set('session/boundaries', () => ({ type: 'boundaries', surface_revision: '37', boundaries: [boundary] }));
  server.handlers.set('session/tree', () => ({ type: 'tree', nodes: [{ id: 'node-A', conversation_id: 'conversation-A', origin: { type: 'new' } }] }));
  server.handlers.set('session/fork', request => {
    if (request.method !== 'session/fork') throw new Error('wrong request');
    expect(request.params).toEqual({ session_id: 'A', node_id: 'node-A', surface_revision: '37', boundary: 'user-cut' });
    server.snapshots.set('fork-child', snapshot('fork-child'));
    return { type: 'session_transition', session: { id: 'fork-child', active_node: 'node-child', active_conversation_id: 'conversation-fork-child', node_count: 1, created_at: '0', updated_at: '0' } };
  });
  await mount(); server.held.add('session/fork');
  await act(async () => fireEvent.click(screen.getAllByText('Fork Session')[0]));
  const popup = screen.getByRole('dialog', { name: '/fork' });
  fireEvent.click(within(popup).getByRole('option', { name: /Fork this native boundary/ }));
  const request = await server.waitFor('session/fork', 1);
  const committed = server.commit(request);
  fireEvent.change(screen.getByLabelText('Choose Workspace'), { target: { value: 'wB' } });
  await act(async () => server.socket.deliver(committed));
  expect(server.snapshots.has('fork-child')).toBe(true);
  expect(server.client.getSnapshot().views['fork-child']).toBeUndefined();
  expect(document.querySelector('.session-toolbar')).toBeNull();
  expect((screen.getByLabelText('Choose Workspace') as HTMLSelectElement).value).toBe('wB');
  expect(methods()).not.toContain('session/unload'); expect(methods()).not.toContain('turn/cancel');
});

it('late native rename completion cannot replace a newer metadata query', async () => {
  await mount(); server.held.add('session/name');
  server.handlers.set('session/name', () => ({ type: 'session', session: { id: 'A', active_node: 'node-A', active_conversation_id: 'conversation-A', node_count: 1, created_at: '0', updated_at: '0' } }));
  await act(async () => fireEvent.click(screen.getAllByText('Rename Session')[0]));
  fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'Renamed' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save name' }));
  const rename = await server.waitFor('session/name', 1);
  await act(async () => fireEvent.change(screen.getByLabelText('Search Session metadata'), { target: { value: 'new query' } }));
  const lists = methods().filter(method => method === 'session/list').length;
  await act(async () => server.reply(rename));
  expect(methods().filter(method => method === 'session/list')).toHaveLength(lists);
  expect((screen.getByLabelText('Search Session metadata') as HTMLInputElement).value).toBe('new query');
});

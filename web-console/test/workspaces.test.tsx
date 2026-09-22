import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
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
});
afterEach(() => { cleanup(); server.client.disconnect(); });
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(done => { resolve = done; }); return { promise, resolve }; }
function hostFixture(picker = true) {
  let rows = ['A', 'B'].map(id => ({ id: `w${id}`, displayName: `Workspace ${id}`, location: `root-${id}`, displayPath: `/workspace/${id}` }));
  const host: ProductHostWorkspaces = {
    listWorkspaces: vi.fn(async (): Promise<WorkspaceCatalog> => ({ endpoint, workspaces: rows, picker: picker ? { kind: 'configured', locations: [{ id: 'root-C', displayName: 'Workspace C' }] } : { kind: 'unavailable', reason: 'Directory picker unavailable' } })),
    classifyLocations: vi.fn(async (cwds: readonly string[]) => cwds.map(cwd => ['/workspace/A', '/workspace/B'].includes(cwd) ? { authorized: true as const, workspaceId: rows.find(row => row.displayPath === cwd)?.id } : { authorized: false as const })),
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
  expect(host.listWorkspaces).toHaveBeenCalled();
  expect(server.client.getSnapshot()).not.toHaveProperty('sessionResidencies');
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Select Workspace Workspace A' })));
  expect(screen.getByRole('button', { name: 'Settings' })).toBeTruthy();
  expect(methods()).toEqual(['initialize', 'session/list']);
  expect(host.resolveWorkspace).not.toHaveBeenCalled(); expect(server.loaded.size).toBe(0);
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Session A' })));
  expect(server.coldLoads.get('A')).toBe(1);
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Select Workspace Workspace B' })));
  expect(server.loaded.has('A')).toBe(true);
  expect(methods()).not.toContain('session/unload'); expect(methods()).not.toContain('turn/cancel');
});
it('Workspace settings have no trust gate; names and unregister stay Host-owned', async () => {
  server.handlers.set('session/settings', () => ({ type: 'settings', revision: '0', settings: { cwd: '/workspace/A' } }));
  const host = await mount();
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Select Workspace Workspace A' })));
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Session A' })));
  expect(screen.queryByText(/Untrusted project source/)).toBeNull();
  expect(screen.getByRole('button', { name: 'Settings' })).toBeTruthy();
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Workspace actions for Workspace A' })));
  await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'Rename' })));
  fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'Renamed A' } });
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Save name' })));
  expect(host.renameWorkspace).toHaveBeenCalledWith('wA', 'Renamed A');
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Workspace actions for Renamed A' })));
  await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'Unregister Workspace' })));
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
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Session actions for Session A' })));
  await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'Rename' })));
  fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'Native name' } });
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Save name' })));
  expect(server.requests.find(row => row.request.method === 'session/name')?.request.params).toEqual({ session_id: 'A', name: 'Native name' });
  expect(host.renameWorkspace).not.toHaveBeenCalled();
});
it('Host resolution supplies exact cwd, rejects unauthorized identifiers, and a late resolution cannot create or reopen', async () => {
  await server.connect(); const host = hostFixture(), navigation = new NavigationEpoch();
  const gate = deferred<{ cwd: string }>(); vi.mocked(host.resolveWorkspace).mockReturnValueOnce(gate.promise);
  const pending = createWorkspaceSession(host, 'wA', server.client, navigation.capture());
  navigation.invalidate(); gate.resolve({ cwd: '/workspace/A' }); expect(await pending).toBeUndefined();
  expect(methods()).not.toContain('session/create');
  await expect(createWorkspaceSession(host, '/arbitrary/path', server.client, navigation.capture())).rejects.toThrow('Unauthorized');
  server.handlers.set('session/create', request => {
    if (request.method !== 'session/create') throw new Error('wrong request');
    expect(request.params.settings.cwd).toBe('/workspace/A');
    server.snapshots.set('created', snapshot('created'));
    return { type: 'session_transition', session: { id: 'created', active_node: 'node-created', active_conversation_id: 'conversation-created', node_count: 1, created_at: '0', updated_at: '0' } };
  });
  expect((await createWorkspaceSession(host, 'wA', server.client, navigation.capture()))?.session.id).toBe('created');
});
it('late metadata searches cannot replace newer results or results after Workspace navigation', async () => {
  await mount(); server.held.add('session/list');
  server.handlers.set('session/list', request => {
    if (request.method !== 'session/list') throw new Error('wrong request');
    const id = request.params.query!;
    return { type: 'sessions', sessions: [{ id, cwd: '/workspace/A', active_node: `node-${id}`, name: id, updated_at: '0' }] };
  });
  fireEvent.change(screen.getByLabelText('Search Session metadata'), { target: { value: 'old' } });
  const old = await server.waitFor('session/list', 2);
  fireEvent.change(screen.getByLabelText('Search Session metadata'), { target: { value: 'new' } });
  const fresh = await server.waitFor('session/list', 3);
  await act(async () => server.reply(fresh)); await act(async () => server.reply(old));
  expect(server.client.getSnapshot().sessions[0].id).toBe('new');
  fireEvent.change(screen.getByLabelText('Search Session metadata'), { target: { value: 'obsolete' } });
  const obsolete = await server.waitFor('session/list', 4);
  fireEvent.click(screen.getAllByRole('button', { name: 'New Session' })[0]);
  fireEvent.change(screen.getByLabelText('Choose Workspace'), { target: { value: 'wB' } });
  await act(async () => server.reply(obsolete));
  expect(server.client.getSnapshot().sessions[0].id).toBe('new');
});
it('late cold open cannot restore focus after Workspace navigation', async () => {
  await mount(); server.held.add('session/attach');
  fireEvent.click(screen.getByRole('button', { name: 'Open Session A' }));
  const request = await server.waitFor('session/attach', 1);
  fireEvent.click(screen.getByRole('button', { name: 'Select Workspace Workspace B' }));
  await act(async () => { server.reply(request); });
  expect(document.querySelector('.session-toolbar')).toBeNull();
  expect(screen.getByRole('button', { name: 'Select Workspace Workspace B' }).getAttribute('aria-current')).toBe('page');
  expect(methods()).not.toContain('session/unload'); expect(methods()).not.toContain('turn/cancel');
});
it('stale or unloaded snapshots cannot claim running work', () => {
  const view = { id: 'A', attachmentIntent: 'wanted' as const, attachment: 'stale' as const, snapshot: snapshot('A') };
  expect(sessionObservation({ ...server.client.getSnapshot(), views: { A: view }, connection: 'connected' }, 'A')).toBe('Connection interrupted');
  expect(sessionObservation({ ...server.client.getSnapshot(), views: { A: view }, connection: 'disconnected' }, 'A')).toBe('Connection interrupted');
});
it('sidebar Fork uses the exact native boundary and late completion cannot undo Workspace focus', async () => {
  const boundary = { surface_revision: '37', message: { id: 'user-cut', kind: 'message' as const, source: 'human' as const, content: [{ type: 'text' as const, text: 'Fork this native boundary' }] } };
  server.handlers.set('session/boundaries', () => ({ type: 'boundaries', surface_revision: '37', boundaries: [boundary] }));
  server.handlers.set('session/tree', () => ({ type: 'tree', nodes: [{ id: 'node-A', conversation_id: 'conversation-A', ordinal: '1', origin: { type: 'new' } }] }));
  server.handlers.set('session/fork', request => {
    if (request.method !== 'session/fork') throw new Error('wrong request');
    expect(request.params).toEqual({ side: 'before', session_id: 'A', node_id: 'node-A', surface_revision: '37', boundary: 'user-cut' });
    server.snapshots.set('fork-child', snapshot('fork-child'));
    return { type: 'session_transition', session: { id: 'fork-child', active_node: 'node-child', active_conversation_id: 'conversation-fork-child', node_count: 1, created_at: '0', updated_at: '0' } };
  });
  await mount(); server.held.add('session/fork');
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Session actions for Session A' })));
  await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'Fork session' })));
  const popup = screen.getByRole('dialog', { name: '/fork' });
  fireEvent.click(within(popup).getByRole('option', { name: /Fork this native boundary/ }));
  const request = await server.waitFor('session/fork', 1);
  const committed = server.commit(request);
  fireEvent.click(screen.getByRole('button', { name: 'Select Workspace Workspace B' }));
  await act(async () => server.socket.deliver(committed));
  expect(server.snapshots.has('fork-child')).toBe(true);
  expect(server.client.getSnapshot().views['fork-child']).toBeUndefined();
  expect(document.querySelector('.session-toolbar')).toBeNull();
  expect(screen.getByRole('button', { name: 'Select Workspace Workspace B' }).getAttribute('aria-current')).toBe('page');
  expect(methods()).not.toContain('session/unload'); expect(methods()).not.toContain('turn/cancel');
});

it('late native rename completion cannot replace a newer metadata query', async () => {
  await mount(); server.held.add('session/name');
  server.handlers.set('session/name', () => ({ type: 'session', session: { id: 'A', active_node: 'node-A', active_conversation_id: 'conversation-A', node_count: 1, created_at: '0', updated_at: '0' } }));
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Session actions for Session A' })));
  await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'Rename' })));
  fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'Renamed' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save name' }));
  const rename = await server.waitFor('session/name', 1);
  await act(async () => fireEvent.change(screen.getByLabelText('Search Session metadata'), { target: { value: 'new query' } }));
  const lists = methods().filter(method => method === 'session/list').length;
  await act(async () => server.reply(rename));
  expect(methods().filter(method => method === 'session/list')).toHaveLength(lists);
  expect((screen.getByLabelText('Search Session metadata') as HTMLInputElement).value).toBe('new query');
});

it('registered authorization is checked from current native settings before exactly one cold attach', async () => {
  const host = await mount();
  server.handlers.set('session/settings', () => ({ type: 'settings', revision: '0', settings: { cwd: '/workspace/A' } }));
  vi.mocked(host.classifyLocations).mockClear();
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Session A' })));
  expect(host.classifyLocations).toHaveBeenCalledWith(['/workspace/A'], endpoint);
  const sequence = methods();
  expect(sequence.indexOf('session/settings')).toBeLessThan(sequence.indexOf('session/attach'));
  expect(sequence.filter(method => method === 'session/attach')).toHaveLength(1);
  expect(server.coldLoads.get('A')).toBe(1);
  expect(screen.getByRole('button', { name: 'Select Workspace Workspace A' }).getAttribute('aria-current')).toBe('page');
  expect(screen.queryByText(/Untrusted project source/)).toBeNull();
});
it('unregister retains authorization and permits ungrouped cold open without recreating registration', async () => {
  const host = hostFixture(); await host.removeWorkspace('wA'); await mount(host);
  expect(screen.getByRole('button', { name: 'Select Workspace Ungrouped Sessions' }).closest('[class]')?.textContent).toContain('Ungrouped');
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Session A' })));
  expect(server.loaded.has('A')).toBe(true);
  expect(document.querySelector('[aria-label^="Select Workspace"][aria-current="page"]')).toBeNull();
  expect((await host.listWorkspaces()).workspaces.map(row => row.id)).toEqual(['wB']);
  expect(host.adoptWorkspace).not.toHaveBeenCalled();
});
it('stale authorized summary cannot authorize current outside cwd', async () => {
  const host = await mount();
  server.handlers.set('session/settings', () => ({ type: 'settings', revision: '0', settings: { cwd: '/outside/roots' } }));
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Session A' })));
  expect(screen.getByRole('alert').textContent).toContain('not authorized');
  expect(screen.getByRole('button', { name: 'Open Session A' })).toBeTruthy();
  expect(host.classifyLocations).toHaveBeenCalledWith(['/outside/roots'], endpoint);
  expect(methods()).not.toContain('session/attach'); expect(server.loaded.size).toBe(0);
  expect(methods()).not.toContain('settings/replace'); expect(methods()).not.toContain('session/delete');
});
it('unauthorized saved views cannot cold attach on initial restoration or reconnect', async () => {
  const host = hostFixture();
  localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint, openViews: ['A'] }));
  server.handlers.set('session/settings', () => ({ type: 'settings', revision: '0', settings: { cwd: '/outside/roots' } }));
  await act(async () => { render(<App client={server.client} workspaceHost={host} />); await server.connect(); });
  await act(async () => { server.client.disconnect(); await server.connect(); });
  expect(host.classifyLocations).toHaveBeenCalledWith(['/outside/roots'], endpoint);
  expect(methods()).not.toContain('session/attach'); expect(server.loaded.size).toBe(0);
  expect(server.client.getSnapshot().sessions.map(row => row.id)).toContain('A');
});
it('toolbar cold resume and sidebar Fork share admission and refuse an unauthorized current cwd', async () => {
  await mount();
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Session A' })));
  await act(async () => server.client.release('A'));
  server.handlers.set('session/settings', () => ({ type: 'settings', revision: '0', settings: { cwd: '/outside/roots' } }));
  const baseline = methods().filter(method => method === 'session/attach').length;
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Session' })));
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Session actions for Session A' })));
  await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'Fork session' })));
  expect(methods().filter(method => method === 'session/attach')).toHaveLength(baseline);
  expect(methods()).not.toContain('session/fork'); expect(server.loaded.has('A')).toBe(true);
});
it.each(['navigation', 'connection'] as const)('late Host authorization cannot attach after superseding %s', async supersession => {
  const host = await mount(), gate = deferred<Awaited<ReturnType<ProductHostWorkspaces['classifyLocations']>>>();
  vi.mocked(host.classifyLocations).mockReturnValueOnce(gate.promise);
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Session A' })));
  expect(methods()).not.toContain('session/attach');
  if (supersession === 'navigation') fireEvent.click(screen.getByRole('button', { name: 'Select Workspace Workspace B' }));
  else await act(async () => { await server.client.disconnect(); });
  await act(async () => gate.resolve([{ authorized: true, workspaceId: 'wA' }]));
  expect(methods()).not.toContain('session/attach'); expect(server.loaded.size).toBe(0);
  if (supersession === 'navigation') expect(screen.getByRole('button', { name: 'Select Workspace Workspace B' }).getAttribute('aria-current')).toBe('page');
});
async function invokeNew() {
  const input = screen.getByLabelText('Message');
  fireEvent.change(input, { target: { value: '/new' } });
  await act(async () => fireEvent.keyDown(input, { key: 'Enter' }));
}
it('Sidebar selection synchronizes Workspace context and /new resolves A after visiting B', async () => {
  const host = await mount();
  server.handlers.set('session/create', request => {
    if (request.method !== 'session/create') throw new Error('wrong request');
    expect(request.params.settings.cwd).toBe('/workspace/A'); server.snapshots.set('child', snapshot('child'));
    return { type: 'session_transition', session: { id: 'child', active_node: 'node-child', active_conversation_id: 'conversation-child', node_count: 1, created_at: '0', updated_at: '0' } };
  });
  server.handlers.set('session/settings', request => ({ type: 'settings', revision: '0', settings: { cwd: request.method === 'session/settings' && request.params.session_id === 'B' ? '/workspace/B' : '/workspace/A' } }));
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Session A' })));
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Session B' })));
  expect(screen.getByRole('button', { name: 'Select Workspace Workspace B' }).getAttribute('aria-current')).toBe('page');
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Session A' })));
  expect(screen.getByRole('button', { name: 'Open Session A' }).getAttribute('aria-current')).toBe('page');
  expect(screen.getByRole('button', { name: 'Select Workspace Workspace A' }).getAttribute('aria-current')).toBe('page');
  await invokeNew();
  expect(host.resolveWorkspace).toHaveBeenCalledExactlyOnceWith('wA', endpoint);
  expect(methods().filter(method => method === 'session/create')).toHaveLength(1);
});
it('focusing an authorized-unregistered Session clears old Workspace context and /new cannot reuse B', async () => {
  const host = await mount();
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Session A' })));
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Session B' })));
  await host.removeWorkspace('wA');
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Session A' })));
  expect(document.querySelector('[aria-label^="Select Workspace"][aria-current="page"]')).toBeNull();
  await invokeNew();
  expect(host.resolveWorkspace).not.toHaveBeenCalled(); expect(methods()).not.toContain('session/create');
  expect(screen.getByRole('alert').textContent).toContain('Select a Host-authorized Workspace');
});
it('browser binding accepts the same URL normalization as Host routing', async () => {
  const host = hostFixture(), catalog = await host.listWorkspaces();
  vi.mocked(host.listWorkspaces).mockResolvedValue({ ...catalog, endpoint: endpoint.slice(0, -1) });
  await mount(host);
  expect(screen.getByRole('button', { name: 'Select Workspace Workspace A' })).toBeTruthy();
  expect(host.classifyLocations).toHaveBeenCalled();
});

it('an already attached source cannot Fork a child after current cwd authorization is refused', async () => {
  const boundary = { surface_revision: '1', message: { id: 'cut', kind: 'message' as const, source: 'human' as const, content: [{ type: 'text' as const, text: 'Native Fork boundary' }] } };
  server.handlers.set('session/boundaries', () => ({ type: 'boundaries', surface_revision: '1', boundaries: [boundary] }));
  server.handlers.set('session/tree', () => ({ type: 'tree', nodes: [{ id: 'node-A', conversation_id: 'conversation-A', ordinal: '1', origin: { type: 'new' } }] }));
  await mount();
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Session actions for Session A' })));
  await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'Fork session' })));
  const popup = screen.getByRole('dialog', { name: '/fork' });
  server.handlers.set('session/settings', () => ({ type: 'settings', revision: '1', settings: { cwd: '/outside/roots' } }));
  await act(async () => fireEvent.click(within(popup).getByRole('option', { name: /Native Fork boundary/ })));
  expect(methods()).not.toContain('session/fork');
  expect(server.loaded.has('A')).toBe(true); // No hot revocation/unload.
  expect(methods()).not.toContain('session/unload');
  expect(popup.textContent).toContain('not authorized');
});

it('the shared transport attachment entry fails closed without an admission owner', async () => {
  await server.connect();
  const dispose = server.client.setAttachmentAdmission(async () => true); dispose();
  await expect(server.client.attach('A')).rejects.toThrow('No Web attachment admission owner');
  expect(methods()).not.toContain('session/attach'); expect(server.loaded.size).toBe(0);
});

it('a newer Open is not swallowed by an obsolete authorization for the same Session', async () => {
  const host = await mount(), gate = deferred<Awaited<ReturnType<ProductHostWorkspaces['classifyLocations']>>>();
  vi.mocked(host.classifyLocations).mockReturnValueOnce(gate.promise);
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Session A' })));
  fireEvent.click(screen.getByRole('button', { name: 'Select Workspace Workspace B' }));
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Session A' })));
  await act(async () => gate.resolve([{ authorized: true, workspaceId: 'wA' }]));
  expect(methods().filter(method => method === 'session/attach')).toHaveLength(1);
  expect(screen.getByRole('button', { name: 'Select Workspace Workspace A' }).getAttribute('aria-current')).toBe('page');
  expect(server.client.getSnapshot().views.A.attachment).toBe('attached');
});

it('repeated product remount, Session navigation and reconnect release every presentation subscription', async () => {
  const subscribed = new Set<() => void>();
  const original = server.client.subscribe;
  vi.spyOn(server.client, 'subscribe').mockImplementation(listener => {
    subscribed.add(listener);
    const release = original(listener);
    return () => { subscribed.delete(listener); release(); };
  });
  await server.connect();
  let baseline: number | undefined;
  for (let cycle = 0; cycle < 4; cycle++) {
    let view!: ReturnType<typeof render>;
    await act(async () => { view = render(<App client={server.client} workspaceHost={hostFixture()} />); });
    for (const name of ['Open Session A', 'Open Session B']) {
      await act(async () => fireEvent.click(screen.getByRole('button', { name })));
    }
    baseline ??= subscribed.size;
    expect(baseline).toBeGreaterThan(0);
    expect(subscribed.size).toBe(baseline);
    await act(async () => { server.client.disconnect(); await server.connect(); });
    expect(subscribed.size).toBe(baseline);
    await act(async () => view.unmount());
    expect(subscribed.size).toBe(0);
    // UI cleanup must not become a runtime shutdown owner.
    expect(server.loaded.has('A')).toBe(true);
    expect(server.loaded.has('B')).toBe(true);
  }
  expect(methods()).not.toContain('session/unload');
  expect(methods()).not.toContain('turn/cancel');
});

it('classification belongs to exactly the native summary page that requested it', async () => {
  const host = hostFixture();
  const pending = deferred<Awaited<ReturnType<ProductHostWorkspaces['classifyLocations']>>>();
  await mount(host);
  host.classifyLocations = vi.fn(() => pending.promise);
  server.handlers.set('session/list', () => ({ type: 'sessions', sessions: [{ id: 'C', name: 'Fresh Session', cwd: '/workspace/B', active_node: 'c', updated_at: '2026-09-18T00:00:00Z' }], }));
  await act(async () => { await server.client.listSessions(); });
  const groupContaining = () => screen.getByRole('button', { name: 'Open Fresh Session' }).closest('[data-workspace-group]')!.textContent;
  expect(groupContaining()).toContain('Ungrouped Sessions');
  expect(groupContaining()).not.toContain('Workspace A');
  await act(async () => pending.resolve([{ authorized: true, workspaceId: 'wB' }]));
  expect(groupContaining()).toContain('Workspace B');
  expect(JSON.stringify(localStorage)).not.toContain('workspaceId');
  expect(methods()).toEqual(['initialize', 'session/list', 'session/list']);
});

// Blocking finding 2 — the whole real path: SessionConfiguration → App owner
// navigation → the concrete Settings target. Nothing here mocks the callback or
// inspects a fabricated `source:*` string.
async function failedSessionConfiguration(sources: readonly import('../../protocol/app-server/v18').SourceTarget[], host = hostFixture()) {
  server.handlers.set('session/settings', () => ({ type: 'settings', revision: '0', settings: { cwd: '/workspace/A' } }));
  server.handlers.set('session/configuration', () => ({
    type: 'session_configuration',
    // A real Session application: `scope` is the Session identity the App
    // Server read it under, and the authored owners are a separate fact.
    application: { scope: 'A', sources: [...sources], version: '2', desired: { input_revision: 'input-1', attempt: '1' },
      units: { capabilities: { status: 'failed', diagnostic: 'resource failed' } }, candidate: null, eligibility: { status: 'unavailable' } },
  }));
  await mount(host);
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Select Workspace Workspace A' })));
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Session A' })));
  await screen.findByText(/Some configuration preparation failed/);
  return host;
}

it('S1-10 a Workspace-owned Session failure opens the exact Workspace Settings target', async () => {
  await failedSessionConfiguration([{ kind: 'user' }, { kind: 'workspace', directory: '/workspace/A' }]);
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Workspace Settings — /workspace/A' })));
  expect(screen.getByRole('heading', { name: 'Workspace Settings — Workspace A' })).toBeTruthy();
  // The concrete target is the registered Workspace, read through its own scope.
  await waitFor(() => expect(server.requests.some(row => row.request.method === 'session/create')).toBe(false));
  expect(screen.queryByLabelText('Configuration owner')).toBeNull();
});

it('S1-10 a User-owned Session failure opens User Settings', async () => {
  await failedSessionConfiguration([{ kind: 'user' }]);
  expect(screen.queryByRole('button', { name: /Open Workspace Settings/ })).toBeNull();
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open User Settings' })));
  expect(screen.getByRole('heading', { name: 'User Settings' })).toBeTruthy();
});

it('S1-10 an unregistered owning Workspace reports an explicit error and never falls back to User Settings', async () => {
  const host = hostFixture();
  await failedSessionConfiguration([{ kind: 'user' }, { kind: 'workspace', directory: '/workspace/revoked' }], host);
  const before = methods().length;
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Workspace Settings — /workspace/revoked' })));
  expect(screen.getByRole('alert').textContent).toContain('/workspace/revoked is not registered by this Product Host');
  // No Settings instance is opened at all, least of all User authoring.
  expect(screen.queryByRole('heading', { name: 'User Settings' })).toBeNull();
  expect(screen.queryByRole('heading', { name: /^Workspace Settings/ })).toBeNull();
  // No hidden Workspace, Session or runtime is allocated to resolve it.
  expect(host.adoptWorkspace).not.toHaveBeenCalled();
  expect(methods().slice(before)).toEqual([]);
});

it('S1-10 Session focus changes never retarget an opened owning Settings editor', async () => {
  await failedSessionConfiguration([{ kind: 'user' }, { kind: 'workspace', directory: '/workspace/A' }]);
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Workspace Settings — /workspace/A' })));
  expect(screen.getByRole('heading', { name: 'Workspace Settings — Workspace A' })).toBeTruthy();
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Select Workspace Workspace B' })));
  expect(screen.getByRole('heading', { name: 'Workspace Settings — Workspace A' })).toBeTruthy();
});

// Settings navigation is linearized by one App-owned epoch. A delayed owning
// Workspace catalog lookup is preparation, never authority to override a newer
// navigation decision.
async function pendingOwnershipLookup(sources: readonly import('../../protocol/app-server/v18').SourceTarget[]) {
  const host = await failedSessionConfiguration(sources);
  const catalog = await host.listWorkspaces();
  const gate = deferred<WorkspaceCatalog>();
  host.listWorkspaces = vi.fn(() => gate.promise);
  fireEvent.click(screen.getByRole('button', { name: 'Open Workspace Settings — /workspace/A' }));
  return { gate, catalog };
}

it('S1-10 a newer User Settings decision rejects a late owning Workspace lookup', async () => {
  const { gate, catalog } = await pendingOwnershipLookup([{ kind: 'user' }, { kind: 'workspace', directory: '/workspace/A' }]);
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Settings' })));
  await screen.findByRole('heading', { name: 'User Settings' });
  await act(async () => { gate.resolve(catalog); });
  expect(screen.getByRole('heading', { name: 'User Settings' })).toBeTruthy();
  expect(screen.queryByRole('heading', { name: /^Workspace Settings/ })).toBeNull();
});

it('S1-10 a newer Workspace B Settings decision rejects a late owning Workspace A lookup', async () => {
  const { gate, catalog } = await pendingOwnershipLookup([{ kind: 'user' }, { kind: 'workspace', directory: '/workspace/A' }]);
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Workspace actions for Workspace B' })));
  await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'Workspace settings' })));
  await screen.findByRole('heading', { name: 'Workspace Settings — Workspace B' });
  await act(async () => { gate.resolve(catalog); });
  expect(screen.getByRole('heading', { name: 'Workspace Settings — Workspace B' })).toBeTruthy();
  expect(screen.queryByRole('heading', { name: 'Workspace Settings — Workspace A' })).toBeNull();
});

it('S1-10 closing Settings rejects a late owning Workspace lookup', async () => {
  const { gate, catalog } = await pendingOwnershipLookup([{ kind: 'user' }, { kind: 'workspace', directory: '/workspace/A' }]);
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Settings' })));
  await screen.findByRole('heading', { name: 'User Settings' });
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Close Settings' })));
  await act(async () => { gate.resolve(catalog); });
  expect(screen.queryByRole('dialog', { name: 'Settings' })).toBeNull();
});

it('S1-10 an authority replacement rejects a late owning Workspace lookup', async () => {
  const { gate, catalog } = await pendingOwnershipLookup([{ kind: 'user' }, { kind: 'workspace', directory: '/workspace/A' }]);
  const before = server.client.getSnapshot();
  vi.spyOn(server.client, 'getSnapshot').mockReturnValue({ ...before, authorityRevision: (before.authorityRevision ?? 0) + 1 });
  await act(async () => { gate.resolve(catalog); });
  expect(screen.queryByRole('dialog', { name: 'Settings' })).toBeNull();
});

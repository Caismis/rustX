import { localeController } from '../src/locale/controller';
import { translator } from '../src/locale/translation';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { App } from '../src/app/App';
import { NavigationEpoch } from '../src/client/navigation';
import { firstSubmitPort } from '../src/app/new-conversation/port';
import { sessionObservation, WorkspaceNavigation } from '../src/workspaces/WorkspaceNavigation';
import { WorkspaceAuthority } from '../src/workspaces/authority';
import { WorkspaceAssociations } from '../src/workspaces/associations';
import { selectShell } from '../src/client/selectors';
import { WorkspaceHostError, type ProductHostWorkspaces, type WorkspaceCatalog, type WorkspaceAuthorityScope } from '../src/workspaces/host';
import { configurationSystem } from '../src/app/settings/machines/system';
import { Server, endpoint, snapshot } from './fixture';
let server: Server;
beforeEach(() => {
  server = new Server(); localStorage.clear();
});
afterEach(() => { cleanup(); server.client.disconnect(); localeController.setLocale('en'); });
function deferred<T>() {
  let resolve!: (value: T) => void, reject!: (cause: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
function hostFixture(picker = true) {
  let rows = ['A', 'B'].map(id => ({ id: `w${id}`, displayName: `Workspace ${id}`, location: `root-${id}`, displayPath: `/workspace/${id}` }));
  const host: ProductHostWorkspaces = {
    listWorkspaces: vi.fn(async (): Promise<WorkspaceCatalog> => ({ authorityId: 'fixture-host', endpoint, workspaces: rows, picker: picker ? { kind: 'configured', locations: [{ id: 'root-C', displayName: 'Workspace C' }] } : { kind: 'unavailable', reason: 'Directory picker unavailable' } })),
    classifyLocations: vi.fn(async (cwds: readonly string[]) => cwds.map(cwd => ['/workspace/A', '/workspace/B'].includes(cwd) ? { authorized: true as const, workspaceId: rows.find(row => row.displayPath === cwd)?.id } : { authorized: false as const, reason: 'denied' as const })),
    resolveWorkspace: vi.fn(async id => { const row = rows.find(row => row.id === id); if (!row) throw new Error('Unauthorized'); return { cwd: row.displayPath }; }),
    renameWorkspace: vi.fn(async (_scope, id, displayName) => { rows = rows.map(row => row.id === id ? { ...row, displayName } : row); }),
    reorderWorkspace: vi.fn(async () => {}), removeWorkspace: vi.fn(async (_scope, id) => { rows = rows.filter(row => row.id !== id); }),
    adoptWorkspace: vi.fn(async () => {}),
  };
  return host;
}
function holdAdmission(host: ProductHostWorkspaces, promise: ReturnType<ProductHostWorkspaces['classifyLocations']>) {
  const classify = host.classifyLocations;
  let pending = true;
  host.classifyLocations = vi.fn((cwds, route, authority, signal) => {
    if (signal === undefined && pending) { pending = false; return promise; }
    return classify(cwds, route, authority, signal);
  });
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
  expect(host.resolveWorkspace).toHaveBeenCalledWith('wA', endpoint); expect(methods()).not.toContain('session/create'); expect(server.loaded.size).toBe(0);
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Session A' })));
  expect(server.coldLoads.get('A')).toBe(1);
  // Session focus changes beneath the Settings modal, which hides the page it
  // covers from the accessibility tree.
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Select Workspace Workspace B', hidden: true })));
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
  expect(host.renameWorkspace).toHaveBeenCalledWith({ authorityId: 'fixture-host', endpoint }, 'wA', 'Renamed A');
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Workspace actions for Renamed A' })));
  await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'Unregister Workspace' })));
  await act(async () => fireEvent.click(screen.getByRole('button', { name: /^Unregister$/ })));
  expect(host.removeWorkspace).toHaveBeenCalledWith({ authorityId: 'fixture-host', endpoint }, 'wA'); expect(server.snapshots.has('A')).toBe(true);
  expect(server.loaded.has('A')).toBe(true); expect(methods()).not.toContain('settings/replace'); expect(methods()).not.toContain('session/delete');
});
it.each(['same scope', 'replacement Host'] as const)('UX-04 unregister completion commits only to its initiating scope: %s', async kind => {
  await server.connect();
  const host = hostFixture();
  let currentCatalog = await host.listWorkspaces();
  vi.mocked(host.listWorkspaces).mockImplementation(async () => currentCatalog);
  const authority = new WorkspaceAuthority(host), owner = new WorkspaceAssociations(server.client, authority);
  let stop!: () => void;
  const metadataChanged = vi.fn();
  await act(async () => {
    stop = owner.start();
    render(<WorkspaceNavigation associations={owner} host={host} client={server.client} state={selectShell(server.client.getSnapshot())}
      endpoint={endpoint} navigation={server.client.navigation} wide expand={vi.fn()} metadataChanged={metadataChanged}
      selectWorkspace={vi.fn()} openSession={vi.fn()} openViews={[]} closeView={vi.fn()} closeAllViews={vi.fn()}
      createSession={vi.fn()} forkSession={vi.fn()} deleteSession={vi.fn()} />);
  });
  try {
    expect(owner.getSnapshot().entries.get('A')).toMatchObject({ status: 'ready', confirmed: { workspaceId: 'wA' } });
    const completion = deferred<void>(), started = deferred<void>();
    vi.mocked(host.removeWorkspace).mockImplementation(() => { started.resolve(); return completion.promise; });
    await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Workspace actions for Workspace A' })));
    await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'Unregister Workspace' })));
    await act(async () => fireEvent.click(screen.getByRole('button', { name: /^Unregister$/ })));
    await started.promise;
    expect(host.removeWorkspace).toHaveBeenCalledWith({ authorityId: 'fixture-host', endpoint }, 'wA');
    if (kind === 'replacement Host') {
      currentCatalog = { ...currentCatalog, authorityId: 'host-B' };
      await act(async () => { await authority.observe(); });
      // B legitimately owns the same Workspace ID, fully classified before A completes.
      expect(authority.getCatalog()?.authorityId).toBe('host-B');
      expect(owner.getSnapshot()).toMatchObject({ status: 'ready', catalog: { authorityId: 'host-B' } });
      expect(owner.getSnapshot().catalog?.workspaces.map(row => row.id)).toEqual(['wA', 'wB']);
      expect(owner.getSnapshot().entries.get('A')).toMatchObject({ status: 'ready', confirmed: { workspaceId: 'wA' } });
    }
    const before = owner.getSnapshot(), reads = vi.mocked(host.listWorkspaces).mock.calls.length;
    // Hold any accidental corrective reread: no self-healing can mask deletion.
    const reread = deferred<WorkspaceCatalog>();
    vi.mocked(host.listWorkspaces).mockReturnValue(reread.promise);
    await act(async () => completion.resolve());
    if (kind === 'replacement Host') {
      expect(authority.getCatalog()?.authorityId).toBe('host-B');
      expect(owner.getSnapshot()).toBe(before);
      expect(owner.getSnapshot().catalog?.workspaces.map(row => row.id)).toEqual(['wA', 'wB']);
      expect(owner.getSnapshot().entries.get('A')).toMatchObject({ status: 'ready', confirmed: { workspaceId: 'wA' } });
      expect(host.listWorkspaces).toHaveBeenCalledTimes(reads);
      expect(metadataChanged).not.toHaveBeenCalled();
    } else {
      expect(owner.getSnapshot().catalog?.workspaces.map(row => row.id)).toEqual(['wB']);
      expect(owner.getSnapshot().entries.get('A')?.confirmed).toEqual({});
      expect(host.listWorkspaces).toHaveBeenCalledTimes(reads + 1);
      expect(metadataChanged).toHaveBeenCalledWith('wA');
      // Failure is independent of the committed removal; it cannot restore wA.
      await act(async () => reread.reject(new Error('reread failed')));
      expect(owner.getSnapshot().status).toBe('unavailable');
      expect(owner.getSnapshot().catalog?.workspaces.map(row => row.id)).toEqual(['wB']);
      expect(owner.getSnapshot().entries.get('A')?.confirmed).toEqual({});
    }
    expect(methods()).toEqual(['initialize', 'session/list']);
  } finally { stop(); }
});
it.each(['success', 'failure'] as const)('UX-04 pending unregister survives App native replacement and late catalog %s', async outcome => {
  const capture = vi.spyOn(WorkspaceAssociations.prototype, 'captureMutation');
  const host = await mount();
  const completion = deferred<void>(), started = deferred<void>();
  vi.mocked(host.removeWorkspace).mockImplementation(() => { started.resolve(); return completion.promise; });
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Workspace actions for Workspace A' })));
  await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'Unregister Workspace' })));
  await act(async () => fireEvent.click(screen.getByRole('button', { name: /^Unregister$/ })));
  await started.promise;
  const owner = capture.mock.contexts[0] as WorkspaceAssociations;
  const baseline = owner.getSnapshot().catalog!, r1 = deferred<WorkspaceCatalog>(), r2 = deferred<WorkspaceCatalog>();
  expect(owner.getSnapshot().entries.get('A')?.confirmed).toEqual({ workspaceId: 'wA' });
  const displayReads: AbortSignal[] = [];
  let currentCatalog = baseline;
  vi.mocked(host.listWorkspaces).mockImplementation(signal => {
    // Composer reads are independent; gate the display owner's exact read lane.
    if (!signal) return Promise.resolve(currentCatalog);
    displayReads.push(signal);
    return displayReads.length === 1 ? r1.promise : r2.promise;
  });
  server.authorityId = 'native-2';
  await act(async () => { await server.client.disconnect(); await server.connect(); });
  expect(server.client.getSnapshot().authorityId).toBe('native-2');
  expect(owner.getSnapshot().catalog).toBeUndefined();
  expect(displayReads).toHaveLength(1); expect(displayReads[0].aborted).toBe(false);
  const lists = vi.mocked(host.listWorkspaces).mock.calls.length;
  // Registration is removed; exact-root authorization remains independent.
  vi.mocked(host.classifyLocations).mockImplementation(async cwds => cwds.map(cwd => ({ authorized: true, workspaceId: cwd === '/workspace/B' ? 'wB' : undefined })));
  currentCatalog = { ...baseline, workspaces: baseline.workspaces.filter(row => row.id !== 'wA') };
  await act(async () => completion.resolve());
  expect(displayReads).toHaveLength(2); expect(displayReads[0].aborted).toBe(true);
  expect(host.listWorkspaces).toHaveBeenCalledTimes(lists + 1);
  expect(capture).toHaveBeenCalledTimes(1); // The old UI continuation commits through the surviving owner.
  await act(async () => r2.resolve(currentCatalog));
  expect(owner.getSnapshot().status).toBe('ready');
  expect(owner.getSnapshot().catalog?.workspaces.map(row => row.id)).toEqual(['wB']);
  expect(owner.getSnapshot().entries.get('A')?.confirmed).toEqual({});
  const settled = owner.getSnapshot();
  await act(async () => {
    if (outcome === 'success') r1.resolve(baseline); else r1.reject(new Error('late pre-commit failure'));
  });
  expect(owner.getSnapshot()).toBe(settled);
  expect(screen.queryByRole('button', { name: 'Select Workspace Workspace A' })).toBeNull();
  expect(host.listWorkspaces).toHaveBeenCalledTimes(lists + 1);
});
it.each(['add', 'rename', 'remove'] as const)('Sidebar %s preserves the opened intent scope after Host replacement', async kind => {
  await server.connect(); const host = hostFixture();
  let catalog = await host.listWorkspaces();
  const scope = { authorityId: catalog.authorityId, endpoint }, written = vi.fn();
  vi.mocked(host.listWorkspaces).mockImplementation(async () => catalog);
  const write = async (expected: WorkspaceAuthorityScope) => {
    if (expected.authorityId !== catalog.authorityId) throw new WorkspaceHostError('Workspace Host authority replaced', 'authority_replaced');
    written();
  };
  vi.mocked(host.adoptWorkspace).mockImplementation(write);
  vi.mocked(host.renameWorkspace).mockImplementation(write);
  vi.mocked(host.removeWorkspace).mockImplementation(write);
  const authority = new WorkspaceAuthority(host), owner = new WorkspaceAssociations(server.client, authority), metadataChanged = vi.fn();
  const capture = vi.spyOn(WorkspaceAssociations.prototype, 'captureMutation');
  let stop!: () => void;
  await act(async () => {
    stop = owner.start();
    render(<WorkspaceNavigation associations={owner} host={host} client={server.client} state={selectShell(server.client.getSnapshot())}
      endpoint={endpoint} navigation={server.client.navigation} wide expand={vi.fn()} metadataChanged={metadataChanged}
      selectWorkspace={vi.fn()} openSession={vi.fn()} openViews={[]} closeView={vi.fn()} closeAllViews={vi.fn()}
      createSession={vi.fn()} forkSession={vi.fn()} deleteSession={vi.fn()} />);
  });
  try {
    if (kind === 'add') await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Add Workspace' })));
    else {
      await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Workspace actions for Workspace A' })));
      await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: kind === 'rename' ? 'Rename' : 'Unregister Workspace' })));
      if (kind === 'rename') fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'renamed' } });
    }
    // The Sidebar captures its intent against the exact catalog that rendered the action.
    expect(capture).toHaveBeenCalledExactlyOnceWith(catalog);
    catalog = { ...catalog, authorityId: 'host-B' };
    await act(async () => { await authority.observe(); });
    const before = owner.getSnapshot(), reads = vi.mocked(host.listWorkspaces).mock.calls.length;
    expect(before.catalog?.authorityId).toBe('host-B');
    await act(async () => fireEvent.click(screen.getByRole('button', { name: kind === 'add' ? 'Workspace C' : kind === 'rename' ? 'Save name' : /^Unregister$/ })));
    if (kind === 'add') expect(host.adoptWorkspace).toHaveBeenCalledExactlyOnceWith(scope, 'root-C');
    else if (kind === 'rename') expect(host.renameWorkspace).toHaveBeenCalledExactlyOnceWith(scope, 'wA', 'renamed');
    else expect(host.removeWorkspace).toHaveBeenCalledExactlyOnceWith(scope, 'wA');
    expect(written).not.toHaveBeenCalled(); expect(metadataChanged).not.toHaveBeenCalled();
    expect(owner.getSnapshot()).toBe(before); expect(host.listWorkspaces).toHaveBeenCalledTimes(reads);
    expect(screen.getByRole('alert').textContent).toContain('authority replaced');
  } finally { stop(); }
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
  expect(host.adoptWorkspace).toHaveBeenCalledWith({ authorityId: 'fixture-host', endpoint }, 'root-C');
  const classifications = vi.mocked(host.classifyLocations).mock.calls.length;
  const catalogs = vi.mocked(host.listWorkspaces).mock.calls.length;
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Session actions for Session A' })));
  await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'Rename' })));
  fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'Native name' } });
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Save name' })));
  expect(server.requests.find(row => row.request.method === 'session/name')?.request.params).toEqual({ session_id: 'A', name: 'Native name' });
  expect(host.renameWorkspace).not.toHaveBeenCalled();
  expect(host.classifyLocations).toHaveBeenCalledTimes(classifications);
  expect(host.listWorkspaces).toHaveBeenCalledTimes(catalogs);
});
it('Host resolution supplies exact cwd, rejects unauthorized identifiers, and a late resolution cannot create or reopen', async () => {
  await server.connect(); const host = hostFixture(), navigation = new NavigationEpoch();
  const gate = deferred<{ cwd: string }>(); vi.mocked(host.resolveWorkspace).mockReturnValueOnce(gate.promise);
  const pending = firstSubmitPort(server.client, host, navigation.capture(), () => {}).create({ workspaceId: 'wA', text: 'hello', files: [] }, () => {});
  navigation.invalidate(); const rejected = expect(pending).rejects.toThrow('authority changed'); gate.resolve({ cwd: '/workspace/A' }); await rejected;
  expect(methods()).not.toContain('session/create');
  await expect(firstSubmitPort(server.client, host, navigation.capture(), () => {}).create({ workspaceId: '/arbitrary/path', text: 'hello', files: [] }, () => {})).rejects.toThrow('Unauthorized');
  server.handlers.set('session/create', request => {
    if (request.method !== 'session/create') throw new Error('wrong request');
    expect(request.params.settings.cwd).toBe('/workspace/A');
    server.snapshots.set('created', snapshot('created'));
    return { type: 'session_transition', session: { id: 'created', active_node: 'node-created', active_conversation_id: 'conversation-created', node_count: 1, created_at: '0', updated_at: '0' } };
  });
  expect((await firstSubmitPort(server.client, host, navigation.capture(), () => {}).create({ workspaceId: 'wA', text: 'hello', files: [] }, () => {})).id).toBe('created');
});
it('late metadata searches cannot replace newer results or results after Workspace navigation', async () => {
  await mount(); server.held.add('session/list');
  server.handlers.set('session/list', request => {
    if (request.method !== 'session/list') throw new Error('wrong request');
    const id = request.params.query!;
    return { type: 'sessions', sessions: [{ ownership_generation: '1', id, cwd: '/workspace/A', active_node: `node-${id}`, name: id, updated_at: '0' }] };
  });
  fireEvent.change(screen.getByLabelText('Search Session metadata'), { target: { value: 'old' } });
  const old = await server.waitFor('session/list', 2);
  fireEvent.change(screen.getByLabelText('Search Session metadata'), { target: { value: 'new' } });
  const fresh = await server.waitFor('session/list', 3);
  await act(async () => server.reply(fresh)); await act(async () => server.reply(old));
  expect(server.client.getSnapshot().sessions[0].id).toBe('new');
  fireEvent.change(screen.getByLabelText('Search Session metadata'), { target: { value: 'obsolete' } });
  const obsolete = await server.waitFor('session/list', 4);
  await act(async () => fireEvent.click(screen.getAllByRole('button', { name: 'New Conversation' })[0]));
  fireEvent.click(screen.getByRole('button', { name: 'Choose Workspace' }));
  fireEvent.click(screen.getByRole('menuitem', { name: 'Workspace B' }));
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
  expect(sessionObservation(translator('en'), { ...server.client.getSnapshot(), views: { A: view }, connection: 'connected' }, 'A')).toBe('Connection interrupted');
  expect(sessionObservation(translator('en'), { ...server.client.getSnapshot(), views: { A: view }, connection: 'disconnected' }, 'A')).toBe('Connection interrupted');
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
  expect(host.classifyLocations).toHaveBeenCalledWith(['/workspace/A'], endpoint, 'fixture-host', undefined);
  const sequence = methods();
  expect(sequence.indexOf('session/settings')).toBeLessThan(sequence.indexOf('session/attach'));
  expect(sequence.filter(method => method === 'session/attach')).toHaveLength(1);
  expect(server.coldLoads.get('A')).toBe(1);
  expect(screen.getByRole('button', { name: 'Select Workspace Workspace A' }).getAttribute('aria-current')).toBe('page');
  expect(screen.queryByText(/Untrusted project source/)).toBeNull();
});
it('unregister retains authorization and permits ungrouped cold open without recreating registration', async () => {
  const host = hostFixture(); await host.removeWorkspace(await host.listWorkspaces(), 'wA'); await mount(host);
  expect(screen.queryByRole('button', { name: 'Select Workspace Ungrouped Sessions' })).toBeNull();
  fireEvent.click(screen.getByText('Sessions outside registered Workspaces'));
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
  expect(screen.getAllByRole('alert').some(alert => alert.textContent?.includes('not authorized'))).toBe(true);
  expect(screen.getByRole('button', { name: 'Open Session A' })).toBeTruthy();
  expect(host.classifyLocations).toHaveBeenCalledWith(['/outside/roots'], endpoint, 'fixture-host', undefined);
  expect(methods()).not.toContain('session/attach'); expect(server.loaded.size).toBe(0);
  expect(methods()).not.toContain('settings/replace'); expect(methods()).not.toContain('session/delete');
});
it('unauthorized saved views cannot cold attach on initial restoration or reconnect', async () => {
  const host = hostFixture();
  localStorage.setItem('rustx-console-view-v2', JSON.stringify({ authorityId: 'fixture-host', endpoint, openViews: ['A'] }));
  server.handlers.set('session/settings', () => ({ type: 'settings', revision: '0', settings: { cwd: '/outside/roots' } }));
  await act(async () => { render(<App client={server.client} workspaceHost={host} />); await server.connect(); });
  await act(async () => { server.client.disconnect(); await server.connect(); });
  expect(host.classifyLocations).toHaveBeenCalledWith(['/outside/roots'], endpoint, 'fixture-host', undefined);
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
  holdAdmission(host, gate.promise);
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
it('Sidebar selection synchronizes Workspace context and /new drafts in A after visiting B without creation', async () => {
  await mount();
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
  expect(screen.getByRole('button', { name: 'Choose Workspace' }).textContent).toContain('Workspace A');
  expect(methods()).not.toContain('session/create');
});
it('focusing an authorized-unregistered Session clears old Workspace context and /new cannot reuse B', async () => {
  const host = await mount();
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Session A' })));
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Session B' })));
  await host.removeWorkspace(await host.listWorkspaces(), 'wA');
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'View options' })));
  await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'Refresh list' })));
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Session A' })));
  expect(document.querySelector('[aria-label^="Select Workspace"][aria-current="page"]')).toBeNull();
  vi.mocked(host.resolveWorkspace).mockClear();
  await invokeNew();
  expect(host.resolveWorkspace).not.toHaveBeenCalled(); expect(methods()).not.toContain('session/create');
  expect(screen.getByRole('button', { name: 'Choose Workspace' }).textContent).toContain('Choose Workspace');
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
  const dispose = server.client.setAttachmentAdmission(async () => ({ current: () => true, validate: async () => true })); dispose();
  await expect(server.client.attach('A')).rejects.toThrow('No Web attachment admission owner');
  expect(methods()).not.toContain('session/attach'); expect(server.loaded.size).toBe(0);
});

it('a newer Open is not swallowed by an obsolete authorization for the same Session', async () => {
  const host = await mount(), gate = deferred<Awaited<ReturnType<ProductHostWorkspaces['classifyLocations']>>>();
  holdAdmission(host, gate.promise);
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Session A' })));
  fireEvent.click(screen.getByRole('button', { name: 'Select Workspace Workspace B' }));
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Session A' })));
  await act(async () => gate.resolve([{ authorized: true, workspaceId: 'wA' }]));
  expect(methods().filter(method => method === 'session/attach')).toHaveLength(1);
  expect(screen.getByRole('button', { name: 'Select Workspace Workspace A' }).getAttribute('aria-current')).toBe('page');
  expect(server.client.getSnapshot().views.A.attachment).toBe('attached');
});

it('repeated product remount, Session navigation and reconnect release every presentation subscription', async () => {
  // The configuration actor system observes its client's authority for the
  // client's whole lifetime. That is not a presentation subscription, so it is
  // established before the ones this test counts.
  configurationSystem(server.client);
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
  server.handlers.set('session/list', () => ({ type: 'sessions', sessions: [{ ownership_generation: '1', id: 'C', name: 'Fresh Session', cwd: '/workspace/B', active_node: 'c', updated_at: '2026-09-18T00:00:00Z' }], }));
  await act(async () => { await server.client.listSessions(); });
  const groupContaining = () => screen.getByRole('button', { name: 'Open Fresh Session' }).closest('[data-workspace-group]')?.textContent ?? '';
  expect(groupContaining()).toBe('');
  expect(groupContaining()).not.toContain('Workspace A');
  await act(async () => pending.resolve([{ authorized: true, workspaceId: 'wB' }]));
  expect(groupContaining()).toContain('Workspace B');
  expect(JSON.stringify(localStorage)).not.toContain('workspaceId');
  expect(methods()).toEqual(['initialize', 'session/list', 'session/list']);
});

// Blocking finding 2 — the whole real path: SessionConfiguration → App owner
// navigation → the concrete Settings target. Nothing here mocks the callback or
// inspects a fabricated `source:*` string.
async function failedSessionConfiguration(sources: readonly import('../../protocol/app-server/v38').SourceTarget[], host = hostFixture()) {
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
  expect(screen.getAllByRole('alert').some(alert => alert.textContent?.includes('/workspace/revoked is not registered by this Product Host'))).toBe(true);
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
  // Session focus changes beneath the Settings modal, which hides the page it
  // covers from the accessibility tree.
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Select Workspace Workspace B', hidden: true })));
  expect(screen.getByRole('heading', { name: 'Workspace Settings — Workspace A' })).toBeTruthy();
});

// Settings navigation is linearized by one App-owned epoch. A delayed owning
// Workspace catalog lookup is preparation, never authority to override a newer
// navigation decision.
async function pendingOwnershipLookup(sources: readonly import('../../protocol/app-server/v38').SourceTarget[]) {
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

it('UX-04 keeps confirmed groups while a cloned native page refresh is gated', async () => {
  const host = await mount();
  const group = () => screen.getByRole('button', { name: 'Open Session A' }).closest('[data-workspace-group]')?.getAttribute('data-workspace-group');
  expect(group()).toBe('wA');
  const gate = deferred<Awaited<ReturnType<ProductHostWorkspaces['classifyLocations']>>>();
  vi.mocked(host.classifyLocations).mockReturnValueOnce(gate.promise);
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'View options' })));
  await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'Refresh list' })));
  expect(group()).toBe('wA');
  await act(async () => gate.resolve([{ authorized: true, workspaceId: 'wA' }, { authorized: true, workspaceId: 'wB' }]));
});
it.each(['success', 'failure'] as const)('UX-04 normal next-page demand settles before the old page %s', async outcome => {
  server.handlers.set('session/list', request => {
    if (request.method !== 'session/list') throw new Error('wrong request');
    return { type: 'sessions', sessions: [server.summary(request.params.offset === 0 ? 'A' : 'B')], next_offset: request.params.offset === 0 ? 32 : undefined };
  });
  const host = hostFixture(), a = deferred<Awaited<ReturnType<ProductHostWorkspaces['classifyLocations']>>>(), b = deferred<Awaited<ReturnType<ProductHostWorkspaces['classifyLocations']>>>();
  vi.mocked(host.classifyLocations).mockReturnValueOnce(a.promise).mockReturnValueOnce(b.promise);
  await mount(host);
  expect(host.classifyLocations).toHaveBeenCalledTimes(1);
  expect(vi.mocked(host.classifyLocations).mock.calls[0][0]).toEqual(['/workspace/A']);
  const lists = vi.mocked(host.listWorkspaces).mock.calls.length;
  await act(async () => fireEvent.click(screen.getByRole('button', { name: /^Next$/ })));
  expect(server.client.getSnapshot().sessions.map(row => row.id)).toEqual(['B']);
  expect(host.classifyLocations).toHaveBeenCalledTimes(2);
  expect(vi.mocked(host.classifyLocations).mock.calls[1][0]).toEqual(['/workspace/B']);
  expect(vi.mocked(host.classifyLocations).mock.calls[0][3]?.aborted).toBe(false);
  await act(async () => b.resolve([{ authorized: true, workspaceId: 'wB' }]));
  const group = () => screen.getByRole('button', { name: 'Open Session B' }).closest('[data-workspace-group]')?.getAttribute('data-workspace-group');
  expect(group()).toBe('wB');
  expect(screen.queryByText(translator('en')('workspace:association.refreshing'))).toBeNull();
  // A remains unresolved until B's group and aggregate ready notice are proven.
  await act(async () => {
    if (outcome === 'success') a.resolve([{ authorized: true, workspaceId: 'wA' }]);
    else a.reject(new Error('old page failed'));
  });
  expect(group()).toBe('wB');
  expect(screen.queryByText(translator('en')('workspace:association.refreshing'))).toBeNull();
  expect(screen.queryByText(translator('en')('workspace:association.unavailable'))).toBeNull();
  expect(host.classifyLocations).toHaveBeenCalledTimes(2);
  expect(host.listWorkspaces).toHaveBeenCalledTimes(lists);
});

it('UX-04 shares selected membership across a gated reconnect without authorizing attachment', async () => {
  const host = await mount();
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Session A' })));
  const selected = () => screen.getByRole('button', { name: 'Select Workspace Workspace A' }).getAttribute('aria-current');
  expect(selected()).toBe('page');
  const gate = deferred<Awaited<ReturnType<ProductHostWorkspaces['classifyLocations']>>>();
  host.classifyLocations = vi.fn((_cwds, _endpoint, _authority, signal) => signal ? gate.promise : Promise.resolve([{ authorized: false as const, reason: 'denied' as const }]));
  await act(async () => { await server.client.disconnect(); });
  expect(selected()).toBe('page');
  expect(screen.getByRole('button', { name: 'Open Session A' }).closest('[data-workspace-group]')?.getAttribute('data-workspace-group')).toBe('wA');
  const before = methods().filter(method => method === 'session/attach').length;
  await act(async () => { await server.connect(); });
  expect(selected()).toBe('page');
  expect(methods().filter(method => method === 'session/attach')).toHaveLength(before);
  expect(server.client.getSnapshot().views.A.attachment).not.toBe('attached');
  await act(async () => gate.resolve([{ authorized: true, workspaceId: 'wA' }, { authorized: true, workspaceId: 'wB' }]));
  expect(selected()).toBe('page');
});

it('UX-04 native authority replacement at the same endpoint retires presentation and old callbacks', async () => {
  const host = await mount();
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Session A' })));
  const old = deferred<Awaited<ReturnType<ProductHostWorkspaces['classifyLocations']>>>();
  vi.mocked(host.classifyLocations).mockReturnValueOnce(old.promise);
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'View options' })));
  await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'Refresh list' })));
  const revision = server.client.getSnapshot().authorityRevision ?? 0;
  server.authorityId = 'replacement-native';
  await act(async () => { await server.client.disconnect(); await server.connect(); });
  expect(server.client.getSnapshot().authorityRevision).toBe(revision + 1);
  expect(document.querySelector('[aria-label^="Select Workspace"][aria-current="page"]')).toBeNull();
  expect(server.client.getSnapshot().views).toEqual({});
  await act(async () => old.resolve([{ authorized: true, workspaceId: 'wB' }, { authorized: true, workspaceId: 'wA' }]));
  expect(screen.getByRole('button', { name: 'Open Session A' }).closest('[data-workspace-group]')?.getAttribute('data-workspace-group')).toBe('wA');
});

it('UX-04 replacement of the Product Host admission owner fences an older successful admission', async () => {
  await server.connect();
  const old = deferred<false | import('../src/client/app-server').OperationAdmission>();
  server.client.setAttachmentAdmission(() => old.promise);
  const pending = server.client.admitAttachment('A');
  server.client.setAttachmentAdmission(async () => false);
  old.resolve({ current: () => true, validate: async () => true });
  expect(await pending).toBe(false);
  expect(methods()).not.toContain('session/attach');
});

it.each(['success', 'failure', 'unavailable'] as const)('current page notice ignores historical off-page %s', async outcome => {
  const host = hostFixture();
  let reject!: (error: Error) => void, resolve!: (value: Awaited<ReturnType<ProductHostWorkspaces['classifyLocations']>>) => void;
  const old = new Promise<Awaited<ReturnType<ProductHostWorkspaces['classifyLocations']>>>((yes, no) => { resolve = yes; reject = no; });
  vi.mocked(host.classifyLocations).mockReturnValueOnce(old);
  await mount(host);
  if (outcome === 'unavailable') await act(async () => reject(new Error('old page unavailable')));
  server.handlers.set('session/list', () => ({ type: 'sessions', sessions: [server.summary('B')] }));
  await act(async () => server.client.listSessions());
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'View options' })));
  await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'Refresh list' })));
  expect(screen.getByRole('button', { name: 'Open Session B' }).closest('[data-workspace-group]')?.getAttribute('data-workspace-group')).toBe('wB');
  if (outcome === 'success') await act(async () => resolve([{ authorized: true, workspaceId: 'wA' }, { authorized: true, workspaceId: 'wB' }]));
  if (outcome === 'failure') await act(async () => reject(new Error('obsolete error')));
  expect(screen.queryByText(translator('en')('workspace:association.refreshing'))).toBeNull();
  expect(screen.queryByText(translator('en')('workspace:association.unavailable'))).toBeNull();
});

it('one refresh moves sidebar and selected Session together to the replacement registration', async () => {
  const host = await mount();
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Session A' })));
  const catalog = await host.listWorkspaces();
  catalog.workspaces = catalog.workspaces.map(row => row.id === 'wA' ? { ...row, id: 'replacement-A', displayName: 'Replacement A' } : row);
  vi.mocked(host.listWorkspaces).mockResolvedValue(catalog);
  const gate = deferred<Awaited<ReturnType<ProductHostWorkspaces['classifyLocations']>>>();
  vi.mocked(host.classifyLocations).mockReturnValueOnce(gate.promise);
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'View options' })));
  await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'Refresh list' })));
  const group = () => screen.getByRole('button', { name: 'Open Session A' }).closest('[data-workspace-group]')?.getAttribute('data-workspace-group');
  expect(group()).toBeUndefined();
  expect(screen.getByRole('button', { name: 'Select Workspace Replacement A' }).getAttribute('aria-current')).not.toBe('page');
  await act(async () => gate.resolve([{ authorized: true, workspaceId: 'replacement-A' }, { authorized: true, workspaceId: 'wB' }]));
  expect(group()).toBe('replacement-A');
  expect(screen.getByRole('button', { name: 'Select Workspace Replacement A' }).getAttribute('aria-current')).toBe('page');
});

it.each(['en', 'zh'] as const)('selected off-page pending and unavailable demand remains visible in %s', async locale => {
  const host = await mount();
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Session A' })));
  server.handlers.set('session/list', () => ({ type: 'sessions', sessions: [server.summary('B')] }));
  await act(async () => server.client.listSessions());
  const gate = deferred<Awaited<ReturnType<ProductHostWorkspaces['classifyLocations']>>>();
  vi.mocked(host.classifyLocations).mockReturnValueOnce(gate.promise);
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'View options' })));
  await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'Refresh list' })));
  act(() => localeController.setLocale(locale)); const tx = translator(locale);
  expect(screen.getByText(tx('workspace:association.refreshing'))).toBeTruthy();
  expect(server.client.getSnapshot().sessions.map(row => row.id)).toEqual(['B']);
  expect(vi.mocked(host.classifyLocations).mock.calls.at(-1)?.[0]).toEqual(['/workspace/A', '/workspace/B']);
  await act(async () => gate.resolve([{ authorized: false, reason: 'unavailable' }, { authorized: true, workspaceId: 'wB' }]));
  expect(screen.getByText(tx('workspace:association.unavailable'))).toBeTruthy();
  expect(screen.queryByText(tx('workspace:association.refreshing'))).toBeNull();
});

it('current blank conversation is pinned, reused, relocated and hidden on departure without native creation', async () => {
  await mount();
  const draft = () => document.querySelector('[data-draft-conversation]')!;
  expect(draft()).toBeNull();
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'New conversation in Workspace A' })));
  expect(draft().getAttribute('aria-selected')).toBe('true');
  expect(draft().closest('[data-workspace-group]')?.textContent).toContain('Workspace A');
  expect(draft().nextElementSibling?.querySelector('[data-session-id]')?.getAttribute('data-session-id')).toBe('A');
  expect(draft().querySelector('[data-session-id], [data-session-actions]')).toBeNull();
  fireEvent.change(screen.getByLabelText('Message'), { target: { value: 'unsent draft' } });
  await act(async () => fireEvent.click(within(draft() as HTMLElement).getByRole('button')));
  expect(document.activeElement).toBe(screen.getByLabelText('Message'));
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'New conversation in Workspace A' })));
  expect(document.querySelectorAll('[data-draft-conversation]')).toHaveLength(1);
  expect((screen.getByLabelText('Message') as HTMLTextAreaElement).value).toBe('unsent draft');
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Choose Workspace' })));
  await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'Workspace B' })));
  expect(draft().closest('[data-workspace-group]')?.textContent).toContain('Workspace B');
  expect((screen.getByLabelText('Message') as HTMLTextAreaElement).value).toBe('unsent draft');
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Session A' })));
  expect(draft()).toBeNull();
  expect(methods()).not.toContain('session/create');
  expect(methods()).not.toContain('session/delete');
});

it('preserves native recency order in grouped, flat and search views', async () => {
  server.summaries.set('A', { cwd: '/workspace/A', updated_at: '2026-10-01T00:00:00Z' });
  server.summaries.set('B', { cwd: '/workspace/A', updated_at: '2026-10-02T00:00:00Z' });
  await mount();
  const order = () => within(screen.getByRole('tree', { name: 'Session browser' }))
    .getAllByRole('button', { name: /^Open Session / }).map(node => node.getAttribute('aria-label'));
  expect(order()).toEqual(['Open Session B', 'Open Session A']);
  expect(server.client.getSnapshot().sessions.map(row => row.id)).toEqual(['B', 'A']);
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'View options' })));
  await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'Flat view' })));
  expect(order()).toEqual(['Open Session B', 'Open Session A']);
  server.summaries.set('A', { cwd: '/workspace/A', updated_at: '2026-10-03T00:00:00Z' });
  await act(async () => server.invalidateSummary('A', server.socket, true));
  await waitFor(() => expect(order()).toEqual(['Open Session A', 'Open Session B']));
  // Equal times use identity, independent of the catalog response order.
  server.summaries.set('B', { cwd: '/workspace/A', updated_at: '2026-10-03T00:00:00Z' });
  await act(async () => server.invalidateSummary('B', server.socket, true));
  expect(order()).toEqual(['Open Session A', 'Open Session B']);
  await act(async () => fireEvent.change(screen.getByRole('textbox', { name: 'Search Session metadata' }), { target: { value: 'Session' } }));
  expect(within(screen.getByRole('tree', { name: 'Session browser' })).getAllByRole('treeitem', { name: /^Open Session / }).map(node => node.getAttribute('aria-label'))).toEqual(['Open Session A', 'Open Session B']);
});

it('native activity invalidation replaces the current page with a recently active off-page Session', async () => {
  let recent = false;
  server.snapshots.set('older', snapshot('older'));
  server.summaries.set('older', { cwd: '/workspace/A', name: 'Older conversation', updated_at: '2026-10-07T00:00:00Z' });
  server.handlers.set('session/list', () => ({ type: 'sessions', sessions: recent
    ? [server.summary('older'), server.summary('A')] : [server.summary('A'), server.summary('B')], next_offset: 32 }));
  await mount();
  expect(screen.queryByRole('button', { name: 'Open Older conversation' })).toBeNull();
  recent = true;
  await act(async () => server.invalidateSummary('older', server.socket, true));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Open Older conversation' })).toBeTruthy());
  expect(server.client.getSnapshot().sessions.map(row => row.id)).toEqual(['older', 'A']);
  expect(methods().filter(method => method === 'session/list')).toHaveLength(2);
  expect(methods()).not.toContain('session/attach');
});

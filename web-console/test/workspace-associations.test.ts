import { expect, it, vi } from 'vitest';
import { NavigationEpoch } from '../src/client/navigation';
import { WorkspaceAssociations } from '../src/workspaces/associations';
import type { AppServerClient, ClientView } from '../src/client/app-server';
import { WorkspaceHostError, type ProductHostWorkspaces, type SessionLocation, type WorkspaceCatalog } from '../src/workspaces/host';

function deferred<T>() {
  let resolve!: (value: T) => void, reject!: (cause: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
const endpoint = 'ws://localhost:8080/';
const row = (id: string, cwd = `/${id}`) => ({ id, cwd, updated_at: '0', active_node: `node-${id}` });
const catalog = (authorityId = 'host-1'): WorkspaceCatalog => ({ authorityId, endpoint, workspaces: ['A', 'B'].map(id => ({ id, location: id, displayName: id, displayPath: `/${id}` })), picker: { kind: 'unavailable', reason: 'test' } });
async function fixture() {
  let state: ClientView = { connection: 'connected', authorityId: 'native-1', authorityRevision: 0, endpoint, generation: 1, sessions: [row('A'), row('B')], views: {}, uncertain: [], interactionOperations: {} };
  const listeners = new Set<() => void>();
  const deletions = new Set<(id: string) => void>();
  const reads: { cwds: readonly string[]; gate: ReturnType<typeof deferred<SessionLocation[]>>; signal?: AbortSignal }[] = [];
  const host: ProductHostWorkspaces = {
    listWorkspaces: vi.fn(async () => catalog()),
    classifyLocations: vi.fn((cwds, _endpoint, _authority, signal) => {
      const gate = deferred<SessionLocation[]>(); reads.push({ cwds: [...cwds], gate, signal }); return gate.promise;
    }),
    adoptWorkspace: vi.fn(), removeWorkspace: vi.fn(), renameWorkspace: vi.fn(), reorderWorkspace: vi.fn(), resolveWorkspace: vi.fn(),
  };
  const navigation = new NavigationEpoch();
  const client = { navigation, subscribeSessionDeletion: (listener: (id: string) => void) => { deletions.add(listener); return () => deletions.delete(listener); }, getSnapshot: () => state, subscribe: (listener: () => void) => { listeners.add(listener); return () => listeners.delete(listener); } } as unknown as AppServerClient;
  const owner = new WorkspaceAssociations(client, host);
  const stop = owner.start();
  const publish = (patch: Partial<ClientView>) => { state = { ...state, ...patch }; listeners.forEach(listener => listener()); };
  const observed = async (count: number) => { await vi.waitFor(() => expect(reads).toHaveLength(count)); return reads[count - 1]; };
  const accept = async (index: number, locations?: SessionLocation[]) => {
    const read = reads[index]; read.gate.resolve(locations ?? read.cwds.map(cwd => ({ authorized: true, workspaceId: cwd.slice(1) })));
    await vi.waitFor(() => expect(owner.getSnapshot().entries.get(read.cwds[0].slice(1))?.status).toBe('ready'));
  };
  await observed(1);
  return { owner, navigation, host, reads, stop, publish, observed, accept, listeners, remove: (id: string) => { publish({ sessions: state.sessions.filter(row => row.id !== id), views: {} }); deletions.forEach(listener => listener(id)); }, state: () => state };
}
it('retains confirmed evidence during cloned, title-only, reordered and disconnected observations without extra reads', async () => {
  const f = await fixture(); await f.accept(0);
  const confirmed = f.owner.getSnapshot().entries.get('A')?.confirmed;
  f.publish({ sessions: [row('B'), { ...row('A'), name: 'renamed' }] });
  expect(f.reads).toHaveLength(1);
  expect(f.owner.getSnapshot().entries.get('A')?.confirmed).toEqual(confirmed);
  f.publish({ connection: 'disconnected', generation: 2 });
  expect(f.owner.getSnapshot().entries.get('A')).toMatchObject({ confirmed, status: 'disconnected' });
  f.publish({ connection: 'connected' });
  await f.observed(2);
  expect(f.owner.getSnapshot().entries.get('A')).toMatchObject({ confirmed, status: 'refreshing' });
  expect(f.host.resolveWorkspace).not.toHaveBeenCalled(); expect(f.host.adoptWorkspace).not.toHaveBeenCalled();
  f.stop(); expect(f.listeners.size).toBe(0);
});
it('correlates captured IDs with duplicate cwds and retains unrelated page metadata', async () => {
  const f = await fixture(); await f.accept(0);
  f.publish({ sessions: [row('X', '/A'), row('Y', '/A')] });
  const pending = await f.observed(2);
  f.publish({ sessions: [row('Y', '/A'), row('X', '/A')] });
  expect(f.reads).toHaveLength(2);
  pending.gate.resolve([{ authorized: true, workspaceId: 'A' }, { authorized: true, workspaceId: 'A' }]);
  await vi.waitFor(() => expect(f.owner.getSnapshot().entries.get('Y')?.status).toBe('ready'));
  expect(f.owner.getSnapshot().entries.get('X')?.confirmed?.workspaceId).toBe('A');
  expect(f.owner.getSnapshot().entries.get('B')?.confirmed?.workspaceId).toBe('B');
  f.stop();
});
it.each(['success', 'failure'])('rejects obsolete %s after a newer cwd observation publishes', async outcome => {
  const f = await fixture(); const old = f.reads[0];
  f.publish({ sessions: [row('A', '/B')] });
  const fresh = await f.observed(2);
  fresh.gate.resolve([{ authorized: true, workspaceId: 'B' }]);
  await vi.waitFor(() => expect(f.owner.getSnapshot().entries.get('A')?.confirmed?.workspaceId).toBe('B'));
  if (outcome === 'success') old.gate.resolve([{ authorized: true, workspaceId: 'A' }, { authorized: true, workspaceId: 'B' }]);
  else old.gate.reject(new Error('obsolete failure'));
  await old.gate.promise.catch(() => {});
  expect(f.owner.getSnapshot().entries.get('A')).toMatchObject({ cwd: '/B', confirmed: { workspaceId: 'B' }, status: 'ready' });
  f.stop();
});
it.each(['classification', 'catalog', 'filesystem'])('retains evidence when %s is unavailable', async kind => {
  const f = await fixture(); await f.accept(0);
  if (kind === 'catalog') vi.mocked(f.host.listWorkspaces).mockRejectedValueOnce(new Error('catalog offline'));
  f.owner.refresh();
  expect(f.owner.getSnapshot().entries.get('A')?.confirmed?.workspaceId).toBe('A');
  if (kind !== 'catalog') {
    const next = await f.observed(2);
    if (kind === 'classification') next.gate.reject(new Error('offline'));
    else next.gate.resolve([{ authorized: false, reason: 'unavailable' }, { authorized: false, reason: 'unavailable' }]);
  }
  await vi.waitFor(() => expect(f.owner.getSnapshot().entries.get('A')?.status).toBe('unavailable'));
  expect(f.owner.getSnapshot().entries.get('A')?.confirmed?.workspaceId).toBe('A');
  expect(f.owner.getSnapshot().entries.get('A')).not.toHaveProperty('authorized'); f.stop();
});
it('unknown, confirmed ungrouped and revoked observations are distinct', async () => {
  const f = await fixture();
  expect(f.owner.getSnapshot().entries.get('A')).toMatchObject({ status: 'pending' });
  expect(f.owner.getSnapshot().entries.get('A')?.confirmed).toBeUndefined();
  f.reads[0].gate.resolve([{ authorized: true }, { authorized: false, reason: 'denied' }]);
  await vi.waitFor(() => expect(f.owner.getSnapshot().entries.get('A')?.status).toBe('ready'));
  expect(f.owner.getSnapshot().entries.get('A')?.confirmed).toEqual({});
  expect(f.owner.getSnapshot().entries.get('B')).toMatchObject({ status: 'revoked' }); f.stop();
});
it.each(['success', 'failure'])('unregister fences late %s even when its independent catalog reread fails', async outcome => {
  const f = await fixture(); await f.accept(0); f.owner.refresh(); const old = await f.observed(2);
  vi.mocked(f.host.listWorkspaces).mockRejectedValueOnce(new Error('reread failed'));
  f.owner.refresh('A');
  expect(f.owner.getSnapshot().entries.get('A')?.confirmed).toEqual({});
  if (outcome === 'success') old.gate.resolve([{ authorized: true, workspaceId: 'A' }, { authorized: true, workspaceId: 'B' }]);
  else old.gate.reject(new Error('pre-unregister failure'));
  await vi.waitFor(() => expect(f.owner.getSnapshot().status).toBe('unavailable'));
  expect(f.owner.getSnapshot().entries.get('A')?.confirmed).toEqual({});
  expect(f.owner.getSnapshot().catalog?.workspaces.map(row => row.id)).toEqual(['B']); f.stop();
});
it.each(['endpoint', 'native', 'revision'])('retires incompatible %s identity before late replies or errors', async kind => {
  const f = await fixture(); await f.accept(0); f.owner.refresh(); const old = await f.observed(2);
  f.publish(kind === 'endpoint' ? { endpoint: 'ws://localhost:9000/' } : kind === 'native' ? { authorityId: 'native-2' } : { authorityRevision: 1 });
  expect(f.owner.getSnapshot().entries.get('A')?.confirmed).toBeUndefined();
  old.gate.reject(new Error('retired error')); await old.gate.promise.catch(() => {});
  expect(f.owner.getSnapshot().entries.get('A')?.confirmed).toBeUndefined(); f.stop();
});
it.each(['success', 'failure'])('Host replacement clears old groups before an obsolete catalog %s', async outcome => {
  const f = await fixture(); await f.accept(0);
  const old = deferred<WorkspaceCatalog>(), fresh = deferred<WorkspaceCatalog>();
  vi.mocked(f.host.listWorkspaces).mockReturnValueOnce(old.promise).mockReturnValueOnce(fresh.promise);
  const oldOperation = f.navigation.capture();
  f.owner.refresh(); f.owner.refresh(); fresh.resolve(catalog('host-2'));
  await f.observed(2);
  expect(f.owner.getSnapshot().entries.get('A')?.confirmed).toBeUndefined();
  if (outcome === 'success') old.resolve(catalog('host-1')); else old.reject(new Error('obsolete catalog'));
  await old.promise.catch(() => {});
  expect(f.owner.getSnapshot().catalog?.authorityId).toBe('host-2');
  expect(oldOperation()).toBe(false); f.stop();
});
it.each([{ invalid: [] }, { invalid: [{ authorized: true, workspaceId: 'missing' }] }, { invalid: [{ authorized: 'yes' }] }])('rejects an invalid complete batch atomically: %j', async ({ invalid }) => {
  const f = await fixture(); await f.accept(0); f.owner.refresh(); const read = await f.observed(2);
  read.gate.resolve(invalid as SessionLocation[]);
  await vi.waitFor(() => expect(f.owner.getSnapshot().entries.get('A')?.status).toBe('unavailable'));
  expect(f.owner.getSnapshot().entries.get('A')?.confirmed?.workspaceId).toBe('A');
  expect(f.owner.getSnapshot().entries.get('B')?.confirmed?.workspaceId).toBe('B'); f.stop();
});
it('bounds retained identities and in-flight work while coalescing repeated demands and disposing resources', async () => {
  const f = await fixture();
  for (let page = 0; page < 20; page++) f.publish({ sessions: Array.from({ length: 32 }, (_, i) => row(`${page}-${i}`, '/A')) });
  expect(f.reads.length).toBeLessThanOrEqual(2);
  expect(f.owner.getSnapshot().entries.size).toBeLessThanOrEqual(128);
  f.owner.select('19-0'); f.owner.select('19-0');
  expect(f.reads.length).toBeLessThanOrEqual(2);
  f.stop(); expect(f.reads.every(read => read.signal?.aborted)).toBe(true); expect(f.listeners.size).toBe(0);
});
it('splits a full page plus selected off-page demand into at most 32 per batch and publishes atomically', async () => {
  const f = await fixture(); await f.accept(0);
  f.publish({ views: { outside: { id: 'outside', attachment: 'detached', attachmentIntent: 'released', summary: row('outside', '/B') } } });
  f.owner.select('outside');
  f.publish({ sessions: Array.from({ length: 32 }, (_, i) => row(`page-${i}`, '/A')) });
  const batch = await f.observed(2); expect(batch.cwds).toHaveLength(32);
  batch.gate.resolve(batch.cwds.map(() => ({ authorized: true, workspaceId: 'A' })));
  const selected = await f.observed(3); expect(selected.cwds).toEqual(['/B']);
  expect(f.owner.getSnapshot().entries.get('page-0')?.confirmed).toBeUndefined();
  selected.gate.resolve([{ authorized: true, workspaceId: 'B' }]);
  await vi.waitFor(() => expect(f.owner.getSnapshot().entries.get('outside')?.status).toBe('ready'));
  expect(f.owner.getSnapshot().entries.get('page-0')?.confirmed?.workspaceId).toBe('A'); f.stop();
});

it('deletion retires an exact ID; a new Session with the same title inherits nothing', async () => {
  const f = await fixture(); await f.accept(0); f.owner.refresh(); const old = await f.observed(2);
  f.remove('A');
  f.publish({ sessions: [{ ...row('replacement', '/B'), name: 'same title' }] });
  expect(f.owner.getSnapshot().entries.has('A')).toBe(false);
  expect(f.owner.getSnapshot().entries.get('replacement')?.confirmed).toBeUndefined();
  old.gate.resolve([{ authorized: true, workspaceId: 'A' }, { authorized: true, workspaceId: 'B' }]);
  await old.gate.promise;
  expect(f.owner.getSnapshot().entries.has('A')).toBe(false); f.stop();
});
it('selected demand overlapping a pending page coalesces instead of issuing a duplicate read', async () => {
  const f = await fixture();
  f.owner.select('A'); f.owner.select('A');
  expect(f.reads).toHaveLength(1);
  await f.accept(0);
  expect(f.reads).toHaveLength(1); f.stop();
});
it('disposed owners can restart subscriptions without retaining disposed request authority', async () => {
  const f = await fixture(); const old = f.reads[0];
  f.stop(); const stop = f.owner.start();
  const fresh = await f.observed(2);
  fresh.gate.resolve([{ authorized: true, workspaceId: 'B' }, { authorized: true, workspaceId: 'B' }]);
  await vi.waitFor(() => expect(f.owner.getSnapshot().entries.get('A')?.confirmed?.workspaceId).toBe('B'));
  old.gate.reject(new Error('disposed')); await old.gate.promise.catch(() => {});
  expect(f.owner.getSnapshot().entries.get('A')?.status).toBe('ready'); stop();
});

it('a definitive Host replacement refusal retires evidence even if the replacement catalog is unavailable', async () => {
  const f = await fixture(); await f.accept(0); f.owner.refresh(); const read = await f.observed(2);
  vi.mocked(f.host.listWorkspaces).mockRejectedValueOnce(new Error('replacement unavailable'));
  read.gate.reject(new WorkspaceHostError('replaced', 'authority_replaced'));
  await vi.waitFor(() => expect(f.owner.getSnapshot().status).toBe('unavailable'));
  expect(f.owner.getSnapshot().entries.size).toBe(0);
  f.owner.refresh();
  await vi.waitFor(() => expect(f.owner.getSnapshot().status).toBe('unavailable'));
  expect(f.owner.getSnapshot().entries.size).toBe(0); expect(f.owner.getSnapshot().catalog).toBeUndefined(); f.stop();
});

it.each([false, true])('registration replacement classifies on the first refresh (explicit unregister: %s)', async explicit => {
  const f = await fixture(); await f.accept(0);
  const next = catalog(); next.workspaces[0] = { ...next.workspaces[0], id: 'new-A' };
  vi.mocked(f.host.listWorkspaces).mockResolvedValue(next);
  f.owner.refresh(explicit ? 'A' : undefined);
  const read = await f.observed(2);
  expect(read.cwds).toEqual(['/A', '/B']);
  expect(f.owner.getSnapshot().entries.get('A')?.confirmed?.workspaceId).toBeUndefined();
  expect(f.owner.getSnapshot().entries.get('A')?.status).not.toBe('ready');
  read.gate.resolve([{ authorized: true, workspaceId: 'new-A' }, { authorized: true, workspaceId: 'B' }]);
  await vi.waitFor(() => expect(f.owner.getSnapshot().entries.get('A')?.confirmed).toEqual({ workspaceId: 'new-A' }));
  expect(f.reads).toHaveLength(2); f.stop();
});
it.each(['success', 'failure'])('current demand settles independently of obsolete off-page %s', async outcome => {
  const f = await fixture(); const old = f.reads[0];
  f.publish({ sessions: [row('X', '/B')] }); f.owner.refresh();
  const fresh = await f.observed(2);
  fresh.gate.resolve([{ authorized: true, workspaceId: 'B' }]);
  await vi.waitFor(() => expect(f.owner.getSnapshot().entries.get('X')?.status).toBe('ready'));
  expect(f.owner.getSnapshot().status).toBe('ready');
  if (outcome === 'success') old.gate.resolve([{ authorized: true, workspaceId: 'A' }, { authorized: true, workspaceId: 'B' }]);
  else old.gate.reject(new Error('obsolete'));
  await old.gate.promise.catch(() => {});
  expect(f.owner.getSnapshot().status).toBe('ready');
  expect(f.owner.getSnapshot().entries.has('A')).toBe(true); f.stop();
});
it('selected off-page demand participates in status, including unavailable and queued reads', async () => {
  const f = await fixture(); await f.accept(0);
  f.publish({ views: { outside: { id: 'outside', attachment: 'detached', attachmentIntent: 'released', summary: row('outside', '/A') } } });
  f.owner.select('outside'); const read = await f.observed(2);
  expect(['pending', 'refreshing']).toContain(f.owner.getSnapshot().status);
  read.gate.resolve(read.cwds.map(cwd => cwd === '/B' ? { authorized: true, workspaceId: 'B' } : { authorized: false, reason: 'unavailable' }));
  await vi.waitFor(() => expect(f.owner.getSnapshot().entries.get('outside')?.status).toBe('unavailable'));
  expect(f.owner.getSnapshot().status).toBe('unavailable'); f.stop();
});

it('inactive unavailable evidence is retained without affecting a healthy page, then reused on return', async () => {
  const f = await fixture(); await f.accept(0);
  f.publish({ sessions: [row('X', '/A')] }); const read = await f.observed(2);
  read.gate.reject(new Error('X unavailable'));
  await vi.waitFor(() => expect(f.owner.getSnapshot().status).toBe('unavailable'));
  f.publish({ sessions: [row('B')] });
  expect(f.owner.getSnapshot().status).toBe('ready');
  expect(f.owner.getSnapshot().entries.get('X')?.status).toBe('unavailable');
  expect(f.owner.getSnapshot().entries.get('A')?.confirmed).toEqual({ workspaceId: 'A' });
  f.publish({ sessions: [row('A')] });
  expect(f.owner.getSnapshot().status).toBe('ready'); expect(f.reads).toHaveLength(2); f.stop();
});
it('queued current demand remains pending while both classification slots are occupied', async () => {
  const f = await fixture();
  f.owner.refresh(); await f.observed(2);
  f.publish({ sessions: [row('X', '/B')] }); f.owner.refresh();
  await vi.waitFor(() => expect(f.host.listWorkspaces).toHaveBeenCalledTimes(3));
  expect(f.reads).toHaveLength(2);
  expect(f.owner.getSnapshot().status).toBe('pending');
  f.reads[0].gate.reject(new Error('obsolete slot'));
  const fresh = await f.observed(3);
  expect(fresh.cwds).toEqual(['/B']);
  fresh.gate.resolve([{ authorized: true, workspaceId: 'B' }]);
  await vi.waitFor(() => expect(f.owner.getSnapshot().status).toBe('ready'));
  f.stop();
});

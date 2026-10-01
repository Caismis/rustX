// @vitest-environment node
/** Final operation admission against a real Product Host process (Node only). */
import { afterEach, expect, it, vi } from 'vitest';
import { mkdirSync, mkdtempSync, realpathSync, rmSync, symlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { LocalWorkspaceHost } from '../host/workspaces';
import { CommandSession } from '../src/app/commands/native';
import { WorkspaceAuthority } from '../src/workspaces/authority';
import { WorkspaceSessionNavigation } from '../src/workspaces/navigation';
import type { ProductHostWorkspaces, SessionLocation } from '../src/workspaces/host';
import type { MethodResult } from '../../protocol/app-server/v30';
import { Server, endpoint } from './fixture';

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(done => { resolve = done; });
  return { promise, resolve };
}
const directories: string[] = [];
afterEach(() => { directories.splice(0).forEach(path => rmSync(path, { force: true, recursive: true })); });

/** A real Product Host process. Its Nth catalog read can be held; classification is never faked. */
function realHost(server: Server) {
  const directory = mkdtempSync(join(tmpdir(), 'workspace-admission-')); directories.push(directory);
  const root = join(directory, 'A'); mkdirSync(root);
  const cwd = realpathSync(root);
  const local = new LocalWorkspaceHost({ endpoint, picker: false, metadataFile: join(directory, 'registrations.json'), roots: [{ id: 'a', cwd: root, displayName: 'A' }] });
  let calls = 0;
  const holds = new Map<number, { entered: () => void; released: Promise<void> }>();
  const lists: string[] = [];
  const classified: { authorityId?: string; locations: SessionLocation[] }[] = [];
  const unused = async () => { throw new Error('unused'); };
  const host: ProductHostWorkspaces = {
    listWorkspaces: async () => {
      const hold = holds.get(++calls);
      if (hold) { hold.entered(); await hold.released; }
      const catalog = await local.listWorkspaces(); lists.push(catalog.authorityId); return catalog;
    },
    classifyLocations: async (cwds, target, authorityId) => {
      const locations = await local.classifyLocations(cwds, target, authorityId); classified.push({ authorityId, locations }); return locations;
    },
    adoptWorkspace: unused, renameWorkspace: unused, reorderWorkspace: unused, removeWorkspace: unused, resolveWorkspace: unused,
  };
  server.handlers.set('session/settings', () => ({ type: 'settings', revision: '0', settings: { cwd } }));
  const authority = new WorkspaceAuthority(host);
  const accepted = vi.fn(); authority.subscribe(accepted);
  server.client.setAttachmentAdmission(new WorkspaceSessionNavigation(authority, server.client, server.client.navigation).admit);
  return { root, directory, authority, accepted, lists, classified, calls: () => calls,
    /** Hold catalog read number `at` (1-based) until released. */
    holdList: (at: number) => {
      const entered = deferred<void>(), released = deferred<void>();
      holds.set(at, { entered: () => entered.resolve(), released: released.promise });
      return { entered: entered.promise, release: () => released.resolve() };
    } };
}
type RealHost = ReturnType<typeof realHost>;
const rootChanges = {
  // The configured directory disappears: fresh classification is unavailable.
  deleted: { reason: 'unavailable', apply: (f: RealHost) => rmSync(f.root, { recursive: true }) },
  // The configured path now resolves to another physical directory: denied.
  'replaced by a symlink': { reason: 'denied', apply: (f: RealHost) => {
    const other = join(f.directory, 'other'); mkdirSync(other);
    rmSync(f.root, { recursive: true }); symlinkSync(other, f.root);
  } },
} as const;

it.each(Object.keys(rootChanges) as (keyof typeof rootChanges)[])('same Host process: a root %s after initial admission refuses the final attach', async change => {
  const server = new Server(); await server.connect();
  const f = realHost(server);
  try {
    // Hold the final validation's Host observation: dispatch capacity is reserved, nothing is sent.
    const final = f.holdList(2);
    const attach = server.client.attach('A');
    await final.entered;
    expect(f.classified).toEqual([{ authorityId: f.lists[0], locations: [expect.objectContaining({ authorized: true })] }]);
    expect(server.requests.filter(row => row.request.method === 'session/attach')).toHaveLength(0);
    rootChanges[change].apply(f);
    final.release();
    await expect(attach).rejects.toThrow(`no longer authorized by this Product Host (${rootChanges[change].reason})`);
    // Same Product Host process throughout: its identity alone would have admitted this.
    expect(f.lists).toEqual([f.lists[0], f.lists[0]]);
    expect(f.accepted).toHaveBeenCalledTimes(1);
    expect(f.classified[1]).toEqual({ authorityId: f.lists[0], locations: [{ authorized: false, reason: rootChanges[change].reason }] });
    expect(server.requests.filter(row => row.request.method === 'session/attach')).toHaveLength(0);
    expect(server.client.getSnapshot().uncertain).toEqual([]);
  } finally { server.client.disconnect(); }
});

it.each(Object.keys(rootChanges) as (keyof typeof rootChanges)[])('Fork uses the same final authorization: a root %s after admission sends no session/fork', async change => {
  const server = new Server(); await server.connect();
  const f = realHost(server);
  try {
    await server.client.attach('A');
    const { conversation_id } = server.client.target('A');
    server.handlers.set('session/boundaries', () => ({ type: 'boundaries', surface_revision: '1', boundaries: [{ surface_revision: '1', message: { id: 'user-1', kind: 'message', source: 'human', content: [{ type: 'text', text: 'hi' }] } }] }) as MethodResult);
    server.handlers.set('session/tree', () => ({ type: 'tree', nodes: [{ id: 'node-A', conversation_id, ordinal: '1', origin: { type: 'new' } }] }) as MethodResult);
    const scope = new CommandSession(server.client, 'A', () => true);
    const [selection] = (await scope.boundaries()).selections;
    // Fork admission reads the Host once, then its final validation reads it again.
    const final = f.holdList(f.calls() + 2);
    const fork = scope.transition('fork', selection);
    await final.entered;
    expect(f.classified.at(-1)?.locations).toEqual([expect.objectContaining({ authorized: true })]);
    rootChanges[change].apply(f);
    final.release();
    await expect(fork).rejects.toThrow(`no longer authorized by this Product Host (${rootChanges[change].reason})`);
    expect(new Set(f.lists).size).toBe(1);
    expect(f.classified.at(-1)?.locations).toEqual([{ authorized: false, reason: rootChanges[change].reason }]);
    expect(server.requests.filter(row => row.request.method === 'session/fork')).toHaveLength(0);
    expect(server.client.getSnapshot().uncertain).toEqual([]);
  } finally { server.client.disconnect(); }
});

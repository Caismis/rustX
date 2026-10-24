import { settingsReceipt } from './fixture';
// @vitest-environment node
/** Final operation admission against a real Product Host process (Node only). */
import { afterEach, expect, it, vi } from 'vitest';
import { mkdirSync, mkdtempSync, realpathSync, rmSync, symlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { LocalWorkspaceHost } from '../host/workspaces';
import { CommandSession } from '../src/app/commands/native';
import { firstSubmitPort } from '../src/app/new-conversation/port';
import { RequestNotDispatched } from '../src/client/errors';
import { WorkspaceAuthority } from '../src/workspaces/authority';
import { WorkspaceSessionAdmission } from '../src/workspaces/session-admission';
import type { ProductHostWorkspaces, SessionLocation } from '../src/workspaces/host';
import type { MethodResult } from '../../protocol/app-server/v57';
import { Server, endpoint, snapshot } from './fixture';

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
  const createLocal = () => new LocalWorkspaceHost({ endpoint, picker: false, metadataFile: join(directory, 'registrations.json'), roots: [{ id: 'a', cwd: root, displayName: 'A' }] });
  let local = createLocal();
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
    adoptWorkspace: unused, renameWorkspace: unused, reorderWorkspace: unused, removeWorkspace: unused,
    resolveWorkspace: (id, endpoint) => local.resolveWorkspace(id, endpoint),
  };
  server.handlers.set('session/settings', () => (settingsReceipt({ cwd },'0')));
  server.handlers.set('session/create', () => ({ type: 'session_transition', session: { id: 'A', active_node: 'node-A', active_conversation_id: 'conversation-A', node_count: 1, created_at: '0', updated_at: '0' } }));
  const authority = new WorkspaceAuthority(host);
  const accepted = vi.fn(); authority.subscribe(accepted);
  server.client.setSessionAdmission(new WorkspaceSessionAdmission(authority, server.client).admit);
  return { root, directory, authority, accepted, lists, classified, host, calls: () => calls,
    replaceHost: () => { local = createLocal(); },
    workspaceId: async () => (await local.listWorkspaces()).workspaces[0].id,
    /** Hold catalog read number `at` (1-based) until released. */
    holdList: (at: number) => {
      const entered = deferred<void>(), released = deferred<void>();
      holds.set(at, { entered: () => entered.resolve(), released: released.promise });
      return { entered: entered.promise, release: () => released.resolve() };
    } };
}
type RealHost = ReturnType<typeof realHost>;

/** Reach the production pipeline's six ordinary slots through actual lifecycle admission. */
async function holdSixAttachments(server: Server) {
  server.held.add('session/attach');
  const openings: Promise<void>[] = [];
  for (const [index, id] of ['A', 'B', 'C', 'D', 'E', 'F'].entries()) {
    server.snapshots.set(id, snapshot(id));
    const opening = server.client.attach(id, `node-${id}`);
    void opening.catch(() => {}); // Cleanup disconnect settles every still-owned actor.
    openings.push(opening);
    await server.waitFor('session/attach', index + 1);
  }
  const requests = server.requests.filter(row => row.request.method === 'session/attach');
  expect(requests).toHaveLength(6);
  const settings = server.requests.filter(row => row.request.method === 'session/settings');
  expect(new Set(settings.map(row => 'session_id' in row.request.params ? row.request.params.session_id : undefined))).toEqual(new Set(['A', 'B', 'C', 'D', 'E', 'F']));
  expect(server.client.getSnapshot().views.A.nodeId).toBe('node-A');
  expect(server.loaded.size).toBe(0);
  return { openings, requests, settingsCount: settings.length, reads: server.requests.filter(row => row.request.method === 'session/read').length };
}

it.each(['known', 'unknown'] as const)('bounded metadata admission for a %s node reaches intent while six attachment RPCs occupy every ordinary slot', async selection => {
  const server = new Server(); await server.connect();
  const f = realHost(server);
  let held: Awaited<ReturnType<typeof holdSixAttachments>> | undefined;
  let submitting: Promise<unknown> | undefined;
  try {
    held = await holdSixAttachments(server);
    const id = selection === 'known' ? 'A' : 'G';
    if (selection === 'unknown') {
      server.snapshots.set(id, snapshot(id));
      expect(server.client.getSnapshot().views[id]?.nodeId).toBeUndefined();
    }
    const requests = vi.spyOn(server.client, 'request');
    const port = firstSubmitPort(server.client, f.host, () => true, () => () => {}, true);
    submitting = port.readExisting!(id);
    void submitting.catch(() => {});
    const intent = await server.waitFor('session/submissionIntent', 1);
    expect(intent.params).toEqual({ session_id: id, node_id: `node-${id}` });
    expect(server.requests.filter(row => row.request.method === 'session/attach')).toEqual(held.requests);
    expect(server.loaded.size).toBe(0); // No attachment response released to free a slot.
    expect(requests.mock.calls.filter(([request]) => request.method === 'session/settings')).toHaveLength(1);
    expect(server.requests.filter(row => row.request.method === 'session/settings')).toHaveLength(held.settingsCount + 1);
    expect(f.classified.at(-1)?.locations).toEqual([expect.objectContaining({ authorized: true })]);
    const settings = server.requests.filter(row => row.request.method === 'session/settings').at(-1)!;
    expect(settings.request.params).toEqual({ session_id: id });
    expect(requests.mock.calls.filter(([request]) => request.method === 'session/read')).toHaveLength(selection === 'known' ? 0 : 1);
    expect(server.requests.filter(row => row.request.method === 'session/read')).toHaveLength(held.reads + (selection === 'known' ? 0 : 1));
    expect(server.requests.indexOf(settings)).toBeLessThan(server.requests.findIndex(row => row.request === intent));

    // A previously unseen Session can begin attachment after its intent has
    // reached Native; its seventh ordinary RPC waits for these six to settle.
    if (selection === 'unknown') {
      const opening = server.client.attach(id, `node-${id}`);
      void opening.catch(() => {});
      held.openings.push(opening);
    }
    server.held.delete('session/attach');
    for (const { request, socket } of held.requests) server.reply(request, socket);
    await Promise.all(held.openings);
    await expect(submitting).resolves.toEqual({ id, node: `node-${id}`, conversation: `conversation-${id}` });
    expect(server.requests.filter(row => row.request.method === 'session/submissionIntent')).toHaveLength(1);
  } finally {
    server.client.disconnect();
    await Promise.allSettled([...(held?.openings ?? []), ...(submitting ? [submitting] : [])]);
  }
});

it('Workspace directory revocation while create ACK is held prevents native submission intent and attachment', async () => {
  const server = new Server(); await server.connect();
  const f = realHost(server);
  const acknowledged = vi.fn();
  try {
    server.held.add('session/create');
    const port = firstSubmitPort(server.client, f.host, () => true, () => () => {});
    const creating = port.create({ workspaceId: await f.workspaceId(), text: 'task', files: [] }, acknowledged);
    const refused = expect(creating).rejects.toThrow('Session cwd is not authorized by this Product Host');
    const request = await server.waitFor('session/create', 1);
    rmSync(f.root, { recursive: true });
    server.reply(request);
    await refused;
    expect(acknowledged).toHaveBeenCalledTimes(1);
    expect(server.requests.filter(row => ['session/submissionIntent', 'session/attach', 'turn/start'].includes(row.request.method))).toHaveLength(0);
  } finally { server.client.disconnect(); }
});

it.each(['new', 'existing'] as const)('%s Session submission intent rechecks Host authorization in its reserved dispatch slot', async kind => {
  const server = new Server(); await server.connect();
  const f = realHost(server);
  try {
    const final = f.holdList(2);
    const port = firstSubmitPort(server.client, f.host, () => true, () => () => {}, kind === 'existing');
    const submitting = kind === 'new'
      ? port.create({ workspaceId: await f.workspaceId(), text: 'task', files: [] }, () => {})
      : port.readExisting!('A');
    const refused = expect(submitting).rejects.toThrow('no longer authorized by this Product Host (denied)');
    await final.entered;
    expect(f.classified.at(-1)?.locations).toEqual([expect.objectContaining({ authorized: true })]);
    expect(server.requests.filter(row => row.request.method === 'session/submissionIntent')).toHaveLength(0);
    rootChanges['replaced by a symlink'].apply(f);
    final.release();
    await refused;
    expect(server.requests.filter(row => ['session/submissionIntent', 'session/attach', 'turn/start'].includes(row.request.method))).toHaveLength(0);
    expect(server.client.getSnapshot().uncertain).toEqual([]);
  } finally { server.client.disconnect(); }
});

const rootChanges = {
  // The configured directory disappears: fresh classification is unavailable.
  deleted: { reason: 'unavailable', apply: (f: RealHost) => rmSync(f.root, { recursive: true }) },
  // The configured path now resolves to another physical directory: denied.
  'replaced by a symlink': { reason: 'denied', apply: (f: RealHost) => {
    const other = join(f.directory, 'other'); mkdirSync(other);
    rmSync(f.root, { recursive: true }); symlinkSync(other, f.root);
  } },
} as const;

it.each(Object.keys(rootChanges) as (keyof typeof rootChanges)[])('with six held attachments, bounded metadata admission still refuses a root %s at final Host validation', async change => {
  const server = new Server(); await server.connect();
  const f = realHost(server);
  let held: Awaited<ReturnType<typeof holdSixAttachments>> | undefined;
  let final: ReturnType<RealHost['holdList']> | undefined;
  let submitting: Promise<unknown> | undefined;
  try {
    held = await holdSixAttachments(server);
    const requests = vi.spyOn(server.client, 'request');
    final = f.holdList(f.calls() + 2);
    const port = firstSubmitPort(server.client, f.host, () => true, () => () => {}, true);
    submitting = port.readExisting!('A');
    const refused = submitting.catch(error => error);
    await final.entered;
    expect(f.classified.at(-1)?.locations).toEqual([expect.objectContaining({ authorized: true })]);
    rootChanges[change].apply(f);
    final.release();
    expect(await refused).toMatchObject({ message: expect.stringContaining(`no longer authorized by this Product Host (${rootChanges[change].reason})`) });
    expect(requests.mock.calls.filter(([request]) => request.method === 'session/settings')).toHaveLength(1);
    expect(server.requests.filter(row => row.request.method === 'session/settings')).toHaveLength(held.settingsCount + 1);
    expect(server.requests.filter(row => row.request.method === 'session/attach')).toEqual(held.requests);
    expect(server.requests.filter(row => ['session/submissionIntent', 'turn/start'].includes(row.request.method))).toHaveLength(0);
    expect(server.loaded.size).toBe(0);
    expect(server.client.getSnapshot().uncertain).toEqual([]);
  } finally {
    final?.release();
    server.client.disconnect();
    await Promise.allSettled([...(held?.openings ?? []), ...(submitting ? [submitting] : [])]);
  }
});

it('with six held attachments, Host authority replacement retires final submission-intent admission', async () => {
  const server = new Server(); await server.connect();
  const f = realHost(server);
  let held: Awaited<ReturnType<typeof holdSixAttachments>> | undefined;
  let final: ReturnType<RealHost['holdList']> | undefined;
  let submitting: Promise<unknown> | undefined;
  try {
    held = await holdSixAttachments(server);
    const requests = vi.spyOn(server.client, 'request');
    final = f.holdList(f.calls() + 2);
    const port = firstSubmitPort(server.client, f.host, () => true, () => () => {}, true);
    submitting = port.readExisting!('A');
    const refused = submitting.catch(error => error);
    await final.entered;
    const initial = f.lists.at(-1);
    expect(f.classified.at(-1)?.locations).toEqual([expect.objectContaining({ authorized: true })]);
    f.replaceHost();
    final.release();
    expect(await refused).toBeInstanceOf(RequestNotDispatched);
    expect(f.lists.at(-1)).not.toBe(initial);
    expect(f.accepted).toHaveBeenCalledTimes(2);
    expect(requests.mock.calls.filter(([request]) => request.method === 'session/settings')).toHaveLength(1);
    expect(server.requests.filter(row => row.request.method === 'session/settings')).toHaveLength(held.settingsCount + 1);
    expect(server.requests.filter(row => row.request.method === 'session/attach')).toEqual(held.requests);
    expect(server.requests.filter(row => ['session/submissionIntent', 'turn/start'].includes(row.request.method))).toHaveLength(0);
    expect(server.loaded.size).toBe(0);
    expect(server.client.getSnapshot().uncertain).toEqual([]);
  } finally {
    final?.release();
    server.client.disconnect();
    await Promise.allSettled([...(held?.openings ?? []), ...(submitting ? [submitting] : [])]);
  }
});

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
    server.handlers.set('session/boundaries', () => ({ type: 'boundaries', surface_revision: '1', boundaries: [{ surface_revision: '1', locator:{type:'by_message_id',message_id:'user-1'}, block_count:'1',content:[],collections:[], message: { id: 'user-1', kind: 'message', source: 'human', content: [{ type: 'text', text: 'hi' }] } }] }) as MethodResult);
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

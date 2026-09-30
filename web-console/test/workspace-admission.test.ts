import { afterEach, expect, it, vi } from 'vitest';
import { WorkspaceAuthority } from '../src/workspaces/authority';
import { HttpWorkspaceHost } from '../src/workspaces/http-host';
import { WorkspaceSessionNavigation } from '../src/workspaces/navigation';
import type { SessionLocation, WorkspaceCatalog } from '../src/workspaces/host';
import { Server, endpoint } from './fixture';

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(done => { resolve = done; });
  return { promise, resolve };
}
afterEach(() => vi.unstubAllGlobals());

it.each([true, false])('same adapter, callback and native authority reject delayed old Host success; replacement authorized=%s', async authorized => {
  const server = new Server(); await server.connect();
  let hostId = 'old-host';
  const entered = deferred<void>(), old = deferred<SessionLocation[]>();
  const requests: { method: string; body: Record<string, unknown> }[] = [];
  vi.stubGlobal('fetch', vi.fn(async (url: string, init: RequestInit) => {
    const method = new URL(url).pathname.split('/').at(-1)!;
    const body = JSON.parse(init.body as string); requests.push({ method, body });
    if (method === 'list') return Response.json({ authorityId: hostId, endpoint, workspaces: [], picker: { kind: 'unavailable', reason: 'test' } } satisfies WorkspaceCatalog);
    expect(method).toBe('classify'); expect(body.cwds).toEqual(['/workspace/A']); expect(body.endpoint).toBe(endpoint);
    if (body.authorityId === 'old-host') { entered.resolve(); return Response.json(await old.promise); }
    expect(body.authorityId).toBe('replacement-host');
    return Response.json([authorized ? { authorized: true } : { authorized: false, reason: 'denied' }]);
  }));
  const adapter = new HttpWorkspaceHost();
  const authority = new WorkspaceAuthority(adapter);
  const navigation = new WorkspaceSessionNavigation(authority, server.client, server.client.navigation);
  server.client.setAttachmentAdmission(navigation.admit);
  const native = server.client.getSnapshot().authorityId, generation = server.client.getSnapshot().generation;
  try {
    // Deliberately never construct WorkspaceAssociations.
    const pending = server.client.attach('A');
    await entered.promise;
    expect(server.requests.filter(row => row.request.method === 'session/settings')).toHaveLength(1);
    expect(requests.map(row => row.method)).toEqual(['list', 'classify']);
    hostId = 'replacement-host'; // Adapter and injected admission callback remain identical.
    old.resolve([{ authorized: true }]);
    await expect(pending).rejects.toThrow('Authority changed before dispatch');
    expect(requests.map(row => row.method)).toEqual(['list', 'classify', 'list']);
    expect(server.requests.filter(row => row.request.method === 'session/attach')).toHaveLength(0);
    expect(server.client.getSnapshot().authorityId).toBe(native);
    expect(server.client.getSnapshot().generation).toBe(generation);
    if (authorized) await server.client.attach('A');
    else await expect(server.client.attach('A')).rejects.toThrow('not authorized');
    expect(server.requests.filter(row => row.request.method === 'session/attach')).toHaveLength(authorized ? 1 : 0);
    expect(server.client.getSnapshot().views.A.attachment).toBe(authorized ? 'attached' : 'error');
  } finally { server.client.disconnect(); }
});

it.each(['host', 'callback'] as const)('queued attach rechecks %s authority at the actual socket dispatch boundary', async replacement => {
  const server = new Server(); await server.connect();
  let hostId = 'old-host', lists = 0;
  const verifying = deferred<void>(), verification = deferred<void>();
  vi.stubGlobal('fetch', vi.fn(async (url: string) => {
    if (new URL(url).pathname.endsWith('/list')) {
      ++lists;
      return Response.json({ authorityId: hostId, endpoint, workspaces: [], picker: { kind: 'unavailable', reason: 'test' } });
    }
    verifying.resolve(); await verification.promise;
    return Response.json([{ authorized: true }]);
  }));
  const authority = new WorkspaceAuthority(new HttpWorkspaceHost());
  const navigation = new WorkspaceSessionNavigation(authority, server.client, server.client.navigation);
  server.client.setAttachmentAdmission(navigation.admit);
  const queued = deferred<void>();
  const request = server.client.request.bind(server.client);
  vi.spyOn(server.client, 'request').mockImplementation((...args) => {
    const result = request(...args);
    if (args[0].method === 'session/attach') queued.resolve();
    return result;
  });
  try {
    const pending = server.client.attach('A'); await verifying.promise;
    server.held.add('session/summary');
    const occupied = Array.from({ length: 8 }, () => server.client.request({ method: 'session/summary', params: { session_id: 'A' } }, 'session_summary'));
    await server.waitFor('session/summary', 8);
    verification.resolve(); await queued.promise;
    expect(lists).toBe(1); // Final Host observation must wait for dispatch capacity.
    expect(server.requests.filter(row => row.request.method === 'session/attach')).toHaveLength(0);
    if (replacement === 'host') hostId = 'replacement-host';
    else server.client.setAttachmentAdmission(async () => ({ current: () => true, validate: async () => true }));
    const summaries = server.requests.filter(row => row.request.method === 'session/summary');
    server.reply(summaries[0].request);
    await expect(pending).rejects.toThrow('Authority changed before dispatch');
    expect(server.requests.filter(row => row.request.method === 'session/attach')).toHaveLength(0);
    for (const row of summaries.slice(1)) server.reply(row.request);
    await Promise.all(occupied);
  } finally { server.client.disconnect(); }
});

it.each(['host', 'callback', 'native', 'unavailable'] as const)('final in-flight dispatch observation fails closed on %s retirement', async replacement => {
  const server = new Server(); await server.connect();
  let hostId = 'old-host', lists = 0;
  const entered = deferred<void>(), release = deferred<void>();
  vi.stubGlobal('fetch', vi.fn(async (url: string) => {
    if (!new URL(url).pathname.endsWith('/list')) return Response.json([{ authorized: true }]);
    const captured = hostId;
    if (++lists === 2) {
      entered.resolve(); await release.promise;
      if (replacement === 'unavailable') throw new Error('Host unavailable');
    }
    return Response.json({ authorityId: captured, endpoint, workspaces: [], picker: { kind: 'unavailable', reason: 'test' } });
  }));
  const authority = new WorkspaceAuthority(new HttpWorkspaceHost());
  const navigation = new WorkspaceSessionNavigation(authority, server.client, server.client.navigation);
  server.client.setAttachmentAdmission(navigation.admit);
  try {
    const pending = server.client.attach('A');
    const refused = expect(pending).rejects.toThrow();
    await entered.promise;
    expect(server.requests.filter(row => row.request.method === 'session/attach')).toHaveLength(0);
    if (replacement === 'host') { hostId = 'replacement-host'; await authority.observe(); }
    if (replacement === 'callback') server.client.setAttachmentAdmission(async () => ({ current: () => true, validate: async () => true }));
    if (replacement === 'native') server.client.disconnect();
    release.resolve(); await refused;
    expect(server.requests.filter(row => row.request.method === 'session/attach')).toHaveLength(0);
    expect(server.client.getSnapshot().uncertain).toEqual([]);
    if (replacement === 'host') expect(authority.getCatalog()?.authorityId).toBe('replacement-host');
  } finally { server.client.disconnect(); }
});

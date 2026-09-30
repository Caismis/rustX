import { afterEach, expect, it, vi } from 'vitest';
import { WorkspaceAuthority } from '../src/workspaces/authority';
import { HttpWorkspaceHost } from '../src/workspaces/http-host';
import { WorkspaceSessionNavigation } from '../src/workspaces/navigation';
import type { SessionLocation, WorkspaceCatalog } from '../src/workspaces/host';
import type { OperationAdmission } from '../src/client/app-server';
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

/** Eight operations whose final Host observation never answers until released. */
async function stalledFinalValidations(retirement: 'navigation' | 'timeout' | 'host') {
  const server = new Server(); await server.attached('B');
  let hostId = 'host', stall = false;
  const stalled: { signal: AbortSignal; release: () => void }[] = [];
  vi.stubGlobal('fetch', vi.fn(async (url: string, init: RequestInit) => {
    if (!new URL(url).pathname.endsWith('/list')) return Response.json([{ authorized: true }]);
    // The Host never answers until released. Only a real replacement exercises
    // fetch cancellation; otherwise the stalled read ignores it.
    const captured = hostId;
    if (stall) {
      const release = deferred<void>(); stalled.push({ signal: init.signal!, release: () => release.resolve() });
      if (retirement === 'host') init.signal!.addEventListener('abort', () => release.resolve());
      await release.promise; init.signal!.throwIfAborted();
    }
    return Response.json({ authorityId: captured, endpoint, workspaces: [], picker: { kind: 'unavailable', reason: 'test' } } satisfies WorkspaceCatalog);
  }));
  const authority = new WorkspaceAuthority(new HttpWorkspaceHost());
  server.client.setAttachmentAdmission(new WorkspaceSessionNavigation(authority, server.client, server.client.navigation).admit);
  const admitted = await server.client.admitAttachment('A');
  if (!admitted) throw new Error('fixture admission refused');
  const validations: Promise<boolean>[] = [];
  const proof: OperationAdmission = { current: admitted.current, validate: signal => { const work = admitted.validate(signal); validations.push(work); return work; } };
  stall = true;
  if (retirement === 'timeout') vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
  const baseline = server.requests.length;
  const sent = (method: string) => server.requests.slice(baseline).filter(row => row.request.method === method);
  const operations = Array.from({ length: 8 }, () => server.client.request({ method: 'session/summary', params: { session_id: 'A' } }, 'session_summary', undefined, proof));
  const refused = Promise.allSettled(operations);
  const live = () => stalled.filter(row => !row.signal.aborted).length;
  // Validation is bounded to two reservations; six waiting operations hold nothing.
  expect(stalled).toHaveLength(2);
  // Cancellation of already-running native work crosses the socket at once,
  // without any Host read settling or any validation deadline elapsing.
  const cancel = server.client.request({ method: 'turn/cancel', params: { target: server.client.target('B') } }, 'cancellation_accepted');
  expect(sent('turn/cancel')).toHaveLength(1);
  await expect(cancel).resolves.toMatchObject({ type: 'cancellation_accepted' });
  expect(sent('session/summary')).toHaveLength(0);
  return { server, authority, stalled, live, validations, refused, sent, unstall: () => { stall = false; }, replace: (id: string) => { hostId = id; } };
}
/** Exactly eight native RPC slots remain: a ninth read waits for one to settle. */
async function expectRpcCapacity(server: Server) {
  const before = server.requests.filter(row => row.request.method === 'session/summary').length;
  server.held.add('session/summary');
  const reads = Array.from({ length: 9 }, () => server.client.request({ method: 'session/summary', params: { session_id: 'A' } }, 'session_summary'));
  await server.waitFor('session/summary', before + 8);
  const held = server.requests.filter(row => row.request.method === 'session/summary').slice(before);
  expect(held).toHaveLength(8);
  server.held.delete('session/summary');
  for (const row of held) server.reply(row.request);
  await Promise.all(reads);
  expect(server.requests.filter(row => row.request.method === 'session/summary')).toHaveLength(before + 9);
}

it.each(['navigation', 'host', 'timeout'] as const)('%s retirement of stalled final validations never blocks cancellation; late Host answers send nothing', async retirement => {
  const f = await stalledFinalValidations(retirement);
  try {
    if (retirement === 'navigation') f.server.client.navigation.invalidate();
    else if (retirement === 'host') { f.unstall(); f.replace('replacement-host'); await f.authority.observe(); }
    else {
      // The deadline starts at reservation, not at socket send. Each expiry
      // releases its reservation exactly once, admitting exactly one successor.
      for (let round = 1; round <= 4; round++) {
        await vi.advanceTimersByTimeAsync(29_999);
        expect(f.stalled).toHaveLength(2 * round); expect(f.live()).toBe(2);
        await vi.advanceTimersByTimeAsync(1);
        expect(f.stalled).toHaveLength(Math.min(8, 2 * round + 2)); expect(f.live()).toBe(round < 4 ? 2 : 0);
      }
      vi.useRealTimers();
    }
    for (const result of await f.refused) {
      expect(result.status).toBe('rejected');
      expect(String((result as PromiseRejectedResult).reason)).toContain(retirement === 'timeout' ? 'timed out' : 'Authority changed before dispatch');
    }
    // Queued operations whose proof retired were refused without ever reading the Host.
    expect(f.stalled).toHaveLength(retirement === 'timeout' ? 8 : 2);
    expect(f.stalled.every(row => row.signal.aborted)).toBe(true);
    // Late Host answers (still authorizing after a timeout) cannot send or release again.
    f.unstall(); f.stalled.forEach(row => row.release());
    await Promise.allSettled(f.validations);
    expect(f.sent('session/summary')).toHaveLength(0);
    expect(f.server.client.getSnapshot().uncertain).toEqual([]);
    await expectRpcCapacity(f.server);
    // Fresh admission is unaffected and still dispatches through final validation.
    await f.server.client.attach('A');
    expect(f.sent('session/attach')).toHaveLength(1);
  } finally { vi.useRealTimers(); f.server.client.disconnect(); }
});

it('stalled validations reserve their RPC slots, so ordinary native reads never burst past capacity', async () => {
  const f = await stalledFinalValidations('navigation');
  try {
    f.server.held.add('session/summary');
    const before = f.server.requests.filter(row => row.request.method === 'session/summary').length;
    const reads = Array.from({ length: 7 }, () => f.server.client.request({ method: 'session/summary', params: { session_id: 'A' } }, 'session_summary'));
    // Two validation reservations + six native reads = the eight-slot bound.
    expect(f.sent('session/summary')).toHaveLength(6);
    f.server.held.delete('session/summary');
    f.server.reply(f.sent('session/summary')[0].request);
    await f.server.waitFor('session/summary', before + 7);
    expect(f.sent('session/summary')).toHaveLength(7);
    expect(f.live()).toBe(2);
    f.server.client.navigation.invalidate();
    for (const row of f.sent('session/summary').slice(1)) f.server.reply(row.request);
    await Promise.all(reads);
    for (const result of await f.refused) expect(result.status).toBe('rejected');
    expect(f.server.client.getSnapshot().uncertain).toEqual([]);
    await expectRpcCapacity(f.server);
  } finally { f.server.client.disconnect(); }
});

it('a cancelled final validation settles once; its late authorizing answer sends nothing', async () => {
  const server = new Server(); await server.connect();
  let live = true;
  const late = deferred<boolean>();
  const proof: OperationAdmission = { current: () => live, validate: () => late.promise };
  const settled = vi.fn();
  try {
    const operation = server.client.request({ method: 'session/summary', params: { session_id: 'A' } }, 'session_summary', undefined, proof);
    void operation.then(settled, settled);
    live = false; server.client.navigation.invalidate();
    await expect(operation).rejects.toThrow('Authority changed before dispatch');
    live = true; late.resolve(true); await late.promise; await Promise.resolve();
    expect(settled).toHaveBeenCalledTimes(1);
    expect(server.requests.filter(row => row.request.method === 'session/summary')).toHaveLength(0);
    await expectRpcCapacity(server);
  } finally { server.client.disconnect(); }
});

it.each([['another endpoint', 'ws://127.0.0.1:9090/', false], ['a normalized-equal endpoint', 'ws://127.0.0.1:8080', true]] as const)('the same Host process reporting %s at final validation', async (_, reported, dispatches) => {
  const server = new Server(); await server.connect();
  let catalogEndpoint: string = endpoint, lists = 0;
  const entered = deferred<void>(), release = deferred<void>();
  const classified: string[] = [];
  vi.stubGlobal('fetch', vi.fn(async (url: string, init: RequestInit) => {
    if (new URL(url).pathname.endsWith('/list')) {
      if (++lists === 2) { entered.resolve(); await release.promise; }
      return Response.json({ authorityId: 'host', endpoint: catalogEndpoint, workspaces: [], picker: { kind: 'unavailable', reason: 'test' } } satisfies WorkspaceCatalog);
    }
    classified.push(JSON.parse(init.body as string).endpoint);
    return Response.json([{ authorized: true }]);
  }));
  const authority = new WorkspaceAuthority(new HttpWorkspaceHost());
  server.client.setAttachmentAdmission(new WorkspaceSessionNavigation(authority, server.client, server.client.navigation).admit);
  try {
    const attach = server.client.attach('A');
    await entered.promise;
    const proof = authority.capture()!;
    catalogEndpoint = reported;
    release.resolve();
    if (dispatches) await attach;
    else await expect(attach).rejects.toThrow('Authority changed before dispatch');
    // Same authorityId either way; only the normalized endpoint decides the scope.
    expect(proof.current()).toBe(dispatches);
    expect(authority.getCatalog()).toMatchObject({ authorityId: 'host', endpoint: reported });
    // A foreign scope is never asked to classify native endpoint A.
    expect(classified).toEqual(dispatches ? [endpoint, endpoint] : [endpoint]);
    expect(server.requests.filter(row => row.request.method === 'session/attach')).toHaveLength(dispatches ? 1 : 0);
    expect(server.client.getSnapshot().uncertain).toEqual([]);
  } finally { server.client.disconnect(); }
});

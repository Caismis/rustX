// @vitest-environment node
import { createServer, type Server } from 'node:http';
import { afterEach, expect, it, vi } from 'vitest';
import { workspaceHandler } from '../host/http';
import { HttpWorkspaceHost } from '../src/workspaces/http-host';
import { FilePreviewCoordinator } from '../src/client/session-files';
import { WorkspaceAuthority } from '../src/workspaces/authority';
import type { AppServerClient } from '../src/client/app-server';
import { WorkspaceHostError, type ProductHostWorkspaces } from '../src/workspaces/host';
import type { DocumentRequest } from '../shared/documents';
function gate<T = void>() { let resolve!: (value: T) => void; const promise = new Promise<T>(r => { resolve = r; }); return { promise, resolve }; }
const scope = { authorityId: 'host', endpoint: 'ws://127.0.0.1:7777' };
const request: DocumentRequest = { target: { session_id: 'ses_00000000-0000-7000-8000-000000000001', conversation_id: 'conv_00000000-0000-7000-8000-000000000001', attachment_id: 'a', runtime_incarnation: '1' }, source: { kind: 'artifact', artifact_id: 'a' }, extension: 'docx' };
const result = { digest: 'a'.repeat(64), preview: { kind: 'pdf' as const, data: new TextEncoder().encode('%PDF') } };
let server: Server | undefined;
afterEach(async () => { vi.unstubAllGlobals(); if (server) { server.closeAllConnections(); await new Promise<void>(resolve => server!.close(() => resolve())); server = undefined; } });
async function serve(previewDocument: NonNullable<ProductHostWorkspaces['previewDocument']>, readDelivery?: ProductHostWorkspaces['readDelivery'], observation?: ProductHostWorkspaces['observeDelivery']) {
  const unexpected = async (): Promise<never> => { throw new Error('Unexpected Workspace metadata operation'); };
  const host: ProductHostWorkspaces = { previewDocument, readDelivery, observeDelivery: observation,
    listWorkspaces: async () => ({ ...scope, workspaces: [], picker: { kind: 'unavailable', reason: 'fixture' } }),
    adoptWorkspace: unexpected, renameWorkspace: unexpected, reorderWorkspace: unexpected, removeWorkspace: unexpected,
    resolveWorkspace: unexpected, classifyLocations: unexpected };
  const handler = workspaceHandler(host);
  server = createServer((req, res) => { void handler(req, res); });
  await new Promise<void>(resolve => server!.listen(0, '127.0.0.1', resolve));
  const address = server.address(); if (!address || typeof address === 'string') throw new Error('Missing server address');
  const origin = `http://127.0.0.1:${address.port}`;
  vi.stubGlobal('location', { href: origin, origin });
  return new HttpWorkspaceHost();
}

it.each(['before ready', 'after ready'] as const)('observation cancellation %s waits for real watcher retirement', async timing => {
  const entered = gate<AbortSignal>(), ready = gate(), aborted = gate(), physicalEnd = gate();
  let changed!: () => void;
  const host = await serve(async () => result, undefined, async (_scope, _read, _file, hint, signal) => {
    entered.resolve(signal); changed = hint;
    signal.addEventListener('abort', () => aborted.resolve(), { once: true });
    await ready.promise;
    return { version: 'v1', done: physicalEnd.promise, close: async () => physicalEnd.promise };
  });
  const controller = new AbortController(), hints = vi.fn(), processedReady = gate();
  const fetch = globalThis.fetch;
  vi.stubGlobal('fetch', async (input: string, init?: RequestInit) => {
    const response = await fetch(input, init);
    if (!input.endsWith('/file-observe')) return response;
    return { ok: response.ok, headers: response.headers, json: async () => { const value = await response.json(); processedReady.resolve(); return value; } };
  });
  const file = { scope: { conversation_id: 'conv_00000000-0000-7000-8000-000000000002', device: '1', inode: '2' }, path: 'a.txt', name: 'a.txt', mime_type: 'text/plain' };
  const work = host.observeDelivery(scope, { target: request.target, locator: { type: 'by_message_id', message_id: 'm' }, delivery_index: 0 }, file, hints, controller.signal);
  const terminal = vi.fn(); void work.then(terminal, terminal);
  const rejected = timing === 'before ready' ? expect(work).rejects.toThrow() : undefined;
  try {
    await entered.promise;
    if (timing === 'after ready') ready.resolve();
    const observation = timing === 'after ready' ? await work : undefined;
    controller.abort(); await aborted.promise;
    ready.resolve();
    // The ready frame traverses the HTTP carrier while teardown is still gated.
    // A hint cannot trigger publication after cancellation.
    changed(); expect(hints).not.toHaveBeenCalled();
    let retired = false; const retirement = observation?.close().then(() => { retired = true; });
    if (timing === 'before ready') { await processedReady.promise; await new Promise<void>(resolve => setImmediate(resolve)); expect(terminal).not.toHaveBeenCalled(); }
    else expect(retired).toBe(false);
    physicalEnd.resolve();
    if (rejected) await rejected; else { await retirement; expect(retired).toBe(true); }
  } finally { ready.resolve(); physicalEnd.resolve(); if (rejected) await rejected; else await Promise.allSettled([work]); }
});
it('eight target observations share one live stream and leave the file-read carrier usable', async () => {
  const probes = vi.fn(), releases: { resolve(): void }[] = [];
  const file = { scope: { conversation_id: 'conv_00000000-0000-7000-8000-000000000002', device: '1', inode: '2' }, path: 'a.txt', name: 'a.txt', mime_type: 'text/plain' };
  const host = await serve(async () => result, async () => { probes(); return { file, version: 'v1', data: new Uint8Array([1]), bytes: 1, offset: 0, eof: true }; },
    async (_scope, _read, _file, _changed, signal) => { const retired = gate(); releases.push(retired); signal.addEventListener('abort', () => retired.resolve(), { once: true }); return { version: 'v1', done: retired.promise, close: async () => retired.promise }; });
  const fetch = globalThis.fetch; let streams = 0;
  vi.stubGlobal('fetch', (input: string, init?: RequestInit) => { if (input.endsWith('/file-observe-events')) streams++; return fetch(input, init); });
  const read = { target: request.target, locator: { type: 'by_message_id' as const, message_id: 'm' }, delivery_index: 0 };
  const observations = [];
  try {
    for (let i = 0; i < 8; i++) observations.push(await host.observeDelivery(scope, read, file, () => {}, new AbortController().signal));
    expect(streams).toBe(1); await host.readDelivery(scope, read); expect(probes).toHaveBeenCalledOnce();
    await Promise.all(observations.map(observation => observation.close()));
    const next = await host.observeDelivery(scope, read, file, () => {}, new AbortController().signal); expect(streams).toBe(2); await next.close();
  } finally { releases.forEach(release => release.resolve()); await Promise.all(observations.map(observation => observation.close())); }
});
it('a canonical Unicode basename above 255 UTF-8 bytes crosses observation admission without a platform-specific byte cap', async () => {
  const retired = gate(), name = `${'水'.repeat(100)}.txt`;
  const file = { scope: { conversation_id: 'conv_00000000-0000-7000-8000-000000000002', device: '1', inode: '2' }, path: `sub/${name}`, name, mime_type: 'text/plain' };
  const observe = vi.fn<NonNullable<ProductHostWorkspaces['observeDelivery']>>(async (_scope, _read, _file, _changed, signal) => {
    signal.addEventListener('abort', () => retired.resolve(), { once: true });
    return { version: 'v1', done: retired.promise, close: async () => retired.resolve() };
  });
  const host = await serve(async () => result, undefined, observe), read = { target: request.target, locator: { type: 'by_message_id' as const, message_id: 'm' }, delivery_index: 0 };
  expect(Buffer.byteLength(name)).toBe(304);
  const observation = await host.observeDelivery(scope, read, file, () => {}, new AbortController().signal);
  try { expect(observe).toHaveBeenCalledExactlyOnceWith(scope, read, file, expect.any(Function), expect.any(AbortSignal)); expect(observation.version).toBe('v1'); }
  finally { await observation.close(); await observation.done; }
});

it('600 KiB file bytes cross the actual HTTP carrier as multipart without Base64 expansion', async () => {
  const data = Uint8Array.from({ length: 600 * 1024 }, (_, index) => index % 256);
  const file = { scope: { conversation_id: 'conv_00000000-0000-7000-8000-000000000002', device: '1', inode: '2' }, path: 'large.bin', name: 'large.bin', mime_type: 'application/octet-stream' };
  const host = await serve(async () => result, async () => ({ file, data, version: 'v1', bytes: data.length, offset: 0, eof: true }));
  const fetch = globalThis.fetch;
  let wire: Promise<ArrayBuffer> | undefined;
  vi.stubGlobal('fetch', async (input: string, init?: RequestInit) => {
    const response = await fetch(input, init);
    if (input.endsWith('/file-read')) {
      expect(response.headers.get('Content-Type')).toContain('multipart/form-data');
      wire = response.clone().arrayBuffer();
    }
    return response;
  });
  const read = await host.readDelivery(scope, { target: request.target, locator: { type: 'by_message_id', message_id: 'delivery' }, delivery_index: 0 });
  expect(read.data).toEqual(data);
  expect((await wire)!.byteLength).toBeLessThan(data.length + 4096);
});
it.each(['before header acknowledgement', 'after header acknowledgement'] as const)('exact cancellation %s retains admission through physical Host settlement', async timing => {
  const entered = gate<AbortSignal>(), aborted = gate(), settled = gate(), headers = gate<Response>(), deliverHeaders = gate();
  const host = await serve(async (_scope, _request, signal) => {
    entered.resolve(signal!); signal!.addEventListener('abort', () => aborted.resolve(), { once: true });
    await settled.promise; signal!.throwIfAborted(); return result;
  });
  const fetch = globalThis.fetch;
  vi.stubGlobal('fetch', async (input: string, init?: RequestInit) => {
    const response = await fetch(input, init);
    if (input.endsWith('/document-preview')) {
      headers.resolve(response);
      if (timing === 'before header acknowledgement') await deliverHeaders.promise;
    }
    return response;
  });
  const controller = new AbortController(), work = host.previewDocument(scope, request, controller.signal);
  const rejected = expect(work).rejects.toThrow(); let terminal = false; void work.then(() => { terminal = true; }, () => { terminal = true; });
  try {
    const signal = await entered.promise; await headers.promise;
    controller.abort();
    if (timing === 'before header acknowledgement') { expect(signal.aborted).toBe(false); deliverHeaders.resolve(); }
    await aborted.promise; expect(signal.aborted).toBe(true); expect(terminal).toBe(false);
    settled.resolve(); await rejected; expect(terminal).toBe(true);
  } finally { deliverHeaders.resolve(); settled.resolve(); await rejected; }
});
it('cancellation requires the exact operation token and authority scope; final response releases the sole carrier', async () => {
  const entered = gate<AbortSignal>(), settled = gate(); let first = true;
  const host = await serve(async (_scope, _request, signal) => {
    if (first) { first = false; entered.resolve(signal!); await settled.promise; }
    return result;
  });
  const fetch = globalThis.fetch, acknowledged = gate<Response>();
  vi.stubGlobal('fetch', async (input: string, init?: RequestInit) => {
    const response = await fetch(input, init); if (input.endsWith('/document-preview')) acknowledged.resolve(response); return response;
  });
  const work = host.previewDocument(scope, request);
  try {
    const signal = await entered.promise, response = await acknowledged.promise;
    const operationId = response.headers.get('X-Rustx-Document-Operation');
    const post = async (body: unknown) => fetch(`${location.origin}/product-host/document-cancel`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
    await post({ scope, operationId: 'wrong' }); expect(signal.aborted).toBe(false);
    await post({ scope: { ...scope, authorityId: 'other' }, operationId }); expect(signal.aborted).toBe(false);
    await expect(host.previewDocument(scope, request)).rejects.toThrow('capacity');
    settled.resolve(); expect(await work).toEqual(result);
    expect(await host.previewDocument(scope, request)).toEqual(result);
  } finally { settled.resolve(); await work; }
});
it.each(['lost body', 'invalid envelope'] as const)('%s is typed as unknown settlement instead of reusable admission', async failure => {
  vi.stubGlobal('location', { href: 'http://127.0.0.1', origin: 'http://127.0.0.1' });
  vi.stubGlobal('fetch', vi.fn(async (_input, init) => {
    const { operationId } = JSON.parse(init.body);
    return { ok: true, headers: new Headers({ 'X-Rustx-Document-Operation': operationId }), json: async () => { if (failure === 'invalid envelope') return {}; throw new Error('lost body'); } };
  }));
  await expect(new HttpWorkspaceHost().previewDocument(scope, request)).rejects.toMatchObject({ kind: 'document_settlement_unknown' });
});

it('two original reads keep their physical slots after abort; queued Download starts only when one settles', async () => {
  const firstEntered = gate(), secondEntered = gate(), thirdEntered = gate(), firstAborted = gate();
  const releases = [gate(), gate(), gate()]; let calls = 0;
  const file = { scope: { conversation_id: 'conv_00000000-0000-7000-8000-000000000002', device: '1', inode: '2' }, path: 'original.txt', name: 'original.txt', mime_type: 'text/plain' };
  const host = await serve(async () => result, async (_scope, _read, signal) => {
    const index = calls++; [firstEntered, secondEntered, thirdEntered][index].resolve();
    if (index === 0) signal!.addEventListener('abort', () => firstAborted.resolve(), { once: true });
    await releases[index].promise; signal!.throwIfAborted(); return { file, data: new TextEncoder().encode('original bytes'), bytes: 14, version: 'v1', offset: 0, eof: true };
  });
  const authority = new WorkspaceAuthority(host); await authority.observe();
  const client = { subscribe: () => () => {}, target: () => request.target, getSnapshot: () => ({ generation: 1, authorityRevision: 1, views: { [request.target.session_id]: { target: request.target } } }) } as unknown as AppServerClient;
  const coordinator = new FilePreviewCoordinator(client, request.target.session_id, host, authority);
  const source = { kind: 'session_file' as const, conversationId: request.target.conversation_id, locator: {type:'by_message_id' as const,message_id:'m'}, index: 0, file };
  const a = coordinator.acquire(1, source), b = coordinator.acquire(2, source);
  const clicked = vi.fn(), revoked = vi.fn(); let urls = 0;
  vi.stubGlobal('document', { createElement: () => ({ click: clicked }) });
  vi.stubGlobal('URL', Object.assign(URL, { createObjectURL: () => `blob:${++urls}`, revokeObjectURL: revoked }));
  const first = a.load(), rejected = expect(first).rejects.toThrow(), second = b.load();
  try {
    await firstEntered.promise; await secondEntered.promise;
    const download = coordinator.download(source, file.name);
    a.dispose(); await firstAborted.promise; expect(calls).toBe(2);
    releases[0].resolve(); await rejected; await thirdEntered.promise; expect(calls).toBe(3);
    releases[1].resolve(); releases[2].resolve(); await Promise.all([second, download]);
    expect(clicked).toHaveBeenCalledOnce(); expect(urls).toBe(2); expect(revoked).toHaveBeenCalledOnce();
    coordinator.dispose(); expect(revoked).toHaveBeenCalledTimes(2);
  } finally { releases.forEach(release => release.resolve()); coordinator.dispose(); await Promise.allSettled([first, second]); }
});


it.each(['converter', 'file', 'document'] as const)('canceled document carrier preserves %s physical uncertainty through the terminal envelope', async kind => {
  const entered = gate(), aborted = gate(), settle = gate();
  const host = await serve(async (_scope, _request, signal) => {
    entered.resolve(); signal!.addEventListener('abort', () => aborted.resolve(), { once: true });
    await settle.promise;
    throw new WorkspaceHostError('physical owner unknown', `${kind}_settlement_unknown`);
  });
  const abort = new AbortController(), work = host.previewDocument(scope, request, abort.signal);
  const rejected = expect(work).rejects.toMatchObject({ kind: `${kind}_settlement_unknown` });
  try {
    await entered.promise; abort.abort(); await aborted.promise;
    settle.resolve(); await rejected;
  } finally { settle.resolve(); await rejected; }
});

// @vitest-environment node
import { afterEach, expect, it, vi } from 'vitest';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { LocalWorkspaceHost } from '../host/workspaces';
import { openNativeDeliveryPreview } from '../host/file-read';
import { HttpWorkspaceHost } from '../src/workspaces/http-host';
import { workspaceHandler } from '../host/http';
import { createServer } from 'node:http';
const roots: string[] = [];
afterEach(() => { vi.unstubAllGlobals(); vi.useRealTimers(); for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true }); });
const identity = '00000000-0000-4000-8000-000000000001';
const reference = { scope: { conversation_id: 'conv_00000000-0000-7000-8000-000000000001', device: '1', inode: '2' }, path: 'part.stl', name: 'part.stl', description: null, mime_type: 'application/octet-stream' };
const read = { target: { session_id: 'ses_00000000-0000-7000-8000-000000000001', conversation_id: reference.scope.conversation_id, runtime_incarnation: '1', attachment_id: 'attachment' }, locator: { type: 'by_message_id' as const, message_id: 'canonical-result' }, delivery_index: 0 };
class Socket extends EventTarget {
  static current: Socket;
  sent: Record<string, any>[] = [];
  closed = false;
  constructor() { super(); Socket.current = this; }
  send(payload: string) { this.sent.push(JSON.parse(payload)); }
  close() { this.closed = true; }
  opened() { this.dispatchEvent(new Event('open')); }
  terminal(wasClean = true) { this.dispatchEvent(Object.assign(new Event('close'), { wasClean })); }
  json(id: number, result: unknown) { this.dispatchEvent(new MessageEvent('message', { data: JSON.stringify({ jsonrpc: '2.0', id, result }) })); }
  page(id: number, offset: number, bytes: number, data: number[]) {
    this.json(id, { type: 'session_file_bytes', file: reference, version: 'v1', bytes, offset, eof: offset + data.length === bytes, length: data.length });
    this.dispatchEvent(new MessageEvent('message', { data: Uint8Array.from(data).buffer }));
  }
}
async function native(signal = new AbortController().signal) {
  vi.stubGlobal('WebSocket', Socket);
  const opening = openNativeDeliveryPreview('ws://localhost:1234/', 'private', read, ['/root'], signal);
  const socket = Socket.current; socket.opened();
  expect(socket.sent[0]).toMatchObject({ operation: 'open', request: { roots: ['/root'], source: { kind: 'session_file' } } });
  socket.json(0, { identity, version: 'v1', bytes: 6, file: reference });
  return { operation: await opening, socket };
}
it('reuses one native connection for two in-flight range ids and final secure validation', async () => {
  const { operation, socket } = await native();
  const first = operation.read({ offset: 0, length: 3 }), second = operation.read({ offset: 3, length: 3 });
  expect(socket.sent.slice(1)).toEqual([
    { operation: 'read', identity, id: 1, range: { offset: 0, length: 3 } },
    { operation: 'read', identity, id: 2, range: { offset: 3, length: 3 } },
  ]);
  socket.page(2, 3, 6, [4, 5, 6]); socket.page(1, 0, 6, [1, 2, 3]);
  expect((await first).data).toEqual(Uint8Array.of(1, 2, 3)); expect((await second).data).toEqual(Uint8Array.of(4, 5, 6));
  const closing = operation.close(); expect(socket.sent.at(-1)).toEqual({ operation: 'close', identity });
  socket.json(0, { closed: true }); socket.terminal(); await closing;
});
it.each([true, false])('cancellation retains both native reads until physical close acknowledgement (clean=%s)', async clean => {
  const abort = new AbortController(), { operation, socket } = await native(abort.signal);
  const a = operation.read({ offset: 0, length: 3 }), b = operation.read({ offset: 3, length: 3 });
  let settled = 0; void a.catch(() => { settled++; }); void b.catch(() => { settled++; });
  abort.abort(); await Promise.resolve(); expect(socket.closed).toBe(true); expect(settled).toBe(0);
  const closing = operation.close(); socket.terminal(clean);
  const failures = await Promise.allSettled([a, b, closing]);
  expect(failures.every(value => value.status === 'rejected')).toBe(true);
  if (!clean) failures.forEach(value => { if (value.status === 'rejected') expect(value.reason.kind).toBe('file_settlement_unknown'); });
  else failures.forEach(value => { if (value.status === 'rejected') expect(value.reason.kind).not.toBe('file_settlement_unknown'); });
});
async function workspace() {
  const root = mkdtempSync(join(tmpdir(), 'rustx-preview-operation-')); roots.push(root); writeFileSync(join(root, 'part.stl'), '0123456789');
  const host = new LocalWorkspaceHost({ endpoint: 'ws://localhost:1234/', metadataFile: join(root, 'registry.json'), picker: false, roots: [{ id: 'root', cwd: root, displayName: 'Root' }] });
  const scope = await host.listWorkspaces(), target = { workspace_id: scope.workspaces[0].id };
  const call = { target, request: { kind: 'preview' as const, path: 'part.stl', read: { kind: 'bytes' as const } } };
  return { root, host, scope, call };
}
it('bounds idle owners independently and permits Download while both preview owners are idle', async () => {
  const { host, scope, call } = await workspace();
  const [a, b] = await Promise.all([host.openWorkspacePreview(scope, call, new AbortController().signal), host.openWorkspacePreview(scope, call, new AbortController().signal)]);
  await expect(host.openWorkspacePreview(scope, call, new AbortController().signal)).rejects.toMatchObject({ kind: 'capacity' });
  expect((await host.workbench(scope, call)).preview?.bytes).toBe(10);
  await Promise.all([a.close(), b.close()]); await host.close();
});
it('registration retirement closes an idle operation and makes source reads obsolete', async () => {
  const { host, scope, call } = await workspace();
  const operation = await host.openWorkspacePreview(scope, call, new AbortController().signal);
  await host.removeWorkspace(scope, scope.workspaces[0].id);
  await expect(operation.read({ offset: 0, length: 1 })).rejects.toThrow();
  await expect(operation.close()).rejects.toThrow(); await host.close();
});
it('absolute source deadline retires idle owners without extending on page activity', async () => {
  const { host, scope, call } = await workspace(); vi.useFakeTimers();
  const operation = await host.openWorkspacePreview(scope, call, new AbortController().signal);
  await vi.advanceTimersByTimeAsync(60_000);
  await expect(operation.read({ offset: 0, length: 1 })).rejects.toThrow();
  await expect(operation.close()).rejects.toThrow(); vi.useRealTimers(); await host.close();
});
it('HTTP open/read/close retains a server-owned identity and rejects unknown ids without poisoning reads', async () => {
  const { host, scope, call } = await workspace(), handler = workspaceHandler(host);
  const server = createServer((request, response) => { void handler(request, response); });
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const address = server.address(); if (!address || typeof address === 'string') throw new Error('No server address');
  vi.stubGlobal('location', new URL(`http://127.0.0.1:${address.port}/`));
  const adapter = new HttpWorkspaceHost(`http://127.0.0.1:${address.port}/product-host`);
  try {
    const operation = await adapter.openWorkspacePreview(scope, call, new AbortController().signal);
    const [a, b] = await Promise.all([operation.read({ offset: 0, length: 3 }), operation.read({ offset: 3, length: 3 })]);
    expect(new TextDecoder().decode(a.data)).toBe('012'); expect(new TextDecoder().decode(b.data)).toBe('345');
    await operation.close();
    const obsolete = await fetch(`http://127.0.0.1:${address.port}/product-host/preview-close`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ scope, operationId: identity, identity: 'unknown' }) });
    expect(obsolete.status).toBe(400); expect((await obsolete.json()).kind).not.toBe('file_settlement_unknown');
    const next = await adapter.openWorkspacePreview(scope, call, new AbortController().signal); expect(next.bytes).toBe(10); await next.close();
  } finally { await host.close(); server.closeAllConnections(); await new Promise<void>(resolve => server.close(() => resolve())); }
});


it('canceled open responses retire already opened descriptors immediately and permit both owner slots to be reused', async () => {
  const { host, scope, call } = await workspace(), handler = workspaceHandler(host);
  let opened!: () => void, release!: () => void;
  const admitted = new Promise<void>(yes => { opened = yes; }), gate = new Promise<void>(yes => { release = yes; });
  const open = host.openWorkspacePreview.bind(host);
  vi.spyOn(host, 'openWorkspacePreview').mockImplementationOnce(async (...args) => {
    const operation = await open(...args); opened(); await gate; return operation;
  });
  const server = createServer((request, response) => { void handler(request, response); });
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const address = server.address(); if (!address || typeof address === 'string') throw new Error('No server address');
  const base = `http://127.0.0.1:${address.port}/product-host`;
  const post = (method: string, body: unknown) => fetch(`${base}/${method}`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  try {
    const response = await post('preview-open', { scope, operationId: identity, source: 'workspace', call });
    await admitted;
    await post('file-cancel', { scope, operationId: identity }); release(); await response.arrayBuffer();
    vi.stubGlobal('location', new URL(`http://127.0.0.1:${address.port}/`)); const adapter = new HttpWorkspaceHost(base);
    const [a, b] = await Promise.all([adapter.openWorkspacePreview(scope, call, new AbortController().signal), adapter.openWorkspacePreview(scope, call, new AbortController().signal)]);
    await Promise.all([a.close(), b.close()]);
  } finally { release(); await host.close(); server.closeAllConnections(); await new Promise<void>(resolve => server.close(() => resolve())); }
});


it.each([true, false])('Host retains two page permits until native retirement and reports physical uncertainty (clean=%s)', async clean => {
  const { host, scope, call } = await workspace(), abort = new AbortController(); vi.stubGlobal('WebSocket', Socket);
  // Supply native access through a separate Host configuration with the same real root.
  const root = roots.at(-1)!;
  const nativeHost = new LocalWorkspaceHost({ endpoint: scope.endpoint, productHostToken: 'private', metadataFile: join(root, 'native-registry.json'), picker: false, roots: [{ id: 'root', cwd: root, displayName: 'Root' }] });
  const nativeScope = await nativeHost.listWorkspaces(), workspaceCall = { ...call, target: { workspace_id: nativeScope.workspaces[0].id } };
  const opening = nativeHost.openDeliveryPreview(nativeScope, read, abort.signal), socket = Socket.current; socket.opened(); socket.json(0, { identity, version: 'v1', bytes: 6, file: reference });
  const operation = await opening;
  const a = operation.read({ offset: 0, length: 3 }), b = operation.read({ offset: 3, length: 3 });
  const failures = [expect(a).rejects.toThrow(), expect(b).rejects.toThrow()];
  abort.abort();
  await expect(nativeHost.workbench(nativeScope, workspaceCall)).rejects.toMatchObject({ kind: 'capacity' });
  socket.terminal(clean); await Promise.all(failures); await expect(operation.close()).rejects.toThrow();
  if (clean) {
    expect((await nativeHost.workbench(nativeScope, workspaceCall)).preview?.bytes).toBe(10); await nativeHost.close();
  } else {
    await expect(nativeHost.workbench(nativeScope, workspaceCall)).rejects.toMatchObject({ kind: 'capacity' });
    await expect(nativeHost.close()).rejects.toBeInstanceOf(AggregateError);
  }
  await host.close();
});

it('an unclean idle cancellation has no unknown physical read to poison', async () => {
  const abort = new AbortController(), { operation, socket } = await native(abort.signal);
  abort.abort(); const closing = operation.close(); socket.terminal(false);
  await expect(closing).rejects.not.toMatchObject({ kind: 'file_settlement_unknown' });
});


it('cancellation during final native validation waits for its physical retirement acknowledgement', async () => {
  const abort = new AbortController(), { operation, socket } = await native(abort.signal);
  let settled = false;
  const closing = operation.close().finally(() => { settled = true; }); const failure = expect(closing).rejects.toThrow('revoked');
  expect(socket.sent.at(-1)).toEqual({ operation: 'close', identity }); abort.abort(); await Promise.resolve(); expect(settled).toBe(false);
  socket.terminal(); await failure;
});


it('a valid native failure is a settlement witness even if its later clean close is lost', async () => {
  const { operation, socket } = await native();
  const reading = operation.read({ offset: 0, length: 3 });
  const failure = expect(reading).rejects.toHaveProperty('error.data.reason', 'replaced');
  const closing = operation.close(), retired = expect(closing).rejects.not.toMatchObject({ kind: 'file_settlement_unknown' });
  socket.dispatchEvent(new MessageEvent('message', { data: JSON.stringify({ jsonrpc: '2.0', id: 1, error: { code: -32000, message: 'Changed source', data: { kind: 'session_file_read', reason: 'replaced' } } }) }));
  await failure; socket.terminal(false); await retired;
});


it('normal native close drains in-flight range ids before accepting final validation', async () => {
  const { operation, socket } = await native();
  const a = operation.read({ offset: 0, length: 3 }), b = operation.read({ offset: 3, length: 3 });
  let settled = false; const closing = operation.close().then(() => { settled = true; });
  expect(socket.sent.at(-1)).toEqual({ operation: 'close', identity });
  socket.page(1, 0, 6, [1, 2, 3]); socket.page(2, 3, 6, [4, 5, 6]); await Promise.all([a, b]);
  expect(settled).toBe(false); socket.json(0, { closed: true }); socket.terminal(); await closing;
});


it('late canceled successful opens retire their server identities before both source owner slots are reused', async () => {
  const { host, scope, call } = await workspace(), handler = workspaceHandler(host);
  const server = createServer((request, response) => { void handler(request, response); });
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const address = server.address(); if (!address || typeof address === 'string') throw new Error('No server address');
  const base = `http://127.0.0.1:${address.port}/product-host`; vi.stubGlobal('location', new URL(`http://127.0.0.1:${address.port}/`));
  const adapter = new HttpWorkspaceHost(base), fetcher = globalThis.fetch;
  const buffered: (() => void)[] = []; let hold = true;
  vi.stubGlobal('fetch', async (input: RequestInfo | URL, init?: RequestInit) => {
    const response = await fetcher(input, init);
    if (!hold || !String(input).endsWith('/preview-open') || !response.ok) return response;
    // The Host has sent a physical success and forgotten the per-call cancel
    // id. Delay its terminal body at the browser carrier, retaining the opaque
    // source identity that must survive an intervening caller cancellation.
    const body = await response.arrayBuffer();
    await new Promise<void>(release => { buffered.push(release); });
    return new Response(body, { status: response.status, headers: response.headers });
  });
  try {
    for (let round = 0; round < 2; round++) {
      const a = new AbortController(), b = new AbortController();
      const opening = [adapter.openWorkspacePreview(scope, call, a.signal), adapter.openWorkspacePreview(scope, call, b.signal)];
      const rejected = opening.map(value => expect(value).rejects.toMatchObject({ name: 'AbortError' }));
      await vi.waitFor(() => { expect(buffered).toHaveLength((round + 1) * 2); });
      a.abort(); b.abort(); buffered.slice(round * 2).forEach(release => release()); await Promise.all(rejected);
      hold = false;
      const [first, second] = await Promise.all([adapter.openWorkspacePreview(scope, call, new AbortController().signal), adapter.openWorkspacePreview(scope, call, new AbortController().signal)]);
      expect((await first.read({ offset: 0, length: 3 })).data).toEqual(new TextEncoder().encode('012'));
      await Promise.all([first.close(), second.close()]); hold = true;
    }
  } finally { buffered.forEach(release => release()); await host.close(); server.closeAllConnections(); await new Promise<void>(resolve => server.close(() => resolve())); }
});

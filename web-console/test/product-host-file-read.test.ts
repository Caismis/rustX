// @vitest-environment node
/** Product Host policy/lifetime tests. Native credential enforcement is covered
 * by the real Rust socket regression and browser acceptance, not this carrier. */
import { afterEach, expect, it, vi } from 'vitest';
import { mkdtempSync, mkdirSync, realpathSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { LocalWorkspaceHost } from '../host/workspaces';
const directories: string[] = [];
afterEach(() => { vi.unstubAllGlobals(); directories.splice(0).forEach(path => rmSync(path, { recursive: true, force: true })); });

class GatedSocket extends EventTarget {
  static instances: GatedSocket[] = [];
  payload?: { roots: string[] };
  closed = false;
  closeCalls = 0;
  constructor(readonly url: URL, readonly protocols: string[]) { super(); GatedSocket.instances.push(this); }
  send(payload: string) { this.payload = JSON.parse(payload); }
  close() { this.closed = true; this.closeCalls++; }
  open() { this.dispatchEvent(new Event('open')); }
  acknowledge(wasClean = true) { this.dispatchEvent(Object.assign(new Event('close'), { wasClean, code: wasClean ? 1000 : 1006 })); }
  complete() { this.dispatchEvent(new MessageEvent('message', { data: JSON.stringify({ jsonrpc: '2.0', id: 0, result: { type: 'session_file_bytes', file: {}, data: 'aGk=' } }) })); }
}
function fixture() {
  const directory = mkdtempSync(join(tmpdir(), 'host-file-policy-')); directories.push(directory);
  const root = join(directory, 'registered'); mkdirSync(root);
  const other = join(directory, 'possible'); mkdirSync(other);
  GatedSocket.instances = []; vi.stubGlobal('WebSocket', GatedSocket);
  const host = new LocalWorkspaceHost({ endpoint: 'ws://127.0.0.1:1234/', transportToken: 'browser-token', productHostToken: 'private-token', metadataFile: join(directory, 'registrations.json'), picker: true, roots: [{ id: 'a', cwd: root, displayName: 'A' }, { id: 'b', cwd: other, displayName: 'B' }] });
  const read = { target: { session_id: 'session', conversation_id: 'view', runtime_incarnation: '1', attachment_id: 'attachment' }, message_id: 'canonical-result', delivery_index: 0 };
  return { host, read, root: realpathSync(root), other: realpathSync(other) };
}
it('only registrations authorize files; picker/classification roots alone do not', async () => {
  const { host, read, root, other } = fixture();
  const scope = await host.listWorkspaces();
  await host.removeWorkspace(scope, scope.workspaces[1].id);
  expect(await host.classifyLocations([other], scope.endpoint)).toEqual([{ authorized: true }]);
  const pending = host.readDelivery(scope, read); const socket = GatedSocket.instances[0]; socket.open();
  expect(socket.payload?.roots).toEqual([root]);
  expect(socket.protocols).toEqual(['rustx.product-host.file-read.v2', 'rustx-product-host.private-token']);
  expect(socket.url.href).toBe('ws://127.0.0.1:1234/product-host/file-read');
  expect(JSON.stringify(socket.payload)).not.toContain('private-token');
  socket.complete(); expect((await pending).data).toBe('aGk='); expect(socket.closed).toBe(true);
  await host.removeWorkspace(scope, scope.workspaces[0].id);
  await expect(host.readDelivery(scope, read)).rejects.toThrow('roots unavailable');
  expect(GatedSocket.instances).toHaveLength(1);
  await host.adoptWorkspace(scope, 'a');
  const adopted = host.readDelivery(scope, read); const next = GatedSocket.instances[1]; next.open(); expect(next.payload?.roots).toEqual([root]); next.complete(); await adopted;
  host.close();
});
it.each(['registration removal', 'host close', 'caller abort'] as const)('%s retires a gated native read and ignores its later bytes', async action => {
  const { host, read } = fixture(); const scope = await host.listWorkspaces(); const abort = new AbortController();
  const pending = host.readDelivery(scope, read, abort.signal); const rejected = expect(pending).rejects.toThrow('revoked');
  const socket = GatedSocket.instances[0]; socket.open(); expect(socket.payload).toBeDefined();
  if (action === 'registration removal') await host.removeWorkspace(scope, scope.workspaces[0].id);
  else if (action === 'host close') host.close(); else abort.abort();
  expect(socket.closed).toBe(true); socket.complete(); await rejected;
  host.close();
});
it('browser paths/root lists and replaced scopes fail before socket admission', async () => {
  const { host, read } = fixture(); const scope = await host.listWorkspaces();
  await expect(host.readDelivery(scope, { ...read, roots: ['/'] } as typeof read)).rejects.toThrow('coordinates');
  await expect(host.readDelivery({ ...scope, authorityId: 'other' }, read)).rejects.toThrow('authority replaced');
  expect(GatedSocket.instances).toHaveLength(0); host.close();
});
it('advanced admission rejects capacity before a second native read and revocation cancels the owned operation', async () => {
  const { host, read } = fixture(), scope = await host.listWorkspaces();
  const request = { target: read.target, source: { kind: 'session_file' as const, message_id: read.message_id, delivery_index: read.delivery_index }, extension: 'xlsx' as const, digest: '0'.repeat(64) };
  const work = host.previewDocument(scope, request), rejected = expect(work).rejects.toThrow('revoked');
  await expect(host.previewDocument(scope, request)).rejects.toThrow('capacity');
  expect(GatedSocket.instances).toHaveLength(1);
  const socket = GatedSocket.instances[0]; socket.open();
  host.close(); expect(socket.closed).toBe(true); socket.acknowledge(); await rejected;
});
it.each([
  ['missing', 'source_missing'], ['unauthorized', 'authorization_revoked'], ['unavailable', 'source_unavailable'],
  ['replaced', 'source_changed'], ['too_large', 'too_large'], ['capacity', 'capacity'],
])('advanced reauthorization preserves the actionable %s category without native diagnostics', async (reason, code) => {
  const { host, read } = fixture(), scope = await host.listWorkspaces();
  const request = { target: read.target, source: { kind: 'session_file' as const, message_id: read.message_id, delivery_index: read.delivery_index }, extension: 'xlsx' as const, digest: '0'.repeat(64) };
  const work = host.previewDocument(scope, request), rejected = expect(work).rejects.toThrow(code);
  const socket = GatedSocket.instances[0]; socket.open();
  socket.dispatchEvent(new MessageEvent('message', { data: JSON.stringify({ jsonrpc: '2.0', id: 0, error: { code: -32000, message: '/private/native/detail', data: { kind: 'session_file_read', reason } } }) }));
  await rejected; expect(socket.closed).toBe(true); host.close();
});

it('two dispatched read slots remain occupied after caller abort until an exact clean native close acknowledgement', async () => {
  const { host, read } = fixture(), scope = await host.listWorkspaces();
  const first = new AbortController(), second = new AbortController();
  const one = host.readDelivery(scope, read, first.signal), two = host.readDelivery(scope, read, second.signal);
  const oneRejected = expect(one).rejects.toThrow('revoked'), twoRejected = expect(two).rejects.toThrow('revoked');
  const [a, b] = GatedSocket.instances; a.open(); b.open(); first.abort(); second.abort();
  expect(a.closed).toBe(true); expect(b.closed).toBe(true);
  await expect(host.readDelivery(scope, read)).rejects.toThrow('capacity');
  a.acknowledge(); await oneRejected;
  const third = host.readDelivery(scope, read), c = GatedSocket.instances[2]; c.open();
  await expect(host.readDelivery(scope, read)).rejects.toThrow('capacity');
  c.complete(); await third; b.acknowledge(); await twoRejected;
  expect([a.closeCalls, b.closeCalls, c.closeCalls]).toEqual([1, 1, 1]); host.close();
});

it.each(['error', 'abnormal close'] as const)('post-dispatch %s preserves unknown physical ownership even after cancellation', async terminal => {
  const { host, read } = fixture(), scope = await host.listWorkspaces();
  for (let index = 0; index < 2; index++) {
    const abort = new AbortController(), work = host.readDelivery(scope, read, abort.signal);
    const rejected = expect(work).rejects.toMatchObject({ kind: 'file_settlement_unknown' });
    const socket = GatedSocket.instances[index]; socket.open(); abort.abort();
    if (terminal === 'error') socket.dispatchEvent(new Event('error')); else socket.acknowledge(false);
    await rejected; socket.acknowledge(); socket.complete();
  }
  await expect(host.readDelivery(scope, read)).rejects.toThrow('capacity');
  expect(GatedSocket.instances).toHaveLength(2); host.close();
});

it.each(['abort', 'error'] as const)('pre-dispatch %s releases admission without sending a native payload', async terminal => {
  const { host, read } = fixture(), scope = await host.listWorkspaces();
  const abort = new AbortController(), work = host.readDelivery(scope, read, abort.signal);
  const rejected = expect(work).rejects.toThrow(terminal === 'abort' ? 'revoked' : 'connection unavailable');
  const first = GatedSocket.instances[0];
  if (terminal === 'abort') abort.abort(); else first.dispatchEvent(new Event('error'));
  await rejected; first.open(); expect(first.payload).toBeUndefined();
  const one = host.readDelivery(scope, read), two = host.readDelivery(scope, read);
  const [a, b] = GatedSocket.instances.slice(1); a.open(); b.open(); a.complete(); b.complete();
  await Promise.all([one, two]); host.close();
});

it.each(['session_file', 'artifact'] as const)('unknown original %s settlement retains the document slot and is not hidden by caller abort', async kind => {
  const { host, read } = fixture(), scope = await host.listWorkspaces(), abort = new AbortController();
  const source = kind === 'artifact' ? { kind, artifact_id: 'original' } : { kind, message_id: read.message_id, delivery_index: read.delivery_index };
  const request = { target: read.target, source, extension: 'xlsx' as const, digest: '0'.repeat(64) };
  const work = host.previewDocument(scope, request, abort.signal), rejected = expect(work).rejects.toMatchObject({ kind: 'file_settlement_unknown' });
  const socket = GatedSocket.instances[0]; socket.open(); abort.abort(); socket.acknowledge(false);
  await rejected; await expect(host.previewDocument(scope, request)).rejects.toThrow('capacity'); host.close();
});

it('malformed post-dispatch response requests retirement and waits for its clean acknowledgement', async () => {
  const { host, read } = fixture(), scope = await host.listWorkspaces();
  const one = host.readDelivery(scope, read), two = host.readDelivery(scope, read);
  const rejected = expect(one).rejects.toThrow('Invalid native file response');
  const [a, b] = GatedSocket.instances; a.open(); b.open();
  a.dispatchEvent(new MessageEvent('message', { data: JSON.stringify({ jsonrpc: '2.0', id: 99, result: { type: 'session_file_bytes', data: 'aGk=' } }) }));
  expect(a.closed).toBe(true); await expect(host.readDelivery(scope, read)).rejects.toThrow('capacity');
  a.acknowledge(); await rejected; b.complete(); await two; host.close();
});

it('a terminal response proves retirement without restoring publication after an earlier invalid response', async () => {
  const { host, read } = fixture(), scope = await host.listWorkspaces();
  const work = host.readDelivery(scope, read), rejected = expect(work).rejects.toThrow('Invalid native file response');
  const socket = GatedSocket.instances[0]; socket.open();
  socket.dispatchEvent(new MessageEvent('message', { data: 'invalid' })); socket.complete();
  await rejected; expect(socket.closeCalls).toBe(1);
  const one = host.readDelivery(scope, read), two = host.readDelivery(scope, read);
  const [a, b] = GatedSocket.instances.slice(1); a.open(); b.open(); a.complete(); b.complete();
  await Promise.all([one, two]); host.close();
});

it('a valid native error proves retirement after caller cancellation without exposing native diagnostics', async () => {
  const { host, read } = fixture(), scope = await host.listWorkspaces(), abort = new AbortController();
  const work = host.readDelivery(scope, read, abort.signal), rejected = expect(work).rejects.toThrow('revoked');
  const socket = GatedSocket.instances[0]; socket.open(); abort.abort();
  socket.dispatchEvent(new MessageEvent('message', { data: JSON.stringify({ jsonrpc: '2.0', id: 0, error: { code: -32000, message: '/private/native/detail' } }) }));
  await rejected; expect(socket.closeCalls).toBe(1);
  const one = host.readDelivery(scope, read), two = host.readDelivery(scope, read);
  const [a, b] = GatedSocket.instances.slice(1); a.open(); b.open(); a.complete(); b.complete();
  await Promise.all([one, two]); host.close();
});

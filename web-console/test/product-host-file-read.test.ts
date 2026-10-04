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
  constructor(readonly url: URL, readonly protocols: string[]) { super(); GatedSocket.instances.push(this); }
  send(payload: string) { this.payload = JSON.parse(payload); }
  close() { this.closed = true; }
  open() { this.dispatchEvent(new Event('open')); }
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
  host.close(); await rejected; expect(GatedSocket.instances[0].closed).toBe(true);
});
it.each([
  ['missing', 'source_missing'], ['unauthorized', 'authorization_revoked'], ['unavailable', 'source_unavailable'],
  ['replaced', 'source_changed'], ['too_large', 'too_large'], ['capacity', 'capacity'],
])('advanced reauthorization preserves the actionable %s category without native diagnostics', async (reason, code) => {
  const { host, read } = fixture(), scope = await host.listWorkspaces();
  const request = { target: read.target, source: { kind: 'session_file' as const, message_id: read.message_id, delivery_index: read.delivery_index }, extension: 'xlsx' as const, digest: '0'.repeat(64) };
  const work = host.previewDocument(scope, request), rejected = expect(work).rejects.toThrow(code);
  const socket = GatedSocket.instances[0]; socket.open();
  socket.dispatchEvent(new MessageEvent('message', { data: JSON.stringify({ error: { code: -32000, message: '/private/native/detail', data: { kind: 'session_file_read', reason } } }) }));
  await rejected; expect(socket.closed).toBe(true); host.close();
});

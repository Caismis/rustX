// @vitest-environment node
import { mkdtempSync, mkdirSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { expect, it, vi } from 'vitest';
const boundary = vi.hoisted(() => ({ observe: vi.fn(), read: vi.fn() }));
vi.mock('../host/native-file-observation.ts', () => ({ observeNativeDelivery: boundary.observe }));
vi.mock('../host/file-read.ts', async original => ({ ...await original<typeof import('../host/file-read.ts')>(), readNativeDelivery: boundary.read }));
import { LocalWorkspaceHost } from '../host/workspaces';
function gate<T = void>() { let resolve!: (value: T) => void; const promise = new Promise<T>(yes => { resolve = yes; }); return { promise, resolve }; }
it('post-ack root revalidation joins the admitted native observer before rejection and later reuse', async () => {
  const root = mkdtempSync(join(tmpdir(), 'rustx-delivery-observation-')), a = join(root, 'a'), b = join(root, 'b');
  mkdirSync(a); mkdirSync(b); writeFileSync(join(a, 'file.txt'), 'ok');
  const stat = statSync(a, { bigint: true }), file = { scope: { conversation_id: 'conv_00000000-0000-7000-8000-000000000002', device: String(stat.dev), inode: String(stat.ino) }, path: 'file.txt', name: 'file.txt', mime_type: 'text/plain' };
  const entered = gate(), acknowledge = gate(), closing = gate(), physicallyClosed = gate();
  let active = 0, ended = false;
  const close = vi.fn(async () => { closing.resolve(); await physicallyClosed.promise; active--; });
  boundary.observe.mockImplementationOnce(async () => { active++; entered.resolve(); await acknowledge.promise; return { file, version: 'v1', done: physicallyClosed.promise, close }; });
  const host = new LocalWorkspaceHost({ endpoint: 'ws://localhost:8080', nativeFilesystem: 'shared', productHostToken: 'private', roots: [{ id: 'a', cwd: a, displayName: 'A' }, { id: 'b', cwd: b, displayName: 'B' }], picker: false, metadataFile: join(root, 'registry.json') });
  const scope = await host.listWorkspaces(), read = { target: { session_id: 'ses_00000000-0000-7000-8000-000000000001', conversation_id: 'conv_00000000-0000-7000-8000-000000000001', attachment_id: 'a', runtime_incarnation: '1' }, locator: { type: 'by_message_id' as const, message_id: 'm' }, delivery_index: 0 };
  const work = host.observeDelivery(scope, read, file, () => {}, new AbortController().signal);
  const settlement = work.then(() => { ended = true; return 'settled'; }, () => { ended = true; return 'settled'; });
  try {
    await entered.promise; rmSync(b, { recursive: true }); acknowledge.resolve();
    expect(await Promise.race([closing.promise.then(() => 'closing'), settlement])).toBe('closing');
    expect(ended).toBe(false); expect(active).toBe(1); expect(close).toHaveBeenCalledOnce();
    physicallyClosed.resolve(); await expect(work).rejects.toThrow(); expect(active).toBe(0);
    mkdirSync(b);
    const retired = gate(); let successorActive = true;
    boundary.observe.mockResolvedValueOnce({ file, version: 'v1', done: retired.promise, close: async () => { successorActive = false; retired.resolve(); } });
    boundary.read.mockResolvedValue({ file, data: new Uint8Array([111]), version: 'v1', bytes: 2, offset: 0, eof: false });
    const successor = await host.observeDelivery(scope, read, file, () => {}, new AbortController().signal);
    await successor.close(); expect(successorActive).toBe(false); expect(boundary.observe).toHaveBeenCalledTimes(2);
  } finally { acknowledge.resolve(); physicallyClosed.resolve(); await settlement; await host.close(); rmSync(root, { recursive: true, force: true }); }
});

// @vitest-environment node
import { createHash } from 'node:crypto';
import { readFileSync, mkdtempSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { expect, it, vi } from 'vitest';
const boundary = vi.hoisted(() => ({ read: vi.fn(), sandbox: vi.fn(), cleanup: vi.fn() }));
vi.mock('../host/file-read.ts', async original => ({ ...await original<typeof import('../host/file-read.ts')>(), readNativeSource: boundary.read }));
vi.mock('../host/documents/office-sandbox.ts', () => ({ runOfficeSandbox: boundary.sandbox }));
vi.mock('node:fs/promises', async original => {
  const actual = await original<typeof import('node:fs/promises')>();
  return { ...actual, rm: async (...args: Parameters<typeof actual.rm>) => { await actual.rm(...args); await boundary.cleanup(); } };
});
import { OfficeSettlementError } from '../host/documents/office-cgroup.ts';
import { LocalWorkspaceHost } from '../host/workspaces.ts';
function gate() { let release!: () => void; const promise = new Promise<void>(r => { release = r; }); return { promise, release }; }
it.each(['cgroup', 'input cleanup'] as const)('%s failure: Host capacity spans converter physical settlement AND input cleanup; cancellation cannot publish and the next operation is admitted', async failure => {
  boundary.read.mockReset(); boundary.sandbox.mockReset(); boundary.cleanup.mockReset();
  const directory = mkdtempSync(join(tmpdir(), 'document-host-'));
  const host = new LocalWorkspaceHost({ endpoint: 'ws://127.0.0.1:1234/', transportToken: 'browser', productHostToken: 'private', metadataFile: join(directory, 'workspaces.json'), picker: true, roots: [{ id: 'a', cwd: directory, displayName: 'A' }] });
  const entered = gate(), closed = gate(), cleaning = gate(), cleaned = gate();
  const bytes = readFileSync(new URL('./fixtures/documents/sample.docx', import.meta.url));
  boundary.read.mockResolvedValue({ data: bytes.toString('base64') });
  boundary.sandbox.mockImplementationOnce(async () => { entered.release(); await closed.promise; return Buffer.from('%PDF-late'); }).mockResolvedValue(Buffer.from('%PDF-next'));
  boundary.cleanup.mockImplementationOnce(async () => { cleaning.release(); await cleaned.promise; });
  try {
    const scope = await host.listWorkspaces();
    const request = { target: { session_id: 's', conversation_id: 'c', runtime_incarnation: '1', attachment_id: 'a' }, source: { kind: 'artifact' as const, artifact_id: 'artifact_1' }, extension: 'docx' as const, digest: createHash('sha256').update(bytes).digest('hex') };
    const controller = new AbortController();
    const work = host.previewDocument(scope, request, controller.signal), rejected = expect(work).rejects.toThrow();
    await entered.promise; controller.abort();
    await expect(host.previewDocument(scope, request)).rejects.toThrow('capacity');
    closed.release(); await cleaning.promise;
    await expect(host.previewDocument(scope, request)).rejects.toThrow('capacity');
    cleaned.release(); await rejected;
    expect((await host.previewDocument(scope, request)).preview).toEqual({ kind: 'pdf', data: Buffer.from('%PDF-next').toString('base64') });
    expect(boundary.sandbox).toHaveBeenCalledTimes(2);
    if (failure === 'cgroup') boundary.sandbox.mockRejectedValueOnce(new OfficeSettlementError());
    else boundary.cleanup.mockRejectedValueOnce(new Error('input cleanup denied'));
    await expect(host.previewDocument(scope, request)).rejects.toThrow('converter_unavailable');
    await expect(host.previewDocument(scope, request)).rejects.toThrow('capacity');
  } finally { closed.release(); cleaned.release(); host.close(); rmSync(directory, { recursive: true, force: true }); }
});

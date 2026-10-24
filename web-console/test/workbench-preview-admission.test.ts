// @vitest-environment node
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, expect, it, vi } from 'vitest';
const boundary = vi.hoisted(() => ({ read: vi.fn(), derive: vi.fn() }));
vi.mock('../host/workspace-preview.ts', () => ({ readWorkspacePreview: boundary.read }));
vi.mock('../host/documents/operation.ts', () => ({ deriveWorkspaceOffice: boundary.derive, deriveDocument: vi.fn() }));
import { LocalWorkspaceHost } from '../host/workspaces';
import { OfficeSettlementError } from '../host/documents/office-cgroup';
const roots: string[] = [];
afterEach(() => { boundary.read.mockReset(); boundary.derive.mockReset(); for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true }); });
function gate<T = void>() { let resolve!: (value: T) => void; const promise = new Promise<T>(yes => { resolve = yes; }); return { promise, resolve }; }
async function fixture() {
  const root = mkdtempSync(join(tmpdir(), 'rustx-workbench-admission-')); roots.push(root);
  const host = new LocalWorkspaceHost({ endpoint: 'ws://localhost:8080', productHostToken: 'private', roots: [{ id: 'root', cwd: root, displayName: 'Root' }], picker: false, metadataFile: join(root, 'registry.json') });
  const scope = await host.listWorkspaces(), target = { workspace_id: scope.workspaces[0].id };
  return { host, scope, target };
}
const result = { data: new Uint8Array([1]), version: 'v1', offset: 0, bytes: 1, eof: true };
it('workspace byte reads retain both Host slots until actual read settlement after cancellation', async () => {
  const f = await fixture(), releases = [gate(), gate()], entered = [gate<AbortSignal>(), gate<AbortSignal>()];
  boundary.read.mockImplementation(async (_root, _path, _options, signal) => { const index = boundary.read.mock.calls.length - 1; entered[index].resolve(signal); await releases[index].promise; signal.throwIfAborted(); return result; });
  const call = { target: f.target, request: { kind: 'preview' as const, path: 'a.txt', read: { kind: 'bytes' as const } } };
  const abort = new AbortController(), a = f.host.workbench(f.scope, call, abort.signal), b = f.host.workbench(f.scope, call), rejected = expect(a).rejects.toThrow();
  try {
    await entered[0].promise; await entered[1].promise; abort.abort();
    expect((await entered[0].promise).aborted).toBe(true);
    await expect(f.host.workbench(f.scope, call)).rejects.toMatchObject({ kind: 'capacity' }); expect(boundary.read).toHaveBeenCalledTimes(2);
    releases[0].resolve(); await rejected; releases[1].resolve(); await b;
    boundary.read.mockResolvedValue(result); await f.host.workbench(f.scope, call); expect(boundary.read).toHaveBeenCalledTimes(3);
  } finally { releases.forEach(release => release.resolve()); await Promise.allSettled([a, b]); await f.host.close(); }
});
it('workspace and delivered Office share one Host derivation slot through physical retirement and uncertainty', async () => {
  const f = await fixture(), entered = gate<AbortSignal>(), retire = gate();
  boundary.derive.mockImplementationOnce(async (_extension, _read, signal) => { entered.resolve(signal); await retire.promise; signal.throwIfAborted(); });
  const call = { target: f.target, request: { kind: 'office' as const, path: 'a.docx' } }, abort = new AbortController();
  const work = f.host.workbench(f.scope, call, abort.signal), rejected = expect(work).rejects.toThrow();
  try {
    await entered.promise; abort.abort();
    await expect(f.host.workbench(f.scope, call)).rejects.toThrow('capacity');
    await expect(f.host.previewDocument(f.scope, { target: { session_id: 's', conversation_id: 'c', attachment_id: 'a', runtime_incarnation: 'r' }, source: { kind: 'artifact', artifact_id: 'a' }, extension: 'docx' })).rejects.toThrow('capacity');
    expect(boundary.derive).toHaveBeenCalledOnce(); retire.resolve(); await rejected;
    boundary.derive.mockRejectedValueOnce(new OfficeSettlementError());
    await expect(f.host.workbench(f.scope, call)).rejects.toMatchObject({ kind: 'converter_settlement_unknown' });
    await expect(f.host.workbench(f.scope, call)).rejects.toThrow('capacity');
    boundary.read.mockResolvedValue(result);
    await f.host.workbench(f.scope, { target: f.target, request: { kind: 'preview', path: 'a.txt', read: { kind: 'bytes' } } }); expect(boundary.read).toHaveBeenCalledOnce();
  } finally { retire.resolve(); await rejected; await f.host.close(); }
});

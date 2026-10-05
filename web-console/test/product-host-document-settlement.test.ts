// @vitest-environment node
import { createHash } from 'node:crypto';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { afterEach, expect, it, vi } from 'vitest';
import { LocalWorkspaceHost } from '../host/workspaces';
import { OfficeSettlementError } from '../host/documents/office-cgroup';
import { WorkspaceHostError } from '../src/workspaces/host';

const boundary = vi.hoisted(() => ({ read: vi.fn(), convert: vi.fn() }));
vi.mock('../host/file-read', async original => ({ ...await original<typeof import('../host/file-read')>(), readNativeDelivery: boundary.read }));
vi.mock('../host/documents/converter', () => ({ convertOffice: boundary.convert }));
const directories: string[] = [];
afterEach(() => { vi.resetAllMocks(); directories.splice(0).forEach(path => rmSync(path, { recursive: true, force: true })); });

it.each(['converter', 'native reread'] as const)('real document orchestration preserves %s settlement ownership at the Host boundary', async owner => {
  const directory = mkdtempSync(join(tmpdir(), 'document-owner-')); directories.push(directory);
  const host = new LocalWorkspaceHost({ endpoint: 'ws://127.0.0.1:1234/', productHostToken: 'host-private',
    metadataFile: join(directory, 'registry.json'), picker: false, roots: [{ id: 'a', cwd: directory, displayName: 'A' }] });
  const scope = await host.listWorkspaces();
  const bytes = readFileSync(new URL('./fixtures/documents/sample.docx', import.meta.url));
  const file = { scope: { conversation_id: 'c', device: '1', inode: '2' }, path: 'sample.docx', name: 'sample.docx' };
  const read = { target: { session_id: 's', conversation_id: 'c', runtime_incarnation: 'r', attachment_id: 'a' }, message_id: 'm', delivery_index: 0 };
  const request = { target: read.target, source: { kind: 'session_file' as const, message_id: 'm', delivery_index: 0 }, extension: 'docx' as const, digest: createHash('sha256').update(bytes).digest('hex') };
  boundary.read.mockResolvedValue({ file, data: bytes.toString('base64') });
  boundary.convert.mockRejectedValue(new OfficeSettlementError());
  if (owner === 'native reread') boundary.read.mockResolvedValueOnce({ file, data: bytes.toString('base64') })
    .mockRejectedValueOnce(new WorkspaceHostError('native read retirement unknown', 'file_settlement_unknown'));
  try {
    await expect(host.previewDocument(scope, request)).rejects.toMatchObject({ kind: owner === 'converter' ? 'converter_settlement_unknown' : 'file_settlement_unknown' });
    // Initial native read and reauthorization precede converter admission.
    expect(boundary.read).toHaveBeenCalledTimes(2);
    expect(boundary.convert).toHaveBeenCalledTimes(owner === 'converter' ? 1 : 0);
    await expect(host.previewDocument(scope, request)).rejects.toThrow('capacity');
    expect(boundary.read).toHaveBeenCalledTimes(2);
    if (owner === 'converter') {
      expect(await host.readDelivery(scope, read)).toEqual({ file, data: bytes.toString('base64') });
      expect(boundary.read).toHaveBeenCalledTimes(3);
    }
  } finally { host.close(); }
});

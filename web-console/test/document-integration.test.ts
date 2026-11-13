// @vitest-environment node
import { readFileSync } from 'node:fs';
import { beforeEach, afterEach, expect, it, vi } from 'vitest';
const conversion = vi.hoisted(() => ({ real: false }));
vi.mock('../host/documents/converter', async original => {
  const actual = await original<typeof import('../host/documents/converter')>();
  return { ...actual, OfficeConverter: class extends actual.OfficeConverter {
    warm() { return conversion.real ? super.warm() : Promise.resolve(); }
    render(...args: Parameters<InstanceType<typeof actual.OfficeConverter>['render']>) { return conversion.real ? super.render(...args) : Promise.resolve(Buffer.from('%PDF-fixture')); }
  } };
});
afterEach(() => { conversion.real = false; });
import { OfficeDocuments } from '../host/documents/operation';
import { OfficeConverter } from '../host/documents/converter';
import type { DocumentRequest } from '../shared/documents.ts';

function fixture(extension: DocumentRequest['extension']) {
  const bytes = readFileSync(new URL(`./fixtures/documents/sample.${extension}`, import.meta.url));
  const request: DocumentRequest = { extension, source: { kind: 'artifact', artifact_id: 'fixture' }, target: { session_id: 's', conversation_id: 'c', runtime_incarnation: '1', attachment_id: 'a' } };
  return { request, source: { data: Uint8Array.from(bytes) } };
}
it.skipIf(!['linux', 'darwin'].includes(process.platform))('different Office files share the real converter process, without a ready-PDF cache', async () => {
  conversion.real = true;
  const converter = new OfficeConverter();
  try {
    let identity: Awaited<OfficeConverter['identity']>;
    const results = [];
    for (const name of ['linked.docx', 'sample.docx', 'linked.pptx', 'slides.pptx']) {
      const pdf = await converter.render(readFileSync(new URL(`./fixtures/documents/${name}`, import.meta.url)), name.endsWith('docx') ? 'docx' : 'pptx', new AbortController().signal);
      expect(pdf.subarray(0, 5).toString()).toBe('%PDF-'); results.push(pdf);
      const current = await converter.identity;
      identity ??= current; expect(current).toEqual(identity);
    }
    expect(new Set(results.map(pdf => pdf.toString('base64'))).size).toBe(4);
  } finally { await converter.dispose(); }
}, 130000);
it('Host authorizes its own source snapshot and reauthorizes before publication', async () => {
  const { request, source } = fixture('docx'); let reads = 0;
  const result = await documents.deriveDocument(request, async () => { reads++; return source; }, new AbortController().signal);
  expect(reads).toBe(3); expect(result.preview.kind).toBe('pdf');
});
it('rejects source version changes and revoked authorization without publishing a derived result', async () => {
  const { request, source } = fixture('docx');
  let reads = 0;
  await expect(documents.deriveDocument(request, async () => ++reads === 1 ? source : { data: new TextEncoder().encode('changed') }, new AbortController().signal)).rejects.toThrow('source_changed');
  reads = 0;
  await expect(documents.deriveDocument(request, async () => { if (++reads === 2) throw new Error('revoked'); return source; }, new AbortController().signal)).rejects.toThrow('revoked');
});
it('real sandbox converts real DOCX and PPTX and reauthorizes all three reads', async () => {
  conversion.real = true;
  if (!['linux', 'darwin'].includes(process.platform)) {
    const { request, source } = fixture('docx');
    await expect(documents.deriveDocument(request, async () => source, new AbortController().signal)).rejects.toThrow('converter_unavailable');
    return;
  }
  for (const extension of ['docx', 'pptx'] as const) {
    const { request, source } = fixture(extension); let reads = 0;
    const result = await documents.deriveDocument(request, async () => { reads++; return source; }, new AbortController().signal);
    expect(reads).toBe(3); expect(result.preview.kind).toBe('pdf');
    if (result.preview.kind === 'pdf') expect(Buffer.from(result.preview.data).subarray(0, 5).toString()).toBe('%PDF-');
  }
}, process.platform === 'darwin' ? 130000 : 40000);

it.skipIf(!['linux', 'darwin'].includes(process.platform)).each(['docx', 'pptx'] as const)('workspace converts %s with ordinary hyperlinks and printer settings', async extension => {
  conversion.real = true;
  const bytes = readFileSync(new URL(`./fixtures/documents/linked.${extension}`, import.meta.url));
  let reads = 0;
  const result = await documents.deriveWorkspaceOffice(extension, async () => { reads++; return { data: Uint8Array.from(bytes) }; }, new AbortController().signal);
  expect(reads).toBe(3);
  expect(result.preview.kind).toBe('pdf');
  if (result.preview.kind === 'pdf') expect(Buffer.from(result.preview.data).subarray(0, 5).toString()).toBe('%PDF-');
}, process.platform === 'darwin' ? 70000 : 20000);

it('reauthorization compares Session-file fields, not their JSON property order', async () => {
  const { request, source } = fixture('docx');
  const file = { scope: { conversation_id: 'c', device: '1', inode: '2' }, path: 'a.docx', name: 'a.docx', mime_type: 'application/docx' };
  const reordered = { mime_type: file.mime_type, name: file.name, path: file.path, scope: { inode: '2', device: '1', conversation_id: 'c' } };
  let reads = 0;
  await expect(documents.deriveDocument(request, async () => ({ ...source, file: ++reads === 1 ? file : reordered }), new AbortController().signal)).resolves.toMatchObject({ preview: { kind: 'pdf' } });
  reads = 0;
  await expect(documents.deriveDocument(request, async () => ({ ...source, file: ++reads === 1 ? file : { ...reordered, scope: { ...reordered.scope, inode: '3' } } }), new AbortController().signal)).rejects.toThrow('source_changed');
});

let documents: OfficeDocuments;
beforeEach(() => { documents = new OfficeDocuments(); });
afterEach(async () => { await documents.dispose(); });

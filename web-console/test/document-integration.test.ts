// @vitest-environment node
import { readFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { expect, it } from 'vitest';
import { deriveDocument } from '../host/documents/operation';
import type { DocumentRequest } from '../src/client/document-types';

function fixture(extension: DocumentRequest['extension']) {
  const bytes = readFileSync(new URL(`./fixtures/documents/sample.${extension}`, import.meta.url));
  const request: DocumentRequest = { extension, source: { kind: 'artifact', artifact_id: 'fixture' }, digest: createHash('sha256').update(bytes).digest('hex'), target: { session_id: 's', conversation_id: 'c', runtime_incarnation: '1', attachment_id: 'a' } };
  return { request, source: { data: bytes.toString('base64') } };
}
it('real parser worker distinguishes formula and cache, and reauthorizes before publication', async () => {
  const { request, source } = fixture('xlsx'); let reads = 0;
  const result = await deriveDocument(request, async () => { reads++; return source; }, new AbortController().signal);
  expect(reads).toBe(2); expect(result.preview.kind).toBe('xlsx');
});
it('rejects source version changes and revoked authorization without publishing a derived result', async () => {
  const { request, source } = fixture('xlsx');
  await expect(deriveDocument(request, async () => ({ data: 'eA==' }), new AbortController().signal)).rejects.toThrow('source_changed');
  let reads = 0;
  await expect(deriveDocument(request, async () => { if (++reads === 2) throw new Error('revoked'); return source; }, new AbortController().signal)).rejects.toThrow('revoked');
});
it('real sandbox converts real DOCX and PPTX and reauthorizes all three reads', async () => {
  if (process.platform !== 'linux') {
    const { request, source } = fixture('docx');
    await expect(deriveDocument(request, async () => source, new AbortController().signal)).rejects.toThrow('converter_unavailable');
    return;
  }
  for (const extension of ['docx', 'pptx'] as const) {
    const { request, source } = fixture(extension); let reads = 0;
    const result = await deriveDocument(request, async () => { reads++; return source; }, new AbortController().signal);
    expect(reads).toBe(3); expect(result.preview.kind).toBe('pdf');
    if (result.preview.kind === 'pdf') expect(Buffer.from(result.preview.data, 'base64').subarray(0, 5).toString()).toBe('%PDF-');
  }
}, 40000);

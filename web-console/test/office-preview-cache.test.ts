// @vitest-environment node
import { readFileSync } from 'node:fs';
import { afterEach, expect, it, vi } from 'vitest';
const convert = vi.hoisted(() => vi.fn());
vi.mock('../host/documents/converter', () => ({ convertOffice: convert }));
import { OfficePreviewCache } from '../host/documents/cache';
import { deriveWorkspaceOffice } from '../host/documents/operation';
const source = (name = 'linked.docx') => Uint8Array.from(readFileSync(new URL(`./fixtures/documents/${name}`, import.meta.url)));
afterEach(() => convert.mockReset());
it('shares the converted content while each open verifies its own current source and owns its returned bytes', async () => {
  const cache = new OfficePreviewCache(), data = source();
  convert.mockResolvedValue(Buffer.from('%PDF-cached'));
  let reads = 0;
  const read = async () => { reads++; return { data, version: 'first' }; };
  const first = await deriveWorkspaceOffice('docx', read, new AbortController().signal, cache);
  expect(reads).toBe(3); first.preview.data.fill(0);
  reads = 0;
  const next = await deriveWorkspaceOffice('docx', async () => { reads++; return { data, version: 'current' }; }, new AbortController().signal, cache);
  expect(reads).toBe(2); expect(convert).toHaveBeenCalledOnce();
  expect(new TextDecoder().decode(next.preview.data)).toBe('%PDF-cached');
});
it('cached PDF reuse cannot survive revoked access, source replacement, or an extension mismatch', async () => {
  const cache = new OfficePreviewCache(), data = source();
  convert.mockResolvedValue(Buffer.from('%PDF-validated'));
  await deriveWorkspaceOffice('docx', async () => ({ data }), new AbortController().signal, cache);
  await expect(deriveWorkspaceOffice('docx', async () => { throw Error('revoked'); }, new AbortController().signal, cache)).rejects.toThrow('revoked');
  let reads = 0;
  await expect(deriveWorkspaceOffice('docx', async () => ({ data: ++reads === 1 ? data : source('sample.docx') }), new AbortController().signal, cache)).rejects.toThrow('source_changed');
  await expect(deriveWorkspaceOffice('pptx', async () => ({ data }), new AbortController().signal, cache)).rejects.toThrow();
  expect(convert).toHaveBeenCalledOnce();
});
it('a source change causes a fresh conversion and failed or cancelled operations never seed the cache', async () => {
  const cache = new OfficePreviewCache(), data = source();
  convert.mockRejectedValueOnce(Error('converter_failure')).mockResolvedValue(Buffer.from('%PDF-success'));
  await expect(deriveWorkspaceOffice('docx', async () => ({ data }), new AbortController().signal, cache)).rejects.toThrow('converter_failure');
  await deriveWorkspaceOffice('docx', async () => ({ data }), new AbortController().signal, cache);
  await deriveWorkspaceOffice('docx', async () => ({ data: source('sample.docx') }), new AbortController().signal, cache);
  expect(convert).toHaveBeenCalledTimes(3);
  cache.clear();
  const controller = new AbortController(); let reads = 0;
  await expect(deriveWorkspaceOffice('docx', async () => { if (++reads === 3) controller.abort(); return { data }; }, controller.signal, cache)).rejects.toMatchObject({ name: 'AbortError' });
  await deriveWorkspaceOffice('docx', async () => ({ data }), new AbortController().signal, cache);
  expect(convert).toHaveBeenCalledTimes(5);
});
it('bounds retained PDF count and bytes, refreshes LRU order, and releases all entries on clear', () => {
  const cache = new OfficePreviewCache(2, 10);
  cache.retain('a', Buffer.alloc(4)); cache.retain('b', Buffer.alloc(4)); cache.get('a');
  cache.retain('c', Buffer.alloc(4)); expect(cache.get('b')).toBeUndefined(); expect(cache.get('a')).toHaveLength(4);
  cache.retain('large', Buffer.alloc(8)); expect(cache.get('a')).toBeUndefined(); expect(cache.get('c')).toBeUndefined();
  cache.retain('oversized', Buffer.alloc(11)); expect(cache.get('large')).toHaveLength(8); expect(cache.get('oversized')).toBeUndefined();
  cache.clear(); expect(cache.get('large')).toBeUndefined();
});

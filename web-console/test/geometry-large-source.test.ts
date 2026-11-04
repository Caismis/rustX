import { expect, it, vi } from 'vitest';
import { GEOMETRY_LIMITS as L } from '../src/app/components/documents/geometry/limits';
import { readGeometrySource } from '../src/client/geometry-source';
import { FILE_PREVIEW_LIMITS, type FileBytes, type FileReadOptions } from '../shared/file-preview';

it('admits a 200 MiB binary STL without dropping any of its source triangles', () => {
  const bytes = 200 * 1024 * 1024, triangles = Math.floor((bytes - 84) / 50);
  expect(L.sourceBytes).toBeGreaterThanOrEqual(bytes);
  expect(L.triangles).toBeGreaterThanOrEqual(triangles);
  expect(L.vertices).toBeGreaterThanOrEqual(triangles * 3);
  expect(L.decodedBytes).toBeGreaterThanOrEqual(triangles * 3 * 9 * 4);
});

function reader(size = FILE_PREVIEW_LIMITS.pageBytes + 19) {
  const source = Uint8Array.from({ length: size }, (_, i) => i % 251);
  const read = vi.fn(async (options: FileReadOptions): Promise<FileBytes> => {
    if (options.kind !== 'bytes' || !options.range) throw new Error('Expected a bounded byte range');
    const { offset, length } = options.range, data = source.slice(offset, offset + length);
    return { data, offset, bytes: source.length, version: 'one', eof: offset + data.length === source.length };
  });
  return { source, read, signal: new AbortController().signal };
}
it('assembles exact geometry bytes from bounded reads without exposing a partial copy', async () => {
  const f = reader(); const result = await readGeometrySource(f.read, f.signal);
  expect(result.version).toBe('one'); expect(result.data.length).toBe(f.source.length);
  expect(result.data.every((value, index) => value === f.source[index])).toBe(true); expect(f.read).toHaveBeenCalledTimes(2);
  expect(f.read.mock.calls[1][0]).toEqual({ kind: 'bytes', range: { offset: FILE_PREVIEW_LIMITS.pageBytes, length: 19 } });
});
it.each(['version', 'size'])('rejects changed source %s instead of combining different copies', async field => {
  const f = reader(), read = f.read.getMockImplementation()!;
  f.read.mockImplementation(async options => { const page = await read(options); return page.offset ? { ...page, ...(field === 'version' ? { version: 'two' } : { bytes: page.bytes + 1, eof: false }) } : page; });
  await expect(readGeometrySource(f.read, f.signal)).rejects.toThrow('source');
});
it.each(['offset', 'length', 'eof', 'limit', 'lines'])('rejects malformed geometry range %s before publishing bytes', async field => {
  const f = reader(13), read = f.read.getMockImplementation()!;
  f.read.mockImplementation(async options => { const page = await read(options); return { ...page, ...({ offset: { offset: 1 }, length: { data: new Uint8Array(1) }, eof: { eof: false }, limit: { bytes: L.sourceBytes + 1 }, lines: { lines: 1 } }[field]) }; });
  await expect(readGeometrySource(f.read, f.signal)).rejects.toThrow();
});
it('waits for an aborted physical read to settle and starts no subsequent read', async () => {
  const f = reader(), controller = new AbortController(); let finish!: (value: FileBytes) => void;
  const page = await f.read({ kind: 'bytes', range: { offset: 0, length: FILE_PREVIEW_LIMITS.pageBytes } });
  f.read.mockReset().mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  const pending = readGeometrySource(f.read, controller.signal); controller.abort();
  const rejected = expect(pending).rejects.toThrow(); finish(page); await rejected; expect(f.read).toHaveBeenCalledOnce();
});

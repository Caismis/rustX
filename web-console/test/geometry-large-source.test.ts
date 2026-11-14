import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { GEOMETRY_LIMITS as L } from '../src/app/components/documents/geometry/limits';
import { readGeometrySource } from '../src/client/geometry-source';
import { FILE_PREVIEW_LIMITS, type FileBytes, type FileReadOptions } from '../shared/file-preview';

beforeEach(() => { vi.spyOn(performance, 'now').mockReturnValue(0); });
afterEach(() => { vi.restoreAllMocks(); });

it('admits a 200 MiB binary STL without dropping any of its source triangles', () => {
  const bytes = 200 * 1024 * 1024, triangles = Math.floor((bytes - 84) / 50);
  expect(L.sourceBytes).toBeGreaterThanOrEqual(bytes);
  expect(L.triangles).toBeGreaterThanOrEqual(triangles);
  expect(L.vertices).toBeGreaterThanOrEqual(triangles * 3);
  expect(L.decodedBytes).toBeGreaterThanOrEqual(triangles * 3 * 9 * 4);
});

function reader(size = FILE_PREVIEW_LIMITS.pageBytes + 19) {
  const source = Uint8Array.from({ length: size }, (_, i) => i % 251);
  const read = vi.fn(async (options: FileReadOptions, _signal?: AbortSignal): Promise<FileBytes> => {
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
it('reads a small STEP in one ordinary bounded page without sampling probes', async () => {
  const f = reader(1024), header = new TextEncoder().encode('ISO-10303-21;');
  f.source.set(header);
  const result = await readGeometrySource(f.read, f.signal);
  expect(result.data).toEqual(f.source); expect(f.read).toHaveBeenCalledExactlyOnceWith({kind: 'bytes', range: {offset: 0, length: FILE_PREVIEW_LIMITS.pageBytes}}, expect.any(AbortSignal));
  expect(f.read.mock.calls[0][1]!.aborted).toBe(false);
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

const pageBytes = FILE_PREVIEW_LIMITS.pageBytes;
async function flush() { for (let turn = 0; turn < 12; turn++) await Promise.resolve(); }
function controlledReader(size = pageBytes * 4 + 19) {
  const source = Uint8Array.from({ length: size }, (_, i) => i % 251);
  const requests: { offset: number; length: number; signal: AbortSignal; finish: (override?: Partial<FileBytes>) => void }[] = [];
  const read = vi.fn((options: FileReadOptions, signal: AbortSignal) => new Promise<FileBytes>(resolve => {
    if (options.kind !== 'bytes' || !options.range) throw new Error('Expected a bounded byte range');
    const { offset, length } = options.range;
    requests.push({ offset, length, signal, finish: override => {
      const data = source.slice(offset, offset + length);
      resolve({ data, offset, bytes: source.length, version: 'one', eof: offset + data.length === source.length, ...override });
    } });
  }));
  return { source, requests, read };
}
it('rolls two bounded lanes independently and assembles out-of-order pages at their requested offsets', async () => {
  const f = controlledReader(), pending = readGeometrySource(f.read, new AbortController().signal);
  expect(f.requests.map(request => request.offset)).toEqual([0]);
  f.requests[0].finish(); await flush();
  expect(f.requests.map(request => request.offset)).toEqual([0, pageBytes, pageBytes * 2]);
  f.requests[2].finish(); await flush();
  expect(f.requests.map(request => request.offset)).toEqual([0, pageBytes, pageBytes * 2, pageBytes * 3]);
  f.requests[3].finish(); await flush();
  expect(f.requests.map(request => [request.offset, request.length])).toEqual([
    [0, pageBytes], [pageBytes, pageBytes], [pageBytes * 2, pageBytes], [pageBytes * 3, pageBytes], [pageBytes * 4, 19],
  ]);
  f.requests[4].finish(); await flush();
  expect(f.requests).toHaveLength(5);
  f.requests[1].finish();
  const result = await pending;
  expect(result.version).toBe('one'); expect(result.data.every((value, index) => value === f.source[index])).toBe(true);
  expect(result.data.buffer.byteLength).toBe(f.source.length);
});
it('accepts an empty source in one bounded read', async () => {
  const f = reader(0), result = await readGeometrySource(f.read, f.signal);
  expect(result).toEqual({ data: new Uint8Array(0), version: 'one' });
  expect(f.read).toHaveBeenCalledOnce();
});
it('parent cancellation aborts both lanes and waits for both physical reads to settle', async () => {
  const f = controlledReader(), controller = new AbortController(); let settled = false;
  const pending = readGeometrySource(f.read, controller.signal);
  const observed = pending.then(() => { settled = true; return undefined; }, cause => { settled = true; return cause; });
  f.requests[0].finish(); await flush();
  expect(f.requests).toHaveLength(3);
  controller.abort();
  expect(f.requests.slice(1).every(request => request.signal.aborted)).toBe(true);
  f.requests[2].finish(); await flush();
  expect(settled).toBe(false); expect(f.requests).toHaveLength(3);
  f.requests[1].finish();
  expect(await observed).toBe(controller.signal.reason);
  expect(f.requests).toHaveLength(3);
});
it.each(['version', 'size', 'offset', 'duplicate', 'length', 'eof', 'lines'] as const)(
  'a lane with invalid %s aborts its peer and waits for late physical settlement', async field => {
    const f = controlledReader(), controller = new AbortController(); let settled = false;
    const pending = readGeometrySource(f.read, controller.signal);
    const observed = pending.then(() => { settled = true; return undefined; }, cause => { settled = true; return cause as Error; });
    f.requests[0].finish(); await flush();
    const overrides: Record<typeof field, Partial<FileBytes>> = {
      version: { version: 'two' }, size: { bytes: f.source.length + 1 }, offset: { offset: pageBytes + 1 },
      duplicate: { offset: 0 }, length: { data: new Uint8Array(1) }, eof: { eof: true }, lines: { lines: 1 },
    };
    f.requests[1].finish(overrides[field]); await flush();
    expect(f.requests[2].signal.aborted).toBe(true);
    expect(controller.signal.aborted).toBe(false);
    expect(settled).toBe(false); expect(f.requests).toHaveLength(3);
    f.requests[2].finish();
    expect(await observed).toBeInstanceOf(Error);
    expect(f.requests).toHaveLength(3);
  },
);

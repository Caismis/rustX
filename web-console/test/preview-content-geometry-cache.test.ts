import { expect, it, vi } from 'vitest';
import { FILE_PREVIEW_LIMITS as L } from '../shared/file-preview';
import type { GeometrySource, GeometrySourceConsumer } from '../shared/geometry-source';
import { PreviewContentCache, PreviewContentOwner, type PreviewReader } from '../src/client/preview-content';
import { WorkspaceHostError } from '../src/workspaces/host';
const flush = async () => { for (let turn = 0; turn < 40; turn++) await Promise.resolve(); };
function deferred<T>() { let resolve!: (value: T) => void, reject!: (error: unknown) => void; const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; }
function fixture(initial = new Uint8Array([1, 2, 3])) {
  let data = initial, version = 'one', authorized = true;
  let closing = deferred<void>();
  const probes: ReturnType<typeof deferred<string>>[] = [];
  const geometry = vi.fn(async (signal: AbortSignal, consumer: GeometrySourceConsumer) => {
    const copy = data.slice(), capturedVersion = version, finished = closing;
    await consumer.start({ bytes: copy.length, version: capturedVersion });
    for (let offset = 0; offset < copy.length; offset += L.pageBytes) await consumer.chunk(copy.slice(offset, offset + L.pageBytes), offset);
    signal.throwIfAborted();
    await new Promise<void>((resolve, reject) => {
      const abort = () => reject(signal.reason);
      signal.addEventListener('abort', abort, { once: true });
      void finished.promise.then(() => { signal.removeEventListener('abort', abort); resolve(); }, reject);
    });
    signal.throwIfAborted(); return { version: capturedVersion };
  });
  const owner = new PreviewContentOwner({ current: () => authorized, immutable: false, geometry,
    probe: vi.fn(() => { const result = deferred<string>(); probes.push(result); return result.promise; }),
    page: vi.fn(), bytes: vi.fn(), document: vi.fn(),
  } as PreviewReader);
  owner.activate(); owner.ensure('geometry');
  return { owner, geometry, probes, finish: () => closing.resolve(), fail: (error: unknown) => closing.reject(error),
    mutate: (next: Uint8Array<ArrayBuffer>, nextVersion = 'two') => { data = next; version = nextVersion; closing = deferred<void>(); },
    revoke: () => { authorized = false; },
  };
}
const sink = (): GeometrySourceConsumer => ({ start: () => {}, chunk: () => {} });
async function load(f: ReturnType<typeof fixture>, consumer = sink()) {
  const source = f.owner.getSnapshot().source!, pending = source.consume(consumer, new AbortController().signal);
  await flush(); f.finish(); await pending; await flush(); return source;
}
it('copies small source pages before transfer and publishes the cache only after final validation', async () => {
  const f = fixture(), source = f.owner.getSnapshot().source!, before = f.owner.getSnapshot().revision;
  const pending = source.consume({ start: () => {}, chunk: data => { structuredClone(data, { transfer: [data.buffer] }); expect(data.byteLength).toBe(0); } }, new AbortController().signal);
  await flush(); expect(f.owner.getSnapshot().data).toBeUndefined(); expect(f.owner.retainedBytes).toBe(0);
  f.finish(); await pending; await flush();
  expect(f.owner.getSnapshot()).toMatchObject({ source, revision: before, version: 'one' });
  expect(f.owner.getSnapshot().data).toEqual(new Uint8Array([1, 2, 3])); expect(f.owner.retainedBytes).toBe(3); f.owner.dispose();
});
it('reauthorizes on reactivation and replays the original cached copy after disk mutation without rereading', async () => {
  const f = fixture(), source = await load(f), original = f.owner.getSnapshot();
  f.owner.deactivate(); f.mutate(new Uint8Array([9, 9, 9]));
  expect(f.owner.getSnapshot().source).toBe(source); expect(f.owner.retainedBytes).toBe(3);
  await expect(source.consume(sink(), new AbortController().signal)).rejects.toThrow('obsolete');
  f.owner.activate(); await flush(); expect(f.probes).toHaveLength(1); expect(f.owner.getSnapshot().source).toBeUndefined();
  await expect(source.consume(sink(), new AbortController().signal)).rejects.toThrow('obsolete');
  f.probes[0].resolve('two'); await flush();
  const chunks: number[] = [], start = vi.fn();
  await source.consume({ start, chunk: data => { chunks.push(...data); structuredClone(data, { transfer: [data.buffer] }); } }, new AbortController().signal);
  expect(start).toHaveBeenCalledExactlyOnceWith({ bytes: 3, version: 'one' }); expect(chunks).toEqual([1, 2, 3]);
  expect(f.geometry).toHaveBeenCalledOnce(); expect(f.owner.getSnapshot().data).toBe(original.data);
  expect(f.owner.getSnapshot()).toMatchObject({ source, revision: original.revision, version: 'one' }); f.owner.dispose();
});
it('purges a cached stream when reactivation authorization fails', async () => {
  const f = fixture(), source = await load(f); f.owner.deactivate(); f.owner.activate(); await flush();
  f.probes[0].reject(new WorkspaceHostError('denied', 'authorization_revoked')); await flush();
  expect(f.owner.getSnapshot().data).toBeUndefined(); expect(f.owner.getSnapshot().source).toBeUndefined(); expect(f.owner.retainedBytes).toBe(0);
  const start = vi.fn(); await expect(source.consume({ start, chunk: () => {} }, new AbortController().signal)).rejects.toThrow('obsolete'); expect(start).not.toHaveBeenCalled(); f.owner.dispose();
});
it('bounds replay copies to one acknowledged page and stops cancelled replay without detaching the cache', async () => {
  const f = fixture(new Uint8Array(L.pageBytes * 2 + 7)), source = await load(f), cached = f.owner.getSnapshot().data!, controller = new AbortController(), ack = deferred<void>(), offsets: number[] = [];
  const replay = source.consume({ start: () => {}, chunk: (data, offset) => { offsets.push(offset); expect(data.byteLength).toBeLessThanOrEqual(L.pageBytes); expect(data.buffer).not.toBe(cached.buffer); structuredClone(data, { transfer: [data.buffer] }); return ack.promise; } }, controller.signal);
  const rejected = expect(replay).rejects.toMatchObject({ name: 'AbortError' }); await flush(); expect(offsets).toEqual([0]);
  controller.abort(); ack.resolve(); await rejected;
  expect(offsets).toEqual([0]); expect(cached.byteLength).toBe(L.pageBytes * 2 + 7); expect(f.owner.getSnapshot().data).toBe(cached); f.owner.dispose();
});
it('fresh Refresh replaces cached stream identity and fences a suspended old replay', async () => {
  const f = fixture(new Uint8Array(L.pageBytes + 1)), old = await load(f), ack = deferred<void>(), offsets: number[] = [];
  const replay = old.consume({ start: () => {}, chunk: (_data, offset) => { offsets.push(offset); return ack.promise; } }, new AbortController().signal);
  const rejected = expect(replay).rejects.toThrow('obsolete'); await flush();
  f.mutate(new Uint8Array([8]), 'two'); f.owner.refresh(); await flush(); const fresh = f.owner.getSnapshot().source!;
  expect(fresh).not.toBe(old); expect(f.owner.getSnapshot().data).toBeUndefined(); ack.resolve(); await rejected; expect(offsets).toEqual([0]);
  await load(f); expect(f.geometry).toHaveBeenCalledTimes(2); expect(f.owner.getSnapshot().data).toEqual(new Uint8Array([8])); expect(f.owner.getSnapshot().version).toBe('two'); f.owner.dispose();
});
it.each(['evict', 'dispose'] as const)('%s removes cached data and prevents a retained source object from replaying it', async action => {
  const f = fixture(), source = await load(f); f.owner[action]();
  expect(f.owner.getSnapshot().data).toBeUndefined(); expect(f.owner.retainedBytes).toBe(0);
  const start = vi.fn(); await expect(source.consume({ start, chunk: () => {} }, new AbortController().signal)).rejects.toThrow('obsolete'); expect(start).not.toHaveBeenCalled(); f.owner.dispose();
});
it('replay checks current source authority again after each acknowledged page', async () => {
  const f = fixture(new Uint8Array(L.pageBytes + 1)), source = await load(f), ack = deferred<void>(), offsets: number[] = [];
  const replay = source.consume({ start: () => {}, chunk: (_data, offset) => { offsets.push(offset); return ack.promise; } }, new AbortController().signal);
  const rejected = expect(replay).rejects.toThrow('obsolete'); await flush(); f.revoke(); ack.resolve(); await rejected; expect(offsets).toEqual([0]); f.owner.dispose();
});
it('discards provisional cache bytes when final source validation fails', async () => {
  const f = fixture(), source = f.owner.getSnapshot().source!, pending = source.consume(sink(), new AbortController().signal), rejected = expect(pending).rejects.toThrow('final validation failed');
  await flush(); expect(f.owner.getSnapshot().data).toBeUndefined(); f.fail(new Error('final validation failed')); await rejected; await flush();
  expect(f.owner.getSnapshot().data).toBeUndefined(); expect(f.owner.retainedBytes).toBe(0); f.owner.dispose();
});
it('uses the existing inactive cache LRU for completed streamed copies', async () => {
  const cache = new PreviewContentCache(), owners: ReturnType<typeof fixture>[] = [], sources: GeometrySource[] = [];
  for (let index = 0; index < 9; index++) { const f = fixture(new Uint8Array([index])); owners.push(f); sources.push(await load(f)); f.owner.deactivate(); cache.retain(f.owner); }
  expect(owners[0].owner.getSnapshot().source).toBeUndefined(); expect(owners[0].owner.retainedBytes).toBe(0);
  expect(owners[1].owner.getSnapshot().source).toBe(sources[1]);
  for (const f of owners) { cache.release(f.owner); f.owner.dispose(); }
});
it.each([64 * 1024 * 1024, 64 * 1024 * 1024 + 1])('retains only a validated source within the existing 64 MiB cache limit (%i bytes)', async bytes => {
  const owner = new PreviewContentOwner({ current: () => true, immutable: false, page: vi.fn(), bytes: vi.fn(), document: vi.fn(), probe: async () => 'one',
    geometry: async (_signal, consumer) => { await consumer.start({ bytes, version: 'one' }); const page = new Uint8Array(L.pageBytes); for (let offset = 0; offset < bytes; offset += L.pageBytes) await consumer.chunk(offset + page.length <= bytes ? page : page.subarray(0, bytes - offset), offset); return { version: 'one' }; },
  });
  owner.activate(); owner.ensure('geometry'); const source = owner.getSnapshot().source!; await source.consume(sink(), new AbortController().signal); await flush(); owner.deactivate();
  expect(owner.retainedBytes).toBe(bytes <= 64 * 1024 * 1024 ? bytes : 0); expect(owner.getSnapshot().source === source).toBe(bytes <= 64 * 1024 * 1024); owner.dispose();
});

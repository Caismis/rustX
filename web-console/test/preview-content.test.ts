import { expect, it } from 'vitest';
import { PreviewContentCache, PreviewContentOwner, type PreviewTextPage, type PreviewBytes } from '../src/client/preview-content';
import { WorkspaceHostError } from '../src/workspaces/host';
function deferred<T>() { let resolve!: (value: T) => void, reject!: (error: unknown) => void; const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; }
function until(owner: PreviewContentOwner, predicate: () => boolean) {
  if (predicate()) return Promise.resolve();
  return new Promise<void>(resolve => { const stop = owner.subscribe(() => { if (predicate()) { stop(); resolve(); } }); });
}
function fixture(observe = false) {
  const pages: { offset: number; signal: AbortSignal; result: ReturnType<typeof deferred<PreviewTextPage>> }[] = [];
  const probes: { signal: AbortSignal; result: ReturnType<typeof deferred<string>> }[] = [];
  const documents: { signal: AbortSignal; result: ReturnType<typeof deferred<PreviewBytes>> }[] = [];
  let changed = () => {}, active = 0, maximum = 0, authorized = true;
  const entered = new Set<() => void>(), watched = deferred<void>();
  const call = <T>(promise: Promise<T>) => { active++; maximum = Math.max(maximum, active); entered.forEach(fn => fn()); return promise.finally(() => { active--; }); };
  const owner = new PreviewContentOwner({ current: () => authorized, immutable: !observe,
    page: (offset, signal) => { const result = deferred<PreviewTextPage>(); pages.push({ offset, signal, result }); return call(result.promise); },
    bytes: signal => { const result = deferred<PreviewBytes>(); documents.push({ signal, result }); return call(result.promise); },
    document: (_extension, signal) => { const result = deferred<PreviewBytes>(); documents.push({ signal, result }); return call(result.promise); },
    probe: signal => { const result = deferred<string>(); probes.push({ signal, result }); return call(result.promise); },
    observe: observe ? async (_signal, hint) => { changed = hint; return { version: 'v1', done: watched.promise, close: async () => watched.resolve() }; } : undefined });
  const waitCall = (predicate: () => boolean) => predicate() ? Promise.resolve() : new Promise<void>(resolve => { const listener = () => { if (predicate()) { entered.delete(listener); resolve(); } }; entered.add(listener); });
  const page = (at: number, text: string, version = 'v1', eof = true) => pages[at].result.resolve({ text, version, offset: pages[at].offset, lines: text ? 1 : 0, eof });
  return { owner, pages, probes, documents, page, waitCall, hint: () => changed(), lost: () => watched.resolve(), revoke: () => { authorized = false; owner.dispose(); }, get active() { return active; }, get maximum() { return maximum; } };
}
it('refresh fences a pending page, coalesces intents, and holds physical ownership through late failure', async () => {
  const f = fixture(); f.owner.activate(); f.owner.ensure('pages');
  f.page(0, 'prefix', 'v1', false); await until(f.owner, () => !f.owner.getSnapshot().loading);
  f.owner.more(); await f.waitCall(() => f.pages.length === 2); f.owner.refresh(); f.owner.refresh();
  expect(f.pages[1].signal.aborted).toBe(true); expect(f.active).toBe(1); expect(f.pages).toHaveLength(2);
  f.pages[1].result.reject(new Error('late failure')); await f.waitCall(() => f.pages.length === 3);
  expect(f.pages[2].offset).toBe(1); expect(f.owner.getSnapshot().error).toBeUndefined();
  f.page(2, 'fresh'); await until(f.owner, () => !f.owner.getSnapshot().loading);
  expect(f.owner.getSnapshot().text).toBe('fresh'); expect(f.maximum).toBe(1); expect(f.active).toBe(0); f.owner.dispose();
});
it('hide and return cannot bypass the pending operation barrier', async () => {
  const f = fixture(); f.owner.activate(); f.owner.ensure('pages'); f.owner.deactivate(); f.owner.activate(); f.owner.ensure('pages');
  expect(f.pages).toHaveLength(1); expect(f.pages[0].signal.aborted).toBe(true);
  f.page(0, 'obsolete'); await f.waitCall(() => f.pages.length === 2); f.page(1, 'current');
  await until(f.owner, () => !f.owner.getSnapshot().loading); expect(f.owner.getSnapshot().text).toBe('current'); expect(f.maximum).toBe(1); f.owner.dispose();
});
it('close prevents late bytes and errors from recreating a cache', async () => {
  const f = fixture(); f.owner.activate(); f.owner.ensure('bytes'); f.owner.refresh(); f.owner.dispose();
  expect(f.active).toBe(1); f.documents[0].result.resolve({ data: new Uint8Array([1]), version: 'v1' });
  await f.documents[0].result.promise; await Promise.resolve();
  expect(f.owner.retainedBytes).toBe(0); expect(f.documents).toHaveLength(1); expect(f.owner.getSnapshot().data).toBeUndefined();
});
it('unchanged return reauthorizes before restoring every loaded page, without a content read', async () => {
  const f = fixture(true); f.owner.activate(); f.owner.ensure('pages'); await f.waitCall(() => f.pages.length === 1);
  f.page(0, 'one', 'v1', false); await until(f.owner, () => !f.owner.getSnapshot().loading); f.owner.more(); await f.waitCall(() => f.pages.length === 2); f.page(1, 'two'); await until(f.owner, () => !f.owner.getSnapshot().loading);
  f.owner.deactivate(); f.owner.activate(); expect(f.owner.getSnapshot().text).toBeUndefined(); await f.waitCall(() => f.probes.length === 1);
  f.probes[0].result.resolve('v1'); await until(f.owner, () => !f.owner.getSnapshot().loading);
  expect(f.owner.getSnapshot().text).toBe('one\ntwo'); expect(f.pages).toHaveLength(2); expect(f.maximum).toBe(1); f.owner.dispose();
});
it('changed versions retire the entire prefix and restart at one; equal hints do not reread content', async () => {
  const f = fixture(true); f.owner.activate(); f.owner.ensure('pages'); await f.waitCall(() => f.pages.length === 1); f.page(0, 'old'); await until(f.owner, () => !f.owner.getSnapshot().loading);
  f.hint(); await f.waitCall(() => f.probes.length === 1); f.hint(); f.probes[0].result.resolve('v1');
  // A hint arriving after dispatch requests exactly one successor validation.
  await f.waitCall(() => f.probes.length === 2); f.probes[1].result.resolve('v1'); await until(f.owner, () => !f.owner.getSnapshot().loading);
  expect(f.pages).toHaveLength(1); expect(f.owner.getSnapshot().text).toBe('old');
  f.hint(); await f.waitCall(() => f.probes.length === 3); f.probes[2].result.resolve('v2'); await f.waitCall(() => f.pages.length === 2);
  expect(f.owner.getSnapshot().text).toBeUndefined(); expect(f.pages[1].offset).toBe(1); f.page(1, 'new', 'v2'); await until(f.owner, () => !f.owner.getSnapshot().loading);
  expect(f.owner.getSnapshot().text).toBe('new'); f.owner.dispose();
});
it('authorization failure removes cached bytes and does not automatically retry', async () => {
  const f = fixture(true); f.owner.activate(); f.owner.ensure('pages'); await f.waitCall(() => f.pages.length === 1); f.page(0, 'protected'); await until(f.owner, () => !f.owner.getSnapshot().loading);
  f.hint(); await f.waitCall(() => f.probes.length === 1); f.probes[0].result.reject(new WorkspaceHostError('denied', 'session_file_read', false, { kind: 'session_file_read', reason: 'unauthorized' }));
  await until(f.owner, () => f.owner.getSnapshot().error !== undefined); expect(f.owner.retainedBytes).toBe(0); expect(f.pages).toHaveLength(1); expect(f.probes).toHaveLength(1); f.owner.dispose();
});
it('observation loss removes payloads and stops until an explicit retry', async () => {
  const f = fixture(true); f.owner.activate(); f.owner.ensure('pages'); await f.waitCall(() => f.pages.length === 1); f.page(0, 'protected'); await until(f.owner, () => !f.owner.getSnapshot().loading);
  f.lost(); await until(f.owner, () => f.owner.getSnapshot().error !== undefined); expect(f.owner.retainedBytes).toBe(0); expect(f.pages).toHaveLength(1); f.owner.dispose();
});
it('Office refresh waits for real converter settlement and reuses a validated cached result', async () => {
  const f = fixture(true); f.owner.activate(); f.owner.ensure('docx'); await f.waitCall(() => f.documents.length === 1); f.owner.refresh(); f.owner.refresh();
  expect(f.active).toBe(1); f.documents[0].result.resolve({ data: new Uint8Array([1]), version: 'digest-old' }); await f.waitCall(() => f.documents.length === 2);
  f.documents[1].result.resolve({ data: new Uint8Array([2]), version: 'digest-new' }); await until(f.owner, () => !f.owner.getSnapshot().loading);
  f.owner.deactivate(); f.owner.activate(); await f.waitCall(() => f.probes.length === 1); f.probes[0].result.resolve('v1'); await until(f.owner, () => !f.owner.getSnapshot().loading);
  expect(f.documents).toHaveLength(2); expect(f.owner.getSnapshot().data).toEqual(new Uint8Array([2])); expect(f.maximum).toBe(1); f.owner.dispose();
});
it('inactive cache evicts the oldest entry at eight and removes oversized binary results', async () => {
  const cache = new PreviewContentCache(), fixtures = Array.from({ length: 9 }, () => fixture());
  for (const f of fixtures) { f.owner.activate(); f.owner.ensure('pages'); f.page(0, 'cached'); await until(f.owner, () => !f.owner.getSnapshot().loading); f.owner.deactivate(); cache.retain(f.owner); }
  expect(fixtures[0].owner.retainedBytes).toBe(0); expect(fixtures.slice(1).every(f => f.owner.retainedBytes > 0)).toBe(true);
  const big = fixture(); big.owner.activate(); big.owner.ensure('bytes'); big.documents[0].result.resolve({ data: new Uint8Array(64 * 1024 * 1024 + 1), version: 'v1' }); await until(big.owner, () => !big.owner.getSnapshot().loading); big.owner.deactivate(); cache.retain(big.owner);
  expect(big.owner.retainedBytes).toBe(0); [...fixtures, big].forEach(f => { cache.release(f.owner); f.owner.dispose(); });
});
it('repeated refresh retains the loaded prefix target until restoration reaches EOF', async () => {
  const f = fixture(); f.owner.activate(); f.owner.ensure('pages', 3);
  for (let index = 0; index < 3; index++) { await f.waitCall(() => f.pages.length === index + 1); f.page(index, `old-${index}`, 'v1', index === 2); }
  await until(f.owner, () => !f.owner.getSnapshot().loading);
  f.owner.refresh(); await f.waitCall(() => f.pages.length === 4); f.owner.refresh();
  f.page(3, 'obsolete');
  for (let index = 4; index < 7; index++) { await f.waitCall(() => f.pages.length === index + 1); f.page(index, `new-${index}`, 'v2', index === 6); }
  await until(f.owner, () => !f.owner.getSnapshot().loading);
  expect(f.owner.getSnapshot().text).toBe('new-4\nnew-5\nnew-6'); expect(f.owner.getSnapshot().loadedLines).toBe(3); expect(f.maximum).toBe(1); f.owner.dispose();
});
it('equal-version invalidation during More resumes the unfinished offset after actual settlement', async () => {
  const f = fixture(true); f.owner.activate(); f.owner.ensure('pages'); await f.waitCall(() => f.pages.length === 1); f.page(0, 'prefix', 'v1', false);
  await until(f.owner, () => !f.owner.getSnapshot().loading); f.owner.more(); await f.waitCall(() => f.pages.length === 2);
  f.hint(); expect(f.owner.getSnapshot().text).toBe('prefix'); expect(f.pages[1].signal.aborted).toBe(true); expect(f.probes).toHaveLength(0);
  f.page(1, 'retired tail'); await f.waitCall(() => f.probes.length === 1); f.probes[0].result.resolve('v1');
  await f.waitCall(() => f.pages.length === 3); expect(f.pages[2].offset).toBe(2); f.page(2, 'current tail'); await until(f.owner, () => !f.owner.getSnapshot().loading);
  expect(f.owner.getSnapshot().text).toBe('prefix\ncurrent tail'); expect(f.maximum).toBe(1); f.owner.dispose();
});
it('eviction closes the observation once and a return waits for its actual retirement before resubscribing', async () => {
  const retired = deferred<void>(), successor = deferred<void>(); let observations = 0, closes = 0, pages = 0;
  const owner = new PreviewContentOwner({ current: () => true, immutable: false,
    observe: async () => { observations++; if (observations === 2) successor.resolve(); return { version: 'v1', done: retired.promise, close: async () => { closes++; await retired.promise; } }; },
    page: async () => { pages++; return { version: 'v1', text: 'cached', offset: 1, lines: 1, eof: true }; },
    probe: async () => 'v1', bytes: async () => { throw Error('unexpected'); }, document: async () => { throw Error('unexpected'); } });
  owner.activate(); owner.ensure('pages'); await until(owner, () => owner.getSnapshot().text === 'cached');
  owner.deactivate(); owner.evict(); owner.activate(); owner.ensure('pages');
  expect(observations).toBe(1); expect(closes).toBe(1); expect(pages).toBe(1); expect(owner.retainedBytes).toBe(0);
  retired.resolve(); await successor.promise; owner.dispose();
  expect(observations).toBe(2); expect(pages).toBe(1);
});

// @vitest-environment node
import { afterEach, expect, it, vi } from 'vitest';
import { EventEmitter } from 'node:events';
import { mkdtempSync, mkdirSync, realpathSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, dirname, join } from 'node:path';
const boundary = vi.hoisted(() => ({ onClose: () => {}, watchers: [] as { path: string; hint: (event: string, name: Buffer | null) => void; close: ReturnType<typeof vi.fn>; emitter: import('node:events').EventEmitter }[] }));
vi.mock('node:fs', async original => ({ ...await original<typeof import('node:fs')>(), watch: (path: string, _options: unknown, hint: (event: string, name: Buffer | null) => void) => {
  const emitter = new EventEmitter(), close = vi.fn(() => boundary.onClose()); boundary.watchers.push({ path, hint, close, emitter });
  return Object.assign(emitter, { close });
} }));
import { observeFile } from '../host/file-observation';
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(yes => { resolve = yes; }); return { promise, resolve }; }
const directories: string[] = [];
afterEach(() => { boundary.onClose = () => {}; boundary.watchers.splice(0); for (const path of directories.splice(0)) rmSync(path, { recursive: true, force: true }); });
function file() { const root = realpathSync(mkdtempSync(join(tmpdir(), 'rustx-file-observe-'))); directories.push(root); mkdirSync(join(root, 'sub')); const target = join(root, 'sub', 'file.txt'); writeFileSync(target, 'one'); return target; }
function releaseClosed() { for (const watcher of boundary.watchers) if (watcher.close.mock.calls.length) watcher.emitter.emit('close'); }
function waitClosing(watchers = [...boundary.watchers]) {
  if (watchers.every(watcher => watcher.close.mock.calls.length)) return Promise.resolve();
  return new Promise<void>(resolve => { boundary.onClose = () => { if (watchers.every(watcher => watcher.close.mock.calls.length)) resolve(); }; });
}
it('watch readiness precedes the native snapshot; unrelated names never invalidate', async () => {
  const target = file(), snapshot = deferred<string>(), changed = vi.fn(), signal = new AbortController();
  const probe = vi.fn(() => { expect(boundary.watchers.length).toBeGreaterThan(0); return snapshot.promise; });
  const opening = observeFile(target, probe, changed, signal.signal);
  const parent = boundary.watchers.find(watcher => watcher.path === dirname(target))!;
  parent.hint('change', Buffer.from('unrelated.txt')); expect(changed).not.toHaveBeenCalled();
  parent.hint('change', Buffer.from(basename(target))); expect(changed).toHaveBeenCalledOnce();
  snapshot.resolve('native-v1'); const observation = await opening; expect(observation.version).toBe('native-v1'); expect(probe).toHaveBeenCalledOnce();
  let closed = false; const closing = observation.close().then(() => { closed = true; });
  await waitClosing(); expect(closed).toBe(false); expect(boundary.watchers.every(watcher => watcher.close.mock.calls.length === 1)).toBe(true);
  releaseClosed(); await closing; await observation.done; expect(closed).toBe(true);
});
it('ancestor replacement invalidates immediately and rearm waits for every real watcher close', async () => {
  const target = file(), changed = vi.fn(), signal = new AbortController();
  const observation = await observeFile(target, async () => 'v1', changed, signal.signal);
  const count = boundary.watchers.length, parent = boundary.watchers.find(watcher => watcher.path === dirname(dirname(target)))!;
  renameSync(dirname(target), `${dirname(target)}-old`); mkdirSync(dirname(target)); writeFileSync(target, 'two');
  parent.hint('rename', Buffer.from('sub')); expect(changed).toHaveBeenCalledOnce(); expect(boundary.watchers).toHaveLength(count);
  const rearmed = new Promise<void>(resolve => { changed.mockImplementation(() => resolve()); });
  releaseClosed(); await rearmed; expect(boundary.watchers.length).toBeGreaterThan(count);
  const closing = observation.close(); await waitClosing(); releaseClosed(); await closing;
});
it('cancelled setup holds its result until the native probe and watcher closes settle', async () => {
  const target = file(), snapshot = deferred<string>(), signal = new AbortController();
  let settled = false;
  const opening = observeFile(target, () => snapshot.promise, () => {}, signal.signal).finally(() => { settled = true; });
  const rejected = expect(opening).rejects.toThrow(); signal.abort(); await waitClosing(); releaseClosed();
  expect(settled).toBe(false); snapshot.resolve('v1'); await rejected; expect(settled).toBe(true);
});
it('an error without close terminates its watcher but still joins every other physical handle', async () => {
  const observation = await observeFile(file(), async () => 'v1', () => {}, new AbortController().signal);
  const [failed, ...live] = boundary.watchers;
  failed.emitter.emit('error', new Error('native handle already closed'));
  await waitClosing();
  let ended = false; const closing = observation.close().then(() => { ended = true; });
  expect(ended).toBe(false); expect(live.every(watcher => watcher.close.mock.calls.length === 1)).toBe(true);
  for (const watcher of live.slice(1)) watcher.emitter.emit('close');
  await Promise.resolve(); expect(ended).toBe(false);
  live[0].emitter.emit('close'); await closing; await observation.done;
  expect(ended).toBe(true); expect(failed.close).toHaveBeenCalledOnce();
});

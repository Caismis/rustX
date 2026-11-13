// @vitest-environment node
import { afterEach, expect, it, vi } from 'vitest';
import { appendFileSync, fstatSync, mkdtempSync, realpathSync, renameSync, rmSync, symlinkSync, unlinkSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
const boundary = vi.hoisted(() => ({ maxRead: Infinity, before: undefined as (() => void) | undefined, hold: undefined as ((fd: number, complete: () => void) => void) | undefined }));
vi.mock('node:fs', async original => {
  const fs = await original<typeof import('node:fs')>();
  return { ...fs, read: (fd: number, buffer: Uint8Array, offset: number, length: number, position: number, callback: (error: NodeJS.ErrnoException | null, count: number, buffer: Uint8Array) => void) => {
    boundary.before?.();
    return fs.read(fd, buffer, offset, Math.min(length, boundary.maxRead), position, (error, count) => {
      const complete = () => callback(error, count, buffer);
      if (boundary.hold) boundary.hold(fd, complete); else complete();
    });
  } };
});
import { readWorkspacePreview } from '../host/workspace-preview';
import { FILE_PREVIEW_LIMITS } from '../shared/file-preview';
const directories: string[] = [];
afterEach(() => {
  boundary.maxRead = Infinity; boundary.before = undefined; boundary.hold = undefined;
  for (const path of directories.splice(0)) rmSync(path, { recursive: true, force: true });
});
function fixture(data: Uint8Array | string) {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'rustx-preview-'))); directories.push(root);
  const path = join(root, 'file'); writeFileSync(path, data); return { root, path };
}
const signal = () => new AbortController().signal;
it('returns exact concurrent pages, short tails, EOF, beyond EOF and empty bytes', async () => {
  const source = Uint8Array.from({ length: FILE_PREVIEW_LIMITS.pageBytes + 17 }, (_, i) => i % 251);
  const { root } = fixture(source);
  const offsets = [0, FILE_PREVIEW_LIMITS.pageBytes, source.length, source.length + 5];
  const pages = await Promise.all(offsets.map(offset => readWorkspacePreview(root, 'file', { kind: 'bytes', range: { offset, length: FILE_PREVIEW_LIMITS.pageBytes } }, signal())));
  pages.forEach((page, i) => {
    expect(Buffer.compare(page.data, source.subarray(offsets[i], offsets[i] + FILE_PREVIEW_LIMITS.pageBytes))).toBe(0);
    expect(page.offset).toBe(offsets[i]); expect(page.bytes).toBe(source.length); expect(page.eof).toBe(i > 0);
  });
  const complete = await readWorkspacePreview(root, 'file', { kind: 'bytes' }, signal());
  expect(Buffer.compare(complete.data, source)).toBe(0); expect(complete.eof).toBe(true);
  const empty = fixture('');
  await expect(readWorkspacePreview(empty.root, 'file', { kind: 'bytes' }, signal())).resolves.toMatchObject({ data: new Uint8Array(), bytes: 0, eof: true });
});
it('fills pages and complete files across short native reads', async () => {
  boundary.maxRead = 7;
  const source = Uint8Array.from({ length: 133 }, (_, i) => i);
  const { root } = fixture(source);
  const page = await readWorkspacePreview(root, 'file', { kind: 'bytes', range: { offset: 9, length: 99 } }, signal());
  expect(page.data).toEqual(source.slice(9, 108)); expect(page.eof).toBe(false);
  const complete = await readWorkspacePreview(root, 'file', { kind: 'bytes' }, signal());
  expect(Buffer.compare(complete.data, source)).toBe(0); expect(complete.eof).toBe(true);
});
it('enforces complete-file limits including growth past the extra-byte fence', async () => {
  const { root, path } = fixture('12345');
  await expect(readWorkspacePreview(root, 'file', { kind: 'bytes' }, signal(), 4)).rejects.toThrow('too_large');
  await expect(readWorkspacePreview(root, 'file', { kind: 'bytes' }, signal(), 5)).resolves.toMatchObject({ bytes: 5 });
  boundary.before = () => { boundary.before = undefined; appendFileSync(path, '6'); };
  await expect(readWorkspacePreview(root, 'file', { kind: 'bytes' }, signal(), 5)).rejects.toThrow('too_large');
});
it.each(['', 'abc'])('detects growth beyond the stat-sized allocation for %j', async initial => {
  const { root, path } = fixture(initial);
  boundary.before = () => { boundary.before = undefined; appendFileSync(path, 'more'); };
  await expect(readWorkspacePreview(root, 'file', { kind: 'bytes' }, signal())).rejects.toMatchObject({ kind: 'source_changed' });
});
it('waits for the physical read callback before cancellation closes its descriptor', async () => {
  const { root } = fixture('pending'); const abort = new AbortController();
  let release!: () => void, fd!: number, entered!: () => void;
  const started = new Promise<void>(resolve => { entered = resolve; });
  boundary.hold = (descriptor, complete) => { fd = descriptor; release = complete; entered(); };
  let settled = false;
  const reading = readWorkspacePreview(root, 'file', { kind: 'bytes' }, abort.signal).finally(() => { settled = true; });
  const rejected = expect(reading).rejects.toMatchObject({ name: 'AbortError' });
  await started; abort.abort(); await Promise.resolve();
  expect(settled).toBe(false); expect(fstatSync(fd).isFile()).toBe(true);
  release(); await rejected; expect(() => fstatSync(fd)).toThrow();
});
it.each(['replacement', 'same descriptor', 'missing', 'symlink'])('rejects %s changes before publishing a page', async change => {
  const { root, path } = fixture('original');
  boundary.before = () => {
    boundary.before = undefined;
    if (change === 'same descriptor') writeFileSync(path, 'modified');
    else if (change === 'missing') unlinkSync(path);
    else {
      renameSync(path, join(root, 'old'));
      if (change === 'symlink') symlinkSync('old', path); else writeFileSync(path, 'replacement');
    }
  };
  const reading = readWorkspacePreview(root, 'file', { kind: 'bytes', range: { offset: 0, length: 3 } }, signal());
  if (change === 'symlink') await expect(reading).rejects.toBeDefined();
  else await expect(reading).rejects.toMatchObject({ kind: 'source_changed' });
});

it('reports initially missing paths and rejects symlink paths', async () => {
  const { root, path } = fixture('data');
  await expect(readWorkspacePreview(root, 'missing', { kind: 'bytes' }, signal())).rejects.toMatchObject({ kind: 'source_missing' });
  renameSync(path, join(root, 'original')); symlinkSync('original', path);
  await expect(readWorkspacePreview(root, 'file', { kind: 'bytes' }, signal())).rejects.toBeDefined();
});

it('rejects a file truncated while its expected page is being read', async () => {
  const { root, path } = fixture('original');
  boundary.before = () => { boundary.before = undefined; writeFileSync(path, ''); };
  await expect(readWorkspacePreview(root, 'file', { kind: 'bytes' }, signal())).rejects.toMatchObject({ kind: 'source_changed' });
});

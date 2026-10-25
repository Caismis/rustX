// @vitest-environment node
import { afterEach, expect, it, vi } from 'vitest';
import { mkdtempSync, realpathSync, renameSync, rmSync, writeFileSync, type FSWatcher } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
const boundary = vi.hoisted(() => ({ watchers: [] as FSWatcher[] }));
vi.mock('node:fs', async original => {
  const fs = await original<typeof import('node:fs')>();
  return { ...fs, watch: (...args: Parameters<typeof fs.watch>) => { const watcher = fs.watch(...args); boundary.watchers.push(watcher); return watcher; } };
});
import { observeFile } from '../host/file-observation';
import { readWorkspacePreview } from '../host/workspace-preview';
import { LocalWorkspaceHost } from '../host/workspaces';
const directories: string[] = [];
afterEach(() => { boundary.watchers.splice(0); for (const path of directories.splice(0)) rmSync(path, { recursive: true, force: true }); });
function gate<T = void>() { let resolve!: (value: T) => void; const promise = new Promise<T>(yes => { resolve = yes; }); return { promise, resolve }; }
it('a real native watcher error without close settles the observation, frees admission and permits Host shutdown', async () => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'rustx-watch-error-'))); directories.push(root); writeFileSync(join(root, 'file.txt'), 'one');
  const host = new LocalWorkspaceHost({ endpoint: 'ws://localhost:8080', nativeFilesystem: 'shared', roots: [{ id: 'root', cwd: root, displayName: 'Root' }], picker: false, metadataFile: join(root, 'registry.json'), transportToken: 'fixture' }, undefined,
    async () => ({ cwd: root, ownershipGeneration: '1' }), async () => async () => {});
  const scope = await host.listWorkspaces(), call = { target: { session_id: 'A', active_node: 'node-A' }, request: { kind: 'preview' as const, path: 'file.txt', read: { kind: 'bytes' as const } } };
  try {
    const observation = await host.observeWorkspaceFile(scope, call, () => {}, new AbortController().signal);
    const failed = boundary.watchers[0] as FSWatcher & { _handle: { onchange(status: number, event: string, name: string): void } | null };
    const closed = vi.fn(); failed.on('close', closed);
    failed._handle!.onchange(-5, 'change', root); expect(failed._handle).toBeNull();
    await observation.done; await observation.close(); expect(closed).not.toHaveBeenCalled();
    const successors = [];
    for (let i = 0; i < 32; i++) successors.push(await host.observeWorkspaceFile(scope, call, () => {}, new AbortController().signal));
    await expect(host.observeWorkspaceFile(scope, call, () => {}, new AbortController().signal)).rejects.toMatchObject({ kind: 'capacity' });
    await host.close(); await Promise.all(successors.map(value => value.done));
  } finally { await host.close(); }
});
it('Workbench ownership hints ignore unrelated Sessions and physically close their filesystem observations', async () => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'rustx-workbench-observe-'))); directories.push(root);
  writeFileSync(join(root, 'file.txt'), 'one');
  let retired!: (session: string, through: string) => void;
  const host = new LocalWorkspaceHost({ endpoint: 'ws://localhost:8080', roots: [{ id: 'root', cwd: root, displayName: 'Root' }], picker: false, metadataFile: join(root, 'registry.json'), transportToken: 'fixture', nativeFilesystem: 'shared' }, undefined,
    async () => ({ cwd: root, ownershipGeneration: '1' }), async (_endpoint, _token, changed) => { retired = changed; return async () => {}; });
  const changed = vi.fn(), abort = new AbortController();
  try {
    const scope = await host.listWorkspaces();
    const observation = await host.observeWorkspaceFile(scope, { target: { session_id: 'A', active_node: 'node-A' }, request: { kind: 'preview', path: 'file.txt', read: { kind: 'bytes' } } }, changed, abort.signal);
    retired('B', '1'); expect(changed).not.toHaveBeenCalled();
    retired('A', '1'); expect(changed).toHaveBeenCalledOnce();
    await observation.close(); await observation.done;
  } finally { abort.abort(); await host.close(); }
});
it('real filesystem notifications observe atomic file replacement while the initial snapshot is gated', async () => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'rustx-watch-native-'))); directories.push(root);
  const target = join(root, 'file.txt'); writeFileSync(target, 'one');
  const entered = gate(), snapshot = gate<string>(), changed = gate();
  const controller = new AbortController();
  const first = await readWorkspacePreview(root, 'file.txt', { kind: 'bytes', range: { offset: 0, length: 1 } }, controller.signal);
  const opening = observeFile(target, () => { entered.resolve(); return snapshot.promise; }, () => changed.resolve(), controller.signal);
  try {
    await entered.promise;
    writeFileSync(join(root, 'replacement'), 'two'); renameSync(join(root, 'replacement'), target);
    await changed.promise;
    const latest = await readWorkspacePreview(root, 'file.txt', { kind: 'bytes', range: { offset: 0, length: 1 } }, controller.signal);
    expect(latest.version).not.toBe(first.version); snapshot.resolve(latest.version);
    const observation = await opening; expect(observation.version).toBe(latest.version);
    controller.abort(); await observation.done;
  } finally { snapshot.resolve('cancelled'); controller.abort(); await opening.catch(() => {}); }
});
it('workspace text pages preserve exact native separators and distinguish empty files', async () => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'rustx-pages-native-'))); directories.push(root);
  writeFileSync(join(root, 'file.txt'), 'one\r\n\nlast');
  const signal = new AbortController().signal;
  const first = await readWorkspacePreview(root, 'file.txt', { kind: 'text', offset: 1, limit: 2 }, signal);
  expect(new TextDecoder().decode(first.data)).toBe('one\r\n'); expect(first.lines).toBe(2); expect(first.eof).toBe(false);
  const last = await readWorkspacePreview(root, 'file.txt', { kind: 'text', offset: 3, limit: 2 }, signal);
  expect(last.version).toBe(first.version); expect(new TextDecoder().decode(last.data)).toBe('last'); expect(last.lines).toBe(1); expect(last.eof).toBe(true);
  writeFileSync(join(root, 'empty'), '');
  const empty = await readWorkspacePreview(root, 'empty', { kind: 'text', offset: 1, limit: 2 }, signal);
  expect(empty.data).toHaveLength(0); expect(empty.lines).toBe(0); expect(empty.eof).toBe(true);
  await expect(readWorkspacePreview(root, 'missing', { kind: 'text', offset: 1, limit: 2 }, signal)).rejects.toMatchObject({ kind: 'source_missing' });
});

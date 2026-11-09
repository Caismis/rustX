// @vitest-environment node
import { EventEmitter, getEventListeners } from 'node:events';
import { existsSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { PassThrough } from 'node:stream';
import { setImmediate as nextTurn } from 'node:timers/promises';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

const boundary = vi.hoisted(() => ({ worker: vi.fn(), sandbox: vi.fn(), read: vi.fn() }));
vi.mock('node:worker_threads', () => ({ Worker: class { constructor() { return boundary.worker(); } } }));
vi.mock('node:fs/promises', async original => ({ ...await original<typeof import('node:fs/promises')>(), access: async () => {} }));
vi.mock('../host/documents/office-sandbox.ts', () => ({ startOfficeSandbox: boundary.sandbox }));
vi.mock('../host/documents/office-macos.ts', () => ({ startMacOfficeSandbox: (directory: string, command: string[], _roots: string[], signal: AbortSignal) => boundary.sandbox(directory, command, signal) }));
vi.mock('../host/file-read.ts', async original => ({ ...await original<typeof import('../host/file-read.ts')>(), readNativeSource: boundary.read }));

import { deferred } from '../host/documents/deferred';
import { LocalWorkspaceHost } from '../host/workspaces';

const cleanup: (() => Promise<void>)[] = [];
beforeEach(() => { boundary.worker.mockReset(); boundary.sandbox.mockReset(); boundary.read.mockReset(); });
afterEach(async () => { await Promise.all(cleanup.splice(0).map(close => close())); });

function parser() {
  const settled = deferred<void>();
  const worker = Object.assign(new EventEmitter(), { terminate: vi.fn(async () => 0) });
  const remove = worker.removeAllListeners;
  vi.spyOn(worker, 'removeAllListeners').mockImplementation(event => {
    const value = event === undefined ? remove.call(worker) : remove.call(worker, event); settled.resolve(); return value;
  });
  return { worker, settled: settled.promise, complete: () => worker.emit('message', { result: undefined }) };
}

async function fixture() {
  const directory = mkdtempSync(join(tmpdir(), 'rustx-document-warmup-'));
  const host = new LocalWorkspaceHost({ endpoint: 'ws://127.0.0.1:1234/', transportToken: 'browser', productHostToken: 'private', metadataFile: join(directory, 'workspaces.json'), picker: true, roots: [{ id: 'a', cwd: directory, displayName: 'A' }] });
  const started = deferred<void>(), stopping = deferred<void>(), retired = deferred<void>(), closed = deferred<void>();
  const input = new PassThrough(), output = new PassThrough();
  let owner!: AbortSignal, privateDirectory!: string, disposal: Promise<void> | undefined;
  const dispose = vi.fn(() => disposal ??= (async () => {
    stopping.resolve(); await retired.promise; output.end(); closed.resolve();
  })());
  boundary.sandbox.mockImplementation(async (path: string, _command: string[], signal: AbortSignal) => {
    privateDirectory = path; owner = signal;
    signal.addEventListener('abort', () => { void dispose(); }, { once: true });
    started.resolve();
    return { input, output, closed: closed.promise, pid: 123, deadline: async () => {}, dispose };
  });
  boundary.read.mockResolvedValue({ data: new Uint8Array([1, 2, 3]) });
  const scope = await host.listWorkspaces();
  const request = { target: { session_id: 's', conversation_id: 'c', runtime_incarnation: '1', attachment_id: 'a' }, source: { kind: 'artifact' as const, artifact_id: 'artifact_1' }, extension: 'docx' as const };
  cleanup.push(async () => {
    retired.resolve(); await host.close(); input.destroy(); output.destroy(); rmSync(directory, { recursive: true, force: true });
  });
  return { host, scope, request, started: started.promise, stopping: stopping.promise, retired, input, output, dispose,
    get owner() { return owner; }, get privateDirectory() { return privateDirectory; },
    ready: () => output.write('{"ready":true,"engine_pid":42}\n'),
  };
}

it('cancelling a settled parser releases Host admission without retiring its still-warming converter', async () => {
  const f = await fixture(), first = parser(), second = parser();
  const combined = vi.spyOn(AbortSignal, 'any');
  boundary.worker.mockReturnValueOnce(first.worker).mockReturnValueOnce(second.worker);
  const cancel = new AbortController();
  let outcome: string | undefined;
  const work = f.host.previewDocument(f.scope, f.request, cancel.signal).then(() => { outcome = 'published'; }, error => { outcome = error.message; });
  await f.started; first.complete(); await first.settled;
  expect(first.worker.terminate).toHaveBeenCalledOnce();
  cancel.abort();
  // Parser retirement is already proven; cancellation now has only promise
  // continuations to settle. Drain those without an elapsed-time assertion.
  await nextTurn();
  expect(outcome).toBe('obsolete'); await work;
  expect(getEventListeners(combined.mock.results[0].value as AbortSignal, 'abort')).toHaveLength(0);
  expect(f.owner.aborted).toBe(false); expect(f.dispose).not.toHaveBeenCalled();
  expect(existsSync(f.privateDirectory)).toBe(true);

  const nextCancel = new AbortController();
  let nextOutcome: string | undefined;
  const next = f.host.previewDocument(f.scope, f.request, nextCancel.signal).then(() => { nextOutcome = 'published'; }, error => { nextOutcome = error.message; });
  await nextTurn();
  expect(boundary.worker).toHaveBeenCalledTimes(2);
  second.complete(); await second.settled;
  nextCancel.abort(); await nextTurn();
  expect(nextOutcome).toBe('obsolete'); await next;
  expect(boundary.sandbox).toHaveBeenCalledOnce(); expect(f.owner.aborted).toBe(false);

  let hostClosed = false;
  const closing = f.host.close().then(() => { hostClosed = true; });
  await f.stopping;
  expect(f.owner.aborted).toBe(true); expect(hostClosed).toBe(false);
  expect(existsSync(f.privateDirectory)).toBe(true);
  f.retired.resolve(); await closing;
  expect(hostClosed).toBe(true); expect(existsSync(f.privateDirectory)).toBe(false);
});

it('a later request reuses completed Host warmup and cannot revive the cancelled request', async () => {
  const f = await fixture(), first = parser(), second = parser();
  boundary.worker.mockReturnValueOnce(first.worker).mockReturnValueOnce(second.worker);
  const cancel = new AbortController();
  let outcome: string | undefined;
  const work = f.host.previewDocument(f.scope, f.request, cancel.signal).then(() => { outcome = 'published'; }, error => { outcome = error.message; });
  await f.started; first.complete(); await first.settled;
  cancel.abort(); await nextTurn();
  expect(outcome).toBe('obsolete'); await work;

  const next = f.host.previewDocument(f.scope, f.request);
  await nextTurn();
  expect(boundary.worker).toHaveBeenCalledTimes(2);
  second.complete(); await second.settled;
  const rendered = vi.fn((chunk: Buffer) => {
    const { id } = JSON.parse(chunk.toString());
    const pdf = '%PDF-next-document';
    f.output.write(JSON.stringify({ id, length: Buffer.byteLength(pdf) }) + '\n' + pdf);
    f.output.write(JSON.stringify({ id, settled: true, engine_pid: 42 }) + '\n');
  });
  f.input.on('data', rendered); f.ready();
  expect((await next).preview).toEqual({ kind: 'pdf', data: new TextEncoder().encode('%PDF-next-document') });
  expect(rendered).toHaveBeenCalledOnce(); expect(boundary.sandbox).toHaveBeenCalledOnce();
  expect(outcome).toBe('obsolete'); expect(f.owner.aborted).toBe(false);
});

it('cancellation after rendering begins retains Host admission until physical converter retirement', async () => {
  const f = await fixture(), source = parser();
  boundary.worker.mockReturnValueOnce(source.worker);
  const rendering = deferred<void>();
  f.input.once('data', () => { rendering.resolve(); });
  const cancel = new AbortController();
  let outcome: string | undefined;
  const work = f.host.previewDocument(f.scope, f.request, cancel.signal).then(() => { outcome = 'published'; }, error => { outcome = error.message; });
  await f.started; source.complete(); await source.settled; f.ready(); await rendering.promise;
  cancel.abort(); await f.stopping;
  await expect(f.host.previewDocument(f.scope, f.request)).rejects.toThrow('capacity');
  expect(outcome).toBeUndefined(); expect(existsSync(f.privateDirectory)).toBe(true);
  f.retired.resolve(); await work;
  expect(outcome).toBe('obsolete'); expect(existsSync(f.privateDirectory)).toBe(false);
});

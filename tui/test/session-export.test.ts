import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtemp, readFile, rm, access } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { archiveDestination, saveSessionArchive } from '../src/app-server/archive.ts';
import { COMMANDS, parseCommandLine } from '../src/commands/registry.ts';
import { archiveDownloadUrl } from '../../protocol/app-server/download.ts';

test('/export registry and destination parsing are client-local', () => {
  assert.ok(COMMANDS.some(command => command.name === '/export' && command.argumentHint === '[output-path]'));
  assert.equal(parseCommandLine('/export ./debug/session.zip')?.argument, './debug/session.zip');
  assert.equal(archiveDestination('A', '', '/client', '/client/home'), '/client/rustx-session-A.zip');
  assert.equal(archiveDestination('A', '~/session.zip', '/client', '/client/home'), '/client/home/session.zip');
  assert.equal(archiveDestination('A', './a path.zip', '/client', '/client/home'), '/client/a path.zip');
});

test('remote bytes are written incrementally, destination never enters preparation', async () => {
  const root = await mkdtemp(join(tmpdir(), 'rustx-export-client-'));
  const path = join(root, 'local.zip');
  let source!: ReadableStreamDefaultController<Uint8Array>;
  let next!: () => void;
  const pulled = new Promise<void>(resolve => { next = resolve; });
  let pulls = 0;
  const stream = new ReadableStream<Uint8Array>({
    start(controller) { source = controller; controller.enqueue(new TextEncoder().encode('first')); },
    pull() { if (++pulls === 1) next(); },
  }, { highWaterMark: 0 });
  const calls: unknown[][] = [];
  try {
    const saving = saveSessionArchive(path, async (...args: unknown[]) => { calls.push(args); return new Response(stream); });
    await pulled;
    assert.equal(await readFile(path, 'utf8'), 'first');
    assert.deepEqual(calls, [[]]);
    source.enqueue(new TextEncoder().encode('second')); source.close();
    assert.equal(await saving, path);
    assert.equal(await readFile(path, 'utf8'), 'firstsecond');
    const url = archiveDownloadUrl({ path: `/session-archive/${'a'.repeat(43)}`, filename: 'native.zip', loopback_port: null, expires_in_seconds: 60 }, 'wss://remote.example/');
    assert.equal(new URL(url).host, 'remote.example');
    assert.ok(!url.includes(path));
  } finally { await rm(root, { recursive: true, force: true }); }
});

test('stream failures remove partial output and never claim success', async () => {
  const root = await mkdtemp(join(tmpdir(), 'rustx-export-failure-'));
  const path = join(root, 'partial.zip');
  let pulls = 0;
  const stream = new ReadableStream<Uint8Array>({ pull(controller) { if (++pulls === 1) controller.enqueue(new Uint8Array([1,2])); else controller.error(new Error('native read failed')); } }, { highWaterMark: 0 });
  try {
    await assert.rejects(saveSessionArchive(path, async () => new Response(stream)), /native read failed/);
    await assert.rejects(access(path));
    await assert.rejects(saveSessionArchive(path, async () => new Response('denied', { status: 401 })), /HTTP 401/);
    await assert.rejects(access(path));
  } finally { await rm(root, { recursive: true, force: true }); }
});

test('cancellation stops the reader and removes only the partial file', async () => {
  const root = await mkdtemp(join(tmpdir(), 'rustx-export-cancel-'));
  const path = join(root, 'partial.zip');
  const abort = new AbortController();
  let ready!: () => void;
  const written = new Promise<void>(resolve => { ready = resolve; });
  let cancelled = false;
  const stream = new ReadableStream<Uint8Array>({
    start(controller) { controller.enqueue(new Uint8Array([7])); },
    pull() { ready(); },
    cancel() { cancelled = true; },
  }, { highWaterMark: 0 });
  try {
    const saving = saveSessionArchive(path, async () => new Response(stream), abort.signal);
    await written; abort.abort(new Error('client cancelled'));
    await assert.rejects(saving, /client cancelled/);
    assert.equal(cancelled, true);
    await assert.rejects(access(path));
  } finally { await rm(root, { recursive: true, force: true }); }
});

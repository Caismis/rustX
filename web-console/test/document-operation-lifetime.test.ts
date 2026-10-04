// @vitest-environment node
import { EventEmitter } from 'node:events';
import { createHash } from 'node:crypto';
import { afterEach, expect, it, vi } from 'vitest';
const boundary = vi.hoisted(() => ({ create: vi.fn() }));
vi.mock('node:worker_threads', () => ({ Worker: class { constructor(...args: unknown[]) { return boundary.create(...args); } } }));
import { deriveDocument } from '../host/documents/operation';
import type { DocumentRequest } from '../src/client/document-types';
function deferred<T>() { let resolve!: (v: T) => void; const promise = new Promise<T>(r => { resolve = r; }); return { promise, resolve }; }
afterEach(() => { boundary.create.mockReset(); vi.useRealTimers(); });
it.each(['cancel', 'timeout', 'failure'] as const)('parser %s retains settlement until worker termination, then releases timers/listeners', async mode => {
  vi.useFakeTimers();
  const entered = deferred<void>(), stopping = deferred<void>(), terminated = deferred<number>();
  const worker = Object.assign(new EventEmitter(), { terminate: vi.fn(() => { stopping.resolve(); return terminated.promise; }) });
  boundary.create.mockImplementation(() => { entered.resolve(); return worker; });
  const bytes = Buffer.from('bounded source'), signal = new AbortController();
  const request: DocumentRequest = { extension: 'xlsx', target: { session_id: 's', conversation_id: 'c', attachment_id: 'a', runtime_incarnation: '1' }, source: { kind: 'artifact', artifact_id: 'id' }, digest: createHash('sha256').update(bytes).digest('hex') };
  let settled = false;
  const result = deriveDocument(request, async () => ({ data: bytes.toString('base64') }), signal.signal).finally(() => { settled = true; });
  const rejected = expect(result).rejects.toThrow(mode === 'timeout' ? 'parser_timeout' : mode === 'failure' ? 'parser_failure' : 'obsolete');
  await entered.promise;
  if (mode === 'cancel') signal.abort();
  else if (mode === 'timeout') vi.advanceTimersByTime(15000);
  else worker.emit('error', new Error('worker failed'));
  await stopping.promise;
  expect(settled).toBe(false); expect(worker.terminate).toHaveBeenCalledOnce();
  expect(vi.getTimerCount()).toBe(0);
  terminated.resolve(1); await rejected;
  expect(settled).toBe(true); expect(worker.eventNames()).toEqual([]);
});
it('authorization and exact source version are checked before any parser worker exists', async () => {
  const request: DocumentRequest = { extension: 'xlsx', target: { session_id: 's', conversation_id: 'c', attachment_id: 'a', runtime_incarnation: '1' }, source: { kind: 'artifact', artifact_id: 'id' }, digest: '0'.repeat(64) };
  await expect(deriveDocument(request, async () => { throw new Error('revoked'); }, new AbortController().signal)).rejects.toThrow('revoked');
  await expect(deriveDocument(request, async () => ({ data: 'eA==' }), new AbortController().signal)).rejects.toThrow('source_changed');
  expect(boundary.create).not.toHaveBeenCalled();
});

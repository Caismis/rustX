import { expect, it, vi } from 'vitest';
import { DocumentDependencies } from '../src/app/components/workbench-documents/dependencies';
import type { WorkbenchCall } from '../src/app/components/WorkbenchFiles';
import type { WorkbenchResult } from '../src/workspaces/workbench';
import { WorkspaceHostError } from '../src/workspaces/host';
function gate<T>() { let resolve!: (value: T) => void, reject!: (error: unknown) => void; const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; }
function fixture() {
  const calls: { path: string; probe: boolean; result: ReturnType<typeof gate<WorkbenchResult>> }[] = [], changed = vi.fn();
  const watches: { hint: () => void; close: ReturnType<typeof vi.fn>; signal: AbortSignal }[] = [];
  const waiting = new Set<() => void>(); let active = 0, maximum = 0;
  const call: WorkbenchCall = async request => {
    if (request.kind !== 'preview') throw Error('unexpected request');
    const result = gate<WorkbenchResult>(); calls.push({ path: request.path, probe: request.read.kind === 'bytes' && !!request.read.range, result }); active++; maximum = Math.max(maximum, active); waiting.forEach(fn => fn());
    try { return await result.promise; } finally { active--; }
  };
  call.observe = async (_path, hint, signal) => { const closed = gate<void>(), close = vi.fn(async () => closed.resolve()); watches.push({ hint, signal, close }); return { version: 'v1', close, done: closed.promise }; };
  const dependencies = new DocumentDependencies(call, changed, 0);
  const waitCall = (count: number) => calls.length >= count ? Promise.resolve() : new Promise<void>(resolve => { const check = () => { if (calls.length >= count) { waiting.delete(check); resolve(); } }; waiting.add(check); });
  const reply = (index: number, version = 'v1') => calls[index].result.resolve({ preview: { data: new Uint8Array([1]), version, bytes: 1, offset: 0, eof: true } });
  return { dependencies, calls, changed, watches, waitCall, reply, get active() { return active; }, get maximum() { return maximum; } };
}
it('dependency hints reauthorize after the physical read; equal versions retain the result without another content read', async () => {
  const f = fixture(), first = f.dependencies.read('image.png', new AbortController().signal);
  await f.waitCall(1); f.watches[0].hint(); expect(f.calls).toHaveLength(1); expect(f.active).toBe(1);
  f.reply(0); await f.waitCall(2); expect(f.calls[1].probe).toBe(true); f.reply(1);
  expect((await first).version).toBe('v1'); expect(f.changed).not.toHaveBeenCalled();
  await f.dependencies.read('image.png', new AbortController().signal); expect(f.calls).toHaveLength(2); expect(f.maximum).toBe(1); f.dependencies.dispose();
});
it('a changed dependency invalidates the document once and blocks a late asset publication', async () => {
  const f = fixture(), pending = f.dependencies.read('style.css', new AbortController().signal), rejected = expect(pending).rejects.toThrow('obsolete');
  await f.waitCall(1); f.watches[0].hint(); f.reply(0); await f.waitCall(2); f.reply(1, 'v2'); await rejected;
  expect(f.changed).toHaveBeenCalledOnce(); f.watches[0].hint(); expect(f.calls).toHaveLength(2); expect(f.maximum).toBe(1); f.dependencies.dispose();
});
it('closing retains the old operation until settlement and closes its observation exactly once', async () => {
  const f = fixture(), pending = f.dependencies.read('image.png', new AbortController().signal), rejected = expect(pending).rejects.toThrow('obsolete');
  await f.waitCall(1); f.dependencies.dispose(); expect(f.active).toBe(1); expect(f.watches[0].signal.aborted).toBe(true);
  f.reply(0); await rejected; expect(f.active).toBe(0); expect(f.watches[0].close).toHaveBeenCalledOnce(); expect(f.changed).not.toHaveBeenCalled();
});
it('observation capacity failure is explicit and never enters a retry loop or reads unobserved bytes', async () => {
  const changed = vi.fn(), call: WorkbenchCall = vi.fn(async () => ({}));
  call.observe = vi.fn(async () => { throw new WorkspaceHostError('capacity', 'capacity'); });
  const dependencies = new DocumentDependencies(call, changed, 0), signal = new AbortController().signal;
  await expect(dependencies.read('asset.css', signal)).rejects.toMatchObject({ kind: 'capacity' });
  await expect(dependencies.read('asset.css', signal)).rejects.toMatchObject({ kind: 'capacity' });
  expect(call.observe).toHaveBeenCalledOnce(); expect(call).not.toHaveBeenCalled(); expect(changed).not.toHaveBeenCalled(); dependencies.dispose();
});

import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { readGeometrySource } from '../src/client/geometry-source';
import { settlementFailureKind, WorkspaceHostError } from '../src/workspaces/host';
import { PreviewDemand, SESSION_FILE_MAX_TRANSFERS } from '../src/client/session-files';
import { PREVIEW_POLICY } from '../src/client/preview-policy';
import { FILE_PREVIEW_LIMITS as L, type FileBytes, type FileReadOptions } from '../shared/file-preview';

beforeEach(() => { vi.spyOn(performance, 'now').mockReturnValue(0); });
afterEach(() => { vi.restoreAllMocks(); });
async function flush() { for (let turn = 0; turn < 20; turn++) await Promise.resolve(); }
function admittedReads() {
  const demand = new PreviewDemand(SESSION_FILE_MAX_TRANSFERS, PREVIEW_POLICY.activeBodies + PREVIEW_POLICY.downloads, 'unavailable',
    cause => settlementFailureKind(cause) !== undefined);
  const requests: { label: string; offset: number; signal: AbortSignal; finish: (override?: Partial<FileBytes>) => void; fail?: (cause: unknown) => void }[] = [];
  let active = 0, maximum = 0;
  const read = (label: string) => (options: FileReadOptions, signal: AbortSignal) => demand.run(signal, () => new Promise<FileBytes>((resolve, reject) => {
    if (options.kind !== 'bytes' || !options.range) throw new Error('Expected bounded geometry range');
    const { offset, length } = options.range, bytes = L.pageBytes * 3 + 7;
    active++; maximum = Math.max(maximum, active);
    requests.push({ label, offset, signal, fail: cause => { active--; reject(cause); }, finish: override => {
      active--;
      const data = new Uint8Array(Math.min(length, bytes - offset)); data.fill(label === 'A' ? 1 : 2);
      resolve({ data, bytes, offset, version: label, eof: offset + data.length === bytes, ...override });
    } });
  }));
  const download = () => demand.run(new AbortController().signal, () => new Promise<void>(resolve => {
    active++; maximum = Math.max(maximum, active);
    requests.push({ label: 'download', offset: 0, signal: new AbortController().signal, finish: () => { active--; resolve(); } });
  }));
  return { requests, read, download, maximum: () => maximum };
}
it('two rolling previews and a download share capacity two and preserve FIFO admission across pages', async () => {
  const f = admittedReads(), signal = new AbortController().signal;
  const a = readGeometrySource(f.read('A'), signal), b = readGeometrySource(f.read('B'), signal), download = f.download();
  const starts = () => f.requests.map(request => [request.label, request.offset]);
  expect(starts()).toEqual([['A', 0], ['B', 0]]);
  f.requests[0].finish(); await flush();
  expect(starts()).toEqual([['A', 0], ['B', 0], ['download', 0]]);
  f.requests[1].finish(); await flush();
  expect(starts().at(-1)).toEqual(['A', L.pageBytes]);
  f.requests[2].finish(); await flush();
  expect(starts().at(-1)).toEqual(['A', L.pageBytes * 2]);
  f.requests[3].finish(); await flush();
  expect(starts().at(-1)).toEqual(['B', L.pageBytes]);
  f.requests[4].finish(); await flush();
  expect(starts().at(-1)).toEqual(['B', L.pageBytes * 2]);
  f.requests[5].finish(); await flush();
  expect(starts().at(-1)).toEqual(['A', L.pageBytes * 3]);
  f.requests[6].finish(); await flush();
  expect(starts().at(-1)).toEqual(['B', L.pageBytes * 3]);
  f.requests[7].finish(); f.requests[8].finish();
  const [first, second] = await Promise.all([a, b, download]);
  expect(first.data.length).toBe(L.pageBytes * 3 + 7); expect(first.data.every(value => value === 1)).toBe(true);
  expect(second.data.length).toBe(L.pageBytes * 3 + 7); expect(second.data.every(value => value === 2)).toBe(true);
  expect(f.maximum()).toBe(2); expect(f.requests).toHaveLength(9);
});
it('cancelled preview lanes retain shared permits until each physical read settles', async () => {
  const f = admittedReads(), controller = new AbortController(); let settled = false;
  const source = readGeometrySource(f.read('A'), controller.signal);
  const observed = source.catch(cause => { settled = true; return cause; });
  f.requests[0].finish(); await flush();
  expect(f.requests).toHaveLength(3);
  const download = f.download(); controller.abort(); await flush();
  expect(f.requests).toHaveLength(3); expect(settled).toBe(false);
  f.requests[1].finish(); await flush();
  expect(f.requests.at(-1)?.label).toBe('download');
  expect(settled).toBe(false); expect(f.requests[2].signal.aborted).toBe(true);
  f.requests[3].finish(); await download;
  expect(settled).toBe(false);
  f.requests[2].finish();
  expect(await observed).toBe(controller.signal.reason);
  expect(f.maximum()).toBe(2); expect(f.requests).toHaveLength(4);
});
it.each([
  ['parent abort', 'file_settlement_unknown'], ['parent abort', 'document_settlement_unknown'],
  ['source change', 'file_settlement_unknown'], ['source change', 'document_settlement_unknown'],
] as const)('preserves late %s / %s settlement failure and closes shared admission', async (firstFailure, kind) => {
  const f = admittedReads(), controller = new AbortController(); let settled = false;
  const pending = readGeometrySource(f.read('A'), controller.signal);
  const observed = pending.then(() => { settled = true; return undefined; }, cause => { settled = true; return cause; });
  f.requests[0].finish(); await flush();
  if (firstFailure === 'parent abort') controller.abort();
  f.requests[1].finish(firstFailure === 'source change' ? { version: 'changed' } : undefined); await flush();
  expect(f.requests[2].signal.aborted).toBe(true); expect(settled).toBe(false);
  const failure = new WorkspaceHostError('physical settlement lost', kind);
  f.requests[2].fail!(failure);
  expect(await observed).toBe(failure);
  await expect(f.download()).rejects.toThrow('unavailable');
  expect(f.requests).toHaveLength(3); expect(f.maximum()).toBe(2);
});

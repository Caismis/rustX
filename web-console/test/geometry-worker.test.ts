import { afterEach, expect, it, vi } from 'vitest';
import { parseGeometry } from '../src/app/components/documents/geometry/parse';
import { GEOMETRY_LIMITS } from '../src/app/components/documents/geometry/limits';
import type { GeometryModel } from '../src/app/components/documents/geometry/model';

class ImportWorker {
  static instances: ImportWorker[] = [];
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onerror: (() => void) | null = null; onmessageerror: (() => void) | null = null;
  terminated = 0; input?: { bytes: Uint8Array; format: string };
  constructor() { ImportWorker.instances.push(this); }
  postMessage(input: { bytes: Uint8Array; format: string }) { this.input = input; }
  terminate() { this.terminated++; }
}
const model = (): GeometryModel => ({ root: { name: '', meshes: [0], children: [] }, meshes: [{ name: '', positions: new Float32Array([0,0,0, 1,0,0, 0,1,0]), groups: [] }] });
afterEach(() => { vi.unstubAllGlobals(); vi.useRealTimers(); ImportWorker.instances = []; });

it('transfers a separate copy and terminates exactly once after admitted output', async () => {
  vi.stubGlobal('Worker', ImportWorker);
  const bytes = new Uint8Array([1, 2, 3]), controller = new AbortController();
  const result = parseGeometry(bytes, 'step', controller.signal), worker = ImportWorker.instances[0];
  expect(worker.input?.bytes).toEqual(bytes); expect(worker.input?.bytes.buffer).not.toBe(bytes.buffer);
  worker.onmessage!({ data: { ok: true, model: model() } });
  await expect(result).resolves.toMatchObject({ meshes: [expect.anything()] });
  controller.abort(); expect(worker.terminated).toBe(1); expect(bytes).toEqual(new Uint8Array([1, 2, 3]));
});
it('bounds aggregate workers and rejects stale completion after cancellation', async () => {
  vi.stubGlobal('Worker', ImportWorker);
  const controller = new AbortController(), second = new AbortController();
  const a = parseGeometry(new Uint8Array(), 'stl', controller.signal), b = parseGeometry(new Uint8Array(), 'brep', second.signal);
  const aRejected = expect(a).rejects.toMatchObject({ name: 'AbortError' }), bRejected = expect(b).rejects.toMatchObject({ name: 'AbortError' });
  await expect(parseGeometry(new Uint8Array(), 'iges', new AbortController().signal)).rejects.toMatchObject({ code: 'capacity' });
  const old = ImportWorker.instances[0].onmessage!; controller.abort(); old({ data: { ok: true, model: model() } }); second.abort();
  await Promise.all([aRejected, bRejected]);
  expect(ImportWorker.instances.map(worker => worker.terminated)).toEqual([1, 1]);
});
it('terminates timed-out and crashed workers and refuses invalid output before upload', async () => {
  vi.stubGlobal('Worker', ImportWorker); vi.useFakeTimers();
  const timed = parseGeometry(new Uint8Array(), 'step', new AbortController().signal), rejection = expect(timed).rejects.toMatchObject({ code: 'timeout' });
  await vi.advanceTimersByTimeAsync(GEOMETRY_LIMITS.timeoutMs); await rejection;
  const crashed = parseGeometry(new Uint8Array(), 'step', new AbortController().signal); ImportWorker.instances[1].onerror!();
  await expect(crashed).rejects.toMatchObject({ code: 'worker' });
  const invalid = parseGeometry(new Uint8Array(), 'step', new AbortController().signal), broken = model(); broken.meshes[0].positions[0] = Infinity;
  ImportWorker.instances[2].onmessage!({ data: { ok: true, model: broken } });
  await expect(invalid).rejects.toMatchObject({ code: 'invalid' });
  expect(ImportWorker.instances.map(worker => worker.terminated)).toEqual([1, 1, 1]);
});

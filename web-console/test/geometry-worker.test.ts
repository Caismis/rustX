import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { parseGeometry } from '../src/app/components/documents/geometry/parse';
import { GEOMETRY_LIMITS } from '../src/app/components/documents/geometry/limits';
import type { GeometryModel } from '../src/app/components/documents/geometry/model';
import { BufferGeometry } from 'three';
import { geometryPreviewState } from '../src/app/components/documents/geometry/scene-resources';

const code = vi.hoisted(() => ({ load: vi.fn<() => Promise<WebAssembly.Module>>() }));
vi.mock('../src/app/components/documents/geometry/cad-code', () => ({ cadCode: code.load }));
const module = new WebAssembly.Module(new Uint8Array([0,97,115,109,1,0,0,0]));
beforeEach(() => { code.load.mockReset().mockResolvedValue(module); });

class ImportWorker {
  static instances: ImportWorker[] = [];
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onerror: (() => void) | null = null; onmessageerror: (() => void) | null = null;
  terminated = 0; input?: { bytes: Uint8Array; format: string; module?: WebAssembly.Module };
  constructor() { ImportWorker.instances.push(this); }
  postMessage(input: { bytes: Uint8Array; format: string; module?: WebAssembly.Module }) { this.input = input; }
  terminate() { this.terminated++; }
}
const model = (): GeometryModel => ({ root: { name: '', meshes: [0], children: [] }, meshes: [{ name: '', positions: new Float32Array([0,0,0, 1,0,0, 0,1,0]), groups: [] }] });
afterEach(() => { vi.unstubAllGlobals(); vi.useRealTimers(); ImportWorker.instances = []; });

it('transfers a separate copy and terminates exactly once after admitted output', async () => {
  vi.stubGlobal('Worker', ImportWorker);
  const bytes = new Uint8Array([1, 2, 3]), controller = new AbortController();
  const result = parseGeometry(bytes, 'step', controller.signal); await Promise.resolve(); const worker = ImportWorker.instances[0];
  expect(worker.input?.module).toBe(module);
  expect(worker.input?.bytes).toEqual(bytes); expect(worker.input?.bytes.buffer).not.toBe(bytes.buffer);
  worker.onmessage!({ data: { ok: true, model: model() } });
  const prepared = await result; expect(prepared.scene.children).toHaveLength(1); expect(prepared.bounds.max.toArray()).toEqual([1,1,0]); prepared.dispose();
  controller.abort(); expect(worker.terminated).toBe(1); expect(bytes).toEqual(new Uint8Array([1, 2, 3]));
});
it('takes an explicitly relinquished source only when the admitted Worker is ready', async () => {
  vi.stubGlobal('Worker', ImportWorker);
  let complete!: (value: WebAssembly.Module) => void;
  code.load.mockReturnValue(new Promise(resolve => { complete = resolve; }));
  const bytes = new Uint8Array([1,2,3]), slice = vi.spyOn(bytes, 'slice');
  const take = vi.fn(() => structuredClone(bytes, { transfer: [bytes.buffer] }));
  const pending = parseGeometry(bytes, 'step', new AbortController().signal, undefined, take);
  expect(take).not.toHaveBeenCalled(); complete(module); await Promise.resolve();
  const worker = ImportWorker.instances[0]; expect(take).toHaveBeenCalledOnce(); expect(slice).not.toHaveBeenCalled();
  expect(bytes.byteLength).toBe(0); expect([...worker.input!.bytes]).toEqual([1,2,3]);
  worker.onmessage!({ data: { ok:true, model:model() } }); (await pending).dispose();
});
it('bounds aggregate workers and rejects stale completion after cancellation', async () => {
  vi.stubGlobal('Worker', ImportWorker);
  const controller = new AbortController(), second = new AbortController();
  const a = parseGeometry(new Uint8Array(), 'stl', controller.signal), b = parseGeometry(new Uint8Array(), 'brep', second.signal);
  const aRejected = expect(a).rejects.toMatchObject({ name: 'AbortError' }), bRejected = expect(b).rejects.toMatchObject({ name: 'AbortError' });
  await expect(parseGeometry(new Uint8Array(), 'iges', new AbortController().signal)).rejects.toMatchObject({ code: 'capacity' });
  await Promise.resolve();
  const old = ImportWorker.instances[0].onmessage!; controller.abort(); old({ data: { ok: true, model: model() } }); second.abort();
  await Promise.all([aRejected, bRejected]);
  expect(ImportWorker.instances.map(worker => worker.terminated)).toEqual([1, 1]);
});
it('terminates timed-out and crashed workers and refuses invalid output before upload', async () => {
  vi.stubGlobal('Worker', ImportWorker); vi.useFakeTimers();
  const timed = parseGeometry(new Uint8Array(), 'step', new AbortController().signal), rejection = expect(timed).rejects.toMatchObject({ code: 'timeout' });
  await vi.advanceTimersByTimeAsync(GEOMETRY_LIMITS.cadTimeoutMs); await rejection;
  const crashed = parseGeometry(new Uint8Array(), 'step', new AbortController().signal); await Promise.resolve(); ImportWorker.instances[1].onerror!();
  await expect(crashed).rejects.toMatchObject({ code: 'worker' });
  const invalid = parseGeometry(new Uint8Array(), 'step', new AbortController().signal), broken = model(); await Promise.resolve(); broken.meshes[0].positions[0] = Infinity;
  ImportWorker.instances[2].onmessage!({ data: { ok: true, model: broken } });
  await expect(invalid).rejects.toMatchObject({ code: 'invalid' });
  expect(ImportWorker.instances.map(worker => worker.terminated)).toEqual([1, 1, 1]);
});
it.each(['step', 'stl'] as const)('reports only the finite observed stage order for %s and validates the terminal result independently', async format => {
  vi.stubGlobal('Worker', ImportWorker);
  const stage = vi.fn(), result = parseGeometry(new Uint8Array([1]), format, new AbortController().signal, stage); await Promise.resolve(); const worker = ImportWorker.instances[0];
  const send = (data: unknown) => worker.onmessage!({ data });
  send({ kind: 'stage', stage: 'preparing' }); send({ kind: 'stage', stage: 'unknown' });
  send({ kind: 'stage', stage: 'initializing', ok: true, model: model() });
  if (format === 'stl') send({ kind: 'stage', stage: 'initializing' });
  expect(stage.mock.calls.map(([value]) => value)).toEqual(format === 'step' ? ['initializing'] : []); expect(worker.terminated).toBe(0);
  const stages = format === 'stl' ? ['importing', 'preparing'] : ['initializing', 'importing', 'preparing'];
  for (const value of stages) { send({ kind: 'stage', stage: value }); send({ kind: 'stage', stage: value }); }
  send({ kind: 'stage', stage: undefined }); send({ kind: 'stage', stage: 'initializing' });
  expect(stage.mock.calls.map(([value]) => value)).toEqual(stages); expect(worker.terminated).toBe(0);
  const invalid = model(); invalid.meshes[0].positions[0] = Infinity;
  send({ ok: true, model: invalid }); await expect(result).rejects.toMatchObject({ code: 'invalid' });
  expect(worker.terminated).toBe(1);
});
it('stage notifications neither extend the original deadline nor publish after cancellation', async () => {
  vi.stubGlobal('Worker', ImportWorker); vi.useFakeTimers();
  const controller = new AbortController(), stage = vi.fn();
  const result = parseGeometry(new Uint8Array([1]), 'step', controller.signal, stage); await Promise.resolve(); const worker = ImportWorker.instances[0], late = worker.onmessage!;
  const cancelled = expect(result).rejects.toMatchObject({ name: 'AbortError' });
  late({ data: { kind: 'stage', stage: 'initializing' } }); controller.abort();
  late({ data: { kind: 'stage', stage: 'importing' } }); late({ data: { ok: true, model: model() } }); await cancelled;
  expect(stage).toHaveBeenCalledExactlyOnceWith('initializing'); expect(worker.terminated).toBe(1);
  const timed = parseGeometry(new Uint8Array([1]), 'step', new AbortController().signal, stage); await Promise.resolve(); const timedWorker = ImportWorker.instances[1];
  const rejected = expect(timed).rejects.toMatchObject({ code: 'timeout' });
  await vi.advanceTimersByTimeAsync(GEOMETRY_LIMITS.cadTimeoutMs - 1);
  timedWorker.onmessage!({ data: { kind: 'stage', stage: 'initializing' } });
  await vi.advanceTimersByTimeAsync(1); await rejected; expect(timedWorker.terminated).toBe(1);
});

it('loads two admitted Workers during compilation, retires cancelled input and dispatches only live owners', async () => {
  vi.stubGlobal('Worker', ImportWorker);
  let complete!: (value: WebAssembly.Module) => void;
  code.load.mockReturnValue(new Promise(resolve => { complete = resolve; }));
  const first = new AbortController(), second = new AbortController(), third = new AbortController(), source = new Uint8Array([1,2,3]);
  const slice = vi.spyOn(source, 'slice'), stage = vi.fn(), removed = vi.spyOn(first.signal, 'removeEventListener');
  const a = parseGeometry(source, 'step', first.signal, stage), b = parseGeometry(new Uint8Array([4]), 'brep', second.signal);
  const cancelled = expect(a).rejects.toMatchObject({ name: 'AbortError' });
  expect(ImportWorker.instances).toHaveLength(2); expect(slice).not.toHaveBeenCalled();
  expect(ImportWorker.instances.map(worker => worker.input)).toEqual([undefined, undefined]);
  await expect(parseGeometry(source, 'iges', third.signal)).rejects.toMatchObject({ code: 'capacity' });
  first.abort(); await cancelled; expect(removed).toHaveBeenCalledOnce(); expect(ImportWorker.instances[0].terminated).toBe(1);
  const c = parseGeometry(source, 'iges', third.signal);
  expect(ImportWorker.instances).toHaveLength(3); expect(ImportWorker.instances.filter(worker => !worker.terminated)).toHaveLength(2);
  complete(module); await Promise.resolve();
  expect(ImportWorker.instances).toHaveLength(3); expect(stage).toHaveBeenCalledExactlyOnceWith('initializing');
  expect(slice).toHaveBeenCalledOnce(); expect(source).toEqual(new Uint8Array([1,2,3]));
  expect(ImportWorker.instances[0].input).toBeUndefined();
  const workers = ImportWorker.instances.slice(1);
  expect(workers[0].input?.module).toBe(module); expect(workers[1].input?.module).toBe(module);
  expect(workers[1].input?.bytes.buffer).not.toBe(source.buffer);
  for (const worker of workers) worker.onmessage!({ data: { ok: true, model: model() } });
  (await Promise.all([b,c])).forEach(prepared => prepared.dispose()); expect(workers.map(worker => worker.terminated)).toEqual([1,1]);
});
it('keeps the original deadline while awaiting code and fences a late successful compilation', async () => {
  vi.stubGlobal('Worker', ImportWorker); vi.useFakeTimers();
  let complete!: (value: WebAssembly.Module) => void;
  code.load.mockReturnValue(new Promise(resolve => { complete = resolve; }));
  const source = new Uint8Array([1,2,3]), slice = vi.spyOn(source, 'slice');
  const pending = parseGeometry(source, 'step', new AbortController().signal), timed = expect(pending).rejects.toMatchObject({ code: 'timeout' });
  await vi.advanceTimersByTimeAsync(GEOMETRY_LIMITS.cadTimeoutMs); await timed;
  complete(module); await Promise.resolve(); expect(ImportWorker.instances).toHaveLength(1); expect(ImportWorker.instances[0].terminated).toBe(1); expect(ImportWorker.instances[0].input).toBeUndefined(); expect(slice).not.toHaveBeenCalled();
  const controller = new AbortController(), next = parseGeometry(source, 'stl', controller.signal), cancelled = expect(next).rejects.toMatchObject({ name: 'AbortError' });
  expect(ImportWorker.instances).toHaveLength(2); controller.abort(); await cancelled;
});
it('retires the waiting Worker on compilation failure and allows an explicit retry', async () => {
  vi.stubGlobal('Worker', ImportWorker);
  code.load.mockRejectedValueOnce(new TypeError('Unavailable asset'));
  await expect(parseGeometry(new Uint8Array([1]), 'step', new AbortController().signal)).rejects.toMatchObject({ code: 'worker' });
  expect(ImportWorker.instances).toHaveLength(1); expect(ImportWorker.instances[0].terminated).toBe(1); expect(ImportWorker.instances[0].input).toBeUndefined();
  const result = parseGeometry(new Uint8Array([1]), 'step', new AbortController().signal); await Promise.resolve();
  ImportWorker.instances[1].onmessage!({ data: { ok: true, model: model() } }); const prepared = await result; expect(prepared.scene.children).toHaveLength(1); prepared.dispose();
});
it.each(['position', 'normal', 'color', 'unused'])('independently rejects invalid %s values during fused receiver admission', async kind => {
  vi.stubGlobal('Worker', ImportWorker);
  const source = model();
  if (kind === 'position') source.meshes[0].positions[0] = Infinity;
  if (kind === 'normal') source.meshes[0].normals = new Float32Array([NaN,0,1, 0,0,1, 0,0,1]);
  if (kind === 'color') source.meshes[0].colors = new Float32Array([-1,0,0, 0,0,0, 0,0,0]);
  if (kind === 'unused') { const unused = model().meshes[0]; unused.positions[0] = NaN; source.meshes.push(unused); }
  const result = parseGeometry(new Uint8Array([1]), 'stl', new AbortController().signal);
  ImportWorker.instances[0].onmessage!({ data: { ok: true, model: source } });
  await expect(result).rejects.toMatchObject({ code: 'invalid' }); expect(ImportWorker.instances[0].terminated).toBe(1);
});
it('disposes an admitted scene if receiver preparation consumes the original deadline', async () => {
  vi.stubGlobal('Worker', ImportWorker); let now = 0;
  vi.spyOn(performance, 'now').mockImplementation(() => now);
  const setAttribute = BufferGeometry.prototype.setAttribute, disposed = vi.spyOn(BufferGeometry.prototype, 'dispose');
  vi.spyOn(BufferGeometry.prototype, 'setAttribute').mockImplementation(function (this: BufferGeometry, ...args) { now = GEOMETRY_LIMITS.timeoutMs + 1; return Reflect.apply(setAttribute, this, args); });
  const result = parseGeometry(new Uint8Array([1]), 'stl', new AbortController().signal);
  ImportWorker.instances[0].onmessage!({ data: { ok: true, model: model() } });
  await expect(result).rejects.toMatchObject({ code: 'timeout' }); expect(disposed).toHaveBeenCalledOnce();
});
it('fences an early Worker result and retires a crashed waiting Worker before code arrives', async () => {
  vi.stubGlobal('Worker', ImportWorker);
  let complete!: (value: WebAssembly.Module) => void;
  code.load.mockReturnValue(new Promise(resolve => { complete = resolve; }));
  const source = new Uint8Array([1,2,3]), slice = vi.spyOn(source, 'slice');
  const pending = parseGeometry(source, 'step', new AbortController().signal), rejected = expect(pending).rejects.toMatchObject({ code: 'worker' }), worker = ImportWorker.instances[0];
  worker.onmessage!({ data: { ok: true, model: model() } }); expect(worker.terminated).toBe(0);
  worker.onerror!(); await rejected; complete(module); await Promise.resolve();
  expect(worker.terminated).toBe(1); expect(worker.input).toBeUndefined(); expect(slice).not.toHaveBeenCalled(); expect(source).toEqual(new Uint8Array([1,2,3]));
});
it('retains the optional Worker across a slow first GPU frame and accepts its preview after starting', async () => {
  vi.stubGlobal('Worker', ImportWorker); vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'performance'] });
  const source = model(), pending = parseGeometry(new Uint8Array([1]), 'stl', new AbortController().signal), worker = ImportWorker.instances[0];
  worker.onmessage!({ data: { ok:true, model:source, pendingPreviews:[0] } }); const prepared = await pending;
  try {
    expect(worker.terminated).toBe(0); const geometry = (prepared.scene.children[0].children[0] as any).geometry;
    expect(geometryPreviewState(geometry)?.pending).toBe(true);
    const settled = vi.fn(), post = vi.spyOn(worker, 'postMessage'); void prepared.ready.then(settled);
    await vi.advanceTimersByTimeAsync(GEOMETRY_LIMITS.previewTimeoutMs + 1);
    expect(worker.terminated).toBe(0); expect(settled).not.toHaveBeenCalled(); expect(post).not.toHaveBeenCalled();
    prepared.startPreview();
    prepared.startPreview();
    expect(post).toHaveBeenCalledExactlyOnceWith({ kind: 'preview-start' });
    worker.onmessage!({ data:{kind:'preview',previews:[{mesh:0,data:{positions:source.meshes[0].positions.slice(),normals:new Float32Array([0,0,1,0,0,1,0,0,1]),sourceVertices:new Uint32Array([0,1,2]),normalVertices:new Uint32Array([0,1,2]),error:0}}]} });
    await prepared.ready; expect(worker.terminated).toBe(1); expect(geometryPreviewState(geometry)?.geometry).toBeDefined();
    expect(geometryPreviewState(geometry)?.pending).toBe(false);
  } finally { prepared.dispose(); }
});
it('bounds started optional work without extending its budget on repeated starts or replacing the admitted scene', async () => {
  vi.stubGlobal('Worker', ImportWorker); vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'performance'] });
  const pending = parseGeometry(new Uint8Array([1]), 'stl', new AbortController().signal), worker = ImportWorker.instances[0];
  worker.onmessage!({data:{ok:true,model:model(),pendingPreviews:[0]}}); const prepared=await pending;
  try {
    prepared.startPreview();
    await vi.advanceTimersByTimeAsync(GEOMETRY_LIMITS.previewTimeoutMs - 1); expect(worker.terminated).toBe(0);
    prepared.startPreview();
    await vi.advanceTimersByTimeAsync(1); await prepared.ready;
    expect(worker.terminated).toBe(1); expect(prepared.scene.children).toHaveLength(1);
  }
  finally { prepared.dispose(); }
});
it.each([false, true])('keeps the original deadline for optional work even when started late: %s', async started => {
  vi.stubGlobal('Worker', ImportWorker); vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'performance'] });
  const pending = parseGeometry(new Uint8Array([1]), 'stl', new AbortController().signal), worker = ImportWorker.instances[0];
  worker.onmessage!({ data: { ok: true, model: model(), pendingPreviews: [0] } }); const prepared = await pending;
  try {
    await vi.advanceTimersByTimeAsync(GEOMETRY_LIMITS.timeoutMs - 1000); expect(worker.terminated).toBe(0);
    if (started) prepared.startPreview();
    await vi.advanceTimersByTimeAsync(999); expect(worker.terminated).toBe(0);
    await vi.advanceTimersByTimeAsync(1); await prepared.ready;
    expect(worker.terminated).toBe(1); expect(prepared.scene.children).toHaveLength(1);
    const post = vi.spyOn(worker, 'postMessage'); prepared.startPreview(); expect(post).not.toHaveBeenCalled();
  } finally { prepared.dispose(); }
});
it.each(['abort', 'dispose'] as const)('releases waiting optional ownership exactly once on %s and fences late messages', async action => {
  vi.stubGlobal('Worker', ImportWorker); vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'performance'] });
  const controller = new AbortController();
  const pending = parseGeometry(new Uint8Array([1]), 'stl', controller.signal), worker = ImportWorker.instances[0], late = worker.onmessage!;
  late({ data: { ok: true, model: model(), pendingPreviews: [0] } }); const prepared = await pending;
  const geometry = (prepared.scene.children[0].children[0] as any).geometry, settled = vi.fn(); void prepared.ready.then(settled);
  const secondPending = parseGeometry(new Uint8Array([2]), 'stl', new AbortController().signal), second = ImportWorker.instances[1];
  second.onmessage!({ data: { ok: true, model: model(), pendingPreviews: [0] } }); const other = await secondPending;
  let replacement: Awaited<ReturnType<typeof parseGeometry>> | undefined;
  try {
    await expect(parseGeometry(new Uint8Array(), 'stl', new AbortController().signal)).rejects.toMatchObject({ code: 'capacity' });
    await vi.advanceTimersByTimeAsync(GEOMETRY_LIMITS.previewTimeoutMs + 1);
    expect(worker.terminated).toBe(0); expect(second.terminated).toBe(0);
    if (action === 'abort') controller.abort(); else prepared.dispose();
    controller.abort(); prepared.dispose(); await prepared.ready;
    expect(worker.terminated).toBe(1); expect(settled).toHaveBeenCalledOnce();
    expect(worker.onmessage).toBeNull(); expect(geometryPreviewState(geometry)?.geometry).toBeUndefined();
    late({ data: { kind: 'preview', previews: [] } }); late({ data: { ok: true, model: model(), pendingPreviews: [0] } });
    prepared.startPreview(); expect(worker.terminated).toBe(1); expect(settled).toHaveBeenCalledOnce();
    const next = parseGeometry(new Uint8Array([3]), 'stl', new AbortController().signal), nextWorker = ImportWorker.instances[2];
    nextWorker.onmessage!({ data: { ok: true, model: model(), pendingPreviews: [0] } }); replacement = await next;
    await expect(parseGeometry(new Uint8Array(), 'stl', new AbortController().signal)).rejects.toMatchObject({ code: 'capacity' });
    expect(ImportWorker.instances.filter(value => !value.terminated)).toHaveLength(2);
  } finally { prepared.dispose(); other.dispose(); replacement?.dispose(); }
  expect(ImportWorker.instances.map(value => value.terminated)).toEqual([1, 1, 1]); expect(vi.getTimerCount()).toBe(0);
});

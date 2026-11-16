import { afterEach, expect, it, vi } from 'vitest';
import { BufferGeometry, Mesh } from 'three';
import { parseGeometry, type GeometryLod } from '../src/app/components/documents/geometry/parse';
import type { GeometryModel } from '../src/app/components/documents/geometry/model';

vi.mock('../src/app/components/documents/geometry/cad-code', () => ({ cadCode: async () => new WebAssembly.Module(new Uint8Array([0,97,115,109,1,0,0,0])) }));
class ImportWorker {
  static current: ImportWorker;
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onerror = null; onmessageerror = null; input: any; terminate = vi.fn();
  constructor() { ImportWorker.current = this; }
  postMessage(input: unknown) { this.input = input; }
}
const model = (): GeometryModel => ({ root: { name: 'Assembly', meshes: [], children: [{ name: 'First', meshes: [0], children: [] }, { name: 'Second', meshes: [1], children: [] }] }, meshes: [0,2].map(x => ({ name: `Surface ${x}`, positions: new Float32Array([x,0,0,x+1,0,0,x,1,0]), groups: [] })) });
afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); });
it('admits each entire LOD hierarchy before final settlement through one Worker', async () => {
  vi.stubGlobal('Worker', ImportWorker);
  const tiers: GeometryLod[] = [], controller = new AbortController();
  const pending = parseGeometry(new Uint8Array([1]), 'step', controller.signal, undefined, undefined, tier => tiers.push(tier));
  void pending.catch(() => {});
  try {
    await Promise.resolve(); const worker = ImportWorker.current;
    expect(worker.input.lod).toBe(true);
    for (const level of [0,1]) worker.onmessage!({ data: { kind: 'lod', level, model: model() } });
    expect(tiers.map(tier => tier.level)).toEqual([0,1]);
    for (const tier of tiers) {
      const names: string[] = []; tier.scene.traverse(part => { if (part instanceof Mesh) names.push(part.name); });
      expect(names).toEqual(['Surface 0','Surface 2']); expect(tier.scene.getObjectByName('Assembly')?.children.map(part => part.name)).toEqual(['First','Second']);
      expect(tier.bounds.max.toArray()).toEqual([3,1,0]);
    }
    expect(worker.terminate).not.toHaveBeenCalled();
    worker.onmessage!({ data: { ok: true, model: model() } });
    const full = await pending; expect(full.bounds.max.toArray()).toEqual([3,1,0]); full.dispose(); expect(worker.terminate).toHaveBeenCalledOnce();
  } finally { controller.abort(); tiers.forEach(tier => tier.dispose()); await pending.then(value => value.dispose(), () => {}); }
});
it.each(['numeric','bounds','duplicate','reversed','level'] as const)('rejects %s LOD output before its consumer can observe it', async mode => {
  vi.stubGlobal('Worker', ImportWorker); const tiers: GeometryLod[] = [];
  const pending = parseGeometry(new Uint8Array([1]), 'step', new AbortController().signal, undefined, undefined, tier => tiers.push(tier));
  const rejected = expect(pending).rejects.toMatchObject({ code: 'invalid' }); await Promise.resolve(); const worker = ImportWorker.current;
  if (mode === 'duplicate' || mode === 'reversed') worker.onmessage!({ data: { kind: 'lod', level: 1, model: model() } });
  const value = model();
  if (mode === 'numeric') value.meshes[1].positions[0] = NaN;
  if (mode === 'bounds') value.meshes[1].positions[0] = 2e12;
  worker.onmessage!({ data: { kind: 'lod', level: mode === 'duplicate' ? 1 : mode === 'level' ? 2 : 0, model: value } });
  await rejected; expect(tiers).toHaveLength(mode === 'duplicate' || mode === 'reversed' ? 1 : 0);
  expect(worker.terminate).toHaveBeenCalledOnce(); tiers.forEach(tier => tier.dispose());
});
it('disposes a rejected consumer tier and fences every late message after cancellation', async () => {
  vi.stubGlobal('Worker', ImportWorker); const disposed = vi.spyOn(BufferGeometry.prototype, 'dispose'), controller = new AbortController();
  const pending = parseGeometry(new Uint8Array([1]), 'step', controller.signal, undefined, undefined, () => { throw new Error('Consumer unavailable'); });
  const rejected = expect(pending).rejects.toMatchObject({ code: 'invalid' }); await Promise.resolve(); const worker = ImportWorker.current, late = worker.onmessage;
  worker.onmessage!({ data: { kind: 'lod', level: 0, model: model() } }); await rejected;
  expect(disposed).toHaveBeenCalledTimes(2); controller.abort(); late?.({ data: { kind: 'lod', level: 1, model: model() } }); expect(disposed).toHaveBeenCalledTimes(2);
  expect(worker.terminate).toHaveBeenCalledOnce();
});
it('leaves delivered tiers with their owner and fences late LOD after abort', async () => {
  vi.stubGlobal('Worker', ImportWorker); const controller = new AbortController(), tiers: GeometryLod[] = [];
  const pending = parseGeometry(new Uint8Array([1]), 'step', controller.signal, undefined, undefined, tier => tiers.push(tier));
  const cancelled = expect(pending).rejects.toMatchObject({ name: 'AbortError' }); await Promise.resolve(); const worker = ImportWorker.current, late = worker.onmessage!;
  late({ data: { kind: 'lod', level: 1, model: model() } }); controller.abort(); await cancelled;
  late({ data: { kind: 'lod', level: 1, model: model() } }); late({ data: { ok: true, model: model() } }); expect(tiers).toHaveLength(1);
  const disposed = vi.spyOn(BufferGeometry.prototype, 'dispose'); tiers[0].dispose(); tiers[0].dispose(); expect(disposed).toHaveBeenCalledTimes(2);
});

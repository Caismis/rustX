import { afterEach, expect, it, vi } from 'vitest';
import { parseGeometry, type GeometryPartial } from '../src/app/components/documents/geometry/parse';
import type { GeometryModel } from '../src/app/components/documents/geometry/model';
import { GEOMETRY_LIMITS as L } from '../src/app/components/documents/geometry/limits';

vi.mock('../src/app/components/documents/geometry/cad-code', () => ({ cadCode: async () => new WebAssembly.Module(new Uint8Array([0,97,115,109,1,0,0,0])) }));
class ImportWorker {
  static current: ImportWorker;
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onerror = null; onmessageerror = null; input: any; terminate = vi.fn();
  constructor() { ImportWorker.current = this; }
  postMessage(input: unknown) { this.input = input; }
}
const model = (): GeometryModel => ({ root: { name: '', meshes: [0], children: [] }, meshes: [{ name: 'Computed mesh', positions: new Float32Array([0,0,0,1,0,0,0,1,0]), groups: [] }] });
afterEach(() => vi.unstubAllGlobals());
it('admits a computed partial before final settlement while retaining the same Worker and final geometry', async () => {
  vi.stubGlobal('Worker', ImportWorker);
  const parts: GeometryPartial[] = [], controller = new AbortController();
  const pending = parseGeometry(new Uint8Array([1]), 'step', controller.signal, undefined, undefined, part => parts.push(part));
  void pending.catch(() => {});
  try {
    await Promise.resolve(); const worker = ImportWorker.current;
    expect(worker.input.progressive).toBe(true);
    worker.onmessage!({ data: { kind: 'partial', sequence: 1, meshIndex: 0, model: model() } });
    expect(parts).toHaveLength(1); expect(parts[0].bounds.max.toArray()).toEqual([1,1,0]); expect(worker.terminate).not.toHaveBeenCalled();
    worker.onmessage!({ data: { ok: true, model: model() } });
    const full = await pending; expect(full.bounds.max.toArray()).toEqual([1,1,0]); full.dispose(); expect(worker.terminate).toHaveBeenCalledOnce();
  } finally { controller.abort(); parts.forEach(part => part.dispose()); await pending.then(value => value.dispose(), () => {}); }
});
it('rejects invalid numeric partials before their consumer can observe them', async () => {
  vi.stubGlobal('Worker', ImportWorker); const consume = vi.fn();
  const pending = parseGeometry(new Uint8Array([1]), 'step', new AbortController().signal, undefined, undefined, consume);
  const rejected = expect(pending).rejects.toMatchObject({code:'invalid'}); await Promise.resolve();
  const value = model(); value.meshes[0].positions[0] = NaN;
  ImportWorker.current.onmessage!({data:{kind:'partial',sequence:1,meshIndex:0,model:value}});
  await rejected; expect(consume).not.toHaveBeenCalled(); expect(ImportWorker.current.terminate).toHaveBeenCalledOnce();
});
it('bounds the aggregate partial prefix and fences every late message after cancellation', async () => {
  vi.stubGlobal('Worker', ImportWorker); const parts: GeometryPartial[] = [], controller = new AbortController();
  const pending = parseGeometry(new Uint8Array([1]), 'step', controller.signal, undefined, undefined, part => parts.push(part));
  const rejected = expect(pending).rejects.toMatchObject({code:'limit'}); await Promise.resolve(); const worker = ImportWorker.current, late = worker.onmessage;
  for(let i=0;i<=L.partialUpdates;i++) worker.onmessage?.({data:{kind:'partial',sequence:i+1,meshIndex:i,model:model()}});
  await rejected; expect(parts).toHaveLength(L.partialUpdates);
  controller.abort(); late?.({data:{kind:'partial',sequence:L.partialUpdates+2,meshIndex:30,model:model()}});
  expect(parts).toHaveLength(L.partialUpdates); parts.forEach(part => part.dispose());
});

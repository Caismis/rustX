import { beforeEach, expect, it, vi } from 'vitest';
import { importCad, CAD_TRIANGULATION } from '../src/app/components/documents/geometry/cad';
const runtime = vi.hoisted(() => ({ create: vi.fn(), read: vi.fn() }));
vi.mock('occt-import-js', () => ({ default: runtime.create }));
const module = new WebAssembly.Module(new Uint8Array([0,97,115,109,1,0,0,0]));
function result(triangles: number) {
  return { success: true, root: { name: 'Assembly', meshes: [], children: [0,1].map(index => ({ name: `Part ${index}`, meshes: [index], children: [] })) },
    meshes: [0,2].map(x => ({ name: `Surface ${x}`, color: [.2,.4,.6], attributes: { position: { array: [x,0,0,x+1,0,0,x,1,0] } }, index: { array: Array.from({ length: triangles * 3 }, (_, index) => index % 3) } })) };
}
beforeEach(() => { runtime.create.mockReset().mockResolvedValue({ ReadStepFile: runtime.read }); runtime.read.mockReset(); });
it('requests complete native LOD results from the same module/read with unchanged final settings', async () => {
  const coarse = result(1), medium = result(2), full = result(3);
  runtime.read.mockImplementation((_bytes, options) => { options.onLod(0,coarse); options.onLod(1,medium); return full; });
  const tiers: any[] = [], source = new Uint8Array([1,2,3]);
  const final = await importCad(source,'step',module,undefined,false,(level,model) => tiers.push({ level, model }));
  expect(runtime.create).toHaveBeenCalledOnce(); expect(runtime.read).toHaveBeenCalledExactlyOnceWith(source, { ...CAD_TRIANGULATION, onLod: expect.any(Function) });
  expect(tiers.map(tier => tier.level)).toEqual([0,1]);
  expect([...tiers.map(tier => tier.model),final].map(model => model.meshes.map((mesh: any) => mesh.indices.length / 3))).toEqual([[1,1],[2,2],[3,3]]);
  tiers.forEach(tier => expect(tier.model.root).toEqual(final.root));
});
it('omits optional LOD work when no consumer is present', async () => {
  runtime.read.mockReturnValue(result(1)); await importCad(new Uint8Array([1]),'step',module);
  expect(runtime.read.mock.calls[0][1]).toEqual(CAD_TRIANGULATION);
});
it('skips oversized or invalid optional tiers and independently admits the final result', async () => {
  runtime.read.mockImplementation((_bytes, options) => { options.onLod(0,result(65_537)); options.onLod(1,result(2)); options.onLod(1,result(3)); return result(4); });
  const publish = vi.fn(), final = await importCad(new Uint8Array([1]),'step',module,undefined,false,publish);
  expect(publish).toHaveBeenCalledOnce(); expect(publish.mock.calls[0][0]).toBe(1); expect(final.meshes[0].indices).toHaveLength(12);
});
it('copies optional palette/hierarchy data so native later results survive consumer mutation', async () => {
  const raw = result(1);
  runtime.read.mockImplementation((_bytes, options) => { options.onLod(0,raw); options.onLod(1,raw); return raw; });
  const final = await importCad(new Uint8Array([1]),'step',module,undefined,false,(_level,model) => {
    model.meshes.forEach(mesh => { mesh.positions.fill(999); mesh.color!.fill(0); }); model.root.children.forEach(child => { child.meshes.length = 0; }); model.root.children.length = 0;
  });
  expect(final.root.children).toHaveLength(2); expect(final.meshes[0].color).toEqual([.2,.4,.6]); expect(final.meshes[0].positions[0]).toBe(0);
  expect(final.root.children.map(child => child.meshes)).toEqual([[0],[1]]);
});

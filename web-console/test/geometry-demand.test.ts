import { beforeEach, expect, it, vi } from 'vitest';
const runtime = vi.hoisted(() => ({ cad: vi.fn(async () => new WebAssembly.Module(new Uint8Array([0,97,115,109,1,0,0,0]))) }));
vi.mock('../src/app/components/documents/geometry/GeometryPreview', () => ({ default: () => null }));
vi.mock('../src/app/components/documents/geometry/cad-code', () => ({ cadCode: runtime.cad }));
vi.mock('three/addons/loaders/GLTFLoader.js', () => ({}));
beforeEach(() => { vi.resetModules(); runtime.cad.mockClear(); });
it.each(['assembly.iges', 'assembly.step', 'assembly.stp'])('starts immutable CAD compilation for %s on explicit demand before source bytes or a mounted body exist', async name => {
  const { preloadGeometry } = await import('../src/app/components/documents/GeometryDocument');
  expect(runtime.cad).not.toHaveBeenCalled(); preloadGeometry(name); await vi.dynamicImportSettled();
  expect(runtime.cad).toHaveBeenCalledOnce();
});
it('keeps CAD compilation out of ordinary evaluation and STL/GLB demand', async () => {
  const { preloadGeometry } = await import('../src/app/components/documents/GeometryDocument');
  preloadGeometry('mesh.stl'); preloadGeometry('mesh.glb'); await vi.dynamicImportSettled(); expect(runtime.cad).not.toHaveBeenCalled();
});

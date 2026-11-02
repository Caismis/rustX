// @vitest-environment node
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { expect, it, vi } from 'vitest';
import { Frustum, Matrix4, Mesh, MeshStandardMaterial, PerspectiveCamera, Raycaster, Vector2 } from 'three';
import { admitGlb, importGlb } from '../src/app/components/documents/geometry/glb';
import { importStl } from '../src/app/components/documents/geometry/stl';
import { importCad, convertCadResult } from '../src/app/components/documents/geometry/cad';
import { modelScene, validateScene, disposeScene } from '../src/app/components/documents/geometry/scene-resources';
import { viewersForPath, loadsBytes } from '../src/app/components/workbench-documents/definitions';

vi.mock('occt-import-js/dist/occt-import-js.wasm?url', async () => ({ default: (await import('node:url')).fileURLToPath(new URL('../node_modules/occt-import-js/dist/occt-import-js.wasm', import.meta.url)) }));
const fixture = (name: string) => new Uint8Array(readFileSync(fileURLToPath(new URL(`./fixtures/geometry/${name}`, import.meta.url))));
it.each(['      ', '\n\t        ', 'uppercase'])('ASCII STL admission and loading agree with prefix %j without excessive allocation', prefix => {
  const source = new TextDecoder().decode(fixture('cube-ascii.stl')).replaceAll('cube', 'MixedCasePart');
  const text = prefix === 'uppercase' ? source.replace(/^(solid|endsolid|facet normal|outer loop|vertex|endloop|endfacet)\b/gm, token => token.toUpperCase()) : prefix + source;
  const Native = globalThis.Float32Array;
  vi.stubGlobal('Float32Array', new Proxy(Native, { construct(target, args, receiver) {
    if (typeof args[0] === 'number' && args[0] > 3_000_000) throw new Error('Excessive STL allocation intercepted');
    return Reflect.construct(target, args, receiver);
  } }));
  try { const model = importStl(new TextEncoder().encode(text)); expect(model.meshes[0].positions.length).toBe(108); expect(model.meshes[0].name).toBe('MixedCasePart'); }
  finally { vi.unstubAllGlobals(); }
});
function modifyGlb(change: (value: any) => void) {
  const bytes = fixture('cube.glb'), view = new DataView(bytes.buffer), length = view.getUint32(12, true);
  const json = JSON.parse(new TextDecoder().decode(bytes.subarray(20, 20 + length))); change(json);
  const text = new TextEncoder().encode(JSON.stringify(json)), padded = Math.ceil(text.length / 4) * 4;
  const bin = bytes.subarray(20 + length), next = new Uint8Array(20 + padded + bin.length), header = new DataView(next.buffer);
  next.set(bytes.subarray(0, 12)); header.setUint32(8, next.length, true); header.setUint32(12, padded, true); header.setUint32(16, 0x4e4f534a, true);
  next.fill(32, 20, 20 + padded); next.set(text, 20); next.set(bin, 20 + padded); return next;
}
it.each(['GLB', 'stl', 'STP', 'step', 'IGES', 'igs', 'BREP'])('registers %s as a geometry byte read without a text fallback', extension => {
  expect(viewersForPath(`part.${extension}`)).toEqual(['geometry']); expect(loadsBytes('geometry')).toBe(true);
});
it.each(['cube.stl', 'cube-ascii.stl'])('imports the analytic cube from %s with outward normals and preserved scale', name => {
  const model = importStl(fixture(name)), object = modelScene(model);
  try {
    const bounds = validateScene([object]); expect(bounds.min.toArray()).toEqual([0, 0, 0]); expect(bounds.max.toArray()).toEqual([10, 10, 10]);
    expect(model.meshes[0].positions.length).toBe(108);
    const { positions, normals } = model.meshes[0];
    for (let at = 0; at < positions.length; at += 9) {
      const a = Array.from(positions.subarray(at, at + 3)), b = Array.from(positions.subarray(at + 3, at + 6)), c = Array.from(positions.subarray(at + 6, at + 9));
      const cross = [(b[1]-a[1])*(c[2]-a[2])-(b[2]-a[2])*(c[1]-a[1]), (b[2]-a[2])*(c[0]-a[0])-(b[0]-a[0])*(c[2]-a[2]), (b[0]-a[0])*(c[1]-a[1])-(b[1]-a[1])*(c[0]-a[0])];
      expect(cross.reduce((sum, value, axis) => sum + value * normals![at + axis], 0)).toBeGreaterThan(0);
    }
  } finally { disposeScene([object]); }
});
it.each([['cube.step', 'step'], ['cube-inch.step', 'step'], ['cube.iges', 'iges'], ['cube.brep', 'brep']] as const)('imports %s against the independent analytic 10-unit reference', async (name, format) => {
  const model = await importCad(fixture(name), format), object = modelScene(model);
  try { const bounds = validateScene([object]); bounds.min.toArray().forEach(value => expect(value).toBeCloseTo(0, 5)); bounds.max.toArray().forEach(value => expect(value).toBeCloseTo(10, 5)); }
  finally { disposeScene([object]); }
}, 30_000);
it('imports GLB hierarchy, color and translated coordinates', async () => {
  const bytes = modifyGlb(json => { json.nodes[0].translation = [5, 6, 7]; });
  const loaded = await importGlb(bytes, new AbortController().signal);
  try {
    const bounds = validateScene([loaded.scene]); expect(bounds.min.toArray()).toEqual([5, 6, 7]); expect(bounds.max.toArray()).toEqual([15, 16, 17]);
    expect(loaded.scene.getObjectByName('Cube_assembly')).toBeDefined();
    const mesh = loaded.scene.getObjectByName('Analytic_cube'); expect(mesh).toBeInstanceOf(Mesh);
    expect((mesh as Mesh).material).toBeInstanceOf(MeshStandardMaterial);
    expect(((mesh as Mesh).material as MeshStandardMaterial).color.toArray()).toEqual([0.15, 0.55, 0.85]);
  } finally { loaded.dispose(); }
});
it('derives GLB culling and raycast spheres from positions rather than accessor bounds', async () => {
  const loaded = await importGlb(modifyGlb(json => { json.accessors[0].min = [100000,100000,100000]; json.accessors[0].max = [100010,100010,100010]; }), new AbortController().signal);
  try {
    const mesh = loaded.scene.getObjectByName('Analytic_cube') as Mesh;
    const camera = new PerspectiveCamera(45, 1, .1, 100); camera.position.set(20,20,20); camera.lookAt(5,5,5); camera.updateMatrixWorld(true);
    const frustum = new Frustum().setFromProjectionMatrix(new Matrix4().multiplyMatrices(camera.projectionMatrix, camera.matrixWorldInverse));
    expect(frustum.intersectsObject(mesh)).toBe(true);
    const ray = new Raycaster(); ray.setFromCamera(new Vector2(), camera); expect(ray.intersectObject(loaded.scene, true).length).toBeGreaterThan(0);
    expect(mesh.geometry.boundingSphere!.center.toArray()).toEqual([5,5,5]);
  } finally { loaded.dispose(); }
});
it('preserves ASCII solid names and separate selectable geometry', () => {
  const cube = new TextDecoder().decode(fixture('cube-ascii.stl'));
  const model = importStl(new TextEncoder().encode(cube.replaceAll('cube', 'First') + cube.replaceAll('cube', 'Second')));
  expect(model.meshes.map(mesh => mesh.name)).toEqual(['First', 'Second']); expect(model.root.meshes).toEqual([0, 1]);
});
it('preserves CAD hierarchy, base colors and face-color ranges', () => {
  const model = convertCadResult({ success: true, root: { name: 'Assembly', meshes: [], children: [{ name: 'Part', meshes: [0], children: [] }] }, meshes: [{ name: 'Body', color: [1, 0, 0], attributes: { position: { array: [0,0,0, 1,0,0, 0,1,0] } }, index: { array: [0,1,2] }, brep_faces: [{ first: 0, last: 0, color: [0,0,1] }] }] });
  const object = modelScene(model);
  try { expect(object.getObjectByName('Assembly')).toBeDefined(); expect(object.getObjectByName('Part')).toBeDefined(); expect(model.meshes[0].color).toEqual([1,0,0]); expect(model.meshes[0].groups).toEqual([{ start: 0, count: 3, color: [0,0,1] }]); }
  finally { disposeScene([object]); }
});
it.each(['https://example.com/private.bin', '../private.bin', '/product-host/file-read', 'file:///private.bin', 'blob:model-url', 'data:image/svg+xml;base64,PHN2Zz4='])('rejects model-directed resource %s before any loader runs', uri => {
  expect(() => admitGlb(modifyGlb(json => { json.buffers[0].uri = uri; }))).toThrowError('external');
});
it('rejects expansion, recursive graphs, missing decoders and nonfinite transforms', () => {
  expect(() => admitGlb(modifyGlb(json => { json.accessors[0].count = 10_000_000; }))).toThrow();
  expect(() => admitGlb(modifyGlb(json => { json.nodes[1].children = [0]; }))).toThrow();
  expect(() => admitGlb(modifyGlb(json => { json.extensionsRequired = ['KHR_draco_mesh_compression']; }))).toThrowError('unsupported');
  expect(() => admitGlb(modifyGlb(json => { json.nodes[0].translation = [1e30, 0, 0]; }))).toThrow();
  expect(() => admitGlb(fixture('cube.glb').subarray(0, 30))).toThrow();
});
it('rejects nonfinite decoded coordinates and invalid STL/CAD indices', async () => {
  const bytes = fixture('cube.glb'), view = new DataView(bytes.buffer), jsonLength = view.getUint32(12, true);
  view.setFloat32(20 + jsonLength + 8, Infinity, true);
  await expect(importGlb(bytes, new AbortController().signal)).rejects.toThrow();
  expect(() => importStl(fixture('cube.stl').subarray(0, 100))).toThrow();
  expect(() => convertCadResult({ success: true, meshes: [{ attributes: { position: { array: [0,0,0,1,0,0,0,1,0] } }, index: { array: [-1, 1, 2] } }], root: { name: '', meshes: [0], children: [] } })).toThrow();
});

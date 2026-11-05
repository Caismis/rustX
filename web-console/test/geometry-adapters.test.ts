// @vitest-environment node
import { readFileSync } from 'node:fs';
import fs from 'node:fs';
import { fileURLToPath } from 'node:url';
import { expect, it, vi } from 'vitest';
import { BufferGeometry, Frustum, Material, Matrix4, Mesh, MeshStandardMaterial, PerspectiveCamera, Raycaster, Vector2, Vector3 } from 'three';
import { STLLoader } from 'three/addons/loaders/STLLoader.js';
import { admitGlb, importGlb } from '../src/app/components/documents/geometry/glb';
import { importStl } from '../src/app/components/documents/geometry/stl';
import { importCad, convertCadResult } from '../src/app/components/documents/geometry/cad';
import { modelScene, validateScene, disposeScene } from '../src/app/components/documents/geometry/scene-resources';
import { viewersForPath, loadsBytes } from '../src/app/components/workbench-documents/definitions';

const cadModule = await WebAssembly.compile(readFileSync(new URL('../node_modules/occt-import-js/dist/occt-import-js.wasm', import.meta.url)));
const fixture = (name: string) => new Uint8Array(readFileSync(fileURLToPath(new URL(`./fixtures/geometry/${name}`, import.meta.url))));
it('publishes completed CAD meshes before the final result without reducing their precision', async () => {
  const partials: ReturnType<typeof convertCadResult>[] = [];
  let complete = false;
  const result = await importCad(fixture('bracket-array.step'), 'step', cadModule, undefined, false, partial => {
    expect(complete).toBe(false); partials.push(partial);
  });
  complete = true;
  expect(partials.length).toBeGreaterThan(0); expect(partials.length).toBeLessThanOrEqual(16);
  partials.forEach((partial,index) => expect(partial.meshes[0]).toEqual(result.meshes[index]));
});
it('keeps the final CAD result independent of mutations to a displayed fragment', async () => {
  const source=fixture('bracket-array.step'),expected=await importCad(source,'step',cadModule);
  const actual=await importCad(source,'step',cadModule,undefined,false,partial=>{
    const mesh=partial.meshes[0];mesh.positions.fill(999);mesh.color?.fill(0);mesh.groups.forEach(group=>group.color.fill(0));partial.root.meshes.length=0;
  });
  expect(actual).toEqual(expected);
});
it.each([['cube.step','step'],['cube.iges','iges'],['cube.brep','brep']] as const)('relinquishes exclusively owned %s bytes only after native copy, preserving exact geometry', async (file,format) => {
  const source=fixture(file),owned=source.slice();
  const expected=await importCad(source,format,cadModule),actual=await importCad(owned,format,cadModule,undefined,true);
  expect(owned.byteLength).toBe(0);expect(source.byteLength).toBeGreaterThan(0);expect(actual).toEqual(expected);
});
it.each(['uncolored','same-color','base-color'])('admits many CAD faces without charging each %s face as a draw call', mode => {
  const count=1024,color=mode==='uncolored'?null:[.2,.4,.6];
  const model=convertCadResult({success:true,root:{name:'',meshes:[0],children:[]},meshes:[{name:'Dense CAD',...(mode==='base-color'?{color}:{}),attributes:{position:{array:[0,0,0,1,0,0,0,1,0]}},index:{array:Array.from({length:count*3},(_,i)=>i%3)},brep_faces:Array.from({length:count},(_,i)=>({first:i,last:i,color}))}]});
  expect(model.meshes[0].indices!.length).toBe(count*3);
  expect(model.meshes[0].groups).toEqual(mode==='same-color'?[{start:0,count:count*3,color}]:[]);
});
it('preserves CAD color gaps and continues to bound actual alternating material ranges', () => {
  const model = (colors: (number[]|null)[]) => ({success:true,root:{name:'',meshes:[0],children:[]},meshes:[{name:'',attributes:{position:{array:[0,0,0,1,0,0,0,1,0]}},index:{array:Array.from({length:colors.length*3},(_,i)=>i%3)},brep_faces:colors.map((color,i)=>({first:i,last:i,color}))}]});
  expect(convertCadResult(model([[0,0,1],null,[0,0,1]])).meshes[0].groups).toEqual([{start:0,count:3,color:[0,0,1]},{start:6,count:3,color:[0,0,1]}]);
  expect(()=>convertCadResult(model(Array.from({length:513},(_,i)=>i%2?[1,0,0]:[0,0,1])))).toThrow('limit');
});
it('rejects instance initialization failure without falling through to the pinned asset loader', async () => {
  const Native = WebAssembly, read = vi.spyOn(fs, 'readFile');
  const Instance = new Proxy(Native.Instance, { construct() { throw new WebAssembly.RuntimeError('Instance unavailable'); } });
  vi.stubGlobal('WebAssembly', new Proxy(Native, { get(target, key) { return key === 'Instance' ? Instance : Reflect.get(target, key); } }));
  try { await expect(importCad(fixture('cube.step'), 'step', cadModule)).rejects.toThrow('Instance unavailable'); expect(read).not.toHaveBeenCalled(); }
  finally { vi.unstubAllGlobals(); read.mockRestore(); }
});
it('shares compiled CAD code while creating fresh instances, heaps and exact typed results', async () => {
  const Native = WebAssembly, instances: WebAssembly.Instance[] = [], modules: WebAssembly.Module[] = [];
  const Instance = new Proxy(Native.Instance, { construct(target, args) {
    const instance = Reflect.construct(target, args); instances.push(instance); modules.push(args[0]); return instance;
  } });
  vi.stubGlobal('WebAssembly', new Proxy(Native, { get(target, key) { return key === 'Instance' ? Instance : Reflect.get(target, key); } }));
  try {
    const first = await importCad(fixture('cube.step'), 'step', cadModule), second = await importCad(fixture('cube.step'), 'step', cadModule);
    expect(modules).toEqual([cadModule, cadModule]); expect(instances).toHaveLength(2); expect(instances[0]).not.toBe(instances[1]);
    const heap = (instance: WebAssembly.Instance) => Object.values(instance.exports).find(value => value instanceof Native.Memory) as WebAssembly.Memory;
    expect(heap(instances[0]).buffer).not.toBe(heap(instances[1]).buffer);
    expect(first).toEqual(second); expect(first.meshes[0].positions).toBeInstanceOf(Float32Array);
    expect(first.meshes[0].positions.buffer).not.toBe(second.meshes[0].positions.buffer);
  } finally { vi.unstubAllGlobals(); }
});
it('adopts whole STL parser buffers without copying the Worker-owned input or attributes', () => {
  const source = fixture('cube.stl'), geometry = new STLLoader().parse(source.buffer);
  const position = geometry.getAttribute('position').array, normal = geometry.getAttribute('normal').array;
  const parse = vi.spyOn(STLLoader.prototype, 'parse').mockReturnValue(geometry);
  try {
    const model = importStl(source);
    expect(parse.mock.calls[0][0]).toBe(source.buffer);
    expect(model.meshes[0].positions).toBe(position); expect(model.meshes[0].normals).toBe(normal);
  } finally { parse.mockRestore(); }
});
it('bounds STL group backing buffers and preserves exact input views', () => {
  const source = fixture('cube.stl'), padded = new Uint8Array(source.length + 64); padded.fill(255); padded.set(source, 32);
  const model = importStl(padded.subarray(32, 32 + source.length)); expect(model.meshes[0].positions.length).toBe(108);
  const text = new TextDecoder().decode(fixture('cube-ascii.stl'));
  const grouped = importStl(new TextEncoder().encode(text + text.replaceAll('cube', 'second')));
  expect(grouped.meshes).toHaveLength(2);
  for (const mesh of grouped.meshes) { expect(mesh.positions.byteLength).toBe(432); expect(mesh.positions.buffer.byteLength).toBe(mesh.positions.byteLength); }
  expect(grouped.meshes[0].positions.buffer).not.toBe(grouped.meshes[1].positions.buffer);
});
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
it.each(['empty', 'degenerate'] as const)('rejects an %s selected GLB scene and disposes geometry in other scenes', async kind => {
  const bytes = modifyGlb(json => {
    json.scene = 0;
    if (kind === 'empty') json.scenes = [{ nodes: [] }, json.scenes[0]];
    else { json.nodes = [{ mesh: 0, scale: [0,0,0] }, { mesh: 0 }]; json.scenes = [{ nodes: [0] }, { nodes: [1] }]; }
  });
  const geometry = vi.spyOn(BufferGeometry.prototype, 'dispose'), material = vi.spyOn(Material.prototype, 'dispose');
  try {
    await expect(importGlb(bytes, new AbortController().signal)).rejects.toMatchObject({ code: 'empty' });
    expect(geometry).toHaveBeenCalled(); expect(material).toHaveBeenCalled();
  } finally { geometry.mockRestore(); material.mockRestore(); }
});
it.each(['GLB', 'stl', 'STP', 'step', 'IGES', 'igs', 'BREP'])('registers %s as a geometry byte read without a text fallback', extension => {
  expect(viewersForPath(`part.${extension}`)).toEqual(['geometry']); expect(loadsBytes('geometry')).toBe(true);
});
it.each(['cube.stl', 'cube-ascii.stl'])('imports the analytic cube from %s with outward normals and preserved scale', name => {
  const model = importStl(fixture(name)), admitted = modelScene(model), object = admitted.scene;
  try {
    const bounds = admitted.bounds; expect(bounds.min.toArray()).toEqual([0, 0, 0]); expect(bounds.max.toArray()).toEqual([10, 10, 10]);
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
  const model = await importCad(fixture(name), format, cadModule), admitted = modelScene(model), object = admitted.scene;
  try { const bounds = admitted.bounds; bounds.min.toArray().forEach(value => expect(value).toBeCloseTo(0, 5)); bounds.max.toArray().forEach(value => expect(value).toBeCloseTo(10, 5)); }
  finally { disposeScene([object]); }
}, 30_000);
it.each(['step', 'glb'] as const)('imports the named twelve-part product assembly from %s with correct units', async format => {
  const bytes = fixture(`mounting-bracket.${format}`);
  const loaded = format === 'glb' ? await importGlb(bytes, new AbortController().signal) : undefined;
  const object = loaded?.scene ?? modelScene(await importCad(bytes, 'step', cadModule)).scene;
  try {
    const unit = format === 'glb' ? 0.001 : 1, bounds = validateScene([object]), names: string[] = [];
    bounds.min.toArray().forEach((value, index) => expect(value).toBeCloseTo([-60, 0, -40][index] * unit, 5));
    bounds.max.toArray().forEach((value, index) => expect(value).toBeCloseTo([60, 72, 40][index] * unit, 5));
    let meshes = 0;
    object.traverse(part => { names.push(part.userData.name ?? part.name); if (part instanceof Mesh) meshes++; });
    expect(meshes).toBe(12); expect(names).toContain('Mounting bracket'); expect(names).toContain('Base plate'); expect(names).toContain('Bolt 4');
  } finally { if (loaded) loaded.dispose(); else disposeScene([object]); }
}, 30_000);
it('imports GLB hierarchy, color and translated coordinates', async () => {
  const bytes = modifyGlb(json => { json.nodes[0].translation = [5, 6, 7]; });
  const loaded = await importGlb(bytes, new AbortController().signal);
  try {
    const bounds = loaded.bounds; expect(bounds.min.toArray()).toEqual([5, 6, 7]); expect(bounds.max.toArray()).toEqual([15, 16, 17]);
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
  const object = modelScene(model).scene;
  try { expect(object.getObjectByName('Assembly')).toBeDefined(); expect(object.getObjectByName('Part')).toBeDefined(); expect(model.meshes[0].color).toEqual([1,0,0]); expect(model.meshes[0].groups).toEqual([{ start: 0, count: 3, color: [0,0,1] }]); }
  finally { disposeScene([object]); }
});
it('admits the 96-part CAD assembly within the existing geometry budgets', async () => {
  const model = await importCad(fixture('bracket-array.step'), 'step', cadModule), admitted = modelScene(model);
  try {
    expect(admitted.bounds.getSize(new Vector3()).toArray()).toEqual([570, 72, 190]);
    let meshes = 0; admitted.scene.traverse(part => { if (part instanceof Mesh) meshes++; }); expect(meshes).toBe(96);
  } finally { disposeScene([admitted.scene]); }
}, 30_000);
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

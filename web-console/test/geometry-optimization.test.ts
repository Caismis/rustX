// @vitest-environment node
import { expect, it, vi } from 'vitest';
import { optimizeModel } from '../src/app/components/documents/geometry/optimize';
import { validateModel, type GeometryModel } from '../src/app/components/documents/geometry/model';
import { modelScene, disposeScene } from '../src/app/components/documents/geometry/scene-resources';
import { BoxGeometry, PlaneGeometry, SphereGeometry } from 'three';
import { denseBoxStl } from './geometry-fixtures';
import { importStl } from '../src/app/components/documents/geometry/stl';
import type { GeometryMesh } from '../src/app/components/documents/geometry/model';

function triangleBits(mesh: GeometryMesh, key: 'positions' | 'normals' | 'colors') {
  const values = mesh[key]; if (!values) return undefined;
  const bits = new Uint32Array(values.buffer, values.byteOffset, values.length);
  if (!mesh.indices) return Buffer.from(bits.buffer, bits.byteOffset, bits.byteLength);
  const expanded = new Uint32Array(mesh.indices.length * 3);
  for (let i = 0; i < mesh.indices.length; i++) expanded.set(bits.subarray(mesh.indices[i] * 3, mesh.indices[i] * 3 + 3), i * 3);
  return Buffer.from(expanded.buffer);
}
function exactTriangles(source: GeometryMesh, result: GeometryMesh) {
  for (const key of ['positions', 'normals', 'colors'] as const) {
    const before = triangleBits(source, key), after = triangleBits(result, key);
    if (before) expect(before.equals(after!)).toBe(true); else expect(after).toBeUndefined();
  }
}

function grid(side = 180, hole = false, colored = false): GeometryModel {
  const points: number[] = [], colors: number[] = [];
  for (let y = 0; y < side; y++) for (let x = 0; x < side; x++) {
    if (hole && x >= side / 2 - 1 && x < side / 2 + 1 && y >= side / 2 - 1 && y < side / 2 + 1) continue;
    points.push(x,y,0, x+1,y,0, x,y+1,0, x+1,y,0, x+1,y+1,0, x,y+1,0);
    if (colored) for (let vertex = 0; vertex < 6; vertex++) colors.push(...(x < side / 2 ? [1,0,0] : [0,0,1]));
  }
  const positions = new Float32Array(points), normals = new Float32Array(points.length);
  for (let at = 2; at < normals.length; at += 3) normals[at] = 1;
  const model = { meshes: [{ name: 'Grid', positions, normals, colors: colored ? new Float32Array(colors) : undefined, groups: [] }], root: { name: 'Assembly', meshes: [0], children: [] } };
  validateModel(model); return model;
}
function area(model: GeometryModel) {
  const mesh = model.meshes[0], p = mesh.positions, indices = mesh.indices!; let total = 0;
  for (let at = 0; at < indices.length; at += 3) {
    const a = indices[at]*3, b = indices[at+1]*3, c = indices[at+2]*3;
    total += Math.abs((p[b]-p[a])*(p[c+1]-p[a+1])-(p[b+1]-p[a+1])*(p[c]-p[a]))/2;
  }
  return total;
}
it('certifies a convex planar disk and retains its exact surface, hierarchy and original arrays', async () => {
  const source = grid(), before = source.meshes[0].positions.slice(), result = await optimizeModel(source);
  expect(result.meshes[0].indices!.length).toBe(6);
  expect(result.meshes[0].positions.length).toBeLessThan(source.meshes[0].positions.length/2);
  expect(area(result)).toBe(180*180); expect(result.root).toBe(source.root);
  expect(source.meshes[0].positions).toEqual(before); expect(source.meshes[0].indices).toBeUndefined();
  const admitted = modelScene(result);
  try { expect(admitted.bounds.min.toArray()).toEqual([0,0,0]); expect(admitted.bounds.max.toArray()).toEqual([180,180,0]); }
  finally { disposeScene([admitted.scene]); }
});
it('certifies constant RGB tuples while retaining authored signed-zero normals at the hull', () => {
  const model = grid(), mesh = model.meshes[0];
  mesh.colors = new Float32Array(mesh.positions.length);
  for (let at = 0; at < mesh.positions.length; at += 3) {
    mesh.colors.set([0.25, 0.5, 0.75], at);
    mesh.normals![at] = mesh.positions[at] === 180 ? -0 : 0;
    mesh.normals![at + 1] = -0;
  }
  const output = optimizeModel(model, Infinity).meshes[0];
  expect(output.indices).toHaveLength(6);
  for (let at = 0; at < output.positions.length; at += 3) {
    expect([...output.colors!.subarray(at, at + 3)]).toEqual([0.25, 0.5, 0.75]);
    expect(Object.is(output.normals![at], output.positions[at] === 180 ? -0 : 0)).toBe(true);
    expect(Object.is(output.normals![at + 1], -0)).toBe(true);
    expect(output.normals![at + 2]).toBe(1);
  }
});
it('retains a small hole and hard color seams instead of closing or blending them', async () => {
  const source = grid(180, true, true), result = await optimizeModel(source);
  exactTriangles(source.meshes[0], result.meshes[0]);
  expect(area(result)).toBe(180*180-4);
  const {positions:p,indices,colors} = result.meshes[0];
  for (let at = 0; at < indices!.length; at += 3) {
    const values = [indices![at], indices![at+1], indices![at+2]];
    expect(new Set(values.map(index => colors![index*3])).size).toBe(1);
    for (const index of values) expect(p[index*3] <= 90 || colors![index*3] === 0).toBe(true);
  }
});
it('preserves dense curved surfaces and thin closed geometry bit for bit, including normals and winding', () => {
  for (const geometry of [new SphereGeometry(1, 160, 100), new BoxGeometry(1, 1, 1e-6, 60, 60, 60)]) {
    const triangles = geometry.toNonIndexed(); geometry.dispose();
    const mesh: GeometryMesh = { name: 'Surface', positions: triangles.getAttribute('position').array as Float32Array, normals: triangles.getAttribute('normal').array as Float32Array, groups: [] };
    const model = { meshes: [mesh], root: { name: '', meshes: [0], children: [] } };
    exactTriangles(mesh, optimizeModel(model).meshes[0]); triangles.dispose();
  }
});
it.each([false, true])('indexes exact signed-zero attribute tuples without merging seams (colors: %s)', colored => {
  const positions = new Float32Array(60_000 * 3), normals = new Float32Array(positions.length);
  const colors = colored ? new Float32Array(positions.length) : undefined;
  for (let triangle = 0; triangle < 20_000; triangle++) {
    const offset = triangle * 9;
    positions.set([triangle & 1 ? -0 : 0,0,0, 1,0,0, 0,1,1], offset);
    for (let corner = 0; corner < 3; corner++) {
      normals.set([triangle & 2 ? -0 : 0,0,1], offset + corner * 3);
      colors?.set([0.5,triangle & 4 ? -0 : 0,1], offset + corner * 3);
    }
  }
  const mesh: GeometryMesh = { name: 'Exact tuples', positions, normals, colors, groups: [] };
  const source = { meshes: [mesh], root: { name: '', meshes: [0], children: [] } };
  const result = optimizeModel(source, Infinity); validateModel(result);
  expect(result.meshes[0].positions.length / 3).toBe(colored ? 16 : 8);
  exactTriangles(mesh, result.meshes[0]);
});
it('keeps the admitted near-limit six-face STL within the original import deadline without simplification', () => {
  const source = importStl(denseBoxStl()); expect(source.meshes[0].positions.length / 3).toBe(992016);
  const result = optimizeModel(source); validateModel(result); exactTriangles(source.meshes[0], result.meshes[0]);
});
it('skips optional work when its deadline is already consumed and never smooths missing normals', () => {
  const model = grid(); expect(optimizeModel(model, -Infinity)).toBe(model);
  model.meshes[0].normals = undefined; expect(optimizeModel(model)).toBe(model);
});
it('keeps bounds from unused indexed positions and stops a copy when its cooperative budget expires', () => {
  vi.spyOn(performance, 'now').mockReturnValue(0);
  const plane = new PlaneGeometry(1, 1, 230, 230), values = plane.getAttribute('position').array;
  const positions = new Float32Array(values.length + 3), normals = new Float32Array(positions.length); positions.set(values); positions.set([100, 0, 0], values.length);
  for (let at = 2; at < normals.length; at += 3) normals[at] = 1;
  const mesh: GeometryMesh = { name: '', positions, normals, indices: Uint32Array.from(plane.index!.array), groups: [] };
  const model = { meshes: [mesh], root: { name: '', meshes: [0], children: [] } };
  expect(optimizeModel(model)).toBe(model); plane.dispose();
  let calls = 0; vi.mocked(performance.now).mockImplementation(() => ++calls > 60 ? 1000 : 0);
  const pending = grid(); expect(optimizeModel(pending, 200)).toBe(pending);
});
it('keeps a tiny attached surface when widely separated Float32 coordinates cancel in a naive determinant', () => {
  vi.spyOn(performance, 'now').mockReturnValue(0);
  const h = 2 ** 39, u = 2 ** -20, epsilon = 2 ** -43;
  const positions = new Float32Array(50_001 * 3), normals = new Float32Array(positions.length);
  positions.set([-h,-h,0, 0,0,0, u,u+epsilon,0, h,h,0, h,-h,0]);
  for (let at = 2; at < normals.length; at += 3) normals[at] = 1;
  const mesh: GeometryMesh = { name: '', positions, normals, indices: new Uint32Array([0,1,4, 1,3,4, 1,2,3]), groups: [] };
  const model = { meshes: [mesh], root: { name: '', meshes: [0], children: [] } }; validateModel(model);
  const result = optimizeModel(model); exactTriangles(mesh, result.meshes[0]);
});
it('certifies reverse winding and refuses overlapping triangles even when they have matching bounds', () => {
  vi.spyOn(performance, 'now').mockReturnValue(0);
  const reverse = grid();
  for (let at = 0; at < reverse.meshes[0].positions.length; at += 9) for (let component = 0; component < 3; component++) {
    const p = reverse.meshes[0].positions, value = p[at + 3 + component]; p[at + 3 + component] = p[at + 6 + component]; p[at + 6 + component] = value;
  }
  const result = optimizeModel(reverse).meshes[0]; expect(result.indices).toHaveLength(6);
  const p = result.positions, i = result.indices!;
  expect((p[i[1]*3]-p[i[0]*3])*(p[i[2]*3+1]-p[i[0]*3+1])-(p[i[1]*3+1]-p[i[0]*3+1])*(p[i[2]*3]-p[i[0]*3])).toBeLessThan(0);
  const overlap = grid(), source = overlap.meshes[0], positions = new Float32Array(source.positions.length * 2), normals = new Float32Array(positions.length);
  positions.set(source.positions); positions.set(source.positions, source.positions.length); normals.set(source.normals!); normals.set(source.normals!, source.normals!.length);
  source.positions = positions; source.normals = normals;
  exactTriangles(source, optimizeModel(overlap).meshes[0]);
});
it('keeps small meshes and material-range meshes untouched', async () => {
  const small = grid(4); expect(await optimizeModel(small)).toBe(small);
  const colored = grid(); colored.meshes[0].groups.push({start:0,count:3,color:[1,0,0]});
  expect(await optimizeModel(colored)).toBe(colored);
});
it('preserves every triangle of a tiny disconnected closed solid with zero authored normals', async () => {
  const source = grid(100), mesh = source.meshes[0];
  const box = new BoxGeometry(1e-6, 1e-6, 1e-6).toNonIndexed();
  const solid = box.getAttribute('position').array as Float32Array;
  const positions = new Float32Array(mesh.positions.length + solid.length);
  positions.set(mesh.positions);
  for (let i = 0; i < solid.length; i++) positions[mesh.positions.length + i] = solid[i] + (i % 3 === 2 ? 2 : 0);
  mesh.positions = positions; mesh.normals = new Float32Array(positions.length); box.dispose();
  const result = await optimizeModel(source), output = result.meshes[0];
  const vertices = output.indices ?? Uint32Array.from({ length: output.positions.length / 3 }, (_, i) => i);
  let solidTriangles = 0;
  for (let i = 0; i < vertices.length; i += 3) if (output.positions[vertices[i] * 3 + 2] > 1) solidTriangles++;
  expect(solidTriangles).toBe(12);
  const admitted = modelScene(result);
  try { expect(admitted.bounds.max.z).toBe(2.000000476837158); }
  finally { disposeScene([admitted.scene]); }
});

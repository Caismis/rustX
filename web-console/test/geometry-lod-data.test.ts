import { expect, it } from 'vitest';
import { lodAdmission } from '../src/app/components/documents/geometry/lod-data';
import type { GeometryModel } from '../src/app/components/documents/geometry/model';
import { GEOMETRY_LIMITS as L } from '../src/app/components/documents/geometry/limits';
const model = (): GeometryModel => ({ root: { name: 'Assembly', meshes: [0], children: [{ name: 'Instance', meshes: [0], children: [] }] }, meshes: [{ name: 'Surface', positions: new Float32Array([0,0,0,1,0,0,0,1,0]), groups: [] }] });
it('admits independent whole-model tiers in increasing order and allows a skipped coarse tier', () => {
  const admission = lodAdmission(), coarse = model(), medium = model();
  expect(admission.admit(0, coarse)).toBe(coarse); expect(admission.admit(1, medium)).toBe(medium);
  expect(lodAdmission().admit(1, medium)).toBe(medium);
  for (const level of [0,1,2,-1,.5]) expect(() => admission.admit(level, model())).toThrow('invalid');
});
it('bounds complete occurrence triangles even when occurrences share one transferred mesh', () => {
  const value = model(); value.meshes[0].indices = new Uint32Array((L.lodTriangles / 2 + 1) * 3);
  expect(() => lodAdmission().admit(0, value)).toThrow('limit');
});
it('rejects a small stream retaining an oversized transferred backing buffer', () => {
  const value = model(); value.meshes[0].positions = new Float32Array(new ArrayBuffer(L.lodBytes + 4), 0, 9);
  expect(() => lodAdmission().admit(0, value)).toThrow('invalid');
});
it('charges generated normals and all unreferenced mesh streams against each tier', () => {
  const value = model(); value.meshes.push({ name: 'Unused', positions: new Float32Array(L.lodBytes * 3 / 16), indices: new Uint32Array([0,1,2]), groups: [] });
  expect(() => lodAdmission().admit(0, value)).toThrow('limit');
});
it('bounds materials and expanded draw ranges across every mesh and occurrence', () => {
  const value = model(), mesh = value.meshes[0];
  mesh.groups = Array.from({ length: 200 }, (_, i) => ({ start: i * 3, count: 3, color: [i / 200, .4, .6] }));
  mesh.indices = new Uint32Array(600);
  expect(() => lodAdmission().admit(0, value)).toThrow('limit');
  value.root.children = []; value.meshes.push({ ...mesh, positions: mesh.positions.slice(), indices: mesh.indices.slice() });
  expect(() => lodAdmission().admit(0, value)).toThrow('limit');
});

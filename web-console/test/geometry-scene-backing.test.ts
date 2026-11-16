// @vitest-environment node
import { expect, it, vi } from 'vitest';
import { BufferAttribute, BufferGeometry, Group, InterleavedBuffer, InterleavedBufferAttribute, Mesh, MeshStandardMaterial } from 'three';
import { disposeScene, validateScene } from '../src/app/components/documents/geometry/scene-resources';

vi.mock('../src/app/components/documents/geometry/limits', async importOriginal => {
  const actual = await importOriginal<typeof import('../src/app/components/documents/geometry/limits')>();
  return { ...actual, GEOMETRY_LIMITS: Object.freeze({ ...actual.GEOMETRY_LIMITS, decodedBytes: 1024 }) };
});
const triangle = [0,0,0, 1,0,0, 0,1,0];
function geometry(buffer: ArrayBuffer) {
  const positions = new Float32Array(buffer, 0, 9); positions.set(triangle);
  return new BufferGeometry().setAttribute('position', new BufferAttribute(positions, 3));
}
function scene(...geometries: BufferGeometry[]) {
  const root = new Group(), material = new MeshStandardMaterial(); geometries.forEach(value => root.add(new Mesh(value, material))); return root;
}
function verify(root: Group, reject: boolean) {
  try { if (reject) expect(() => validateScene([root])).toThrowError('limit'); else expect(() => validateScene([root])).not.toThrow(); }
  finally { disposeScene([root]); }
}
it('charges padded position backing bytes independently of logical elements', () => verify(scene(geometry(new ArrayBuffer(1028))), true));
it('charges distinct backing buffers across geometry resources', () => verify(scene(geometry(new ArrayBuffer(600)), geometry(new ArrayBuffer(600))), true));
it('deduplicates shared backing across distinct geometry attributes', () => {
  const buffer = new ArrayBuffer(1024); verify(scene(geometry(buffer), geometry(buffer)), false);
});
it('includes padded index backing buffers in the aggregate', () => {
  const value = geometry(new ArrayBuffer(512)), indices = new Uint16Array(new ArrayBuffer(516), 0, 3); indices.set([0,1,2]); value.setIndex(new BufferAttribute(indices, 1)); verify(scene(value), true);
});
it('charges the full backing of interleaved attributes', () => {
  const values = new Float32Array(new ArrayBuffer(1028), 0, 18); values.set([0,0,0,0,0,1, 1,0,0,0,0,1, 0,1,0,0,0,1]);
  const data = new InterleavedBuffer(values, 6), value = new BufferGeometry().setAttribute('position', new InterleavedBufferAttribute(data, 3, 0)).setAttribute('normal', new InterleavedBufferAttribute(data, 3, 3));
  verify(scene(value), true);
});
it('deduplicates one backing shared by interleaved positions, normals and indices', () => {
  const buffer = new ArrayBuffer(1024), values = new Float32Array(buffer, 0, 18); values.set([0,0,0,0,0,1, 1,0,0,0,0,1, 0,1,0,0,0,1]);
  const data = new InterleavedBuffer(values, 6), value = new BufferGeometry().setAttribute('position', new InterleavedBufferAttribute(data, 3, 0)).setAttribute('normal', new InterleavedBufferAttribute(data, 3, 3));
  const indices = new Uint16Array(buffer, 72, 3); indices.set([0,1,2]); value.setIndex(new BufferAttribute(indices, 1)); verify(scene(value), false);
});

import { GEOMETRY_LIMITS as L, requireGeometry as check } from './limits';
import { validateModelStructure, type GeometryModel } from './model';

/** One optional prefix per import, bounded across all messages. Its streams are
 * separate from the final model; it never supplies authoritative metadata. */
export function partialBudget() {
  let updates = 0, triangles = 0, bytes = 0, draws = 0, materials = 0;
  return {
    get remaining() { return { updates: L.partialUpdates - updates, triangles: L.partialTriangles - triangles, bytes: L.partialBytes - bytes }; },
    admit(value: GeometryModel) {
      check(value && Array.isArray(value.meshes) && value.meshes.length === 1);
      const mesh = value.meshes[0];
      check(mesh && mesh.positions instanceof Float32Array && mesh.colors === undefined);
      check(mesh.normals === undefined || mesh.normals instanceof Float32Array);
      check(mesh.indices === undefined || mesh.indices instanceof Uint32Array);
      let size = 0;
      for (const stream of [mesh.positions, mesh.normals, mesh.indices]) if (stream) {
        check(stream.buffer instanceof ArrayBuffer && stream.byteOffset === 0 && stream.byteLength === stream.buffer.byteLength);
        size += stream.byteLength;
      }
      if (!mesh.normals) size += mesh.positions.byteLength;
      const count = (mesh.indices?.length ?? mesh.positions.length / 3) / 3;
      check(updates + 1 <= L.partialUpdates && bytes + size <= L.partialBytes && triangles + count <= L.partialTriangles, 'limit');
      check(Array.isArray(mesh.groups) && draws + 1 + mesh.groups.length * 2 <= L.drawCalls, 'limit');
      check(materials + 1 + mesh.groups.length <= L.materials, 'limit');
      validateModelStructure(value);
      check(value.root.meshes.length === 1 && value.root.meshes[0] === 0 && value.root.children.length === 0);
      updates++; bytes += size; triangles += count; draws += 1 + mesh.groups.length * 2; materials += 1 + mesh.groups.length;
      return value;
    },
  };
}

import { GEOMETRY_LIMITS as L, requireGeometry as check } from './limits';
import { validateModel, validateModelStructure, type GeometryModel } from './model';

export type GeometryLodLevel = 0 | 1;

/** Each tier owns the entire model. Budgets apply independently to its streams
 * and every occurrence; final-model admission remains separate. */
export function lodAdmission() {
  let previous = -1;
  return {
    admit(level: unknown, model: GeometryModel) {
      check((level === 0 || level === 1) && level > previous);
      validateModelStructure(model);
      let bytes = 0, meshTriangles = 0, triangles = 0, materials = 0, draws = 0;
      for (const mesh of model.meshes) {
        for (const stream of [mesh.positions, mesh.normals, mesh.indices, mesh.colors]) if (stream) {
          check(stream.buffer instanceof ArrayBuffer && stream.byteOffset === 0 && stream.byteLength === stream.buffer.byteLength);
          bytes += stream.byteLength;
        }
        if (!mesh.normals) bytes += mesh.positions.byteLength;
        meshTriangles += (mesh.indices?.length ?? mesh.positions.length / 3) / 3;
        materials += 1 + mesh.groups.length;
      }
      const pending = [model.root];
      while (pending.length) {
        const node = pending.pop()!;
        for (const index of node.meshes) {
          const mesh = model.meshes[index];
          triangles += (mesh.indices?.length ?? mesh.positions.length / 3) / 3;
          draws += 1 + mesh.groups.length * 2;
        }
        pending.push(...node.children);
      }
      check(bytes <= L.lodBytes && meshTriangles <= L.lodTriangles && triangles <= L.lodTriangles && materials <= L.materials && draws <= L.drawCalls, 'limit');
      validateModel(model);
      previous = level;
      return model;
    },
  };
}

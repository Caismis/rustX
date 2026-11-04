import type { GeometryMesh, GeometryModel } from './model';
import { planarMesh } from './planar';
import { GEOMETRY_LIMITS as L } from './limits';

const MIN_VERTICES = 50_000;

/** Budgeted preparation in the disposable Worker. Exact tuple indexing and
 * certified planar retriangulation preserve surfaces, attributes and parts. */
export function optimizeModel(model: GeometryModel, deadline = performance.now() + 200): GeometryModel {
  const meshes = model.meshes.map(mesh => {
    if (mesh.positions.length / 3 < MIN_VERTICES || mesh.positions.length / 3 > L.preparationVertices
      || (mesh.indices?.length ?? mesh.positions.length / 3) / 3 > L.preparationTriangles || performance.now() > deadline) return mesh;
    return planarMesh(indexMesh(mesh, deadline), deadline);
  });
  return meshes.every((mesh, index) => mesh === model.meshes[index]) ? model : { ...model, meshes };
}

function indexMesh(mesh: GeometryMesh, deadline: number): GeometryMesh {
  const count = mesh.positions.length / 3;
  // Indexed imports already share vertices. Keep their topology and streams.
  if (mesh.indices || !mesh.normals || count < MIN_VERTICES || mesh.groups.length) return mesh;
  const streams = [mesh.positions, mesh.normals, mesh.colors].filter((stream): stream is Float32Array => stream !== undefined);
  const bits = streams.map(stream => new Uint32Array(stream.buffer, stream.byteOffset, stream.length));
  const [positionsBits, normalBits, colorBits] = bits;
  let capacity = 1;
  while (capacity < count * 2) capacity *= 2;
  if (performance.now() > deadline) return mesh;
  const table = new Uint32Array(capacity), indices = new Uint32Array(count), representatives = new Uint32Array(count);
  const mask = capacity - 1;
  let unique = 0;
  for (let vertex = 0; vertex < count; vertex++) {
    if ((vertex & 4095) === 0 && performance.now() > deadline) return mesh;
    const offset = vertex * 3;
    let hash = 2166136261;
    hash = Math.imul(hash ^ positionsBits[offset], 16777619);
    hash = Math.imul(hash ^ positionsBits[offset + 1], 16777619);
    hash = Math.imul(hash ^ positionsBits[offset + 2], 16777619);
    hash = Math.imul(hash ^ normalBits[offset], 16777619);
    hash = Math.imul(hash ^ normalBits[offset + 1], 16777619);
    hash = Math.imul(hash ^ normalBits[offset + 2], 16777619);
    if (colorBits) {
      hash = Math.imul(hash ^ colorBits[offset], 16777619);
      hash = Math.imul(hash ^ colorBits[offset + 1], 16777619);
      hash = Math.imul(hash ^ colorBits[offset + 2], 16777619);
    }
    let slot = (hash ^ (hash >>> 16)) & mask;
    let probes = 0;
    for (;;) {
      // Untrusted tuples must not turn optional indexing into unbounded probing.
      if (++probes > 32) return mesh;
      const entry = table[slot];
      if (!entry) {
        representatives[unique] = vertex; indices[vertex] = unique++;
        table[slot] = unique; break;
      }
      const canonical = representatives[entry - 1] * 3;
      const equal = positionsBits[offset] === positionsBits[canonical]
        && positionsBits[offset + 1] === positionsBits[canonical + 1]
        && positionsBits[offset + 2] === positionsBits[canonical + 2]
        && normalBits[offset] === normalBits[canonical]
        && normalBits[offset + 1] === normalBits[canonical + 1]
        && normalBits[offset + 2] === normalBits[canonical + 2]
        && (!colorBits || colorBits[offset] === colorBits[canonical]
          && colorBits[offset + 1] === colorBits[canonical + 1]
          && colorBits[offset + 2] === colorBits[canonical + 2]);
      if (equal) { indices[vertex] = entry - 1; break; }
      slot = (slot + 1) & mask;
    }
  }
  // Do not add an index allocation when the complete decoded result would grow.
  if (unique * 3 * streams.length * 4 + indices.byteLength >= count * 3 * streams.length * 4) return mesh;
  const copy = (values: Float32Array | undefined) => {
    if (!values) return undefined;
    if (performance.now() > deadline) return undefined;
    const source = new Uint32Array(values.buffer, values.byteOffset, values.length), result = new Float32Array(unique * 3), output = new Uint32Array(result.buffer);
    for (let vertex = 0; vertex < unique; vertex++) {
      if ((vertex & 4095) === 0 && performance.now() > deadline) return undefined;
      const offset = representatives[vertex] * 3, target = vertex * 3;
      output[target] = source[offset]; output[target + 1] = source[offset + 1]; output[target + 2] = source[offset + 2];
    }
    return result;
  };
  const positions = copy(mesh.positions), normals = copy(mesh.normals), colors = copy(mesh.colors);
  if (!positions || mesh.normals && !normals || mesh.colors && !colors || performance.now() > deadline) return mesh;
  return { ...mesh, positions, normals, colors, indices };
}

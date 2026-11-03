import { GEOMETRY_LIMITS as L, requireGeometry as check } from './limits';
import { validateModel, type GeometryMesh, type GeometryModel } from './model';

// A small relative surface error with locked boundaries and authored normal/color
// seams. Source bytes and occurrence IDs are unchanged.
export const MESH_RELATIVE_ERROR = 0.00001;
const MIN_VERTICES = 50_000;
const UNUSED = 0xffffffff;

/** Disposable Worker work only, after the importer has admitted its raw model. */
export async function optimizeModel(model: GeometryModel): Promise<GeometryModel> {
  if (!model.meshes.some(mesh => mesh.positions.length / 3 >= MIN_VERTICES && mesh.groups.length === 0)) return model;
  const { MeshoptSimplifier: optimizer } = await import('meshoptimizer/simplifier');
  check(optimizer.supported, 'unsupported'); await optimizer.ready;
  const compact = (mesh: GeometryMesh, indices: Uint32Array): GeometryMesh => {
    const [remap, count] = optimizer.compactMesh(indices);
    const copy = (values: Float32Array | undefined) => {
      if (!values) return;
      const result = new Float32Array(count * 3);
      for (let index = 0; index < remap.length; index++) if (remap[index] !== UNUSED) {
        const target = remap[index] * 3, source = index * 3;
        result[target] = values[source]; result[target + 1] = values[source + 1]; result[target + 2] = values[source + 2];
      }
      return result;
    };
    return { ...mesh, positions: copy(mesh.positions)!, normals: copy(mesh.normals), colors: copy(mesh.colors), indices };
  };
  const meshes = model.meshes.map(mesh => {
    const count = mesh.positions.length / 3;
    if (count < MIN_VERTICES || mesh.groups.length) return mesh;
    const remap = optimizer.generatePositionRemap(mesh.positions, 3), seams = new Map<string, number>();
    const attributes = [mesh.normals, mesh.colors].filter((values): values is Float32Array => values !== undefined);
    for (let index = 0; index < count; index++) {
      const canonical = remap[index], source = index * 3, target = canonical * 3;
      if (attributes.every(values => values[source] === values[target] && values[source + 1] === values[target + 1] && values[source + 2] === values[target + 2])) continue;
      const key = `${canonical}:${attributes.map(values => `${values[source]},${values[source + 1]},${values[source + 2]}`).join(':')}`;
      let representative = seams.get(key);
      if (representative === undefined) { representative = index; seams.set(key, representative); }
      remap[index] = representative;
    }
    const indices = mesh.indices ? Uint32Array.from(mesh.indices, index => remap[index]) : remap;
    const welded = compact(mesh, indices), stride = attributes.length * 3;
    let result: Uint32Array, error: number;
    const targetCount = Math.max(3, Math.floor(indices.length / 300) * 3);
    if (stride) {
      const packed = new Float32Array(welded.positions.length / 3 * stride);
      const streams = [welded.normals, welded.colors].filter((values): values is Float32Array => values !== undefined);
      for (let vertex = 0; vertex < welded.positions.length / 3; vertex++) for (let stream = 0; stream < streams.length; stream++) for (let component = 0; component < 3; component++) {
        const value = streams[stream][vertex * 3 + component]; check(Number.isFinite(value) && Math.abs(value) <= L.coordinate);
        packed[vertex * stride + stream * 3 + component] = value;
      }
      [result, error] = optimizer.simplifyWithAttributes(indices, welded.positions, 3, packed, stride, Array(stride).fill(1), null, targetCount, MESH_RELATIVE_ERROR, ['LockBorder']);
    } else [result, error] = optimizer.simplify(indices, welded.positions, 3, targetCount, MESH_RELATIVE_ERROR, ['LockBorder']);
    check(Number.isFinite(error) && error <= MESH_RELATIVE_ERROR * 1.001);
    return compact(welded, result);
  });
  const optimized = { ...model, meshes }; validateModel(optimized); return optimized;
}

import { GEOMETRY_LIMITS as L, requireGeometry as check } from './limits';

export interface GeometryMesh {
  name: string; positions: Float32Array; normals?: Float32Array; indices?: Uint32Array;
  colors?: Float32Array; color?: [number, number, number];
  groups: { start: number; count: number; color: [number, number, number] }[];
}
export interface GeometryNode { name: string; meshes: number[]; children: GeometryNode[] }
export interface GeometryModel { meshes: GeometryMesh[]; root: GeometryNode }

/** Validate output before it crosses the Worker boundary and again before GPU use. */
export function validateModel(model: GeometryModel): void {
  check(Array.isArray(model.meshes) && model.meshes.length > 0, 'empty');
  check(model.meshes.length <= L.parts, 'limit');
  let vertices = 0, triangles = 0, bytes = 0, draws = 0;
  for (const mesh of model.meshes) {
    check(mesh.positions instanceof Float32Array && mesh.positions.length % 3 === 0);
    const count = mesh.positions.length / 3;
    vertices += count; bytes += mesh.positions.byteLength;
    check(count > 0, 'empty');
    for (const value of mesh.positions) check(Number.isFinite(value) && Math.abs(value) <= L.coordinate);
    if (mesh.normals) { check(mesh.normals instanceof Float32Array && mesh.normals.length === mesh.positions.length); bytes += mesh.normals.byteLength; for (const value of mesh.normals) check(Number.isFinite(value)); }
    if (mesh.colors) { check(mesh.colors instanceof Float32Array && mesh.colors.length === mesh.positions.length); bytes += mesh.colors.byteLength; for (const value of mesh.colors) check(Number.isFinite(value) && value >= 0 && value <= 1); }
    if (mesh.indices) { check(mesh.indices instanceof Uint32Array && mesh.indices.length % 3 === 0); bytes += mesh.indices.byteLength; for (const index of mesh.indices) check(index < count); }
    else check(count % 3 === 0);
    const elements = mesh.indices?.length ?? count;
    triangles += elements / 3; draws += Math.max(1, mesh.groups.length);
    check(mesh.groups.length <= L.drawCalls, 'limit');
    for (const group of mesh.groups) {
      check(Number.isSafeInteger(group.start) && group.start >= 0 && Number.isSafeInteger(group.count) && group.count > 0 && group.start % 3 === 0 && group.count % 3 === 0 && group.start + group.count <= elements);
      check(group.color.length === 3 && group.color.every(value => Number.isFinite(value) && value >= 0 && value <= 1));
    }
    if (mesh.color) check(mesh.color.length === 3 && mesh.color.every(value => Number.isFinite(value) && value >= 0 && value <= 1));
    check(mesh.name.length <= 256);
  }
  check(vertices <= L.vertices && triangles <= L.triangles && bytes <= L.decodedBytes && draws <= L.drawCalls, 'limit');
  let nodes = 0, instances = 0;
  const seen = new Set<GeometryNode>(), queue = [{ node: model.root, depth: 0 }];
  while (queue.length) {
    const { node, depth } = queue.pop()!;
    check(node && !seen.has(node)); seen.add(node);
    check(++nodes <= L.nodes && depth <= L.depth, 'limit');
    check(typeof node.name === 'string' && node.name.length <= 256 && Array.isArray(node.meshes) && Array.isArray(node.children));
    for (const index of node.meshes) { check(Number.isSafeInteger(index) && index >= 0 && index < model.meshes.length); check(++instances <= L.parts, 'limit'); }
    for (const child of node.children) queue.push({ node: child, depth: depth + 1 });
  }
  check(instances > 0, 'empty');
}
export function modelTransfers(model: GeometryModel): ArrayBuffer[] {
  return [...new Set(model.meshes.flatMap(mesh => [mesh.positions, mesh.normals, mesh.indices, mesh.colors].filter(value => value !== undefined).map(value => value.buffer as ArrayBuffer)))];
}

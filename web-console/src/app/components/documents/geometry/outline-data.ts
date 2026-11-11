import { GEOMETRY_LIMITS as L, requireGeometry as check } from './limits';

export interface OutlineInput { positions: Float32Array; indices?: Uint32Array }
/** Two endpoints, each with position, two adjacent face normals and edge kind. */
export const OUTLINE_STRIDE = 10;

/** Exact-position adjacency; no coordinate rounding or changes to admitted surfaces. */
export function outlineData(input: OutlineInput, remainingSegments: number, deadline: number): Float32Array {
  const { positions, indices } = input, count = indices?.length ?? positions.length / 3;
  check(positions.length % 3 === 0 && count % 3 === 0 && count / 3 <= L.outlineTriangles, 'limit');
  const normals = new Float64Array(count), welded = new Map<string, number>(), vertices = new Uint32Array(positions.length / 3);
  const edges = new Map<string, { a: number; b: number; first: number; second: number; count: number }>();
  let next = 1;
  const time = () => check(performance.now() <= deadline, 'timeout');
  for (let i = 0; i < positions.length; i += 3) {
    if (i % 3072 === 0) time();
    const x = positions[i], y = positions[i + 1], z = positions[i + 2];
    check([x, y, z].every(value => Number.isFinite(value) && Math.abs(value) <= L.coordinate));
    const key = `${x},${y},${z}`; let id = welded.get(key);
    if (id === undefined) { id = next++; welded.set(key, id); } vertices[i / 3] = id;
  }
  for (let i = 0; i < count; i += 3) {
    if (i % 3072 === 0) time();
    const a = indices ? indices[i] : i, b = indices ? indices[i + 1] : i + 1, c = indices ? indices[i + 2] : i + 2;
    check([a, b, c].every(value => value < vertices.length));
    const ax = positions[b * 3] - positions[a * 3], ay = positions[b * 3 + 1] - positions[a * 3 + 1], az = positions[b * 3 + 2] - positions[a * 3 + 2];
    const bx = positions[c * 3] - positions[a * 3], by = positions[c * 3 + 1] - positions[a * 3 + 1], bz = positions[c * 3 + 2] - positions[a * 3 + 2];
    const nx = ay * bz - az * by, ny = az * bx - ax * bz, nz = ax * by - ay * bx, length = Math.hypot(nx, ny, nz);
    if (!length) continue;
    normals[i] = nx / length; normals[i + 1] = ny / length; normals[i + 2] = nz / length;
    for (const [start, end] of [[a, b], [b, c], [c, a]]) {
      const first = vertices[start], second = vertices[end]; if (first === second) continue;
      const key = first < second ? `${first}:${second}` : `${second}:${first}`, edge = edges.get(key);
      if (edge) { edge.second = i; edge.count++; }
      else edges.set(key, { a: start, b: end, first: i, second: i, count: 1 });
    }
  }
  const data = new Float32Array(Math.min(edges.size, remainingSegments) * 2 * OUTLINE_STRIDE);
  let at = 0, visited = 0;
  for (const edge of edges.values()) {
    if ((visited++ & 1023) === 0) time();
    const a = edge.first, b = edge.second, dot = normals[a] * normals[b] + normals[a + 1] * normals[b + 1] + normals[a + 2] * normals[b + 2];
    // Coplanar tessellation edges cannot form a silhouette. Other paired edges
    // stay available for view-dependent silhouettes, even below the crease angle.
    if (edge.count === 2 && dot >= 1 - 1e-12) continue;
    check(at + 2 * OUTLINE_STRIDE <= data.length, 'limit');
    const kind = edge.count !== 2 ? 2 : dot <= Math.cos(Math.PI / 6) ? 1 : 0;
    for (const vertex of [edge.a, edge.b]) {
      data.set(positions.subarray(vertex * 3, vertex * 3 + 3), at);
      data.set(normals.subarray(a, a + 3), at + 3); data.set(normals.subarray(b, b + 3), at + 6); data[at + 9] = kind; at += OUTLINE_STRIDE;
    }
  }
  time(); return data.slice(0, at);
}

import type { GeometryMesh } from './model';
import { orient2d } from 'robust-predicates';

/** Retriangulate only a certified convex, axis-aligned planar disk with constant
 * authored attributes. Holes, seams, overlaps, degenerates and disconnected
 * boundaries retain their original triangles. No geometric tolerance is used. */
export function planarMesh(mesh: GeometryMesh, deadline: number): GeometryMesh {
  const { positions: p, indices, normals, colors } = mesh;
  if (!indices || !normals || mesh.groups.length) return mesh;
  const count = p.length / 3;
  let axis = -1;
  for (let component = 0; component < 3; component++) {
    let flat = true;
    for (let at = component + 3; at < p.length; at += 3) {
      if ((at & 4095) === 0 && performance.now() > deadline) return mesh;
      if (p[at] !== p[component]) { flat = false; break; }
    }
    if (flat) { axis = component; break; }
  }
  if (axis < 0) return mesh;
  for (const stream of [normals, colors]) if (stream) {
    const first = stream[0], second = stream[1], third = stream[2];
    let at = 3, nextCheck = 4095;
    for (; at + 2 < stream.length; at += 3) {
      if (at === nextCheck) {
        if (performance.now() > deadline) return mesh;
        nextCheck += 4095;
      }
      if (stream[at] !== first || stream[at + 1] !== second || stream[at + 2] !== third) return mesh;
    }
    for (; at < stream.length; at++) {
      if ((at & 4095) === 0 && performance.now() > deadline) return mesh;
      if (stream[at] !== stream[at % 3]) return mesh;
    }
  }
  if (performance.now() > deadline) return mesh;
  const x = (axis + 1) % 3, y = (axis + 2) % 3;
  let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
  for (let vertex = 0; vertex < count; vertex++) {
    if ((vertex & 4095) === 0 && performance.now() > deadline) return mesh;
    minX = Math.min(minX, p[vertex * 3 + x]); minY = Math.min(minY, p[vertex * 3 + y]);
    maxX = Math.max(maxX, p[vertex * 3 + x]); maxY = Math.max(maxY, p[vertex * 3 + y]);
  }
  // Adaptive exact signs are mandatory: cancellation near a huge coordinate
  // must not turn a tiny attached feature into a supposedly collinear edge.
  const cross = (a: number, b: number, c: number) => -orient2d(p[a * 3 + x], p[a * 3 + y], p[b * 3 + x], p[b * 3 + y], p[c * 3 + x], p[c * 3 + y]);
  let capacity = 1;
  while (capacity < indices.length * 2) capacity *= 2;
  if (performance.now() > deadline) return mesh;
  const from = new Uint32Array(capacity), to = new Uint32Array(capacity), mask = capacity - 1, CLOSED = 0x80000000;
  let orientation = 0, area = 0;
  for (let at = 0; at < indices.length; at += 3) {
    if ((at & 4095) === 0 && performance.now() > deadline) return mesh;
    const first = indices[at], second = indices[at + 1], third = indices[at + 2];
    const triangle = cross(first, second, third), sign = Math.sign(triangle);
    if (!sign || orientation && sign !== orientation) return mesh;
    orientation = sign; area += triangle;
    for (let edge = 0; edge < 3; edge++) {
      const a = (edge === 0 ? first : edge === 1 ? second : third) + 1;
      const b = (edge === 0 ? second : edge === 1 ? third : first) + 1;
      const low = Math.min(a, b), high = Math.max(a, b);
      let slot = (Math.imul(low, 73856093) ^ Math.imul(high, 19349663)) & mask, probes = 0;
      for (;;) {
        if (++probes > 32) return mesh;
        if (!from[slot]) { from[slot] = a; to[slot] = b; break; }
        const start = from[slot], end = to[slot] & ~CLOSED;
        if (Math.min(start, end) === low && Math.max(start, end) === high) {
          if (to[slot] & CLOSED || start !== b || end !== a) return mesh;
          to[slot] |= CLOSED; break;
        }
        slot = (slot + 1) & mask;
      }
    }
  }
  const next = new Uint32Array(count), boundary: number[] = [];
  for (let slot = 0; slot < capacity; slot++) {
    if ((slot & 4095) === 0 && performance.now() > deadline) return mesh;
    if (!from[slot] || to[slot] & CLOSED) continue;
    const vertex = from[slot] - 1;
    if (next[vertex] || boundary.length === 8192) return mesh;
    next[vertex] = to[slot]; boundary.push(vertex);
  }
  if (boundary.length < 3 || performance.now() > deadline) return mesh;
  // Monotone hull certification also rejects self-intersecting/star boundaries.
  boundary.sort((a, b) => p[a * 3 + x] - p[b * 3 + x] || p[a * 3 + y] - p[b * 3 + y]);
  const hull: number[] = [];
  for (const vertex of boundary) { while (hull.length >= 2 && cross(hull[hull.length - 2], hull[hull.length - 1], vertex) <= 0) hull.pop(); hull.push(vertex); }
  const lower = hull.length;
  for (let i = boundary.length - 2; i >= 0; i--) { const vertex = boundary[i]; while (hull.length > lower && cross(hull[hull.length - 2], hull[hull.length - 1], vertex) <= 0) hull.pop(); hull.push(vertex); }
  hull.pop();
  if (hull.length < 3 || (hull.length - 2) * 3 >= indices.length) return mesh;
  // Admission frames ALL coordinates, including unreferenced indexed vertices.
  // Do not silently narrow those bounds when building the rendered fan.
  let hullMinX = Infinity, hullMinY = Infinity, hullMaxX = -Infinity, hullMaxY = -Infinity;
  for (const i of hull) {
    hullMinX = Math.min(hullMinX, p[i * 3 + x]); hullMinY = Math.min(hullMinY, p[i * 3 + y]);
    hullMaxX = Math.max(hullMaxX, p[i * 3 + x]); hullMaxY = Math.max(hullMaxY, p[i * 3 + y]);
  }
  if (hullMinX !== minX || hullMaxX !== maxX || hullMinY !== minY || hullMaxY !== maxY) return mesh;
  if (orientation < 0) hull.splice(1, hull.length - 1, ...hull.slice(1).reverse());
  let traversed = 0, vertex = hull[0], hullArea = 0;
  for (let edge = 0; edge < hull.length; edge++) {
    const a = hull[edge], b = hull[(edge + 1) % hull.length];
    do {
      if (++traversed > boundary.length || !next[vertex] || cross(a, b, vertex) !== 0
        || p[vertex * 3 + x] < Math.min(p[a * 3 + x], p[b * 3 + x]) || p[vertex * 3 + x] > Math.max(p[a * 3 + x], p[b * 3 + x])
        || p[vertex * 3 + y] < Math.min(p[a * 3 + y], p[b * 3 + y]) || p[vertex * 3 + y] > Math.max(p[a * 3 + y], p[b * 3 + y])) return mesh;
      vertex = next[vertex] - 1;
    } while (vertex !== b);
    if (edge > 0 && edge < hull.length - 1) hullArea += cross(hull[0], a, b);
  }
  // Exact orientation and the single convex manifold boundary certify the disk.
  // Double-precision area equality is an extra conservative numerical filter.
  if (vertex !== hull[0] || traversed !== boundary.length || area !== hullArea || performance.now() > deadline) return mesh;
  const triangles = new Uint32Array((hull.length - 2) * 3);
  for (let i = 1; i < hull.length - 1; i++) triangles.set([0, i, i + 1], (i - 1) * 3);
  const copy = (stream: Float32Array | undefined) => {
    if (!stream) return undefined;
    const result = new Float32Array(hull.length * 3);
    for (let i = 0; i < hull.length; i++) result.set(stream.subarray(hull[i] * 3, hull[i] * 3 + 3), i * 3);
    return result;
  };
  return { ...mesh, positions: copy(p)!, normals: copy(normals), colors: copy(colors), indices: triangles };
}

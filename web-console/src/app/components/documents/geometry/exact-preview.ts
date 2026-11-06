import { orient2d } from 'robust-predicates';
import type { GeometryPreviewData } from './model';

const CAPACITY = 16_384, MASK = CAPACITY - 1;

/** Certify every triangle of each consecutive constant-normal, axis-planar run.
 * Opposite directed edges cancel as oriented chains. With every triangle's
 * robust orientation nonzero and equal, their coverage is their boundary's
 * winding number. A single simple convex boundary therefore proves coverage
 * exactly once inside and zero outside: no hole, overlap or lost component.
 * This argument needs neither a floating-point area sum nor an error tolerance.
 * The caller has already admitted this binary STL layout and every numeric value.
 * Any incomplete proof abandons the ENTIRE representation, including prior runs.
 */
export function exactPreview(raw: Uint8Array<ArrayBuffer>, deadline: number, maxTriangles: number): GeometryPreviewData | undefined {
  if (performance.now() > deadline || maxTriangles < 1) return;
  const view = new DataView(raw.buffer, raw.byteOffset, raw.byteLength), triangles = view.getUint32(80, true);
  const keys = new Uint32Array(CAPACITY * 6), hashes = new Uint32Array(CAPACITY), directions = new Int8Array(CAPACITY), vertices = new Uint32Array(CAPACITY * 2);
  const live = new Uint32Array(CAPACITY), locations = new Uint32Array(CAPACITY);
  const source = new Uint32Array(maxTriangles * 3), normalSource = new Uint32Array(source.length), point = new Uint32Array(9);
  let active = 0, output = 0, axis = -1, plane = 0, orientation = 0, firstTriangle = 0;
  let normalX = 0, normalY = 0, normalZ = 0;
  const coordinate = (vertex: number, component: number) => view.getFloat32(84 + Math.floor(vertex / 3) * 50 + 12 + (vertex % 3 * 3 + component) * 4, true);
  const cross = (a: number, b: number, c: number) => -orient2d(coordinate(a, (axis + 1) % 3), coordinate(a, (axis + 2) % 3),
    coordinate(b, (axis + 1) % 3), coordinate(b, (axis + 2) % 3), coordinate(c, (axis + 1) % 3), coordinate(c, (axis + 2) % 3));

  function edge(a: number, b: number, first: number, second: number): boolean {
    let direction = 1;
    for (let c = 0; c < 3; c++) if (point[a + c] !== point[b + c]) {
      if (point[a + c] > point[b + c]) { [a, b] = [b, a]; [first, second] = [second, first]; direction = -1; }
      break;
    }
    let hash = 2166136261;
    for (let c = 0; c < 3; c++) { hash = Math.imul(hash ^ point[a + c], 16777619); hash = Math.imul(hash ^ point[b + c], 16777619); }
    hash = (hash ^ hash >>> 16) >>> 0;
    let slot = hash & MASK;
    for (let probes = 0; probes < 32; probes++, slot = slot + 1 & MASK) {
      if (!directions[slot]) {
        if (active === CAPACITY / 2) return false;
        directions[slot] = direction; hashes[slot] = hash;
        for (let c = 0; c < 3; c++) { keys[slot * 6 + c] = point[a + c]; keys[slot * 6 + 3 + c] = point[b + c]; }
        vertices[slot * 2] = first; vertices[slot * 2 + 1] = second; live[active] = slot; locations[slot] = active++;
        return true;
      }
      let equal = hashes[slot] === hash;
      for (let c = 0; c < 3 && equal; c++) equal = keys[slot * 6 + c] === point[a + c] && keys[slot * 6 + 3 + c] === point[b + c];
      if (!equal) continue;
      if (directions[slot] === direction) return false;
      // Backward-shift deletion leaves no tombstones. Each moved entry's probe
      // path crosses the hole; entries whose path does not cross it stay put.
      directions[slot] = 0;
      const removed = locations[slot], last = live[--active]; live[removed] = last; locations[last] = removed;
      let hole = slot;
      for (let at = slot + 1 & MASK, scanned = 0; directions[at]; at = at + 1 & MASK) {
        if (++scanned > 32) return false;
        const ideal = hashes[at] & MASK;
        if (((at - ideal) & MASK) >= ((at - hole) & MASK)) {
          directions[hole] = directions[at]; hashes[hole] = hashes[at];
          const location = locations[at]; live[location] = hole; locations[hole] = location;
          for (let c = 0; c < 6; c++) keys[hole * 6 + c] = keys[at * 6 + c];
          vertices[hole * 2] = vertices[at * 2]; vertices[hole * 2 + 1] = vertices[at * 2 + 1];
          directions[at] = 0; hole = at;
        }
      }
      return true;
    }
    return false;
  }

  function finishRun(endTriangle: number): boolean {
    if (active < 3 || performance.now() > deadline) return false;
    // A nondegenerate single triangle is already a certified convex disk.
    // Frequent authored-normal seams must not scan an entire hash table per run.
    if (endTriangle - firstTriangle === 1) {
      if (output + 3 > source.length) return false;
      for (let corner = 0; corner < 3; corner++) { source[output] = firstTriangle * 3 + corner; normalSource[output++] = firstTriangle * 3; }
      return true;
    }
    const nodes = new Map<string, { vertex: number; next?: string; incoming: boolean }>();
    const key = (slot: number, end: number) => `${keys[slot * 6 + end * 3]},${keys[slot * 6 + end * 3 + 1]},${keys[slot * 6 + end * 3 + 2]}`;
    for (let index = 0; index < active; index++) {
      if ((index & 1023) === 0 && performance.now() > deadline) return false;
      const slot = live[index];
      const from = directions[slot] > 0 ? 0 : 1, to = 1 - from, a = key(slot, from), b = key(slot, to);
      const first = nodes.get(a) ?? { vertex: vertices[slot * 2 + from], incoming: false };
      const second = nodes.get(b) ?? { vertex: vertices[slot * 2 + to], incoming: false };
      if (a === b || first.next !== undefined || second.incoming) return false;
      first.next = b; second.incoming = true; nodes.set(a, first); nodes.set(b, second);
    }
    if (nodes.size !== active || [...nodes.values()].some(node => node.next === undefined || !node.incoming)) return false;
    const x = (axis + 1) % 3, y = (axis + 2) % 3;
    const sorted = [...nodes.keys()].sort((a, b) => coordinate(nodes.get(a)!.vertex, x) - coordinate(nodes.get(b)!.vertex, x)
      || coordinate(nodes.get(a)!.vertex, y) - coordinate(nodes.get(b)!.vertex, y));
    const turn = (a: string, b: string, c: string) => cross(nodes.get(a)!.vertex, nodes.get(b)!.vertex, nodes.get(c)!.vertex);
    const hull: string[] = [];
    for (const vertex of sorted) { while (hull.length >= 2 && turn(hull[hull.length - 2], hull[hull.length - 1], vertex) <= 0) hull.pop(); hull.push(vertex); }
    const lower = hull.length;
    for (let i = sorted.length - 2; i >= 0; i--) { const vertex = sorted[i]; while (hull.length > lower && turn(hull[hull.length - 2], hull[hull.length - 1], vertex) <= 0) hull.pop(); hull.push(vertex); }
    hull.pop();
    if (hull.length < 3 || output + (hull.length - 2) * 3 > source.length) return false;
    if (orientation < 0) hull.reverse();
    let current = hull[0], traversed = 0;
    for (let side = 0; side < hull.length; side++) {
      const start = nodes.get(hull[side])!.vertex, end = nodes.get(hull[(side + 1) % hull.length])!.vertex;
      const component = coordinate(start, x) !== coordinate(end, x) ? x : y, sign = Math.sign(coordinate(end, component) - coordinate(start, component));
      do {
        if (++traversed > active || performance.now() > deadline) return false;
        const node = nodes.get(current)!, next = nodes.get(node.next!)!;
        // The complete cycle must follow every hull edge monotonically. This
        // rejects branches, extra cycles, concavity, crossings and backtracking.
        if (cross(start, end, next.vertex) !== 0 || sign * (coordinate(next.vertex, component) - coordinate(node.vertex, component)) <= 0
          || coordinate(next.vertex, x) < Math.min(coordinate(start, x), coordinate(end, x)) || coordinate(next.vertex, x) > Math.max(coordinate(start, x), coordinate(end, x))
          || coordinate(next.vertex, y) < Math.min(coordinate(start, y), coordinate(end, y)) || coordinate(next.vertex, y) > Math.max(coordinate(start, y), coordinate(end, y))) return false;
        current = node.next!;
      } while (current !== hull[(side + 1) % hull.length]);
    }
    if (current !== hull[0] || traversed !== active) return false;
    for (let i = 1; i < hull.length - 1; i++) for (const key of [hull[0], hull[i], hull[i + 1]]) {
      source[output] = nodes.get(key)!.vertex; normalSource[output++] = firstTriangle * 3;
    }
    return true;
  }

  for (let triangle = 0; triangle < triangles; triangle++) {
    if ((triangle & 1023) === 0 && performance.now() > deadline) return;
    const at = 84 + triangle * 50;
    let nextAxis = -1;
    for (let c = 0; c < 3; c++) if (view.getFloat32(at + 12 + c * 4, true) === view.getFloat32(at + 24 + c * 4, true)
      && view.getFloat32(at + 12 + c * 4, true) === view.getFloat32(at + 36 + c * 4, true)) { nextAxis = c; break; }
    if (nextAxis < 0) return;
    const nextPlane = view.getFloat32(at + 12 + nextAxis * 4, true), nx = view.getUint32(at, true), ny = view.getUint32(at + 4, true), nz = view.getUint32(at + 8, true);
    if (nextAxis !== axis || nextPlane !== plane || nx !== normalX || ny !== normalY || nz !== normalZ) {
      if (triangle && !finishRun(triangle)) return;
      axis = nextAxis; plane = nextPlane; normalX = nx; normalY = ny; normalZ = nz; firstTriangle = triangle; orientation = 0;
      for (let index = 0; index < active; index++) directions[live[index]] = 0;
      active = 0;
    }
    const sign = Math.sign(cross(triangle * 3, triangle * 3 + 1, triangle * 3 + 2));
    if (!sign || orientation && sign !== orientation) return;
    orientation = sign;
    for (let c = 0; c < 9; c++) {
      const bits = view.getUint32(at + 12 + c * 4, true);
      // Geometric equality includes signed zero. Output attributes still copy
      // the selected source vertex's ORIGINAL bits, not these canonical keys.
      point[c] = (bits & 0x7fffffff) === 0 ? 0 : bits;
    }
    if (!edge(0, 3, triangle * 3, triangle * 3 + 1) || !edge(3, 6, triangle * 3 + 1, triangle * 3 + 2) || !edge(6, 0, triangle * 3 + 2, triangle * 3)) return;
  }
  if (!triangles || !finishRun(triangles) || output >= triangles * 3 || performance.now() > deadline) return;
  const positions = new Float32Array(output * 3), normals = new Float32Array(positions.length), positionBits = new Uint32Array(positions.buffer), normalBits = new Uint32Array(normals.buffer);
  for (let vertex = 0; vertex < output; vertex++) {
    if ((vertex & 1023) === 0 && performance.now() > deadline) return;
    const sourceVertex = source[vertex], at = 84 + Math.floor(sourceVertex / 3) * 50 + 12 + sourceVertex % 3 * 12, normalAt = 84 + normalSource[vertex] / 3 * 50;
    for (let c = 0; c < 3; c++) { positionBits[vertex * 3 + c] = view.getUint32(at + c * 4, true); normalBits[vertex * 3 + c] = view.getUint32(normalAt + c * 4, true); }
  }
  if (performance.now() > deadline) return;
  return { positions, normals, sourceVertices: source.slice(0, output), normalVertices: normalSource.slice(0, output), error: 0 };
}

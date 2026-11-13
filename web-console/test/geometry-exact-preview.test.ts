// @vitest-environment node
import { expect, it, vi } from 'vitest';
import { exactPreview } from '../src/app/components/documents/geometry/exact-preview';
import { denseBoxStl, planeAndTinySolidStl } from './geometry-fixtures';
import type { GeometryPreviewData } from '../src/app/components/documents/geometry/model';

type Point = [number, number, number];
type Facet = { points: Point[]; normal: Point };
function stl(facets: Facet[]) {
  const bytes = new Uint8Array(84 + facets.length * 50), view = new DataView(bytes.buffer); view.setUint32(80, facets.length, true);
  facets.forEach(({ points, normal }, triangle) => {
    const at = 84 + triangle * 50;
    normal.forEach((value, c) => view.setFloat32(at + c * 4, value, true));
    points.flat().forEach((value, c) => view.setFloat32(at + 12 + c * 4, value, true));
  });
  return bytes;
}
function grid(width: number, height: number, include = (_x: number, _y: number) => true): Facet[] {
  const facets: Facet[] = [];
  for (let y = 0; y < height; y++) for (let x = 0; x < width; x++) if (include(x, y)) {
    facets.push({ normal: [0, 0, 1], points: [[x,y,0], [x+1,y,0], [x,y+1,0]] },
      { normal: [0, 0, 1], points: [[x+1,y,0], [x+1,y+1,0], [x,y+1,0]] });
  }
  return facets;
}
const prepare = (raw: Uint8Array<ArrayBuffer>, max = 200_000) => exactPreview(raw, Infinity, max);
function provenance(raw: Uint8Array<ArrayBuffer>, data: GeometryPreviewData) {
  const input = new DataView(raw.buffer), positions = new Uint32Array(data.positions.buffer), normals = new Uint32Array(data.normals.buffer);
  for (let i = 0; i < data.sourceVertices.length; i++) {
    const vertex = data.sourceVertices[i], at = 84 + Math.floor(vertex / 3) * 50 + 12 + vertex % 3 * 12, normal = 84 + Math.floor(data.normalVertices[i] / 3) * 50;
    for (let c = 0; c < 3; c++) { expect(positions[i * 3 + c]).toBe(input.getUint32(at + c * 4, true)); expect(normals[i * 3 + c]).toBe(input.getUint32(normal + c * 4, true)); }
  }
}
function surface(data: GeometryPreviewData) {
  const points = new Map<string,number>(), edges = new Map<string,number>(); let volume = 0;
  const p = data.positions, id = (at: number) => { const key = `${p[at]},${p[at+1]},${p[at+2]}`; if (!points.has(key)) points.set(key, points.size); return points.get(key)!; };
  for (let at = 0; at < p.length; at += 9) {
    const ids = [id(at),id(at+3),id(at+6)];
    for (let side = 0; side < 3; side++) { const a = ids[side], b = ids[(side+1)%3], key = a < b ? `${a},${b}` : `${b},${a}`; edges.set(key,(edges.get(key) ?? 0) + 1); }
    volume += (p[at]*(p[at+4]*p[at+8]-p[at+5]*p[at+7])+p[at+1]*(p[at+5]*p[at+6]-p[at+3]*p[at+8])+p[at+2]*(p[at+3]*p[at+7]-p[at+4]*p[at+6]))/6;
  }
  return { closed: [...edges.values()].every(count => count === 2), volume };
}

it('certifies all dense box facets with exact provenance and a closed unit volume', () => {
  const raw = denseBoxStl(), result = prepare(raw)!;
  expect(result).toBeDefined(); expect(result.error).toBe(0); expect(result.positions.length / 9).toBe(12);
  provenance(raw, result); expect(surface(result).closed).toBe(true); expect(surface(result).volume).toBeCloseTo(1, 12);
});
it('keeps a tiny disconnected solid when its distinct planes form additional complete runs', () => {
  const raw = planeAndTinySolidStl(), result = prepare(raw)!;
  expect(result).toBeDefined(); expect(result.positions.length / 9).toBe(14); provenance(raw, result);
  expect(Math.max(...result.positions)).toBeGreaterThan(2);
});
it.each([
  ['hole', () => grid(4,4,(x,y) => x !== 1 || y !== 1)],
  ['disconnected plane', () => grid(4,2,(x) => x !== 1)],
  ['nonconvex boundary', () => grid(4,4,(x,y) => x < 2 || y < 2)],
  ['duplicate overlap', () => { const source = grid(2,2); return [...source,source[0]]; }],
  ['inconsistent winding', () => { const source = grid(2,2); source[3].points.reverse(); return source; }],
  ['degenerate facet', () => { const source = grid(2,2); source[3].points[1] = source[3].points[0]; return source; }],
  ['non-axis plane', () => grid(2,2).map(facet => ({ ...facet, points: facet.points.map(([x,y]) => [x,y,x+y] as Point) }))],
] as const)('abandons the whole candidate for %s', (_name, facets) => {
  // A certified earlier plane must never escape as a partial preview.
  const prefix = grid(2,2).map(facet => ({ ...facet, points: facet.points.map(([x,y]) => [x,y,10] as Point) }));
  expect(prepare(stl([...prefix,...facets()]))).toBeUndefined();
});
it('treats signed-zero coordinates as equal while preserving every authored normal bit and normal seam', () => {
  const facets = grid(2,2);
  facets.forEach((facet, i) => { facet.normal = [i < 4 ? 0 : -0, 0, 1]; facet.points.forEach((point,j) => { if (point[2] === 0 && j % 2) point[2] = -0; }); });
  const raw = stl(facets), result = prepare(raw)!;
  expect(result).toBeDefined(); expect(result.positions.length / 9).toBe(4); provenance(raw, result);
  const normals = new Uint32Array(result.normals.buffer);
  expect([...normals].some(value => value === 0x80000000)).toBe(true); expect([...normals].some(value => value === 0)).toBe(true);
});
it('rejects a boundary with branches even when all source triangles have the same orientation', () => {
  const facets = [...grid(2,2),...grid(2,2).map(facet => ({ ...facet, points: facet.points.map(([x,y,z]) => [x+2,y+2,z] as Point) }))];
  expect(prepare(stl(facets))).toBeUndefined();
});
it('bounds live edges and output without publishing a certified prefix', () => {
  const facets: Facet[] = [];
  for (let i = 0; i < 3000; i++) facets.push({normal:[0,0,1],points:[[i*2,0,0],[i*2+1,0,0],[i*2,1,0]]});
  expect(prepare(stl(facets))).toBeUndefined(); expect(prepare(stl(grid(2,2)), 1)).toBeUndefined();
});
it('checks its deadline during source traversal, certification and packing', () => {
  const raw = stl(grid(100,100));
  let completeCalls = 0;
  const baseline = vi.spyOn(performance, 'now').mockImplementation(() => { completeCalls++; return 0; });
  try { expect(exactPreview(raw, 1, 200_000)).toBeDefined(); } finally { baseline.mockRestore(); }
  for (const callsBeforeExpiry of [0, 2, 25, 45, completeCalls - 1]) {
    let calls = 0;
    const clock = vi.spyOn(performance, 'now').mockImplementation(() => ++calls > callsBeforeExpiry ? 2 : 0);
    try { expect(exactPreview(raw, 1, 200_000)).toBeUndefined(); } finally { clock.mockRestore(); }
  }
});

it('keeps hash cancellation correct across shuffled input and translated coordinates', () => {
  let seed = 73;
  for (let variant = 0; variant < 12; variant++) {
    const facets = grid(12,12).map(facet => ({ ...facet, points: facet.points.map(([x,y,z]) => [x + variant * 1009,y - variant * 17,z] as Point) }));
    for (let i = facets.length - 1; i > 0; i--) { seed = Math.imul(seed,1664525) + 1013904223 | 0; const j = (seed >>> 0) % (i + 1); [facets[i],facets[j]] = [facets[j],facets[i]]; }
    const raw = stl(facets), result = prepare(raw)!;
    expect(result).toBeDefined(); expect(result.positions.length / 9).toBe(2); provenance(raw, result);
  }
});

it('handles thousands of short authored-normal runs while retaining every facet', () => {
  const facets = grid(2,2);
  for (let i = 0; i < 5000; i++) facets.push({normal:[i % 2 ? -0 : 0,0,1],points:[[0,0,i+1],[1,0,i+1],[0,1,i+1]]});
  const raw = stl(facets), result = prepare(raw)!;
  expect(result).toBeDefined(); expect(result.positions.length / 9).toBe(5002); provenance(raw, result);
  expect(result.positions.some(value => value === 5000)).toBe(true);
});
it('rejects a single self-crossing pentagram cycle despite same-winding facets and cancelled radial edges', () => {
  const points: Point[] = Array.from({length:5}, (_, i) => [Math.cos(i * Math.PI * 2 / 5),Math.sin(i * Math.PI * 2 / 5),0]);
  const order = [0,2,4,1,3], facets: Facet[] = order.map((at, i) => ({normal:[0,0,1],points:[[0,0,0],points[at],points[order[(i+1)%5]]]}));
  expect(prepare(stl(facets))).toBeUndefined();
});

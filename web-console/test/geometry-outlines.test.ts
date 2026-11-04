import { afterEach, expect, it, vi } from 'vitest';
import { BoxGeometry, Group, Mesh, MeshBasicMaterial, SphereGeometry } from 'three';
import { outlineData, OUTLINE_STRIDE } from '../src/app/components/documents/geometry/outline-data';
import { createOutlines } from '../src/app/components/documents/geometry/outlines';
import { GEOMETRY_LIMITS as L } from '../src/app/components/documents/geometry/limits';

const input = (geometry: BoxGeometry | SphereGeometry) => ({ positions: new Float32Array(geometry.getAttribute('position').array), indices: new Uint32Array(geometry.index!.array) });
const extract = (geometry: BoxGeometry | SphereGeometry, limit: number = L.outlineSegments) => outlineData(input(geometry), limit, performance.now() + 2000);
afterEach(() => vi.unstubAllGlobals());
it('removes coplanar triangulation, keeps box creases and preserves source arrays', () => {
  const geometry = new BoxGeometry(1, 1, 1, 8, 8, 8), before = input(geometry), data = extract(geometry);
  // Subdivided outer creases remain segments, while face diagonals/interiors disappear.
  expect(data.length / (2 * OUTLINE_STRIDE)).toBe(12 * 8);
  for (let i = 9; i < data.length; i += OUTLINE_STRIDE) expect(data[i]).toBe(1);
  expect(input(geometry)).toEqual(before);
});
it('keeps view-dependent smooth-surface candidates without labelling them crease edges', () => {
  const data = extract(new SphereGeometry(1, 48, 32)); let silhouette = 0, crease = 0;
  for (let i = 9; i < data.length; i += OUTLINE_STRIDE * 2) { if (data[i] === 0) silhouette++; if (data[i] === 1) crease++; }
  expect(silhouette).toBeGreaterThan(1000); expect(crease).toBe(0);
});
it('keeps open boundaries and tiny coordinates without a rounded weld', () => {
  const positions = new Float32Array([0,0,0, 1e-8,0,0, 0,1e-8,0]);
  const data = outlineData({ positions }, 3, performance.now() + 1000);
  expect(data.length).toBe(3 * 2 * OUTLINE_STRIDE);
  for (let i = 9; i < data.length; i += OUTLINE_STRIDE) expect(data[i]).toBe(2);
});
it('rejects complete output above the segment budget and stops on its deadline', () => {
  expect(() => extract(new BoxGeometry(), 11)).toThrow();
  expect(() => outlineData(input(new BoxGeometry()), 12, -1)).toThrow();
});
it('does not start optional work for oversized or translucent geometry', async () => {
  const worker = vi.fn(); vi.stubGlobal('Worker', worker);
  for (const [geometry, material] of [[new BoxGeometry(1, 1, 1, 100, 100, 100), new MeshBasicMaterial()], [new BoxGeometry(), new MeshBasicMaterial({ transparent: true, opacity: .5 })]] as const) {
    const mesh = new Mesh(geometry, material), owner = createOutlines(mesh, () => '#000000');
    expect(await owner.set(true)).toBe('limit'); expect(mesh.children).toHaveLength(0); owner.dispose();
  }
  expect(worker).not.toHaveBeenCalled();
});
it('terminates pending work on disable/retirement and fences late results', async () => {
  const workers: any[] = [];
  vi.stubGlobal('Worker', class {
    terminate = vi.fn(); postMessage = vi.fn(); onmessage: any; onerror: any; onmessageerror: any;
    constructor() { workers.push(this); }
  });
  const root = new Group(); root.add(new Mesh(new BoxGeometry(), new MeshBasicMaterial()));
  const owner = createOutlines(root, () => '#000000'), pending = owner.set(true), worker = workers[0], late = worker.onmessage;
  expect(await owner.set(false)).toBe('off'); expect(await pending).toBe('off'); expect(worker.terminate).toHaveBeenCalledOnce();
  late({ data: { ok: true, data: [extract(new BoxGeometry())] } }); expect(root.children[0].children).toHaveLength(0);
  const next = owner.set(true); owner.dispose(); expect(await next).toBe('off'); expect(workers[1].terminate).toHaveBeenCalledOnce();
});
it('owns shared output buffers/materials, follows occurrence transforms and disposes exactly once', async () => {
  const workers: any[] = [];
  vi.stubGlobal('Worker', class { terminate = vi.fn(); onmessage: any; onerror: any; onmessageerror: any; constructor() { workers.push(this); } postMessage() {} });
  const root = new Group(), a = new Mesh(new BoxGeometry(), new MeshBasicMaterial()), b = a.clone(); b.position.set(100, 200, 300); root.add(a, b);
  const owner = createOutlines(root, () => '#ffffff'), pending = owner.set(true);
  workers[0].onmessage({ data: { ok: true, data: [extract(new BoxGeometry())] } }); expect(await pending).toBe('on');
  const first = a.children[0] as any, second = b.children[0] as any;
  expect(first.geometry).toBe(second.geometry); expect(first.material).toBe(second.material);
  root.updateMatrixWorld(true); expect(second.matrixWorld.elements.slice(12, 15)).toEqual([100, 200, 300]);
  const geometryDispose = vi.spyOn(first.geometry, 'dispose'), materialDispose = vi.spyOn(first.material, 'dispose');
  await owner.set(false); owner.dispose();
  expect(geometryDispose).toHaveBeenCalledOnce(); expect(materialDispose).toHaveBeenCalledOnce(); expect(a.children).toHaveLength(0); expect(b.children).toHaveLength(0);
});
it.each(['invalid normal', 'oversized backing buffer', 'segment limit'])('independently rejects Worker output: %s', async variant => {
  const workers: any[] = [];
  vi.stubGlobal('Worker', class { terminate = vi.fn(); onmessage: any; onerror: any; onmessageerror: any; constructor() { workers.push(this); } postMessage() {} });
  const root = new Mesh(new BoxGeometry(), new MeshBasicMaterial()), owner = createOutlines(root, () => '#ffffff'), pending = owner.set(true);
  let data = extract(new BoxGeometry());
  if (variant === 'invalid normal') data[3] = NaN;
  else if (variant === 'oversized backing buffer') data = new Float32Array(new ArrayBuffer(data.byteLength + 4096), 0, data.length);
  else data = new Float32Array((L.outlineSegments + 1) * 2 * OUTLINE_STRIDE);
  workers[0].onmessage({ data: { ok: true, data: [data] } });
  expect(await pending).not.toBe('on'); expect(root.children).toHaveLength(0); expect(owner.enabled).toBe(false); owner.dispose();
});

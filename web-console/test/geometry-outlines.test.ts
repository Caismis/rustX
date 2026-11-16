import { afterEach, expect, it, vi } from 'vitest';
import { BoxGeometry, BufferGeometry, DoubleSide, Group, InterleavedBufferAttribute, LineSegments, Mesh, MeshBasicMaterial, OrthographicCamera, PerspectiveCamera, ShaderMaterial, SphereGeometry, Vector3, Vector4 } from 'three';
import { createStructure } from '../src/app/components/documents/geometry/structure';
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
for (const scale of [1e-6, 1, 1e6]) for (const generated of [false, true]) it.each(['perspective', 'orthographic'])(`keeps a thin hidden box behind its occluder at scale ${scale} (%s, packed: ${generated})`, async projection => {
  vi.stubGlobal('Worker', class {
    onmessage: any; onerror: any; onmessageerror: any; terminate() {}
    postMessage({ inputs }: { inputs: Parameters<typeof outlineData>[0][] }) {
      queueMicrotask(() => this.onmessage?.({ data: { ok: true, data: inputs.map(value => outlineData(value, L.outlineSegments, performance.now() + 2000)) } }));
    }
  });
  const root = new Group(), front = new Mesh(new BoxGeometry(2, 2, .02), new MeshBasicMaterial()), back = new Mesh(new BoxGeometry(1, 1, .02), new MeshBasicMaterial());
  if (generated) { front.geometry.scale(scale, scale, scale); back.geometry.translate(0, 0, -.04).scale(scale, scale, scale); }
  else { back.position.z = -.04; root.scale.setScalar(scale); }
  root.add(front, back);
  const sourceMaterial = front.material, positions = front.geometry.getAttribute('position').array.slice(), owner = createOutlines(root, () => '#ffffff', { generated });
  expect(await owner.set(true)).toBe('on');
  const shader = (generated ? owner.group.children[0] : front.children[0] as any).material.vertexShader as string;
  // Evaluate the actual shader's depth adjustment against Three's projection.
  // A fixed 1e-6 NDC pull crosses the occluder in this exact Fit Front case.
  const adjustment = shader.match(/gl_Position\.z -= ([^;]+);/)![1]
    .replaceAll('projectionMatrix[3][3]', 'kind').replaceAll('projectionMatrix[3][2]', 'b').replaceAll('projectionMatrix[2][2]', 'a').replaceAll('view.z', 'z').replaceAll('abs(', 'Math.abs(');
  const bias = new Function('kind', 'b', 'a', 'z', `return ${adjustment}`) as (kind: number, b: number, a: number, z: number) => number;
  const radius = Math.hypot(2, 2, .06) * scale / 2, distance = radius / Math.sin(Math.PI / 8) * 1.1;
  const camera = projection === 'perspective' ? new PerspectiveCamera(45, 4 / 3, radius / 10_000, radius * 1000) : new OrthographicCamera(-radius * 1.1 * 4 / 3, radius * 1.1 * 4 / 3, radius * 1.1, -radius * 1.1, radius / 10_000, radius * 1000);
  const clip = (z: number) => new Vector4(0, 0, z * scale - distance, 1).applyMatrix4(camera.projectionMatrix);
  const occluder = clip(.01), hidden = clip(-.03), surfaceDepth = occluder.z / occluder.w;
  const depthBias = (z: number) => bias(camera.projectionMatrix.elements[15], camera.projectionMatrix.elements[14], camera.projectionMatrix.elements[10], z * scale - distance);
  expect((hidden.z - depthBias(-.03)) / hidden.w).toBeGreaterThan(surfaceDepth);
  expect((occluder.z - depthBias(.01)) / occluder.w).toBeLessThanOrEqual(surfaceDepth);
  expect(front.material).toBe(sourceMaterial); expect(front.geometry.getAttribute('position').array).toEqual(positions); owner.dispose();
});
it.each([false, true])('uses parallel orthographic eye rays for front and side faces (packed: %s)', async generated => {
  vi.stubGlobal('Worker', class {
    onmessage: any; onerror: any; onmessageerror: any; terminate() {}
    postMessage() { queueMicrotask(() => this.onmessage?.({ data: { ok: true, data: [extract(new BoxGeometry())] } })); }
  });
  const root = new Group(), mesh = new Mesh(new BoxGeometry(), new MeshBasicMaterial()); root.add(mesh);
  const owner = createOutlines(root, () => '#ffffff', { generated }); expect(await owner.set(true)).toBe('on');
  const shader = ((generated ? owner.group.children[0] : mesh.children[0]) as LineSegments<BufferGeometry, ShaderMaterial>).material.vertexShader;
  const expression = shader.match(/vec3 eyeDirection = ([^;]+);/)![1].replaceAll('projectionMatrix[3][3]', 'projection').replace('-view.xyz', 'perspective').replace('vec3(0.0, 0.0, 1.0)', 'parallel');
  const eye = new Function('projection', 'perspective', 'parallel', `return ${expression}`) as (projection: number, perspective: Vector3, parallel: Vector3) => Vector3;
  const front = new Vector3(0, 0, 1), side = new Vector3(1, 0, 0), parallel = new Vector3(0, 0, 1);
  for (const x of [-10, 10]) { const direction = eye(1, new Vector3(-x, 0, 4), parallel); expect(front.dot(direction)).toBe(1); expect(side.dot(direction)).toBe(0); }
  expect(side.dot(eye(0, new Vector3(10, 0, 4), parallel))).toBeGreaterThan(0); expect(side.dot(eye(0, new Vector3(-10, 0, 4), parallel))).toBeLessThan(0);
  expect(shader).toContain('dot(normalMatrix * faceA, eyeDirection)'); expect(shader).toContain('dot(normalMatrix * faceB, eyeDirection)'); owner.dispose();
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
it('packs generated occurrence lines into two bounded buffers without changing coordinates, normals, picking or inspection', async () => {
  vi.stubGlobal('Worker', class {
    onmessage: any; onerror: any; onmessageerror: any; terminate() {}
    postMessage({ inputs }: { inputs: Parameters<typeof outlineData>[0][] }) { queueMicrotask(() => this.onmessage?.({ data: { ok: true, data: inputs.map(value => outlineData(value, L.outlineSegments, performance.now() + 2000)) } })); }
  });
  const root = new Group(), parent = new Group(), geometry = new BoxGeometry(), material = new MeshBasicMaterial();
  const a = new Mesh(geometry, material), b = a.clone(), c = new Mesh(geometry, new MeshBasicMaterial({ side: DoubleSide })); root.add(a, b, c); parent.position.x = -100; parent.add(root);
  geometry.computeBoundingBox(); geometry.computeBoundingSphere(); parent.updateMatrixWorld(true);
  const structure = createStructure(root, new Vector3(), undefined), inspection = structure.inspect(), parts = [...structure.parts], original = extract(geometry);
  const owner = createOutlines(root, () => '#ffffff', { generated: true }); parent.add(owner.group);
  expect(await owner.set(true)).toBe('on'); expect(owner.group.children).toHaveLength(2);
  const lines = owner.group.children as LineSegments<BufferGeometry, ShaderMaterial>[];
  for (const line of lines) {
    const doubleSided = line.material.uniforms.doubleSided.value === 1, ids = doubleSided ? [2] : [0, 1], data = (line.geometry.getAttribute('position') as InterleavedBufferAttribute).data.array;
    expect(data.byteLength).toBe(original.byteLength * ids.length);
    ids.forEach((id, occurrence) => {
      for (let i = 0; i < original.length; i++) expect(data[occurrence * original.length + i]).toBe(i % OUTLINE_STRIDE === 9 ? original[i] + id * 3 : original[i]);
    });
    parent.updateMatrixWorld(true); expect(line.matrixWorld.equals(a.matrixWorld)).toBe(true);
    expect(line.material.depthWrite).toBe(false); expect(line.renderOrder).toBe(1);
  }
  expect(root.children).toEqual([a, b, c]); expect(a.children).toHaveLength(0); expect(structure.parts).toEqual(parts); expect(structure.inspect()).toEqual(inspection);
  b.visible = false; owner.sync(); expect(lines[0].material.uniforms.occurrenceVisible.value.slice(0, 3)).toEqual(new Float32Array([1, 0, 1]));
  root.visible = false; owner.sync(); expect(owner.group.visible).toBe(false); root.visible = true; owner.sync(); expect(owner.group.visible).toBe(true);
  const geometryDispose = lines.map(line => vi.spyOn(line.geometry, 'dispose')), materialDispose = lines.map(line => vi.spyOn(line.material, 'dispose'));
  await owner.set(false); owner.dispose(); owner.dispose(); geometryDispose.forEach(spy => expect(spy).toHaveBeenCalledOnce()); materialDispose.forEach(spy => expect(spy).toHaveBeenCalledOnce()); expect(parent.children).toEqual([root]);
});
it('keeps generated graphs with local transforms on their original per-occurrence normal-matrix path', async () => {
  vi.stubGlobal('Worker', class {
    onmessage: any; onerror: any; onmessageerror: any; terminate() {}
    postMessage() { queueMicrotask(() => this.onmessage?.({ data: { ok: true, data: [extract(new BoxGeometry())] } })); }
  });
  const root = new Group(), mesh = new Mesh(new BoxGeometry(), new MeshBasicMaterial()); mesh.scale.set(2, 3, 4); root.add(mesh);
  const owner = createOutlines(root, () => '#ffffff', { generated: true }); expect(await owner.set(true)).toBe('on');
  expect(owner.group.children).toHaveLength(0); expect(mesh.children).toHaveLength(1); const line = mesh.children[0] as LineSegments<BufferGeometry, ShaderMaterial>;
  expect(line.material.vertexShader).not.toContain('occurrenceVisible'); root.updateMatrixWorld(true); expect(line.matrixWorld.equals(mesh.matrixWorld)).toBe(true); owner.dispose();
});
it('admits expanded occurrence bytes before allocating packed output', async () => {
  vi.stubGlobal('Worker', class {
    onmessage: any; onerror: any; onmessageerror: any; terminate() {}
    postMessage() { queueMicrotask(() => this.onmessage?.({ data: { ok: true, data: [new Float32Array(40_000 * 2 * OUTLINE_STRIDE)] } })); }
  });
  const root = new Group(), a = new Mesh(new BoxGeometry(), new MeshBasicMaterial()); root.add(a, a.clone());
  const owner = createOutlines(root, () => '#ffffff', { generated: true }); expect(await owner.set(true)).toBe('limit');
  expect(owner.group.children).toHaveLength(0); expect(root.children.every(mesh => mesh.children.length === 0)).toBe(true); owner.dispose();
});

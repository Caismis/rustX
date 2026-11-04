import { Box3, BufferGeometry, Color, DoubleSide, Group, InterleavedBuffer, InterleavedBufferAttribute, LineSegments, Matrix4, Mesh, ShaderMaterial, Sphere, Vector3, type Object3D } from 'three';
import { GEOMETRY_LIMITS as L, GeometryError, requireGeometry as check } from './limits';
import { OUTLINE_STRIDE, type OutlineInput } from './outline-data';

export type OutlineStatus = 'off' | 'on' | 'limit' | 'failed';
/** One optional disposable Worker, bounded line buffers, no render targets or model clones. */
export function createOutlines(root: Object3D, color: () => string, options?: { generated: boolean }) {
  let generation = 0, disposed = false, enabled = false, abort: (() => void) | undefined;
  const group = new Group();
  let packedMeshes: Mesh[] = [], visibility: Float32Array | undefined;
  const lines: LineSegments[] = [], geometries = new Set<BufferGeometry>(), materials = new Set<ShaderMaterial>();
  const release = () => {
    for (const line of lines) line.removeFromParent(); lines.length = 0; packedMeshes = []; visibility = undefined;
    for (const geometry of geometries) geometry.dispose(); geometries.clear();
    for (const material of materials) material.dispose(); materials.clear(); enabled = false;
  };
  const sync = () => {
    if (!visibility) return;
    let any = false;
    for (let i = 0; i < packedMeshes.length; i++) {
      let visible = true;
      for (let item: Object3D | null = packedMeshes[i]; item; item = item.parent) if (!item.visible) { visible = false; break; }
      visibility[i] = visible ? 1 : 0; any ||= visible;
    }
    group.visible = any;
  };
  const current = (version: number) => { if (disposed || generation !== version) throw new DOMException('Outline retired', 'AbortError'); };
  async function set(show: boolean): Promise<OutlineStatus> {
    const version = ++generation; abort?.(); release(); if (!show || disposed) return 'off';
    try {
      const meshes: Mesh[] = [], unique = new Map<BufferGeometry, number>(); let triangles = 0, bytes = 0, draws = 0;
      root.traverse(item => {
        if (!(item instanceof Mesh)) return;
        meshes.push(item); check(meshes.length <= L.parts, 'limit');
        // Alpha-tested/translucent silhouettes need a texture-aware method; keep
        // authored materials intact and report this optional mode unavailable.
        const palette = Array.isArray(item.material) ? item.material : [item.material];
        check(palette.every(material => !material.transparent && material.opacity === 1 && material.alphaTest === 0), 'unsupported');
        const geometry = item.geometry;
        if (unique.has(geometry)) return;
        const position = geometry.getAttribute('position');
        triangles += (geometry.index?.count ?? position.count) / 3; bytes += position.count * 12 + (geometry.index?.count ?? 0) * 4;
        check(triangles <= L.outlineTriangles && bytes <= L.outlineInputBytes, 'limit'); unique.set(geometry, unique.size);
      });
      const inputs: OutlineInput[] = [];
      for (const geometry of unique.keys()) {
        const position = geometry.getAttribute('position'), positions = new Float32Array(position.count * 3), index = geometry.index;
        const indices = index ? new Uint32Array(index.count) : undefined;
        for (let start = 0; start < position.count; start += 8192) {
          current(version);
          for (let i = start; i < Math.min(position.count, start + 8192); i++) { positions[i * 3] = position.getX(i); positions[i * 3 + 1] = position.getY(i); positions[i * 3 + 2] = position.getZ(i); }
          if (start + 8192 < position.count) await new Promise(resolve => setTimeout(resolve, 0));
        }
        if (index && indices) for (let start = 0; start < index.count; start += 32768) {
          current(version); for (let i = start; i < Math.min(index.count, start + 32768); i++) indices[i] = index.getX(i);
          if (start + 32768 < index.count) await new Promise(resolve => setTimeout(resolve, 0));
        }
        inputs.push({ positions, indices });
      }
      current(version);
      const data = await new Promise<Float32Array[]>((resolve, reject) => {
        let worker: Worker;
        try { worker = new Worker(new URL('./outline-worker.ts', import.meta.url), { type: 'module', name: 'rustx-outlines' }); }
        catch { reject(new GeometryError('worker')); return; }
        let finished = false;
        const finish = () => { if (finished) return; finished = true; clearTimeout(timer); worker.onmessage = worker.onerror = worker.onmessageerror = null; worker.terminate(); if (abort === cancel) abort = undefined; };
        const cancel = () => { finish(); reject(new DOMException('Outline retired', 'AbortError')); }; abort = cancel;
        const timer = setTimeout(() => { finish(); reject(new GeometryError('timeout')); }, L.outlineTimeoutMs);
        worker.onerror = worker.onmessageerror = () => { finish(); reject(new GeometryError('worker')); };
        worker.onmessage = event => { finish(); if (event.data?.ok === true) resolve(event.data.data); else reject(new GeometryError(event.data?.code === 'limit' ? 'limit' : 'worker')); };
        try {
          const transfers = inputs.flatMap(input => input.indices ? [input.positions.buffer, input.indices.buffer] : [input.positions.buffer]);
          worker.postMessage({ inputs, deadline: performance.timeOrigin + performance.now() + L.outlineTimeoutMs }, transfers);
        } catch { finish(); reject(new GeometryError('worker')); }
      });
      current(version);
      check(Array.isArray(data) && data.length === unique.size); let segments = 0;
      for (const array of data) {
        check(array instanceof Float32Array && array.buffer instanceof ArrayBuffer && array.byteOffset === 0 && array.byteLength === array.buffer.byteLength && array.length % (OUTLINE_STRIDE * 2) === 0);
        segments += array.length / (OUTLINE_STRIDE * 2); check(segments <= L.outlineSegments, 'limit');
        for (let start = 0; start < array.length; start += OUTLINE_STRIDE * 8192) {
          current(version);
          for (let i = start; i < Math.min(array.length, start + OUTLINE_STRIDE * 8192); i += OUTLINE_STRIDE) {
            for (let component = 0; component < 9; component++) check(Number.isFinite(array[i + component]) && Math.abs(array[i + component]) <= (component < 3 ? L.coordinate : 1));
            check(array[i + 9] === 0 || array[i + 9] === 1 || array[i + 9] === 2);
          }
          if (start + OUTLINE_STRIDE * 8192 < array.length) await new Promise(resolve => setTimeout(resolve, 0));
        }
      }
      let rendered = 0;
      for (const mesh of meshes) { const count = data[unique.get(mesh.geometry)!].length / (OUTLINE_STRIDE * 2); rendered += count; if (count) draws++; }
      check(rendered <= L.outlineSegments && draws <= L.parts, 'limit'); current(version);
      const buffer = (array: Float32Array) => {
        const geometry = new BufferGeometry(), buffer = new InterleavedBuffer(array, OUTLINE_STRIDE);
        geometry.setAttribute('position', new InterleavedBufferAttribute(buffer, 3, 0));
        geometry.setAttribute('faceA', new InterleavedBufferAttribute(buffer, 3, 3)); geometry.setAttribute('faceB', new InterleavedBufferAttribute(buffer, 3, 6));
        geometry.setAttribute('edgeKind', new InterleavedBufferAttribute(buffer, 1, 9)); geometries.add(geometry); return geometry;
      };
      // Generated sources retain identity local transforms. Keep authored and
      // transformed graphs on the per-occurrence normal-matrix path below.
      let packed = options?.generated === true;
      if (packed) { root.updateMatrixWorld(true); const identity = new Matrix4(); root.traverse(item => { if (!item.matrix.equals(identity)) packed = false; }); }
      const buffers = packed ? [] : data.map(buffer);
      if (packed) { packedMeshes = meshes; visibility = new Float32Array(Math.ceil(L.parts / 4) * 4); }
      const palette = new Map<boolean, ShaderMaterial>();
      const doubleSide = (mesh: Mesh) => (Array.isArray(mesh.material) ? mesh.material : [mesh.material]).some(material => material.side === DoubleSide);
      const materialFor = (doubleSided: boolean) => {
        let material = palette.get(doubleSided);
        if (!material) {
          material = new ShaderMaterial({ depthWrite: false, uniforms: { lineColor: { value: new Color(color()) }, doubleSided: { value: doubleSided ? 1 : 0 }, ...(packed ? { occurrenceVisible: { value: visibility } } : {}) },
            vertexShader: `attribute vec3 faceA; attribute vec3 faceB; attribute float edgeKind; uniform float doubleSided; varying float visibleEdge;
              ${packed ? `uniform vec4 occurrenceVisible[${Math.ceil(L.parts / 4)}];` : ''}
              void main() { ${packed ? `int occurrence = int(floor(edgeKind / 3.0));
                if (occurrenceVisible[occurrence / 4][occurrence - (occurrence / 4) * 4] < 0.5) { visibleEdge = 0.0; gl_Position = vec4(2.0, 2.0, 2.0, 1.0); return; }
                float kind = mod(edgeKind, 3.0);` : ''}
                vec4 view = modelViewMatrix * vec4(position, 1.0);
                float a = dot(normalMatrix * faceA, -view.xyz); float b = dot(normalMatrix * faceB, -view.xyz);
                float front = max(step(0.0, a), step(0.0, b));
                visibleEdge = ${packed ? 'kind' : 'edgeKind'} > 0.5 ? max(front, doubleSided) : step(a * b, 0.0);
                gl_Position = projectionMatrix * view;
                // Pull depth toward the eye by one part per million of eye distance.
                // The perspective projection's translation term converts that
                // relative tolerance to clip depth without a fixed NDC offset.
                gl_Position.z -= abs(projectionMatrix[3][2]) * (0.000001 / 0.999999); }`,
            fragmentShader: `uniform vec3 lineColor; varying float visibleEdge;
              void main() { if (visibleEdge < 0.5) discard; gl_FragColor = vec4(lineColor, 1.0);
                #include <colorspace_fragment>
              }`,
          }); palette.set(doubleSided, material); materials.add(material);
        }
        return material;
      };
      if (packed) {
        // Admission above includes repeated occurrences: the two fixed output
        // allocations together retain at most 65,536 * 80 bytes (5 MiB).
        for (const sided of [false, true]) {
          const members = meshes.map((mesh, id) => ({ mesh, id })).filter(({ mesh }) => doubleSide(mesh) === sided);
          const length = members.reduce((sum, { mesh }) => sum + data[unique.get(mesh.geometry)!].length, 0);
          if (!length) continue;
          const array = new Float32Array(length), bounds = new Box3(); let offset = 0;
          for (const { mesh, id } of members) {
            const source = data[unique.get(mesh.geometry)!]; array.set(source, offset);
            for (let i = offset + 9; i < offset + source.length; i += OUTLINE_STRIDE) array[i] += id * 3;
            offset += source.length;
            if (!mesh.geometry.boundingBox) mesh.geometry.computeBoundingBox();
            bounds.union(mesh.geometry.boundingBox!);
          }
          const geometry = buffer(array); geometry.boundingBox = bounds;
          geometry.boundingSphere = new Sphere(bounds.getCenter(new Vector3()), bounds.getSize(new Vector3()).length() / 2);
          const line = new LineSegments(geometry, materialFor(sided)); line.renderOrder = 1; group.add(line); lines.push(line);
        }
        sync();
      } else for (const mesh of meshes) {
        const geometry = buffers[unique.get(mesh.geometry)!]; if (!geometry.getAttribute('position').count) continue;
        geometry.boundingSphere = mesh.geometry.boundingSphere?.clone() ?? null;
        const line = new LineSegments(geometry, materialFor(doubleSide(mesh))); line.renderOrder = 1; mesh.add(line); lines.push(line);
      }
      enabled = true; return 'on';
    } catch (error) {
      if (generation !== version || disposed) return 'off'; release();
      return error instanceof GeometryError && ['limit', 'unsupported'].includes(error.code) ? 'limit' : 'failed';
    }
  }
  return { set, group, sync, get enabled() { return enabled; },
    recolor() { for (const material of materials) material.uniforms.lineColor.value.set(color()); },
    dispose() { if (disposed) return; disposed = true; generation++; abort?.(); release(); group.removeFromParent(); },
  };
}

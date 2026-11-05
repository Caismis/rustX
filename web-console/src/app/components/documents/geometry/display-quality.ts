import { BufferAttribute, BufferGeometry, LineSegments, Mesh, ShaderMaterial, Vector3, type Camera, type Object3D, type Scene, type Raycaster } from 'three';
import { GEOMETRY_LIMITS as L } from './limits';
import { geometryPreviewState, takeGeometryPreview } from './scene-resources';

/** Original materials always render. A bounded bridge keeps motion responsive
 * when optional surface preparation cannot finish; settled draws stay exact. */
export function createDisplayQuality(root: Object3D) {
  const meshes: Mesh[] = [], bridges = new Map<BufferGeometry, BufferGeometry>(), lods = new Map<BufferGeometry, { geometry: BufferGeometry; error: number }>();
  let triangles = 0;
  root.traverse(item => { if (item instanceof Mesh) { meshes.push(item); triangles += (item.geometry.index?.count ?? item.geometry.getAttribute('position').count) / 3; } });
  const originals = meshes.map(mesh => mesh.geometry);
  const stride = Math.max(1, Math.ceil(triangles / L.movingTriangles)), eye = new Vector3(), center = new Vector3();
  function bridge(original: BufferGeometry) {
    let sample = bridges.get(original); if (sample) return sample;
    const position = original.getAttribute('position'), index = original.index, count = Math.ceil((index?.count ?? position.count) / 3 / stride);
    sample = new BufferGeometry();
    for (const [name, attribute] of Object.entries(original.attributes)) {
      const values = new Float32Array(count * 3 * attribute.itemSize);
      for (let triangle = 0; triangle < count; triangle++) for (let corner = 0; corner < 3; corner++) {
        const element = triangle * stride * 3 + corner, vertex = index ? index.getX(element) : element;
        for (let component = 0; component < attribute.itemSize; component++) values[(triangle * 3 + corner) * attribute.itemSize + component] = attribute.getComponent(vertex, component);
      }
      sample.setAttribute(name, new BufferAttribute(values, attribute.itemSize));
    }
    sample.addGroup(0, count * 3, 0); sample.boundingBox = original.boundingBox!.clone(); sample.boundingSphere = original.boundingSphere!.clone();
    bridges.set(original, sample); return sample;
  }
  function representation(mesh: Mesh, camera?: Camera, height = 0) {
    const original = mesh.geometry, state = geometryPreviewState(original), incoming = takeGeometryPreview(original);
    if (incoming) lods.set(original, incoming);
    const lod = lods.get(original);
    if (lod) { bridges.get(original)?.dispose(); bridges.delete(original); }
    if (lod) {
      if (!camera) return lod.geometry;
      const sphere = original.boundingSphere!, scale = mesh.matrixWorld.getMaxScaleOnAxis(); center.copy(sphere.center).applyMatrix4(mesh.matrixWorld);
      const distance = eye.distanceTo(center) - sphere.radius * scale, perspective = camera.projectionMatrix.elements[15] === 0;
      const pixels = height * camera.projectionMatrix.elements[5] / 2 / (perspective ? Math.max(1e-12, distance) : 1);
      return lod.error * scale * pixels <= 1 ? lod.geometry : original;
    }
    return state ? bridge(original) : original;
  }
  const sampled = <T>(work: () => T, camera?: Camera, height?: number) => {
    if (camera) eye.setFromMatrixPosition(camera.matrixWorld);
    try { meshes.forEach(mesh => { mesh.geometry = representation(mesh, camera, height); }); return work(); }
    finally { meshes.forEach((mesh, index) => { mesh.geometry = originals[index]; }); }
  };
  return {
    draw(scene: Scene, moving: boolean, camera: Camera, height: number, submit: () => void) {
      if (!moving) { submit(); return; }
      const lines: LineSegments[] = [];
      try {
        sampled(() => {
          scene.traverse(item => { if (item instanceof LineSegments && item.material instanceof ShaderMaterial && item.visible) { lines.push(item); item.visible = false; } });
          submit();
        }, camera, height);
      } finally { lines.forEach(line => { line.visible = true; }); }
    },
    // A display approximation cannot prove foreground occlusion. Geometry refs
    // are already restored after each submission; pick those admitted originals.
    pick(raycaster: Raycaster, candidates: Mesh[]) { return raycaster.intersectObjects(candidates, false)[0]; },
    refresh() { for (const original of originals) { const incoming = takeGeometryPreview(original); if (incoming) { lods.set(original, incoming); bridges.get(original)?.dispose(); bridges.delete(original); } } },
    dispose() { for (const sample of bridges.values()) sample.dispose(); for (const value of lods.values()) value.geometry.dispose();bridges.clear();lods.clear(); },
  };
}

import { BufferGeometry, LineSegments, Mesh, ShaderMaterial, Vector3, type Camera, type Object3D, type Scene, type Raycaster } from 'three';
import { takeGeometryPreview } from './scene-resources';

/** Motion may use a prepared complete surface within one CSS pixel of error.
 * Pending preparation and settled draws retain the admitted original geometry. */
export function createDisplayQuality(root: Object3D) {
  const meshes: Mesh[] = [], lods = new Map<BufferGeometry, { geometry: BufferGeometry; error: number }>();
  root.traverse(item => { if (item instanceof Mesh) meshes.push(item); });
  const originals = meshes.map(mesh => mesh.geometry), eye = new Vector3(), center = new Vector3();
  function representation(mesh: Mesh, camera: Camera, height: number) {
    const original = mesh.geometry, incoming = takeGeometryPreview(original);
    if (incoming) lods.set(original, incoming);
    const lod = lods.get(original);
    if (lod) {
      const sphere = original.boundingSphere!, scale = mesh.matrixWorld.getMaxScaleOnAxis(); center.copy(sphere.center).applyMatrix4(mesh.matrixWorld);
      const distance = eye.distanceTo(center) - sphere.radius * scale, perspective = camera.projectionMatrix.elements[15] === 0;
      const pixels = height * camera.projectionMatrix.elements[5] / 2 / (perspective ? Math.max(1e-12, distance) : 1);
      return lod.error * scale * pixels <= 1 ? lod.geometry : original;
    }
    return original;
  }
  const withRepresentations = <T>(work: () => T, camera: Camera, height: number) => {
    eye.setFromMatrixPosition(camera.matrixWorld);
    try { meshes.forEach(mesh => { mesh.geometry = representation(mesh, camera, height); }); return work(); }
    finally { meshes.forEach((mesh, index) => { mesh.geometry = originals[index]; }); }
  };
  return {
    draw(scene: Scene, moving: boolean, camera: Camera, height: number, submit: () => void) {
      if (!moving) { submit(); return; }
      const lines: LineSegments[] = [];
      try {
        withRepresentations(() => {
          scene.traverse(item => { if (item instanceof LineSegments && item.material instanceof ShaderMaterial && item.visible) { lines.push(item); item.visible = false; } });
          submit();
        }, camera, height);
      } finally { lines.forEach(line => { line.visible = true; }); }
    },
    // A display approximation cannot prove foreground occlusion. Geometry refs
    // are already restored after each submission; pick those admitted originals.
    pick(raycaster: Raycaster, candidates: Mesh[]) { return raycaster.intersectObjects(candidates, false)[0]; },
    refresh() { for (const original of originals) { const incoming = takeGeometryPreview(original); if (incoming) lods.set(original, incoming); } },
    dispose() { for (const value of lods.values()) value.geometry.dispose(); lods.clear(); },
  };
}

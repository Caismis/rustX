import { BufferAttribute, BufferGeometry, DoubleSide, LineSegments, Mesh, MeshBasicMaterial, ShaderMaterial, type Object3D, type Scene, type Raycaster } from 'three';
import { GEOMETRY_LIMITS as L } from './limits';

/** Disposable display samples, never replacements for admitted geometry.
 * Sampling is O(movingTriangles + parts), independent of source triangle count.
 * Originals are restored before input, inspection or publication can run. */
export function createDisplayQuality(root: Object3D) {
  const meshes: Mesh[] = [];
  let triangles = 0;
  root.traverse(item => {
    if (item instanceof Mesh) { meshes.push(item); triangles += (item.geometry.index?.count ?? item.geometry.getAttribute('position').count) / 3; }
  });
  const stride = Math.max(1, Math.ceil(triangles / L.movingTriangles)), samples = new Map<BufferGeometry, BufferGeometry>();
  const material = new MeshBasicMaterial({ color: 0x9aafc2, side: DoubleSide });
  for (const mesh of meshes) {
    const original = mesh.geometry;
    if (samples.has(original)) continue;
    const position = original.getAttribute('position'), index = original.index;
    const start = Math.ceil(original.drawRange.start / 3), end = Math.floor(Math.min(index?.count ?? position.count, original.drawRange.start + original.drawRange.count) / 3);
    const count = Math.ceil(Math.max(0, end - start) / stride), positions = new Float32Array(count * 9);
    for (let triangle = 0; triangle < count; triangle++) for (let corner = 0; corner < 3; corner++) {
      const element = (start + triangle * stride) * 3 + corner, vertex = index ? index.getX(element) : element, at = triangle * 9 + corner * 3;
      positions[at] = position.getX(vertex); positions[at + 1] = position.getY(vertex); positions[at + 2] = position.getZ(vertex);
    }
    const sample = new BufferGeometry().setAttribute('position', new BufferAttribute(positions, 3));
    sample.addGroup(0, count * 3, 0);
    sample.boundingBox = original.boundingBox?.clone() ?? null; sample.boundingSphere = original.boundingSphere?.clone() ?? null;
    samples.set(original, sample);
  }
  const sampled = <T>(work: () => T, shading = false) => {
    const originals = meshes.map(mesh => mesh.geometry);
    const materials = shading ? meshes.map(mesh => mesh.material) : undefined;
    try { meshes.forEach((mesh, index) => { mesh.geometry = samples.get(originals[index])!; if (shading) mesh.material = material; }); return work(); }
    finally { meshes.forEach((mesh, index) => { mesh.geometry = originals[index]; if (materials) mesh.material = materials[index]; }); }
  };
  return {
    draw(scene: Scene, moving: boolean, submit: () => void) {
      if (!moving) { submit(); return; }
      const lines: LineSegments[] = [];
      scene.traverse(item => { if (item instanceof LineSegments && item.material instanceof ShaderMaterial && item.visible) { lines.push(item); item.visible = false; } });
      try {
        sampled(submit, true);
      } finally {
        lines.forEach(line => { line.visible = true; });
      }
    },
    pick(raycaster: Raycaster, candidates: Mesh[]) { return sampled(() => raycaster.intersectObjects(candidates, false)[0]); },
    dispose() { for (const sample of samples.values()) sample.dispose(); samples.clear(); material.dispose(); },
  };
}

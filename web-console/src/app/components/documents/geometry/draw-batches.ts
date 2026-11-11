import { Box3, Matrix4, Mesh, Sphere, Vector3, type BufferGeometry, type Material, type Object3D } from 'three';
import { mergeGeometries } from 'three/addons/utils/BufferGeometryUtils.js';
import { GEOMETRY_LIMITS as L } from './limits';

/** Exact draw-only batches for small generated assemblies. Identity occurrences
 * with one shared opaque material can share a draw without changing any float.
 * The source graph stays authoritative outside the synchronous submission. */
export function createDrawBatches(root: Object3D, generated: boolean) {
  const groups = new Map<Material, Mesh[]>(), batches: { sources: Mesh[]; originals: BufferGeometry[]; mesh: Mesh }[] = [];
  const identity = new Matrix4(); let bytes = 0;
  if (generated) root.traverse(item => {
    if (!(item instanceof Mesh)) return;
    const geometry: BufferGeometry = item.geometry, material = Array.isArray(item.material) ? item.material.length === 1 ? item.material[0] : undefined : item.material;
    if (!material || material.transparent || material.alphaTest || geometry.groups.length !== 1 || geometry.groups[0].materialIndex !== 0
      || geometry.drawRange.start !== 0 || geometry.drawRange.count < (geometry.index?.count ?? geometry.getAttribute('position').count)) return;
    for (let object: Object3D | null = item; object && object !== root; object = object.parent) if (!object.matrix.equals(identity)) return;
    bytes += Object.values(geometry.attributes).reduce((sum, value) => sum + value.count * value.itemSize * 4, 0) + (geometry.index?.array.byteLength ?? 0);
    const sources = groups.get(material) ?? []; sources.push(item); groups.set(material, sources);
  });
  if (bytes <= L.previewInputBytes) for (const [material, sources] of groups) {
    if (sources.length < 4) continue;
    const originals = sources.map(source => source.geometry), geometry = mergeGeometries(originals, false);
    if (!geometry) continue;
    const bounds = new Box3(); for (const original of originals) bounds.union(original.boundingBox!);
    geometry.boundingBox = bounds; geometry.boundingSphere = new Sphere(bounds.getCenter(new Vector3()), bounds.getSize(new Vector3()).length() / 2);
    batches.push({ sources, originals, mesh: new Mesh(geometry, material) });
  }
  const visible = (mesh: Mesh) => { for (let object: Object3D | null = mesh; object; object = object.parent) if (!object.visible) return false; return true; };
  return {
    draw(submit: () => void) {
      const active = batches.filter(batch => batch.sources.every((mesh, index) => mesh.geometry === batch.originals[index] && visible(mesh)));
      try {
        for (const batch of active) { for (const mesh of batch.sources) mesh.visible = false; root.add(batch.mesh); }
        submit();
      } finally {
        for (const batch of active) { batch.mesh.removeFromParent(); for (const mesh of batch.sources) mesh.visible = true; }
      }
    },
    dispose() { for (const batch of batches) batch.mesh.geometry.dispose(); batches.length = 0; },
  };
}

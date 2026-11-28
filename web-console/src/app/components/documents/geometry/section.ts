import { Color, Mesh, Plane, type Box3, type Intersection, type Material, type Object3D, type Raycaster, type Vector3 } from 'three';
import type { PreviewViewState } from '../../../../presentation/right-panel/preview-view-state';

export type Section = NonNullable<NonNullable<PreviewViewState['geometry']>['section']>;
export const sectionAxes = ['x', 'y', 'z'] as const;

export function sectionCenter(bounds: Box3, axis: Section['axis']) {
  return bounds.min[axis] + (bounds.max[axis] - bounds.min[axis]) / 2;
}

export function createSection(bounds: Box3, center: Vector3, saved?: Section) {
  const defaults: Section = { enabled: false, axis: 'z', offset: sectionCenter(bounds, 'z'), reversed: false };
  const plane = new Plane();
  const planes = [plane];
  let value = defaults;

  function set(candidate: Section) {
    const axis = sectionAxes.includes(candidate.axis) ? candidate.axis : defaults.axis;
    const offset = Number.isFinite(candidate.offset) ? candidate.offset : sectionCenter(bounds, axis);
    value = { axis, offset: Math.min(bounds.max[axis], Math.max(bounds.min[axis], offset)),
      enabled: candidate.enabled === true, reversed: candidate.reversed === true };
    const direction = value.reversed ? -1 : 1;
    // Center first so the tolerance is relative to model size, even for a
    // small model a long way from the origin. The GPU stores planes as floats.
    const tolerance = 8 * 2 ** -23 * (bounds.max[axis] - bounds.min[axis]);
    plane.normal.set(0, 0, 0)[axis] = direction;
    plane.constant = tolerance - direction * (value.offset - center[axis]);
  }
  set(saved ?? defaults);
  return {
    get value() { return value; },
    get planes() { return value.enabled ? planes : []; },
    set,
    reset: () => set({ ...defaults, enabled: value.enabled }),
    retains: (point: Vector3) => !value.enabled || plane.distanceToPoint(point) >= 0,
    frame(box: Box3) {
      if (value.enabled && !box.isEmpty()) {
        const axis = value.axis;
        if (value.reversed) box.max[axis] = Math.min(box.max[axis], plane.constant);
        else box.min[axis] = Math.max(box.min[axis], -plane.constant);
      }
      return box;
    },
  };
}

export function pickGeometry(root: Object3D, raycaster: Raycaster, retains: (point: Vector3) => boolean): Intersection | undefined {
  const candidates: Mesh[] = [];
  root.traverseVisible(item => { if (item instanceof Mesh) candidates.push(item); });
  // The original graph is restored after every synchronous draw. Walk past
  // discarded hits instead of letting a clipped foreground occlude the ray.
  return raycaster.intersectObjects(candidates, false).find(hit => retains(hit.point));
}

/** Use the selected surfaces for the section highlight. A box can intersect
 * the plane even when the selected part has no retained surface at all. */
export function createSectionHighlight() {
  const tint = new Color(0xffbd45);
  const originals = new Map<Mesh, Material | Material[]>();
  const materials = new Map<Material, Material>();
  const clear = () => {
    for (const [mesh, material] of originals) mesh.material = material;
    for (const material of materials.values()) material.dispose();
    originals.clear(); materials.clear();
  };
  const color = (original: Material) => {
    if (!('color' in original) || !(original.color instanceof Color)) return original;
    let material = materials.get(original);
    if (!material) {
      material = original.clone();
      (material as Material & { color: Color }).color.lerp(tint, .55);
      materials.set(original, material);
    }
    return material;
  };
  return {
    clear,
    set(selected?: Object3D) {
      clear();
      selected?.traverse(item => {
        if (!(item instanceof Mesh)) return;
        originals.set(item, item.material);
        item.material = Array.isArray(item.material) ? item.material.map(color) : color(item.material);
      });
    },
  };
}

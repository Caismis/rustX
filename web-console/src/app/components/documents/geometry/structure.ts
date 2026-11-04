import { Box3, Mesh, Vector3, type Object3D } from 'three';
import type { PreviewViewState } from '../../../../presentation/right-panel/preview-view-state';
import { GEOMETRY_LIMITS as L } from './limits';

type Saved = NonNullable<PreviewViewState['geometry']>;
export interface Part { id: string; name: string; depth: number; kind: 'mesh' | 'group' }
export interface Inspection {
  name: string; triangles: number; visibleTriangles: number;
  /** Admitted world axes, before the presentation-only centering translation. */
  min: number[]; max: number[]; dimensions: number[];
}

/** Occurrence operations mutate the admitted graph, never its geometry or materials. */
export function createStructure(root: Object3D, center: Vector3, saved?: Saved) {
  const objects = new Map<string, Object3D>(), occurrenceIds = new Map<Object3D, string>(), parts: Part[] = [];
  const hidden = new Set<string>();
  let selected: string | undefined, isolation: Saved['isolation'];
  function walk(item: Object3D, id: string, depth: number) {
    if (item instanceof Mesh || item.name) {
      const name = typeof item.userData.name === 'string' ? item.userData.name : item.name;
      objects.set(id, item); occurrenceIds.set(item, id);
      parts.push({ id, name: name.slice(0, 256), depth, kind: item instanceof Mesh ? 'mesh' : 'group' });
    }
    item.children.forEach((child, index) => walk(child, `${id}.${index}`, depth + 1));
  }
  walk(root, '0', 0);
  function setHidden(ids: Iterable<string>) {
    const next = new Set([...ids].filter(id => objects.has(id)).slice(0, L.nodes));
    if (next.size === hidden.size && [...next].every(id => hidden.has(id))) return false;
    hidden.clear();
    for (const [id, item] of objects) { item.visible = !next.has(id); if (!item.visible) hidden.add(id); }
    return true;
  }
  setHidden((saved?.hidden ?? []).slice(0, L.nodes));
  if (saved?.selected && objects.has(saved.selected)) selected = saved.selected;
  if (saved?.isolation && objects.has(saved.isolation.target)) isolation = {
    target: saved.isolation.target, hidden: saved.isolation.hidden.slice(0, L.nodes).filter(id => objects.has(id)),
  };
  const visible = (item: Object3D) => {
    for (let ancestor: Object3D | null = item; ancestor; ancestor = ancestor.parent) if (!ancestor.visible) return false;
    return true;
  };
  function bounds(item = root, onlyVisible = false) {
    const result = new Box3();
    if (onlyVisible && !visible(item)) return result;
    const visit = (child: Object3D) => {
      if (child instanceof Mesh && child.geometry.boundingBox) result.union(child.geometry.boundingBox.clone().applyMatrix4(child.matrixWorld));
    };
    if (onlyVisible) item.traverseVisible(visit); else item.traverse(visit);
    return result;
  }
  return {
    parts, occurrenceIds,
    get selected() { return selected; },
    get selectedObject() { return selected ? objects.get(selected) : undefined; },
    get isolated() { return isolation !== undefined; },
    visible, bounds,
    snapshot: () => ({ selected, hidden: [...hidden], isolation: isolation ? { target: isolation.target, hidden: [...isolation.hidden] } : undefined }),
    select(id?: string) {
      const next = id && objects.has(id) ? id : undefined;
      if (selected === next) return false;
      selected = next; return true;
    },
    visibility(id: string, show: boolean) {
      const item = objects.get(id); if (!item || item.visible === show) return false;
      item.visible = show; if (show) hidden.delete(id); else hidden.add(id); return true;
    },
    isolate() {
      if (!selected) return false;
      const target = selected;
      if (isolation?.target === target) return false;
      const previous = isolation?.hidden ?? [...hidden];
      // Keep the selected subtree and its ancestors; whole path segments matter.
      setHidden([...objects.keys()].filter(id => id !== target && !id.startsWith(`${target}.`) && !target.startsWith(`${id}.`)));
      isolation = { target, hidden: previous }; return true;
    },
    restore() {
      if (!isolation) return false;
      setHidden(isolation.hidden); isolation = undefined; return true;
    },
    showAll() {
      const changed = setHidden([]) || isolation !== undefined;
      isolation = undefined; return changed;
    },
    inspect(): Inspection {
      const item = selected ? objects.get(selected)! : root, box = bounds(item).translate(center);
      let triangles = 0, visibleTriangles = 0;
      item.traverse(child => {
        if (!(child instanceof Mesh)) return;
        const count = (child.geometry.index?.count ?? child.geometry.getAttribute('position').count) / 3;
        triangles += count; if (visible(child)) visibleTriangles += count;
      });
      return { name: parts.find(part => part.id === selected)?.name ?? '', triangles, visibleTriangles,
        min: box.isEmpty() ? [] : box.min.toArray(), max: box.isEmpty() ? [] : box.max.toArray(), dimensions: box.isEmpty() ? [] : box.getSize(new Vector3()).toArray() };
    },
  };
}

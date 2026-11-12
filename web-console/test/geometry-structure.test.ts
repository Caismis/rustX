import { expect, it } from 'vitest';
import { Box3, BoxGeometry, Group, Mesh, MeshBasicMaterial, Vector3 } from 'three';
import { createStructure } from '../src/app/components/documents/geometry/structure';

function assembly() {
  const root = new Group(), branch = new Group(), material = new MeshBasicMaterial(), geometry = new BoxGeometry(2, 4, 6);
  branch.name = 'Assembly';
  const a = new Mesh(geometry, material), b = new Mesh(geometry, material), c = new Mesh(geometry, material);
  a.name = 'A'; b.name = 'B'; c.name = 'C'; b.position.x = 10; c.position.x = 20;
  branch.add(a, b); root.add(branch, c); root.updateMatrixWorld(true);
  return { root, branch, a, b, c, geometry, material, structure: createStructure(root, new Vector3()) };
}
it('isolates a hidden descendant in one operation and restores the exact previous visibility set', () => {
  const f = assembly(), s = f.structure;
  s.visibility('0.0', false); s.visibility('0.0.1', false); s.select('0.0.0');
  expect(s.inspect().visibleTriangles).toBe(0); expect(s.isolate()).toBe(true);
  expect(f.branch.visible).toBe(true); expect(f.a.visible).toBe(true); expect(f.b.visible).toBe(false); expect(f.c.visible).toBe(false);
  expect(s.inspect().visibleTriangles).toBe(12); expect(s.isolate()).toBe(false);
  expect(s.restore()).toBe(true); expect(s.snapshot().hidden).toEqual(['0.0', '0.0.1']);
  expect(s.inspect().visibleTriangles).toBe(0); expect(s.restore()).toBe(false);
});
it('changing the isolated selection retains one original snapshot and Show all clears it deliberately', () => {
  const s = assembly().structure; s.visibility('0.1', false); s.select('0.0'); s.isolate();
  s.select('0.1'); s.isolate();
  expect(s.snapshot().isolation).toEqual({ target: '0.1', hidden: ['0.1'] });
  expect(s.snapshot().hidden).toEqual(['0.0', '0.0.0', '0.0.1']);
  s.restore(); expect(s.snapshot().hidden).toEqual(['0.1']);
  s.isolate(); s.showAll(); expect(s.snapshot().hidden).toEqual([]); expect(s.isolated).toBe(false);
  expect(s.showAll()).toBe(false);
});
it('restores inert isolation metadata on the same graph copy without cloning source resources', () => {
  const f = assembly(), s = f.structure; s.visibility('0.0.1', false); s.select('0.0.0'); s.isolate();
  const state = s.snapshot(), fresh = assembly();
  const restored = createStructure(fresh.root, new Vector3(), { copyId: 'same-copy', ...state });
  expect(restored.snapshot()).toEqual(state); restored.restore(); expect(restored.snapshot().hidden).toEqual(['0.0.1']);
  expect(f.a.geometry).toBe(f.geometry); expect(f.b.geometry).toBe(f.geometry); expect(f.a.material).toBe(f.material);
  state.isolation!.hidden.push('bad'); expect(s.snapshot().isolation!.hidden).toEqual(['0.0.1']);
});
it('reports admitted world-axis bounds and occurrence counts before display centering', () => {
  const f = assembly(); f.root.position.set(100, 200, 300); f.root.scale.set(2, 3, 4); f.branch.rotation.z = Math.PI / 2;
  f.root.updateMatrixWorld(true); const reference = new Box3().setFromObject(f.branch);
  const center = new Box3().setFromObject(f.root).getCenter(new Vector3()), display = new Group();
  display.position.copy(center).negate(); display.add(f.root); display.updateMatrixWorld(true);
  const s = createStructure(f.root, center); s.select('0.0'); const info = s.inspect();
  expect(info.triangles).toBe(24); expect(info.visibleTriangles).toBe(24);
  expect(info.min).toEqual(reference.min.toArray()); expect(info.max).toEqual(reference.max.toArray());
  expect(info.dimensions).toEqual(reference.getSize(new Vector3()).toArray());
  s.visibility('0.0.0', false); expect(s.inspect().triangles).toBe(24); expect(s.inspect().visibleTriangles).toBe(12);
  s.visibility('0.0', false); expect(s.bounds(s.selectedObject, true).isEmpty()).toBe(true);
});
it('does not treat numeric sibling path prefixes as ancestry', () => {
  const root = new Group(); for (let i = 0; i <= 10; i++) root.add(new Mesh(new BoxGeometry(), new MeshBasicMaterial()));
  const s = createStructure(root, new Vector3()); s.select('0.1'); s.isolate();
  expect(root.children[1].visible).toBe(true); expect(root.children[10].visible).toBe(false);
});

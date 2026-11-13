import { expect, it, vi } from 'vitest';
import { BoxGeometry, Group, Mesh, MeshStandardMaterial } from 'three';
import { createDrawBatches } from '../src/app/components/documents/geometry/draw-batches';
import { disposeScene, validateScene } from '../src/app/components/documents/geometry/scene-resources';

function assembly() {
  const root = new Group(), material = new MeshStandardMaterial(), branch = new Group(); root.add(branch);
  const meshes = Array.from({ length: 8 }, (_, index) => {
    const geometry = new BoxGeometry().translate(index, 0, 0); geometry.clearGroups(); geometry.addGroup(0, geometry.index!.count, 0);
    const mesh = new Mesh(geometry, [material]); branch.add(mesh); return mesh;
  });
  validateScene([root]); return { root, branch, meshes, material };
}
it('combines exact position/normal/index streams into one draw and restores the authoritative graph', () => {
  const f = assembly(), owner = createDrawBatches(f.root, true), originals = f.meshes.map(mesh => mesh.geometry);
  let batch: Mesh | undefined;
  try {
    owner.draw(() => {
      const visible: Mesh[] = []; f.root.traverseVisible(item => { if (item instanceof Mesh) visible.push(item); });
      expect(visible).toHaveLength(1); batch = visible[0]; expect(batch.material).toBe(f.material);
      for (const attribute of ['position', 'normal']) {
        const expected = originals.flatMap(geometry => [...new Uint32Array(geometry.getAttribute(attribute).array.buffer)]);
        expect([...new Uint32Array(batch.geometry.getAttribute(attribute).array.buffer)]).toEqual(expected);
      }
      expect(batch.geometry.index!.count).toBe(originals.reduce((sum, geometry) => sum + geometry.index!.count, 0));
    });
    expect(f.root.children).toEqual([f.branch]); expect(f.branch.children).toEqual(f.meshes);
    f.meshes.forEach((mesh, index) => { expect(mesh.visible).toBe(true); expect(mesh.geometry).toBe(originals[index]); });
    const disposed = vi.spyOn(batch!.geometry, 'dispose'); owner.dispose(); owner.dispose(); expect(disposed).toHaveBeenCalledOnce();
  } finally { owner.dispose(); disposeScene([f.root]); }
});
it('respects hidden ancestors, geometry changes, and cleanup after a failed submission', () => {
  const f = assembly(), owner = createDrawBatches(f.root, true);
  try {
    f.branch.visible = false; owner.draw(() => expect(f.root.children).toEqual([f.branch])); f.branch.visible = true;
    f.meshes[0].visible = false; owner.draw(() => expect(f.root.children).toEqual([f.branch])); f.meshes[0].visible = true;
    expect(() => owner.draw(() => { throw new Error('draw failed'); })).toThrow('draw failed');
    expect(f.root.children).toEqual([f.branch]); expect(f.meshes.every(mesh => mesh.visible)).toBe(true);
    const original = f.meshes[0].geometry; f.meshes[0].geometry = original.clone();
    owner.draw(() => expect(f.root.children).toEqual([f.branch])); f.meshes[0].geometry.dispose(); f.meshes[0].geometry = original;
  } finally { owner.dispose(); disposeScene([f.root]); }
});
it.each(['authored', 'transform', 'transparent'] as const)('keeps %s scenes on their original draw path', kind => {
  const f = assembly();
  if (kind === 'transform') { f.branch.position.x = 1; f.root.updateMatrixWorld(true); }
  if (kind === 'transparent') f.material.transparent = true;
  const owner = createDrawBatches(f.root, kind !== 'authored');
  try { owner.draw(() => expect(f.root.children).toEqual([f.branch])); }
  finally { owner.dispose(); disposeScene([f.root]); }
});

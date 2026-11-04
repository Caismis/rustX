import { expect, it, vi } from 'vitest';
import { BoxGeometry, Group, LineSegments, BufferGeometry, LineBasicMaterial, Mesh, MeshBasicMaterial, MeshStandardMaterial, ShaderMaterial, Scene } from 'three';
import { createDisplayQuality } from '../src/app/components/documents/geometry/display-quality';
import { GEOMETRY_LIMITS as L } from '../src/app/components/documents/geometry/limits';

it('shares bounded samples across occurrences and restores exact geometry/materials even when submission throws', () => {
  const scene = new Scene(), root = new Group(), geometry = new BoxGeometry(1, 1, 1, 100, 100, 100), material = new MeshStandardMaterial(); scene.add(root);
  const first = new Mesh(geometry, [material]), second = first.clone(); root.add(first, second);
  const lines = new LineSegments(new BufferGeometry(), new ShaderMaterial()), highlight = new LineSegments(new BufferGeometry(), new LineBasicMaterial()); root.add(lines, highlight);
  const quality = createDisplayQuality(root); let sample: BufferGeometry | undefined;
  try {
    expect(() => quality.draw(scene, true, () => {
      sample = first.geometry; expect(second.geometry).toBe(sample); expect(sample).not.toBe(geometry);
      expect(sample.getAttribute('position').count / 3 * 2).toBeLessThanOrEqual(L.movingTriangles + L.parts);
      expect(lines.visible).toBe(false); expect(highlight.visible).toBe(true); expect(first.material).toBeInstanceOf(MeshBasicMaterial); throw new Error('submission failed');
    })).toThrow('submission failed');
    expect(first.geometry).toBe(geometry); expect(second.geometry).toBe(geometry); expect(first.material).toEqual([material]);
    expect(scene.overrideMaterial).toBeNull(); expect(lines.visible).toBe(true);
    quality.draw(scene, false, () => { expect(first.geometry).toBe(geometry); expect(scene.overrideMaterial).toBeNull(); });
    const dispose = vi.spyOn(sample!, 'dispose'), originalDispose = vi.spyOn(geometry, 'dispose'); quality.dispose();
    expect(dispose).toHaveBeenCalledOnce(); expect(originalDispose).not.toHaveBeenCalled();
  } finally { quality.dispose(); geometry.dispose(); material.dispose(); lines.geometry.dispose(); lines.material.dispose(); highlight.geometry.dispose(); highlight.material.dispose(); }
});
it('does not reenable hidden lines and samples only the admitted draw range', () => {
  const scene = new Scene(), root = new Group(), geometry = new BoxGeometry(), mesh = new Mesh(geometry, new MeshStandardMaterial()); scene.add(root); root.add(mesh);
  geometry.setDrawRange(6, 6); const line = new LineSegments(new BufferGeometry(), new LineBasicMaterial()); line.visible = false; root.add(line);
  const quality = createDisplayQuality(root);
  try {
    quality.draw(scene, true, () => { expect(mesh.geometry.getAttribute('position').count).toBe(6); expect(line.visible).toBe(false); });
    expect(line.visible).toBe(false); expect(geometry.drawRange).toEqual({ start: 6, count: 6 });
  } finally { quality.dispose(); geometry.dispose(); mesh.material.dispose(); line.geometry.dispose(); line.material.dispose(); }
});

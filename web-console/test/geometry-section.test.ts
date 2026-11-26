import { expect, it } from 'vitest';
import { Box3, Color, Group, Mesh, MeshStandardMaterial, PlaneGeometry, Raycaster, Vector3 } from 'three';
import { createSection, createSectionHighlight, pickGeometry } from '../src/app/components/documents/geometry/section';

it('uses original world offsets with a centered render graph and retains the boundary', () => {
  const bounds = new Box3(new Vector3(9000, 24, 100), new Vector3(9000.01, 24, 104));
  const center = bounds.getCenter(new Vector3());
  const section = createSection(bounds, center);
  expect(section.value).toEqual({ enabled: false, axis: 'z', offset: 102, reversed: false });
  section.set({ enabled: true, axis: 'x', offset: 9000.003, reversed: false });
  const at = new Vector3(9000.003 - center.x, 0, 0);
  expect(section.retains(at)).toBe(true);
  expect(section.retains(at.clone().addScalar(-.001))).toBe(false);
  expect(section.planes[0].distanceToPoint(at)).toBeGreaterThan(0);
  const saved = section.value;
  const other = createSection(bounds, center, { ...saved, reversed: true });
  expect(other.retains(at)).toBe(true);
  expect(other.retains(at.clone().addScalar(.001))).toBe(false);
  expect(section.retains(at.clone().addScalar(.001))).toBe(true);
  section.set({ ...saved, enabled: false }); section.set({ ...section.value, enabled: true });
  expect(section.value).toEqual(saved);
  section.set({ ...saved, axis: 'y', offset: NaN });
  expect(section.value.offset).toBe(24);
  section.reset();
  expect(section.value).toEqual({ enabled: true, axis: 'z', offset: 102, reversed: false });
});

it('picks the next original hit through a clipped foreground and respects transformed parent visibility', () => {
  const bounds = new Box3(new Vector3(-2, -2, -2), new Vector3(2, 2, 2));
  const section = createSection(bounds, new Vector3(), { enabled: true, axis: 'z', offset: 0, reversed: true });
  const material = new MeshStandardMaterial(), geometry = new PlaneGeometry();
  const root = new Group(), nested = new Group();
  const front = new Mesh(geometry, material), back = new Mesh(geometry, material);
  root.add(front, nested); nested.add(back);
  nested.position.set(.2, .2, -1); nested.rotation.z = .4; nested.scale.set(-2, 3, 1);
  front.position.z = 1; root.updateMatrixWorld(true);
  const ray = new Raycaster(new Vector3(0, 0, 4), new Vector3(0, 0, -1));
  expect(pickGeometry(root, ray, () => true)?.object).toBe(front);
  expect(pickGeometry(root, ray, section.retains)?.object).toBe(back);
  nested.visible = false;
  expect(pickGeometry(root, ray, section.retains)).toBeUndefined();
  const clipped = section.frame(new Box3(new Vector3(-1, -1, .5), new Vector3(1, 1, 1)));
  expect(clipped.isEmpty()).toBe(true);
  geometry.dispose(); material.dispose();
});

it('tints actual selected surfaces without changing source materials, sides or transparency', () => {
  const material = new MeshStandardMaterial({ color: 0x224466, transparent: true, opacity: .5 });
  const geometry = new PlaneGeometry(), selected = new Mesh(geometry, [material]), other = new Mesh(geometry, material);
  const highlight = createSectionHighlight();
  highlight.set(selected);
  expect(selected.material[0]).not.toBe(material);
  expect(selected.material[0]).toMatchObject({ transparent: true, opacity: .5, side: material.side });
  expect(selected.material[0].color.equals(material.color)).toBe(false);
  expect(other.material).toBe(material);
  expect(material.color.equals(new Color(0x224466))).toBe(true);
  highlight.clear();
  expect(selected.material[0]).toBe(material);
  geometry.dispose(); material.dispose();
});

// @vitest-environment node
import { readFileSync } from 'node:fs';
import { expect, it } from 'vitest';
import { STLLoader } from 'three/addons/loaders/STLLoader.js';
import { importStl } from '../src/app/components/documents/geometry/stl';

const cube = readFileSync(new URL('./fixtures/geometry/cube-ascii.stl', import.meta.url), 'utf8');
const named = (name: string) => cube.replaceAll('cube', name);

it.each(['Cube', 'solid plate', 'Vertex Block', 'facet normal housing'])('admits ASCII solid name %s with the pinned loader geometry intact', name => {
  const bytes = new TextEncoder().encode(named(name)), expected = new STLLoader().parse(bytes.buffer);
  try {
    const model = importStl(bytes);
    expect(model.meshes).toHaveLength(1); expect(model.meshes[0].name).toBe(name);
    expect(model.meshes[0].positions).toEqual(expected.getAttribute('position').array);
    expect(model.meshes[0].normals).toEqual(expected.getAttribute('normal').array);
    expect(model.meshes[0].positions.length / 3).toBe(36);
  } finally { expected.dispose(); }
});
it('does not count names in either boundary of multiple normalized ASCII solids', () => {
  const text = '      ' + (named('Vertex Block') + named('facet normal housing')).replace(/^(solid|endsolid|facet normal|outer loop|endloop|endfacet|vertex)\b/gm, word => word.toUpperCase());
  const model = importStl(new TextEncoder().encode(text));
  expect(model.meshes.map(mesh => mesh.name)).toEqual(['Vertex Block', 'facet normal housing']);
  expect(model.meshes.map(mesh => mesh.positions.length / 3)).toEqual([36, 36]);
});
it('charges actual solid records at the existing part limit', () => {
  const text = Array.from({ length: 256 }, (_, index) => named('Vertex Block ' + index)).join('');
  expect(importStl(new TextEncoder().encode(text)).meshes).toHaveLength(256);
  expect(() => importStl(new TextEncoder().encode(text + named('last')))).toThrowError('limit');
});

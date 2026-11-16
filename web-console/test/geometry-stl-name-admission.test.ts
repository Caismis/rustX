// @vitest-environment node
import { readFileSync } from 'node:fs';
import { expect, it, vi } from 'vitest';
import { STLLoader } from 'three/addons/loaders/STLLoader.js';
import { importStl } from '../src/app/components/documents/geometry/stl';

vi.mock('../src/app/components/documents/geometry/limits', async importOriginal => {
  const actual = await importOriginal<typeof import('../src/app/components/documents/geometry/limits')>();
  return { ...actual, GEOMETRY_LIMITS: Object.freeze({ ...actual.GEOMETRY_LIMITS, vertices: 36, triangles: 12 }) };
});
const cube = readFileSync(new URL('./fixtures/geometry/cube-ascii.stl', import.meta.url), 'utf8');

it('keeps actual loader allocations within the admitted count when names contain numeric vertex syntax', () => {
  const name = 'facet vertex 0 0 0 vertex 1 0 0 vertex 0 1 0 housing', decoded: number[] = [];
  const original = STLLoader.prototype.parse;
  const parse = vi.spyOn(STLLoader.prototype, 'parse').mockImplementation(function (this: STLLoader, data) {
    const geometry = original.call(this, data); decoded.push(geometry.getAttribute('position').count); return geometry;
  });
  try {
    let model: ReturnType<typeof importStl> | undefined, error: unknown;
    try { model = importStl(new TextEncoder().encode(cube.replaceAll('cube', name))); } catch (caught) { error = caught; }
    expect(decoded).toEqual([36]); expect(error).toBeUndefined();
    expect(model!.meshes[0].name).toBe(name); expect(model!.meshes[0].positions.length / 3).toBe(36);
  } finally { parse.mockRestore(); }
});

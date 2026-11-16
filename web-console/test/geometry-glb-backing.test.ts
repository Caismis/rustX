// @vitest-environment node
import { expect, it, vi } from 'vitest';
import { Mesh } from 'three';
import { GLTFLoader } from 'three/addons/loaders/GLTFLoader.js';
import { admitGlb, importGlb } from '../src/app/components/documents/geometry/glb';
import { encodeGlb, paddedGlbViews } from './geometry-glb-fixtures';

it('observes eight distinct padded backing allocations through the actual GLTFLoader', async () => {
  const fixture = paddedGlbViews(8, 1024 * 1024), loaded = await importGlb(encodeGlb(fixture.json, fixture.bin), new AbortController().signal);
  try {
    const backing = new Set<ArrayBufferLike>(); let logical = 0;
    loaded.scene.traverse(object => { if (object instanceof Mesh) { const array = object.geometry.getAttribute('position').array; backing.add(array.buffer); logical += array.byteLength; } });
    expect(logical).toBe(288); expect(backing.size).toBe(8); expect([...backing].map(buffer => buffer.byteLength)).toEqual(Array(8).fill(1024 * 1024));
  } finally { loaded.dispose(); }
});
it('rejects a 528 MiB view allocation plan before invoking the loader', async () => {
  const fixture = paddedGlbViews(132, 4 * 1024 * 1024), parse = vi.spyOn(GLTFLoader.prototype, 'parseAsync').mockRejectedValue(new Error('loader invoked before budget refusal'));
  try {
    await expect(importGlb(encodeGlb(fixture.json, fixture.bin), new AbortController().signal)).rejects.toMatchObject({ code: 'limit' });
    expect(parse).not.toHaveBeenCalled();
  } finally { parse.mockRestore(); }
});
it('deduplicates a loaded view shared by many accessors and excludes unloaded views', () => {
  const fixture = paddedGlbViews(132, 4 * 1024 * 1024); fixture.json.accessors.forEach(accessor => { accessor.bufferView = 0; });
  expect(() => admitGlb(encodeGlb(fixture.json, fixture.bin))).not.toThrow();
});
it('does not charge accessors and views unreachable from any loaded scene', async () => {
  const fixture = paddedGlbViews(132, 4 * 1024 * 1024); fixture.json.meshes[0].primitives.splice(1);
  const loaded = await importGlb(encodeGlb(fixture.json, fixture.bin), new AbortController().signal);
  try { let meshes = 0; loaded.scene.traverse(object => { if (object instanceof Mesh) meshes++; }); expect(meshes).toBe(1); }
  finally { loaded.dispose(); }
});

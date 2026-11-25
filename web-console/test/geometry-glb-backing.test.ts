// @vitest-environment node
import { expect, it, vi } from 'vitest';
import { Mesh } from 'three';
import { GLTFLoader } from 'three/addons/loaders/GLTFLoader.js';
import { admitGlb, importGlb } from '../src/app/components/documents/geometry/glb';
import { validateScene } from '../src/app/components/documents/geometry/scene-resources';
import { encodeGlb, paddedGlbViews } from './geometry-glb-fixtures';

it('passes an exact owned input buffer read-only to the real GLTFLoader', async () => {
  const fixture = paddedGlbViews(1, 128), bytes = encodeGlb(fixture.json, fixture.bin), original = bytes.slice();
  const parse = vi.spyOn(GLTFLoader.prototype, 'parseAsync'), slice = vi.spyOn(bytes, 'slice');
  let loaded: Awaited<ReturnType<typeof importGlb>> | undefined;
  try {
    loaded = await importGlb(bytes, new AbortController().signal);
    expect(parse).toHaveBeenCalledOnce(); expect(parse.mock.calls[0][0]).toBe(bytes.buffer); expect(slice).not.toHaveBeenCalled();
    expect(bytes.byteLength).toBe(original.byteLength); expect(bytes).toEqual(original);
    expect(loaded.bounds.min.toArray()).toEqual([0, 0, 0]); expect(loaded.bounds.max.toArray()).toEqual([1, 1, 0]);
  } finally { loaded?.dispose(); slice.mockRestore(); parse.mockRestore(); }
});
it('normalizes only a GLB subview and never exposes its unrelated prefix or suffix to the loader', async () => {
  const fixture = paddedGlbViews(1, 128), bytes = encodeGlb(fixture.json, fixture.bin), owner = new Uint8Array(bytes.length + 64);
  owner.fill(0x77); owner.set(bytes, 32); const view = owner.subarray(32, 32 + bytes.length), original = owner.slice();
  const parse = vi.spyOn(GLTFLoader.prototype, 'parseAsync'), slice = vi.spyOn(view, 'slice');
  let loaded: Awaited<ReturnType<typeof importGlb>> | undefined;
  try {
    loaded = await importGlb(view, new AbortController().signal);
    const input = parse.mock.calls[0][0] as ArrayBuffer;
    expect(input).not.toBe(owner.buffer); expect(input.byteLength).toBe(bytes.byteLength); expect(new Uint8Array(input)).toEqual(bytes);
    expect(slice).toHaveBeenCalledOnce(); expect(owner).toEqual(original); expect(owner.byteLength).toBe(original.byteLength);
    expect(loaded.bounds.max.toArray()).toEqual([1, 1, 0]);
  } finally { loaded?.dispose(); slice.mockRestore(); parse.mockRestore(); }
});
it('keeps the admitted scene valid after the caller releases its original raw GLB buffer', async () => {
  const fixture = paddedGlbViews(1, 128), bytes = encodeGlb(fixture.json, fixture.bin), loaded = await importGlb(bytes, new AbortController().signal);
  try {
    structuredClone(bytes, { transfer: [bytes.buffer] }); expect(bytes.byteLength).toBe(0);
    const bounds = validateScene(loaded.scenes, loaded.scene); expect(bounds.min.toArray()).toEqual([0, 0, 0]); expect(bounds.max.toArray()).toEqual([1, 1, 0]);
    loaded.scene.traverse(object => { if (object instanceof Mesh) expect(object.geometry.getAttribute('position').array).toEqual(new Float32Array([0, 0, 0, 1, 0, 0, 0, 1, 0])); });
  } finally { loaded.dispose(); }
});

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

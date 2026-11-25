// @vitest-environment node
import { readFileSync } from 'node:fs';
import { expect, it } from 'vitest';
import { importStl } from '../src/app/components/documents/geometry/stl';
import { StlStreamDecoder } from '../src/app/components/documents/geometry/stl-stream';

function decode(bytes: Uint8Array<ArrayBuffer>, width: number) {
  const decoder = new StlStreamDecoder(bytes.length);
  for (let offset = 0; offset < bytes.length; offset += width) decoder.chunk(bytes.slice(offset, offset + width), offset);
  return decoder.finish();
}
const ascii = readFileSync(new URL('./fixtures/geometry/cube-ascii.stl', import.meta.url), 'utf8');
it.each([1, 3, 49, 50, 83, 84, 85, 127, 2 * 1024 * 1024])('preserves ASCII names, groups, UTF8 and normals with %i-byte chunks', width => {
  const text = ('  ' + ascii.replaceAll('cube', 'front endsolid part 😀') + ascii.replaceAll('cube', 'facet normal second 😀')).replaceAll('\n', '\r\n');
  const bytes = new TextEncoder().encode(text), model = decode(bytes, width), expected = importStl(bytes);
  expect(model).toEqual(expected);
});
it.each([1, 7, 49, 50, 83, 84, 85, 113])('preserves binary normals and Magics SRGB colors with %i-byte chunks', width => {
  const bytes = new Uint8Array(84 + 3 * 50), view = new DataView(bytes.buffer);
  bytes.set(new TextEncoder().encode('COLOR=')); bytes.set([128, 64, 255, 255], 6); view.setUint32(80, 3, true);
  for (let face = 0; face < 3; face++) {
    const offset = 84 + face * 50;
    for (let axis = 0; axis < 12; axis++) view.setFloat32(offset + axis * 4, (face + 1) * (axis + 1) / 17, true);
    view.setUint16(offset + 48, face ? 0x8000 : 0x3b28, true);
  }
  expect(decode(bytes, width)).toEqual(importStl(bytes));
});
it('rejects incomplete binary records, duplicate chunks and premature EOF', () => {
  const bytes = new TextEncoder().encode(ascii), decoder = new StlStreamDecoder(bytes.length);
  decoder.chunk(bytes.slice(0, 10), 0);
  expect(() => decoder.finish()).toThrow(); expect(() => decoder.chunk(bytes.slice(10), 0)).toThrow();
  expect(() => decode(bytes.slice(0, bytes.length - 8), 1)).toThrow();
});
it.each(['NaN', 'Infinity', '1000000000000.0001'])('rejects invalid authored coordinates before float rounding: %s', value => {
  expect(() => decode(new TextEncoder().encode(ascii.replace(/vertex\s+[^\s]+/, 'vertex ' + value)), 31)).toThrow();
});
it('rejects malformed UTF8 across chunks and missing normals', () => {
  const bytes = new TextEncoder().encode(ascii); bytes[8] = 255;
  expect(() => decode(bytes, 1)).toThrow();
  expect(() => decode(new TextEncoder().encode(ascii.replace('facet normal', 'facet')), 1)).toThrow();
});
it('retains one exact Worker-owned source only for the existing large uncolored binary preview proof', () => {
  const count = 333334, bytes = new Uint8Array(84 + count * 50), view = new DataView(bytes.buffer); view.setUint32(80, count, true);
  view.setFloat32(84 + 12, -0, true); view.setFloat32(84 + 24, 1, true); view.setFloat32(84 + 40, 1, true);
  const decoder = new StlStreamDecoder(bytes.length);
  for (let offset = 0; offset < bytes.length; offset += 2 * 1024 * 1024) decoder.chunk(bytes.slice(offset, offset + 2 * 1024 * 1024), offset);
  const model = decoder.finish();
  expect(decoder.previewSource?.buffer).not.toBe(bytes.buffer); expect(decoder.previewSource?.length).toBe(bytes.length);
  expect(decoder.previewSource?.every((value,index)=>value===bytes[index])).toBe(true);
  expect(Object.is(model.meshes[0].positions[0], -0)).toBe(true);
  const small = new StlStreamDecoder(134), smallBytes = bytes.slice(0,134); new DataView(smallBytes.buffer).setUint32(80,1,true); small.chunk(smallBytes,0);
  expect(small.previewSource).toBeUndefined();
  bytes.set(new TextEncoder().encode('COLOR=')); bytes.set([255,255,255,255],6);
  const colored = new StlStreamDecoder(bytes.length); colored.chunk(bytes.slice(0,84),0); expect(colored.previewSource).toBeUndefined();
});

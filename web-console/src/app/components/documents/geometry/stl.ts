import { STLLoader } from 'three/addons/loaders/STLLoader.js';
import { GEOMETRY_LIMITS as L, requireGeometry as check } from './limits';
import { validateModel, validateModelStructure, type GeometryModel } from './model';

export function importStl(bytes: Uint8Array<ArrayBuffer>): GeometryModel {
  check(bytes.length <= L.sourceBytes, 'limit');
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const binaryCount = bytes.length >= 84 ? view.getUint32(80, true) : 0;
  let input = bytes, expectedVertices = 0;
  if (bytes.length >= 84 && 84 + binaryCount * 50 === bytes.length) {
    check(binaryCount > 0, 'empty'); check(binaryCount * 3 <= L.vertices && binaryCount <= L.triangles, 'limit');
    expectedVertices = binaryCount * 3;
  } else {
    const text = new TextDecoder('utf-8', { fatal: true }).decode(bytes);
    check(/^\s*solid\b/i.test(text) && /endsolid\b/i.test(text));
    let vertices = 0, facets = 0, solids = 0;
    // Consume complete solid-name records so their free-form names cannot
    // contribute vertex, facet or additional solid tokens to admission.
    for (const match of text.matchAll(/^[^\S\r\n]*(solid|endsolid)\b[^\r\n]*|(\bvertex\s+)|(\bfacet\s+normal\b)/gim)) {
      if (match[1]) { if (match[1].toLowerCase() === 'solid') check(++solids <= L.parts, 'limit'); }
      else if (match[2]) check(++vertices <= L.vertices, 'limit');
      else check(++facets <= L.triangles, 'limit');
    }
    check(facets > 0, 'empty'); check(vertices === facets * 3);
    check(!text.includes('\0'));
    // STLLoader detects ASCII only near byte zero, with lowercase keywords.
    // Preserve names while giving it the exact representation admitted above.
    const normalized = text.trimStart().replace(/^([^\S\r\n]*)(solid|endsolid|facet\s+normal|outer\s+loop|endloop|endfacet|vertex)\b/gim,
      (_match, indent: string, keyword: string) => indent + keyword.toLowerCase());
    input = new TextEncoder().encode(normalized); check(input.length >= 84);
    expectedVertices = vertices;
  }
  const buffer = input.byteOffset === 0 && input.byteLength === input.buffer.byteLength ? input.buffer : input.slice().buffer;
  const geometry = new STLLoader().parse(buffer);
  try {
    const positions = geometry.getAttribute('position'), normals = geometry.getAttribute('normal'), colors = geometry.getAttribute('color');
    check(positions.count === expectedVertices);
    const groups = geometry.groups.length ? geometry.groups : [{ start: 0, count: positions.count }];
    check(groups.length <= L.parts, 'limit');
    const pack = (values: typeof positions.array, start: number, end: number) => {
      // Whole parser-owned arrays can transfer directly. Groups get one exact,
      // independently accounted backing buffer instead of retaining a subview.
      if (values instanceof Float32Array && start === 0 && end === values.length && values.byteOffset === 0 && values.byteLength === values.buffer.byteLength) return values;
      return Float32Array.from(values.subarray(start, end));
    };
    const meshes = groups.map((group, index) => {
      const start = group.start * 3, end = start + group.count * 3, label: unknown = geometry.userData.groupNames?.[index];
      return { name: typeof label === 'string' ? label.trim().slice(0, 256) : '', positions: pack(positions.array, start, end), normals: pack(normals.array, start, end), colors: colors ? pack(colors.array, start, end) : undefined, groups: [] };
    });
    const model: GeometryModel = { meshes, root: { name: '', meshes: meshes.map((_, index) => index), children: [] } };
    // Large streams cannot enter optional Worker preparation. Bound their
    // layout here; the receiver scans every value before display/preview ACK.
    if (meshes.every(mesh => mesh.positions.length / 3 > L.preparationVertices)) validateModelStructure(model);
    else validateModel(model);
    return model;
  } finally { geometry.dispose(); }
}

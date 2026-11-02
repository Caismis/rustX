import { STLLoader } from 'three/addons/loaders/STLLoader.js';
import { GEOMETRY_LIMITS as L, requireGeometry as check } from './limits';
import { validateModel, type GeometryModel } from './model';

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
    for (const _match of text.matchAll(/\bsolid\s+/gi)) { void _match; check(++solids <= L.parts, 'limit'); }
    for (const _match of text.matchAll(/\bvertex\s+/gi)) { void _match; check(++vertices <= L.vertices, 'limit'); }
    for (const _match of text.matchAll(/\bfacet\s+normal\b/gi)) { void _match; check(++facets <= L.triangles, 'limit'); }
    check(facets > 0, 'empty'); check(vertices === facets * 3);
    check(!text.includes('\0'));
    // STLLoader detects ASCII only near byte zero, with lowercase keywords.
    // Preserve names while giving it the exact representation admitted above.
    const normalized = text.trimStart().replace(/^([ \t]*)(solid|endsolid|facet\s+normal|outer\s+loop|endloop|endfacet|vertex)\b/gim,
      (_match, indent: string, keyword: string) => indent + keyword.toLowerCase());
    input = new TextEncoder().encode(normalized); check(input.length >= 84);
    expectedVertices = vertices;
  }
  const geometry = new STLLoader().parse(input.slice().buffer);
  try {
    const positions = geometry.getAttribute('position'), normals = geometry.getAttribute('normal'), colors = geometry.getAttribute('color');
    check(positions.count === expectedVertices);
    const groups = geometry.groups.length ? geometry.groups : [{ start: 0, count: positions.count }];
    check(groups.length <= L.parts, 'limit');
    const meshes = groups.map((group, index) => {
      const start = group.start * 3, end = start + group.count * 3, label: unknown = geometry.userData.groupNames?.[index];
      return { name: typeof label === 'string' ? label.trim().slice(0, 256) : '', positions: new Float32Array(positions.array.slice(start, end)), normals: new Float32Array(normals.array.slice(start, end)), colors: colors ? new Float32Array(colors.array.slice(start, end)) : undefined, groups: [] };
    });
    const model: GeometryModel = { meshes, root: { name: '', meshes: meshes.map((_, index) => index), children: [] } };
    validateModel(model); return model;
  } finally { geometry.dispose(); }
}

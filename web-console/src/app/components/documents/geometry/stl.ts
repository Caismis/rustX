import { STLLoader } from 'three/addons/loaders/STLLoader.js';
import { GEOMETRY_LIMITS as L, requireGeometry as check } from './limits';
import { validateModel, type GeometryModel } from './model';

export function importStl(bytes: Uint8Array<ArrayBuffer>): GeometryModel {
  check(bytes.length <= L.sourceBytes, 'limit');
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const binaryCount = bytes.length >= 84 ? view.getUint32(80, true) : 0;
  if (bytes.length >= 84 && 84 + binaryCount * 50 === bytes.length) {
    check(binaryCount > 0, 'empty'); check(binaryCount * 3 <= L.vertices && binaryCount <= L.triangles, 'limit');
  } else {
    const text = new TextDecoder('utf-8', { fatal: true }).decode(bytes);
    check(/^\s*solid\b/i.test(text) && /endsolid\b/i.test(text));
    let vertices = 0, facets = 0, solids = 0;
    for (const _match of text.matchAll(/\bsolid\s+/gi)) { void _match; check(++solids <= L.parts, 'limit'); }
    for (const _match of text.matchAll(/\bvertex\s+/gi)) { void _match; check(++vertices <= L.vertices, 'limit'); }
    for (const _match of text.matchAll(/\bfacet\s+normal\b/gi)) { void _match; check(++facets <= L.triangles, 'limit'); }
    check(facets > 0, 'empty'); check(vertices === facets * 3);
  }
  const geometry = new STLLoader().parse(bytes.slice().buffer);
  try {
    const positions = geometry.getAttribute('position'), normals = geometry.getAttribute('normal'), colors = geometry.getAttribute('color');
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

import createOcct from 'occt-import-js';
import { GEOMETRY_LIMITS as L, requireGeometry as check } from './limits';
import { validateModelStructure, type GeometryModel, type GeometryNode, type GeometryMesh } from './model';
import type { GeometryImportStage } from './stages';
import { lodAdmission, type GeometryLodLevel } from './lod-data';

export const CAD_TRIANGULATION = Object.freeze({ linearUnit: 'millimeter', linearDeflectionType: 'bounding_box_ratio', linearDeflection: 0.001, angularDeflection: 0.5 });
function record(value: unknown): Record<string, unknown> { check(value && typeof value === 'object' && !Array.isArray(value)); return value as Record<string, unknown>; }
function array(value: unknown, limit: number): unknown[] { check(Array.isArray(value)); check(value.length <= limit, 'limit'); return value; }
function floats(value: unknown, limit: number): Float32Array {
  const values = array(value, limit), result = new Float32Array(values.length);
  for (let index = 0; index < values.length; index++) {
    const value = values[index]; check(typeof value === 'number' && Number.isFinite(value) && Math.abs(value) <= L.coordinate);
    result[index] = value;
  }
  return result;
}
function color(value: unknown): [number, number, number] {
  const values = array(value, 3); check(values.length === 3 && values.every(value => typeof value === 'number' && Number.isFinite(value) && value >= 0 && value <= 1));
  return values as [number, number, number];
}
const name = (value: unknown) => typeof value === 'string' ? value.slice(0, 256) : '';

/** The upstream JSON is kept inside the disposable Worker. Only admitted buffers leave it. */
export function convertCadResult(value: unknown, lod = false): GeometryModel {
  const result = record(value); check(result.success === true);
  let vertices = 0, triangles = 0, bytes = 0, faces = 0;
  const meshes = array(result.meshes, L.parts).map(value => {
    const mesh = record(value), attributes = record(mesh.attributes);
    const position = array(record(attributes.position).array, L.vertices * 3);
    const indices = array(record(mesh.index).array, L.triangles * 3);
    vertices += position.length / 3; triangles += indices.length / 3;
    const normal = attributes.normal ? array(record(attributes.normal).array, position.length) : undefined;
    bytes += (position.length + (normal?.length ?? (lod ? position.length : 0))) * 4 + indices.length * 4;
    check(vertices <= L.vertices && triangles <= (lod ? L.lodTriangles : L.triangles) && bytes <= (lod ? L.lodBytes : L.decodedBytes), 'limit');
    const indexBuffer = new Uint32Array(indices.length);
    for (let i = 0; i < indices.length; i++) { const value = indices[i]; check(typeof value === 'number' && Number.isSafeInteger(value) && value >= 0 && value < position.length / 3); indexBuffer[i] = value; }
    const normals = normal ? floats(normal, position.length) : undefined;
    const groups: GeometryMesh['groups'] = [], base = mesh.color ? color(mesh.color) : undefined;
    const surfaceFaces = array(mesh.brep_faces ?? [], L.cadFaces); faces += surfaceFaces.length; check(faces <= L.cadFaces, 'limit');
    for (const value of surfaceFaces) {
      const face = record(value); if (face.color == null) continue;
      check(typeof face.first === 'number' && Number.isSafeInteger(face.first) && face.first >= 0 && typeof face.last === 'number' && Number.isSafeInteger(face.last) && face.last >= face.first && face.last < indices.length / 3);
      const faceColor = color(face.color), start = face.first * 3, count = (face.last - face.first + 1) * 3;
      if (base && base.every((value, channel) => value === faceColor[channel])) continue;
      const previous = groups.at(-1);
      if (previous && previous.start + previous.count === start && previous.color.every((value, channel) => value === faceColor[channel])) previous.count += count;
      else { check(groups.length < L.drawCalls, 'limit'); groups.push({ start, count, color: faceColor }); }
    }
    return { name: name(mesh.name), positions: floats(position, L.vertices * 3), normals, indices: indexBuffer, color: base, groups };
  });
  let nodes = 0;
  const seen = new Set<unknown>();
  function node(value: unknown, depth: number): GeometryNode {
    check(depth <= L.depth && ++nodes <= L.nodes, 'limit'); check(!seen.has(value)); seen.add(value);
    const item = record(value), refs = array(item.meshes, L.parts);
    check(refs.every(index => typeof index === 'number' && Number.isSafeInteger(index) && index >= 0 && index < meshes.length));
    return { name: name(item.name), meshes: [...refs] as number[], children: array(item.children, L.nodes).map(child => node(child, depth + 1)) };
  }
  // Values were admitted while copying into bounded typed streams. The parent
  // still independently admits every stream before graphics upload.
  const model = { meshes, root: node(result.root, 0) }; validateModelStructure(model); return model;
}

export async function importCad(bytes: Uint8Array<ArrayBuffer>, format: 'step' | 'iges' | 'brep', module: WebAssembly.Module, stage?: (stage: GeometryImportStage) => void, ownsInput = false, onLod?: (level: GeometryLodLevel, model: GeometryModel) => void): Promise<GeometryModel> {
  check(bytes.length <= L.sourceBytes, 'limit');
  stage?.('initializing');
  performance.mark('geometry:cad-initialize');
  const occt = await new Promise<Awaited<ReturnType<typeof createOcct>>>((resolve, reject) => {
    void createOcct({ instantiateWasm: (imports, receive) => {
      try {
        const instance = new WebAssembly.Instance(module, imports);
        receive(instance, module); return instance.exports;
      } catch (error) {
        // The pinned glue falls through to its own loader if this callback
        // throws. Fail this instance without starting another initialization.
        reject(error); return {};
      }
    } }).then(resolve, reject);
  });
  performance.measure('geometry:cad-initialize', 'geometry:cad-initialize');
  const read = format === 'step' ? occt.ReadStepFile : format === 'iges' ? occt.ReadIgesFile : occt.ReadBrepFile;
  stage?.('importing');
  performance.mark('geometry:cad-read');
  const admission = lodAdmission();
  const publishLod = onLod ? (level: number, raw: unknown) => {
    try {
      const model = admission.admit(level, convertCadResult(raw, true));
      // Native results are mutable JS arrays. The optional tier must not share
      // palette tuples with later native tessellations or the final result.
      for (const mesh of model.meshes) {
        if (mesh.color) mesh.color = [...mesh.color];
        for (const group of mesh.groups) group.color = [...group.color];
      }
      onLod?.(level as GeometryLodLevel, model);
    } catch { /* Optional LOD cannot invalidate the final tessellation. */ }
  } : undefined;
  const result = read(bytes, { ...CAD_TRIANGULATION, ...(publishLod ? { onLod: publishLod } : {}), ...(ownsInput ? { onInputCopied: () => {
    const buffer = bytes.buffer;
    if ('transfer' in buffer && typeof buffer.transfer === 'function') buffer.transfer(0);
  } } : {}) });
  performance.measure('geometry:cad-read', 'geometry:cad-read');
  stage?.('preparing');
  performance.mark('geometry:cad-convert');
  const model = convertCadResult(result);
  performance.measure('geometry:cad-convert', 'geometry:cad-convert');
  return model;
}

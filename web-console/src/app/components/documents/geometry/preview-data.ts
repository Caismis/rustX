import { GEOMETRY_LIMITS as L } from './limits';
import { exactPreview } from './exact-preview';
import type { GeometryFormat } from './format';
import type { GeometryModel, GeometryPreviewData } from './model';

export interface PreviewJob { mesh: number; positions?: Float32Array; normals?: Float32Array; indices?: Uint32Array; raw?: Uint8Array<ArrayBuffer>; target: number }
const CHUNK_VERTICES = 786_432;

/** Small jobs keep bounded copies; an unprepared binary STL retains its already
 * owned raw buffer and is decoded one fixed-size chunk at a time. */
export function previewJobs(model: GeometryModel, format: GeometryFormat, bytes: Uint8Array<ArrayBuffer>): PreviewJob[] {
  const total = model.meshes.reduce((sum, mesh) => sum + (mesh.indices?.length ?? mesh.positions.length / 3) / 3, 0);
  if (total <= L.movingTriangles) return [];
  const jobs: PreviewJob[] = []; let copied = 0;
  model.meshes.forEach((mesh, index) => {
    const count = (mesh.indices?.length ?? mesh.positions.length / 3) / 3, target = Math.max(12, Math.floor(L.movingTriangles * count / total));
    if (count <= target || !mesh.normals || mesh.colors || mesh.groups.length) return;
    if (format === 'stl' && model.meshes.length === 1 && !mesh.indices && mesh.positions.length / 3 > L.preparationVertices
      && bytes.length >= 84 && 84 + new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).getUint32(80, true) * 50 === bytes.length) {
      jobs.push({ mesh: index, raw: bytes, target }); return;
    }
    const size = mesh.positions.byteLength + mesh.normals.byteLength + (mesh.indices?.byteLength ?? 0);
    if (copied + size > L.previewInputBytes) return;
    copied += size; jobs.push({ mesh: index, positions: mesh.positions.slice(), normals: mesh.normals.slice(), indices: mesh.indices?.slice(), target });
  });
  return jobs;
}

function pack(indices: Uint32Array, positions: Float32Array, error: number, base: number, normals?: Float32Array, raw?: DataView, facet = !!raw): GeometryPreviewData {
  const output = new Float32Array(indices.length * 3), outputNormals = new Float32Array(output.length), sourceVertices = new Uint32Array(indices.length), normalVertices = new Uint32Array(indices.length);
  const inputBits = new Uint32Array(positions.buffer, positions.byteOffset, positions.length), outputBits = new Uint32Array(output.buffer);
  const normalBits = new Uint32Array(outputNormals.buffer), inputNormals = normals && new Uint32Array(normals.buffer, normals.byteOffset, normals.length);
  for (let index = 0; index < indices.length; index++) {
    sourceVertices[index] = base + indices[index];
    for (let c = 0; c < 3; c++) outputBits[index * 3 + c] = inputBits[indices[index] * 3 + c];
  }
  for (let triangle = 0; triangle < indices.length; triangle += 3) {
    let selected = indices[triangle];
    if (facet) {
      const at = triangle * 3;
      const ax = output[at+3] - output[at], ay = output[at+4] - output[at+1], az = output[at+5] - output[at+2];
      const bx = output[at+6] - output[at], by = output[at+7] - output[at+1], bz = output[at+8] - output[at+2];
      const nx = ay*bz - az*by, ny = az*bx - ax*bz, nz = ax*by - ay*bx; let best = -Infinity;
      for (let corner = 0; corner < 3; corner++) {
        const vertex = base + indices[triangle + corner], offset = 84 + Math.floor(vertex / 3) * 50;
        const x = raw ? raw.getFloat32(offset, true) : normals![vertex * 3];
        const y = raw ? raw.getFloat32(offset + 4, true) : normals![vertex * 3 + 1];
        const z = raw ? raw.getFloat32(offset + 8, true) : normals![vertex * 3 + 2];
        const score = (x*nx + y*ny + z*nz) / (Math.hypot(x,y,z) || 1);
        if (score > best) { best = score; selected = indices[triangle + corner]; }
      }
    }
    for (let corner = 0; corner < 3; corner++) {
      const index = triangle + corner, normalVertex = facet ? Math.floor((base + selected) / 3) * 3 : indices[index];
      normalVertices[index] = normalVertex;
      for (let c = 0; c < 3; c++) normalBits[index * 3 + c] = raw ? raw.getUint32(84 + Math.floor(normalVertex / 3) * 50 + c * 4, true) : inputNormals![normalVertex * 3 + c];
    }
  }
  return { positions: output, normals: outputNormals, sourceVertices, normalVertices, error };
}

function refineJoined(data: GeometryPreviewData, simplifier: typeof import('meshoptimizer/simplifier').MeshoptSimplifier) {
  // Join only exact position/normal tuples. Native remapping treats signed zero
  // as equal; preserve its bits explicitly so authored attributes stay intact.
  const count = data.sourceVertices.length, tuples = new Float32Array(count * 3);
  const positionRemap = simplifier.generatePositionRemap(data.positions, 3), normalRemap = simplifier.generatePositionRemap(data.normals, 3);
  for (const [stream, remap, component] of [[data.positions, positionRemap, 0], [data.normals, normalRemap, 1]] as const) {
    const bits = new Uint32Array(stream.buffer, stream.byteOffset, stream.length);
    for (let i = 0; i < count; i++) {
      const at = i * 3, zero = Number(bits[at] === 0x80000000) | Number(bits[at+1] === 0x80000000) << 1 | Number(bits[at+2] === 0x80000000) << 2;
      tuples[at + component] = remap[i] * 8 + zero;
    }
  }
  const indices = simplifier.generatePositionRemap(tuples, 3), scale = simplifier.getScale(data.positions, 3);
  const remaining = Math.max(0, L.previewError - data.error / Math.max(scale, 1e-30));
  const [output, relative] = simplifier.simplifyWithAttributes(indices, data.positions, 3, data.normals, 3, [1,1,1], null,
    Math.min(36, indices.length - 3), Math.min(.0005, remaining), ['LockBorder', 'Sparse']);
  if (!output.length || output.length >= indices.length) return data;
  const next = pack(output, data.positions, data.error + relative * scale, 0, data.normals);
  for (let i = 0; i < next.sourceVertices.length; i++) {
    next.sourceVertices[i] = data.sourceVertices[next.sourceVertices[i]];
    next.normalVertices[i] = data.normalVertices[next.normalVertices[i]];
  }
  return next;
}

export async function preparePreviews(jobs: PreviewJob[], deadline: number) {
  let simplifier: typeof import('meshoptimizer/simplifier').MeshoptSimplifier | undefined;
  const previews: { mesh: number; data: GeometryPreviewData }[] = []; let used = 0, triangles = 0;
  for (const job of jobs) {
    if (performance.now() > deadline) break;
    if (job.raw) {
      const exact = exactPreview(job.raw, deadline, Math.min(L.previewTriangles - triangles, Math.floor((L.previewBytes - used) / 96)));
      if (exact) {
        used += exact.sourceVertices.length * 32; triangles += exact.sourceVertices.length / 3;
        previews.push({ mesh: job.mesh, data: exact }); job.raw = undefined; continue;
      }
    }
    if (performance.now() > deadline) break;
    if (!simplifier) {
      ({ MeshoptSimplifier: simplifier } = await import('meshoptimizer/simplifier'));
      if (!simplifier.supported) break;
      await simplifier.ready;
    }
    let data: GeometryPreviewData | undefined;
    if (job.raw) {
      const view = new DataView(job.raw.buffer, job.raw.byteOffset, job.raw.byteLength), vertices = view.getUint32(80, true) * 3;
      const chunks: GeometryPreviewData[] = []; let total = 0, error = 0, complete = true;
      for (let base = 0; base < vertices; base += CHUNK_VERTICES) {
        if (performance.now() > deadline) { complete = false; break; }
        const count = Math.min(CHUNK_VERTICES, vertices - base), positions = new Float32Array(count * 3);
        for (let index = 0; index < count; index++) {
          const source = base + index, offset = 84 + Math.floor(source / 3) * 50 + 12 + source % 3 * 12;
          for (let c = 0; c < 3; c++) positions[index * 3 + c] = view.getFloat32(offset + c * 4, true);
        }
        const remap = simplifier.generatePositionRemap(positions, 3), target = Math.max(12, Math.floor(job.target * count / vertices)) * 3;
        const [indices, relative] = simplifier.simplify(remap, positions, 3, Math.min(remap.length - 3, target), L.previewError, ['LockBorder', 'Sparse']);
        if (!indices.length || triangles + (total + indices.length) / 3 > L.previewTriangles || used + (total + indices.length) * 32 > L.previewBytes) { complete = false; break; }
        const absolute = relative * simplifier.getScale(positions, 3); error = Math.max(error, absolute);
        chunks.push(pack(indices, positions, absolute, base, undefined, view)); total += indices.length;
      }
      job.raw = undefined;
      if (!complete) continue;
      const positions = new Float32Array(total * 3), normals = new Float32Array(total * 3);
      const sourceVertices = new Uint32Array(total), normalVertices = new Uint32Array(total); let offset = 0;
      for (const chunk of chunks) {
        positions.set(chunk.positions, offset * 3); normals.set(chunk.normals, offset * 3);
        sourceVertices.set(chunk.sourceVertices, offset); normalVertices.set(chunk.normalVertices, offset); offset += chunk.sourceVertices.length;
      }
      data = { positions, normals, sourceVertices, normalVertices, error };
    } else if (job.positions && job.normals) {
      const indices = job.indices ?? simplifier.generatePositionRemap(job.positions, 3), target = Math.min(indices.length - 3, job.target * 3);
      const [output, relative] = job.indices
        ? simplifier.simplifyWithAttributes(indices, job.positions, 3, job.normals, 3, [1,1,1], null, target, L.previewError, ['LockBorder', 'Sparse'])
        : simplifier.simplify(indices, job.positions, 3, target, L.previewError, ['LockBorder', 'Sparse']);
      if (!output.length || output.length >= indices.length || triangles + output.length / 3 > L.previewTriangles || used + output.length * 32 > L.previewBytes) continue;
      data = pack(output, job.positions, relative * simplifier.getScale(job.positions, 3), 0, job.normals, undefined, !job.indices);
    }
    if (data && performance.now() < deadline - 50) {
      // Chunk borders are locked for crack-free construction. Once joined,
      // remove their now-internal redundancy with attribute-aware simplification.
      data = refineJoined(data, simplifier);
    }
    if (data) { used += data.sourceVertices.length * 32; triangles += data.sourceVertices.length / 3; previews.push({ mesh: job.mesh, data }); }
  }
  return previews;
}

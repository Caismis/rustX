import { FILE_PREVIEW_LIMITS } from '../../../../../shared/file-preview.ts';
import { PREVIEW_POLICY } from '../../../../client/preview-policy.ts';

/** Per visible body; two bodies are admitted by the preview workspace. */
export const GEOMETRY_LIMITS = Object.freeze({
  owners: PREVIEW_POLICY.activeBodies,
  sourceBytes: FILE_PREVIEW_LIMITS.geometryBytes, jsonBytes: 2 * 1024 * 1024,
  vertices: 13_000_000, triangles: 5_000_000, nodes: 512, parts: 256,
  depth: 32, materials: 256, drawCalls: 512, decodedBytes: 512 * 1024 * 1024,
  movingTriangles: 20_000, movingPixels: 1_048_576, refineDelayMs: 120,
  preparationVertices: 1_000_000, preparationTriangles: 1_000_000,
  textures: 8, textureDimension: 2048, texturePixels: 8_388_608,
  coordinate: 1e12, timeoutMs: 15_000, canvasPixels: 4_194_304,
  outlineTriangles: 100_000, outlineSegments: 65_536, outlineInputBytes: 16 * 1024 * 1024, outlineTimeoutMs: 2_000,
  // The pinned upstream binary/glue allows at most 2 GiB of WASM memory.
  // This excludes JS objects, source/output copies and the browser process.
  wasmBytes: 2 * 1024 * 1024 * 1024,
});
export type GeometryFailure = 'invalid' | 'limit' | 'external' | 'unsupported' | 'empty' | 'timeout' | 'worker' | 'webgl' | 'capacity';
export class GeometryError extends Error {
  readonly code: GeometryFailure;
  constructor(code: GeometryFailure) { super(code); this.code = code; }
}
export function requireGeometry(condition: unknown, code: GeometryFailure = 'invalid'): asserts condition {
  if (!condition) throw new GeometryError(code);
}

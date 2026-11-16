import { outlineData, OUTLINE_STRIDE, type OutlineInput } from './outline-data';
import { GEOMETRY_LIMITS as L, GeometryError } from './limits';

self.onmessage = (event: MessageEvent<{ inputs: OutlineInput[]; deadline: number }>) => {
  try {
    const data: Float32Array[] = []; let segments = 0;
    for (const input of event.data.inputs) {
      const result = outlineData(input, L.outlineSegments - segments, event.data.deadline - performance.timeOrigin);
      segments += result.length / (2 * OUTLINE_STRIDE); data.push(result);
    }
    self.postMessage({ ok: true, data }, { transfer: data.map(array => array.buffer) });
  } catch (error) { self.postMessage({ ok: false, code: error instanceof GeometryError ? error.code : 'invalid' }); }
};

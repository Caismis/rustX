// @vitest-environment node
import { afterEach, expect, it, vi } from 'vitest';
import { serveGeometry } from '../src/app/components/documents/geometry/worker';
import { validateModel, type GeometryModel } from '../src/app/components/documents/geometry/model';

afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); });
it('returns the admitted original before optional preparation when import consumed its deadline reserve', async () => {
  vi.spyOn(performance, 'now').mockReturnValue(0);
  const positions = new Float32Array(50_001 * 3), normals = new Float32Array(positions.length);
  for (let at = 0; at < positions.length; at += 9) positions.set([0,0,0, 1,0,0, 0,1,0], at);
  for (let at = 2; at < normals.length; at += 3) normals[at] = 1;
  const model: GeometryModel = { meshes: [{ name: 'Original', positions, normals, groups: [] }], root: { name: '', meshes: [0], children: [] } };
  validateModel(model);
  const postMessage = vi.fn(); vi.stubGlobal('postMessage', postMessage); vi.stubGlobal('onmessage', null);
  serveGeometry(() => model);
  Reflect.apply(globalThis.onmessage!, globalThis, [new MessageEvent('message', { data: {
    bytes: new Uint8Array([1]), format: 'stl', deadline: performance.timeOrigin + 900,
  } })]);
  await Promise.resolve();
  expect(postMessage).toHaveBeenCalledOnce(); expect(postMessage.mock.calls[0][0].model).toBe(model);
  expect(postMessage.mock.calls[0][0].ok).toBe(true); expect(model.meshes[0].indices).toBeUndefined();
});

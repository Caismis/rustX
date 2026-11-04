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
it('emits bounded observed stages before the profiled terminal result, without inventing completion', async () => {
  const model: GeometryModel = { meshes: [{ name: 'Triangle', positions: new Float32Array([0,0,0, 1,0,0, 0,1,0]), groups: [] }], root: { name: '', meshes: [0], children: [] } };
  const postMessage = vi.fn(); vi.stubGlobal('postMessage', postMessage); vi.stubGlobal('onmessage', null);
  let complete!: () => void;
  serveGeometry(async (_bytes, _format, stage) => {
    stage('preparing'); stage('initializing'); stage('initializing'); stage('importing');
    await new Promise<void>(resolve => { complete = resolve; }); stage('preparing'); return model;
  });
  Reflect.apply(globalThis.onmessage!, globalThis, [new MessageEvent('message', { data: { bytes: new Uint8Array([1]), format: 'step', deadline: performance.timeOrigin + performance.now() + 15_000, profile: true } })]);
  expect(postMessage.mock.calls.map(([message]) => message)).toEqual([{ kind: 'stage', stage: 'initializing' }, { kind: 'stage', stage: 'importing' }]);
  complete(); await Promise.resolve(); await Promise.resolve();
  const messages = postMessage.mock.calls.map(([message]) => message);
  expect(messages.slice(0, 3)).toEqual(['initializing', 'importing', 'preparing'].map(stage => ({ kind: 'stage', stage })));
  expect(messages).toHaveLength(4); expect(messages[3]).toMatchObject({ ok: true, model, profile: { importMs: expect.any(Number), prepareMs: expect.any(Number), cad: [], stages: ['initializing', 'importing', 'preparing'].map(stage => ({ stage, ms: expect.any(Number) })) } });
});

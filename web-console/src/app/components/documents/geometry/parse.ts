import { GEOMETRY_LIMITS as L, GeometryError, type GeometryFailure } from './limits';
import { validateModel, type GeometryModel } from './model';
import type { GeometryFormat } from './format';

let active = 0;
/** One disposable import Worker per body. Cancellation never detaches retained bytes. */
export function parseGeometry(bytes: Uint8Array<ArrayBuffer>, format: Exclude<GeometryFormat, 'glb'>, signal: AbortSignal): Promise<GeometryModel> {
  signal.throwIfAborted();
  if (bytes.length > L.sourceBytes) return Promise.reject(new GeometryError('limit'));
  if (active >= L.owners) return Promise.reject(new GeometryError('capacity'));
  const deadline = performance.now() + L.timeoutMs;
  return new Promise((resolve, reject) => {
    let worker: Worker;
    try { worker = format === 'stl'
      ? new Worker(new URL('./stl-worker.ts', import.meta.url), { type: 'module', name: 'rustx-stl' })
      : new Worker(new URL('./cad-worker.ts', import.meta.url), { type: 'module', name: 'rustx-cad' }); }
    catch { reject(new GeometryError('worker')); return; }
    active++;
    let finished = false;
    const finish = () => {
      if (finished) return; finished = true;
      clearTimeout(timer); signal.removeEventListener('abort', abort);
      worker.onmessage = worker.onerror = worker.onmessageerror = null; worker.terminate(); active--;
    };
    const fail = (code: GeometryFailure) => { finish(); reject(new GeometryError(code)); };
    const abort = () => { finish(); reject(new DOMException('Geometry preview closed', 'AbortError')); };
    const timer = setTimeout(() => fail('timeout'), Math.max(0, deadline - performance.now()));
    worker.onerror = worker.onmessageerror = () => fail('worker');
    worker.onmessage = event => {
      if (finished || signal.aborted) return;
      if (performance.now() > deadline) { fail('timeout'); return; }
      try {
        if (event.data?.ok !== true) {
          const code = event.data?.code;
          fail(['invalid', 'limit', 'external', 'unsupported', 'empty'].includes(code) ? code : 'worker'); return;
        }
        const model = event.data.model as GeometryModel; validateModel(model);
        if (performance.now() > deadline) { fail('timeout'); return; }
        finish(); resolve(model);
      } catch { fail('invalid'); }
    };
    signal.addEventListener('abort', abort, { once: true });
    try { const copy = bytes.slice(); worker.postMessage({ bytes: copy, format }, [copy.buffer]); }
    catch { fail('worker'); }
  });
}

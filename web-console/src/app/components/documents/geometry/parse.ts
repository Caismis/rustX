import { GEOMETRY_LIMITS as L, GeometryError, type GeometryFailure } from './limits';
import { validateModel, type GeometryModel } from './model';
import type { GeometryFormat } from './format';
import { geometryStageObserver, type GeometryImportStage } from './stages';
import { cadCode } from './cad-code';

let active = 0;
/** One disposable import Worker per body. Cancellation never detaches retained bytes. */
export function parseGeometry(bytes: Uint8Array<ArrayBuffer>, format: Exclude<GeometryFormat, 'glb'>, signal: AbortSignal, onStage?: (stage: GeometryImportStage) => void): Promise<GeometryModel> {
  signal.throwIfAborted();
  if (bytes.length > L.sourceBytes) return Promise.reject(new GeometryError('limit'));
  if (active >= L.owners) return Promise.reject(new GeometryError('capacity'));
  active++;
  return admittedGeometry(bytes, format, signal, performance.now() + L.timeoutMs, onStage);
}

// This mutable input is the continuation's only source reference. Cancellation
// clears it while immutable code compilation may continue for another body.
function admittedGeometry(input: Uint8Array<ArrayBuffer> | undefined, format: Exclude<GeometryFormat, 'glb'>, signal: AbortSignal | undefined, deadline: number, onStage?: (stage: GeometryImportStage) => void): Promise<GeometryModel> {
  return new Promise((resolve, reject) => {
    let worker: Worker | undefined, dispatched = false;
    const stage = geometryStageObserver(format, value => onStage?.(value));
    let finished = false;
    const finish = () => {
      if (finished) return; finished = true;
      clearTimeout(timer); signal?.removeEventListener('abort', abort);
      input = undefined; signal = undefined; onStage = undefined;
      if (worker) { worker.onmessage = worker.onerror = worker.onmessageerror = null; worker.terminate(); worker = undefined; }
      active--;
    };
    const fail = (code: GeometryFailure) => { finish(); reject(new GeometryError(code)); };
    const abort = () => { finish(); reject(new DOMException('Geometry preview closed', 'AbortError')); };
    const timer = setTimeout(() => fail('timeout'), Math.max(0, deadline - performance.now()));
    signal!.addEventListener('abort', abort, { once: true });
    // Fetch/evaluate the disposable Worker while immutable CAD code compiles.
    // The same admission and deadline own this waiting Worker and its input.
    try { worker = format === 'stl'
      ? new Worker(new URL('./stl-worker.ts', import.meta.url), { type: 'module', name: 'rustx-stl' })
      : new Worker(new URL('./cad-worker.ts', import.meta.url), { type: 'module', name: 'rustx-cad' }); }
    catch { fail('worker'); return; }
    worker.onerror = worker.onmessageerror = () => fail('worker');
    worker.onmessage = event => {
      if (finished || signal?.aborted || !dispatched) return;
      if (performance.now() > deadline) { fail('timeout'); return; }
      if (event.data?.kind === 'stage') {
        // Unknown, duplicate and out-of-order observations cannot become a
        // result, prolong admission or change the original deadline.
        if (Object.keys(event.data).length === 2) stage(event.data.stage);
        return;
      }
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
    const start = (module?: WebAssembly.Module) => {
      if (finished || signal?.aborted || !input || !worker) return;
      if (performance.now() > deadline) { fail('timeout'); return; }
      try { const copy = input.slice(); input = undefined; dispatched = true; worker.postMessage({ bytes: copy, format, deadline: performance.timeOrigin + deadline, ...(module ? { module } : {}) }, [copy.buffer]); }
      catch { fail('worker'); }
    };
    if (format === 'stl') start();
    else {
      try {
        stage('initializing');
        const waitStart = performance.now();
        if (!finished) void cadCode().then(module => {
          if (finished) return;
          const name = 'geometry:cad-code-wait'; performance.clearMeasures(name); performance.measure(name, { start: waitStart }); start(module);
        }, () => { if (!finished) fail('worker'); });
      }
      catch { fail('worker'); }
    }
  });
}

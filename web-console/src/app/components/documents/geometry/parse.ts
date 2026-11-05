import { GEOMETRY_LIMITS as L, GeometryError, type GeometryFailure } from './limits';
import { type GeometryModel } from './model';
import { modelScene, disposeScene } from './scene-resources';
import type { GeometryFormat } from './format';
import { geometryStageObserver, type GeometryImportStage } from './stages';
import { cadCode } from './cad-code';
import { partialBudget } from './partial-data';

let active = 0;
export type PreparedGeometry = ReturnType<typeof modelScene> & { prepareStart: number; pendingPreview: boolean; ready: Promise<void>; startPreview(): void; dispose(): void };
export type GeometryPartial = Pick<ReturnType<typeof modelScene>, 'scene' | 'bounds'> & { dispose(): void };
/** One disposable import Worker per body. Only the content owner can relinquish
 * a large copy that cannot fit its inactive cache; other input is copied. */
export function parseGeometry(bytes: Uint8Array<ArrayBuffer>, format: Exclude<GeometryFormat, 'glb'>, signal: AbortSignal, onStage?: (stage: GeometryImportStage) => void, takeBytes?: () => Uint8Array<ArrayBuffer> | undefined, onPartial?: (partial: GeometryPartial) => void): Promise<PreparedGeometry> {
  signal.throwIfAborted();
  if (bytes.length > L.sourceBytes) return Promise.reject(new GeometryError('limit'));
  if (active >= L.owners) return Promise.reject(new GeometryError('capacity'));
  active++;
  return admittedGeometry(bytes, format, signal, performance.now() + (format === 'stl' ? L.timeoutMs : L.cadTimeoutMs), onStage, takeBytes, onPartial);
}

// This mutable input is the continuation's only source reference. Cancellation
// clears it while immutable code compilation may continue for another body.
function admittedGeometry(input: Uint8Array<ArrayBuffer> | undefined, format: Exclude<GeometryFormat, 'glb'>, signal: AbortSignal | undefined, deadline: number, onStage?: (stage: GeometryImportStage) => void, takeBytes?: () => Uint8Array<ArrayBuffer> | undefined, onPartial?: (partial: GeometryPartial) => void): Promise<PreparedGeometry> {
  return new Promise((resolve, reject) => {
    let worker: Worker | undefined, dispatched = false, delivered = false, prepared: ReturnType<typeof modelScene> | undefined;
    let previewTimer: ReturnType<typeof setTimeout> | undefined, settleReady!: () => void;
    const ready = new Promise<void>(resolve => { settleReady = resolve; });
    const stage = geometryStageObserver(format, value => onStage?.(value));
    let finished = false;
    const prefix = partialBudget(); let partialSequence = 0, lastMesh = -1;
    const finish = () => {
      if (finished) return; finished = true;
      clearTimeout(timer); clearTimeout(previewTimer); signal?.removeEventListener('abort', abort);
      prepared?.finishPreviews(); settleReady();
      input = undefined; signal = undefined; onStage = undefined; takeBytes = undefined; onPartial = undefined;
      if (worker) { worker.onmessage = worker.onerror = worker.onmessageerror = null; worker.terminate(); worker = undefined; }
      active--;
    };
    const fail = (code: GeometryFailure) => { finish(); if (!delivered) reject(new GeometryError(code)); };
    const abort = () => { finish(); if (!delivered) reject(new DOMException('Geometry preview closed', 'AbortError')); };
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
      if (event.data?.kind === 'preview') {
        if (!delivered || !prepared) { fail('invalid'); return; }
        try {
          const values = event.data.previews;
          if (!Array.isArray(values) || values.length > L.parts) throw new GeometryError('invalid');
          for (const value of values) prepared.setPreview(value.mesh, value.data);
        } catch { /* Optional representations never replace the validated original. */ }
        finish(); return;
      }
      if (delivered) return;
      if (event.data?.kind === 'partial') {
        try {
          const { sequence, meshIndex } = event.data;
          if (!onPartial || format === 'stl' || sequence !== partialSequence + 1 || !Number.isSafeInteger(meshIndex) || meshIndex <= lastMesh || meshIndex >= L.parts) throw new GeometryError('invalid');
          const result = modelScene(prefix.admit(event.data.model));
          let closed = false;
          const partial = { scene: result.scene, bounds: result.bounds, dispose: () => { if (!closed) { closed = true; disposeScene([result.scene]); } } };
          try { onPartial(partial); } catch (error) { partial.dispose(); throw error; }
          partialSequence = sequence; lastMesh = meshIndex;
        } catch (error) { fail(error instanceof GeometryError ? error.code : 'invalid'); }
        return;
      }
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
        const prepareStart = performance.now(), result = modelScene(event.data.model as GeometryModel);
        prepared = result;
        let disposed = false;
        const dispose = () => { if (!disposed) { disposed = true; finish(); disposeScene([result.scene]); } };
        const pending = event.data.pendingPreviews ?? [];
        try { if (!Array.isArray(pending)) throw new GeometryError('invalid'); result.markPreviews(pending); }
        catch (error) { dispose(); throw error; }
        if (performance.now() > deadline) { dispose(); fail('timeout'); return; }
        delivered = true;
        let previewStarted = false;
        const startPreview = () => {
          if (finished || previewStarted || !pending.length || !worker) return;
          previewStarted = true; clearTimeout(previewTimer);
          previewTimer = setTimeout(finish, Math.min(L.previewTimeoutMs, Math.max(0, deadline - performance.now())));
          try { worker.postMessage({ kind: 'preview-start' }); } catch { finish(); }
        };
        if (pending.length) previewTimer = setTimeout(finish, Math.min(L.previewTimeoutMs, Math.max(0, deadline - performance.now())));
        else finish();
        resolve({ ...result, prepareStart, pendingPreview: pending.length > 0, ready, startPreview, dispose });
      } catch { fail('invalid'); }
    };
    const start = (module?: WebAssembly.Module) => {
      if (finished || signal?.aborted || !input || !worker) return;
      if (performance.now() > deadline) { fail('timeout'); return; }
      try { const copy = takeBytes?.() ?? input.slice(); input = undefined; takeBytes = undefined; dispatched = true; worker.postMessage({ bytes: copy, format, deadline: performance.timeOrigin + deadline, preparePreview: true, progressive: !!onPartial, ...(module ? { module } : {}) }, [copy.buffer]); }
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

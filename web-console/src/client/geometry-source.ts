import { FILE_PREVIEW_LIMITS as L, isFileData, type FileBytes } from '../../shared/file-preview';
import type { FilePreviewOperation, NativeStepPreviewTiming } from '../../shared/file-preview';
import type { GeometrySourceConsumer } from '../../shared/geometry-source';
import { settlementFailureKind, WorkspaceHostError } from '../workspaces/host';

type Open = (signal: AbortSignal) => Promise<FilePreviewOperation>;
/** Internal policies: streamed STL defaults to bounded adaptive reads; complete-input GLB/CAD defaults to two reads. */
export type GeometryReadPolicy = 'fixed1' | 'fixed2' | 'adaptive';
export interface GeometryReadObservation { offset: number; bytes: number; width: number; nextWidth: number; readMs: number; processMs: number }
export function readGeometrySource(open: Open, signal: AbortSignal): Promise<{ data: Uint8Array<ArrayBuffer>; version: string; nativeStep?: NativeStepPreviewTiming }>;
export function readGeometrySource(open: Open, signal: AbortSignal, consumer: GeometrySourceConsumer, policy?: GeometryReadPolicy, observe?: (sample: GeometryReadObservation) => void): Promise<{ version: string }>;
export async function readGeometrySource(open: Open, signal: AbortSignal, consumer?: GeometrySourceConsumer, policy: GeometryReadPolicy = consumer ? 'adaptive' : 'fixed2', observe?: (sample: GeometryReadObservation) => void) {
  const controller = new AbortController(), reading = controller.signal;
  const abort = () => controller.abort(signal.reason);
  signal.addEventListener('abort', abort, { once: true });
  if (signal.aborted) abort();
  let operation: FilePreviewOperation | undefined, failedSettlement: unknown;
  const pending: Promise<FileBytes>[] = [], readTimes = new WeakMap<FileBytes, number>();
  let lastYield = performance.now(), readAverage = 0, processAverage = 0, samples = 0, adaptiveWidth = 1, votes = 0;
  const yieldIfNeeded = async () => {
    if (performance.now() - lastYield < 4) return;
    await new Promise<void>(resolve => {
      const channel = new MessageChannel();
      const finish = () => { reading.removeEventListener('abort', finish); channel.port1.close(); channel.port2.close(); resolve(); };
      reading.addEventListener('abort', finish, { once: true }); channel.port1.onmessage = finish;
      if (reading.aborted) finish(); else channel.port2.postMessage(null);
    });
    reading.throwIfAborted(); lastYield = performance.now();
  };
  const failure = (cause: unknown) => {
    if (settlementFailureKind(cause)) failedSettlement = cause;
    controller.abort(cause);
  };
  try {
    reading.throwIfAborted(); operation = await open(reading); reading.throwIfAborted();
    const { bytes: total, version, nativeStep } = operation;
    if (!Number.isSafeInteger(total) || total < 0 || typeof version !== 'string' || !version) throw new Error('malformed');
    if (total > (nativeStep ? L.nativeStepGlbBytes : L.geometryBytes)) throw new WorkspaceHostError('too_large', 'too_large');
    if (nativeStep && consumer) throw new Error('malformed');
    const transportStart = performance.now();
    let data: Uint8Array<ArrayBuffer> | undefined;
    await consumer?.start({ bytes: total, version });
    let nextOffset = 0, received = 0;
    const start = () => {
      const offset = nextOffset, length = offset === 0 ? L.pageBytes : Math.min(L.pageBytes, Math.max(1, total - offset)); nextOffset += length;
      const readStart = performance.now();
      const task = operation!.read({ offset, length }, reading).then(page => {
        reading.throwIfAborted();
        const elapsed = performance.now() - readStart; readTimes.set(page, elapsed); readAverage = samples ? readAverage * .75 + elapsed * .25 : elapsed;
        if (page.bytes !== total || page.version !== version) throw new WorkspaceHostError('Geometry source changed during read', 'source_changed');
        if (!isFileData(page.data) || page.lines !== undefined || page.offset !== offset
          || page.data.length !== Math.min(length, Math.max(0, total - offset)) || page.eof !== (offset + page.data.length === total)) throw new Error('malformed');
        return page;
      });
      void task.catch(failure); pending.push(task);
    };
    start();
    while (pending.length) {
      const page = await pending.shift()!; reading.throwIfAborted();
      // The current page plus at most two pending pages form the complete
      // reorder window. No lane can run arbitrarily ahead of a slow decoder.
      const width = policy === 'fixed1' ? 1 : policy === 'adaptive' ? adaptiveWidth : 2;
      while (nextOffset < total && pending.length < width) start();
      const pageLength = page.data.length, processStart = performance.now();
      if (consumer) await consumer.chunk(page.data, page.offset);
      else {
        if (total <= L.pageBytes) data = page.data;
        else { data ??= new Uint8Array(total); data.set(page.data, page.offset); }
        await yieldIfNeeded();
      }
      const elapsed = performance.now() - processStart;
      processAverage = samples ? processAverage * .75 + elapsed * .25 : elapsed; samples++;
      if (policy === 'adaptive' && samples >= 2) {
        const wanted = readAverage > processAverage * 1.25 + .1 ? 2 : processAverage > readAverage * .8 ? 1 : adaptiveWidth;
        votes = wanted === adaptiveWidth ? 0 : votes + 1;
        if (votes >= 2) { adaptiveWidth = wanted; votes = 0; }
      }
      observe?.({ offset: page.offset, bytes: pageLength, width, nextWidth: policy === 'adaptive' ? adaptiveWidth : width, readMs: readTimes.get(page)!, processMs: elapsed });
      received += pageLength;
      reading.throwIfAborted();
    }
    if (received !== total) throw new Error('malformed');
    await operation.close(); operation = undefined;
    reading.throwIfAborted();
    if (nativeStep) {
      performance.clearMarks('geometry:native-step');
      performance.mark('geometry:native-step', { detail: { ...nativeStep, bytes: total, transportMs: performance.now() - transportStart } });
    }
    return data ? { data, version, ...(nativeStep ? { nativeStep } : {}) } : { version };
  } catch (cause) {
    failure(cause);
    await Promise.allSettled(pending);
    if (operation) { try { await operation.close(); } catch (closeError) { if (settlementFailureKind(closeError)) failedSettlement = closeError;  } }
    throw failedSettlement ?? cause;
  } finally { signal.removeEventListener('abort', abort); }
}

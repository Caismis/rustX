import { FILE_PREVIEW_LIMITS as L, isFileData, type FileBytes, type FileReadOptions } from '../../shared/file-preview';
import { settlementFailureKind, WorkspaceHostError } from '../workspaces/host';

/** Reuse authorized range reads. A complete, single-version copy is published
 * only after every bounded physical read has settled. The two rolling lanes
 * share caller admission and copy directly into one exact-size destination. */
export async function readGeometrySource(read: (options: FileReadOptions, signal: AbortSignal) => Promise<FileBytes>, signal: AbortSignal) {
  const controller = new AbortController(), reading = controller.signal;
  const abort = () => controller.abort(signal.reason);
  signal.addEventListener('abort', abort, { once: true });
  if (signal.aborted) abort();
  let lastYield = performance.now();
  const yieldTask = () => new Promise<void>(resolve => {
    const channel = new MessageChannel(); let done = false;
    const finish = () => { if (done) return; done = true; reading.removeEventListener('abort', finish); channel.port1.close(); channel.port2.close(); resolve(); };
    reading.addEventListener('abort', finish, { once: true }); channel.port1.onmessage = finish;
    if (reading.aborted) finish(); else channel.port2.postMessage(null);
  });
  const yieldIfNeeded = async () => {
    if (performance.now() - lastYield >= 4) { await yieldTask(); reading.throwIfAborted(); lastYield = performance.now(); }
  };
  const validatePage = (page: FileBytes, offset: number, length: number) => {
    if (!isFileData(page.data) || !Number.isSafeInteger(page.bytes) || page.bytes < 0 || page.bytes > L.geometryBytes
      || page.lines !== undefined || page.offset !== offset || typeof page.version !== 'string' || !page.version
      || page.data.length !== Math.min(length, Math.max(0, page.bytes - offset))
      || page.eof !== (offset + page.data.length === page.bytes)) throw new Error(page.bytes > L.geometryBytes ? 'too_large' : 'malformed');
  };
  try {
    reading.throwIfAborted();
    let total: number, version: string, data: Uint8Array<ArrayBuffer>, nextOffset: number;
    {
      const first = await read({ kind: 'bytes', range: { offset: 0, length: L.pageBytes } }, reading);
      reading.throwIfAborted();
      validatePage(first, 0, L.pageBytes);
      total = first.bytes; version = first.version;
      if (first.eof) return { data: first.data, version };
      data = new Uint8Array(total);
      data.set(first.data);
      nextOffset = first.data.length;
    }
    await yieldIfNeeded();
    let failedSettlement: unknown;
    const lane = async () => {
      try {
        while (nextOffset < total) {
          reading.throwIfAborted();
          const offset = nextOffset, length = Math.min(L.pageBytes, total - offset);
          nextOffset += length;
          const page = await read({ kind: 'bytes', range: { offset, length } }, reading);
          reading.throwIfAborted();
          validatePage(page, offset, length);
          if (version !== page.version || total !== page.bytes) throw new WorkspaceHostError('Geometry source changed during read', 'source_changed');
          data.set(page.data, offset);
          await yieldIfNeeded();
        }
      } catch (cause) {
        // Abort siblings immediately, but retain every caller admission slot until
        // the corresponding physical read settles, including late responses.
        // Lost physical settlement outranks an earlier cancellation or source error.
        if (settlementFailureKind(cause)) failedSettlement = cause;
        controller.abort(cause);
      }
    };
    await Promise.all([lane(), lane()]);
    if (failedSettlement !== undefined) throw failedSettlement;
    reading.throwIfAborted();
    return { data, version };
  } finally {
    signal.removeEventListener('abort', abort);
  }
}

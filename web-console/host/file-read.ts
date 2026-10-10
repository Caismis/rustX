/** Node-only native seam. Never imported by browser code or its protocol log. */
import type { DeliveryBytes, DeliveryRead } from '../src/workspaces/host.ts';
import type { AttachmentTarget, RpcError } from '../../protocol/app-server/v39.ts';
import { WorkspaceHostError } from '../src/workspaces/host.ts';

export class NativeFileReadError extends Error {
  readonly error: RpcError;
  constructor(error: RpcError) { super(error.message); this.error = error; }
}

export function readNativeDelivery(endpoint: string, credential: string, read: DeliveryRead, roots: string[], signal: AbortSignal): Promise<DeliveryBytes> {
  return readNativeSource(endpoint, credential, read.target, { kind: 'session_file', message_id: read.message_id, delivery_index: read.delivery_index }, roots, signal) as Promise<DeliveryBytes>;
}
export function readNativeSource(endpoint: string, credential: string, target: AttachmentTarget, source: import('../shared/documents.ts').DocumentSource, roots: string[], signal: AbortSignal): Promise<{ data: string; file?: DeliveryBytes['file'] }> {
  signal.throwIfAborted();
  const url = new URL(endpoint);
  if (url.pathname !== '/' || url.search || url.hash || url.username || url.password) throw new Error('Invalid Product Host native endpoint');
  url.pathname = '/product-host/file-read';
  // Secret is handshake-only, never reflected in the selected subprotocol.
  const socket = new WebSocket(url, ['rustx.product-host.file-read.v2', `rustx-product-host.${credential}`]);
  return new Promise((resolve, reject) => {
    let settled = false, dispatched = false, closing = false;
    let requestedError: Error | undefined;
    const unknown = () => new WorkspaceHostError('Native file settlement could not be confirmed', 'file_settlement_unknown');
    const close = () => { if (!closing) { closing = true; socket.close(1000); } };
    const finish = (error?: unknown, bytes?: { data: string; file?: DeliveryBytes['file'] }) => {
      if (settled) return;
      settled = true;
      signal.removeEventListener('abort', abort);
      socket.removeEventListener('open', open);
      socket.removeEventListener('message', message);
      socket.removeEventListener('error', failed);
      socket.removeEventListener('close', closed);
      close();
      if (error) reject(error); else resolve(bytes!);
    };
    // Once dispatch is possible, cancellation retires publication immediately,
    // but the read slot remains owned until native retirement is acknowledged.
    const abort = () => {
      requestedError = new Error('Product Host file authority revoked');
      if (dispatched) close(); else finish(requestedError);
    };
    const failed = () => finish(dispatched ? unknown() : new Error('Product Host native file connection unavailable'));
    const closed = (event: CloseEvent) => finish(dispatched && !event.wasClean ? unknown()
      : requestedError ?? new Error('Product Host native file connection unavailable'));
    const open = () => {
      if (signal.aborted) return abort();
      const payload = JSON.stringify({ target, source, roots });
      if (Buffer.byteLength(payload) > 1_048_576) return finish(new Error('Product Host read coordinates exceed limit'));
      dispatched = true;
      try { socket.send(payload); } catch { failed(); }
    };
    const message = (event: MessageEvent) => {
      try {
        if (typeof event.data !== 'string' || Buffer.byteLength(event.data) > 1_048_576) throw new Error('Invalid native file response');
        const response = JSON.parse(event.data);
        if (response?.jsonrpc !== '2.0' || response.id !== 0) throw new Error('Invalid native file response');
        if (response.error) {
          if (response.result !== undefined || typeof response.error.code !== 'number' || typeof response.error.message !== 'string') throw new Error('Invalid native file response');
          finish(requestedError ?? new NativeFileReadError(response.error));
          return;
        }
        if (response.result?.type !== (source.kind === 'session_file' ? 'session_file_bytes' : 'artifact_bytes') || typeof response.result.data !== 'string') throw new Error('Invalid native file response');
        if (requestedError) { finish(requestedError); return; }
        finish(undefined, response.result);
      } catch {
        requestedError = signal.aborted ? new Error('Product Host file authority revoked') : new Error('Invalid native file response');
        if (dispatched) close(); else finish(requestedError);
      }
    };
    socket.addEventListener('open', open);
    socket.addEventListener('message', message);
    socket.addEventListener('error', failed);
    socket.addEventListener('close', closed);
    signal.addEventListener('abort', abort, { once: true });
    if (signal.aborted) abort();
  });
}

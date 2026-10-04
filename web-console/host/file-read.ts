/** Node-only native seam. Never imported by browser code or its protocol log. */
import type { DeliveryBytes, DeliveryRead } from '../src/workspaces/host.ts';
import type { AttachmentTarget, RpcError } from '../../protocol/app-server/v34.ts';

export class NativeFileReadError extends Error {
  readonly error: RpcError;
  constructor(error: RpcError) { super(error.message); this.error = error; }
}

export function readNativeDelivery(endpoint: string, credential: string, read: DeliveryRead, roots: string[], signal: AbortSignal): Promise<DeliveryBytes> {
  return readNativeSource(endpoint, credential, read.target, { kind: 'session_file', message_id: read.message_id, delivery_index: read.delivery_index }, roots, signal) as Promise<DeliveryBytes>;
}
export function readNativeSource(endpoint: string, credential: string, target: AttachmentTarget, source: import('../src/client/document-types.ts').DocumentSource, roots: string[], signal: AbortSignal): Promise<{ data: string; file?: DeliveryBytes['file'] }> {
  signal.throwIfAborted();
  const url = new URL(endpoint);
  if (url.pathname !== '/' || url.search || url.hash || url.username || url.password) throw new Error('Invalid Product Host native endpoint');
  url.pathname = '/product-host/file-read';
  // Secret is handshake-only, never reflected in the selected subprotocol.
  const socket = new WebSocket(url, ['rustx.product-host.file-read.v2', `rustx-product-host.${credential}`]);
  return new Promise((resolve, reject) => {
    let settled = false;
    const finish = (error?: unknown, bytes?: { data: string; file?: DeliveryBytes['file'] }) => {
      if (settled) return;
      settled = true;
      signal.removeEventListener('abort', abort);
      socket.removeEventListener('open', open);
      socket.removeEventListener('message', message);
      socket.removeEventListener('error', failed);
      socket.removeEventListener('close', failed);
      socket.close();
      if (error) reject(error); else resolve(bytes!);
    };
    const abort = () => finish(new Error('Product Host file authority revoked'));
    const failed = () => finish(new Error('Product Host native file connection unavailable'));
    const open = () => {
      if (signal.aborted) return abort();
      const payload = JSON.stringify({ target, source, roots });
      if (Buffer.byteLength(payload) > 1_048_576) return finish(new Error('Product Host read coordinates exceed limit'));
      socket.send(payload);
    };
    const message = (event: MessageEvent) => {
      try {
        if (typeof event.data !== 'string' || Buffer.byteLength(event.data) > 1_048_576) throw new Error('Invalid native file response');
        const response = JSON.parse(event.data);
        if (response.error) throw new NativeFileReadError(response.error);
        if (response.jsonrpc !== '2.0' || response.id !== 0 || response.result?.type !== (source.kind === 'session_file' ? 'session_file_bytes' : 'artifact_bytes')) throw new Error('Invalid native file response');
        signal.throwIfAborted();
        finish(undefined, response.result);
      } catch (error) { finish(error); }
    };
    socket.addEventListener('open', open);
    socket.addEventListener('message', message);
    socket.addEventListener('error', failed);
    socket.addEventListener('close', failed);
    signal.addEventListener('abort', abort, { once: true });
    if (signal.aborted) abort();
  });
}

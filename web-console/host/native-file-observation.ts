/** Native permission/ownership feed for one committed delivery. The private
 * Product Host credential remains handshake-only and never reaches browsers. */
import type { DeliveryRead } from '../src/workspaces/host.ts';
import { WorkspaceHostError } from '../src/workspaces/host.ts';
import { NativeFileReadError, validateDeliveryRead } from './file-read.ts';
import type { SessionFileReference } from '../../protocol/app-server/v55.ts';
import type { PreviewObservation } from '../shared/file-observation.ts';
export async function observeNativeDelivery(endpoint: string, credential: string, read: DeliveryRead, roots: string[], changed: () => void, signal: AbortSignal): Promise<PreviewObservation & { file: SessionFileReference }> {
  signal.throwIfAborted();
  validateDeliveryRead(read);
  // Serialize before socket admission. No caller-controlled getter or malformed
  // coordinate can escape a WebSocket EventTarget callback.
  const payload = JSON.stringify({ target: read.target, source: { kind: 'session_file', locator: read.locator, delivery_index: read.delivery_index, agent_id: read.agent_id }, roots, read: { kind: 'bytes', max_bytes: 32 * 1024 * 1024, range: { offset: 0, length: 1 } } });
  if (Buffer.byteLength(payload) > 65536) throw new Error('File observation coordinates exceed limit');
  const url = new URL(endpoint);
  if (url.pathname !== '/' || url.search || url.hash || url.username || url.password) throw new Error('Invalid Product Host native endpoint');
  url.pathname = '/product-host/file-observe';
  const socket = new WebSocket(url, ['rustx.product-host.file-observe.v1', `rustx-product-host.${credential}`]);
  let dispatched = false, ready = false, closed = false, requestedError: unknown;
  let accept!: (value: { file: SessionFileReference; version: string }) => void, fail!: (error: unknown) => void, complete!: () => void;
  const initial = new Promise<{ file: SessionFileReference; version: string }>((resolve, reject) => { accept = resolve; fail = reject; });
  const done = new Promise<void>(resolve => { complete = resolve; });
  const unknown = () => new WorkspaceHostError('Native file observation settlement unavailable', 'file_settlement_unknown');
  const stop = () => { if (!closed) { closed = true; socket.close(1000); } };
  const abort = () => { requestedError = new Error('Product Host file authority revoked'); stop(); };
  const open = () => {
    if (signal.aborted) return abort();
    dispatched = true;
    try { socket.send(payload); } catch { failed(); }
  };
  const message = (event: MessageEvent) => {
    try {
      if (typeof event.data !== 'string' || Buffer.byteLength(event.data) > 1_048_576) throw new Error('Invalid native observation');
      const value = JSON.parse(event.data);
      if (ready && value.kind === 'changed') { if (!signal.aborted) changed(); return; }
      if (ready || value.jsonrpc !== '2.0' || value.id !== 0) throw new Error('Invalid native observation');
      if (value.error) { requestedError = new NativeFileReadError(value.error); stop(); return; }
      if (!value.result?.file || typeof value.result.version !== 'string' || !value.result.version) throw new Error('Invalid native observation');
      ready = true;
      if (signal.aborted) { requestedError = signal.reason; stop(); } else accept(value.result);
    } catch { requestedError = unknown(); stop(); }
  };
  const failed = () => { requestedError ??= dispatched && !ready ? unknown() : new Error('Native file observation unavailable'); stop(); };
  const closing = (event: CloseEvent) => {
    signal.removeEventListener('abort', abort); socket.removeEventListener('open', open); socket.removeEventListener('message', message); socket.removeEventListener('error', failed); socket.removeEventListener('close', closing);
    if (!ready || requestedError) fail(dispatched && !ready && !event.wasClean ? unknown() : requestedError ?? new Error('Native file observation unavailable'));
    complete();
  };
  socket.addEventListener('open', open); socket.addEventListener('message', message); socket.addEventListener('error', failed); socket.addEventListener('close', closing);
  signal.addEventListener('abort', abort, { once: true }); if (signal.aborted) abort();
  try { return { ...await initial, done, close: async () => { stop(); await done; } }; }
  catch (error) { stop(); await done; throw error; }
}

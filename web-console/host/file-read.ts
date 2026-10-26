/** Node-only native seam. Never imported by browser code or its protocol log. */
import type { DeliveryBytes, DeliveryRead } from '../src/workspaces/host.ts';
import type { AttachmentTarget, RpcError } from '../../protocol/app-server/v59.ts';
import { WorkspaceHostError } from '../src/workspaces/host.ts';
import { FILE_PREVIEW_LIMITS, validateFileRead, type FileBytes, type FileReadOptions } from '../shared/file-preview.ts';

const object = (value: unknown): value is Record<string, unknown> => !!value && typeof value === 'object' && !Array.isArray(value);
const text = (value: unknown): value is string => typeof value === 'string' && !/[\uD800-\uDFFF]/u.test(value);
const coordinate = (value: unknown): value is string => text(value) && value.length > 0 && Buffer.byteLength(value) <= 256;
const identityNumber = (value: unknown) => typeof value === 'string' && /^(0|[1-9][0-9]{0,19})$/.test(value) && BigInt(value) <= 18446744073709551615n;
const uuidV7 = /^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const nativeId = (value: unknown, prefix: string) => typeof value === 'string' && value.length === prefix.length + 36 && value.startsWith(prefix) && uuidV7.test(value.slice(prefix.length));
/** Coordinates select canonical evidence; only native mapping grants access. */
export function validDeliveryLocator(value: unknown): boolean {
  if (!object(value)) return false;
  return value.type === 'by_message_id'
    ? coordinate(value.message_id) && Object.keys(value).every(key => ['type', 'message_id'].includes(key))
    : value.type === 'by_ledger_position' && Number.isSafeInteger(value.position) && Number(value.position) > 0 && Object.keys(value).every(key => ['type', 'position'].includes(key));
}
/** Shared by reads and observations, before either consumes native capacity. */
export function validateDeliveryRead(read: unknown): asserts read is DeliveryRead {
  if (!object(read) || Object.keys(read).some(key => !['target', 'locator', 'delivery_index', 'agent_id', 'read'].includes(key))
    || !Number.isSafeInteger(read.delivery_index) || Number(read.delivery_index) < 0 || !validDeliveryLocator(read.locator)
    || !object(read.target) || Object.keys(read.target).some(key => !['session_id', 'conversation_id', 'attachment_id', 'runtime_incarnation'].includes(key))
    || !['session_id', 'conversation_id', 'attachment_id', 'runtime_incarnation'].every(key => coordinate((read.target as Record<string, unknown>)[key]))) throw new Error('Invalid delivery coordinates');
  if (!nativeId(read.target.session_id, 'ses_') || !nativeId(read.target.conversation_id, 'conv_') || !identityNumber(read.target.runtime_incarnation)) throw new Error('Invalid delivery coordinates');
  if (read.agent_id !== undefined && !coordinate(read.agent_id)) throw new Error('Invalid Agent coordinates');
  validateFileRead(read.read === undefined ? { kind: 'bytes' } : read.read as FileReadOptions);
}
export function validateDeliveryObservation(read: unknown, file: unknown): void {
  validateDeliveryRead(read);
  if (!object(file) || Object.keys(file).some(key => !['scope', 'path', 'name', 'description', 'mime_type'].includes(key))
    || !object(file.scope) || Object.keys(file.scope).some(key => !['conversation_id', 'device', 'inode'].includes(key))
    || !nativeId(file.scope.conversation_id, 'conv_')
    || !identityNumber(file.scope.device) || !identityNumber(file.scope.inode)
    || !text(file.path) || !file.path || Buffer.byteLength(file.path) > 4096 || file.path.startsWith('/') || file.path.includes('\0')
    || file.path.split('/').some(part => !part || part === '.' || part === '..')
    || !text(file.name) || file.name !== file.path.split('/').at(-1)
    || !text(file.mime_type) || Buffer.byteLength(file.mime_type) > 128
    || file.description != null && (!text(file.description) || [...file.description].length > 512)) throw new Error('Invalid delivery file identity');
}

export class NativeFileReadError extends Error {
  readonly error: RpcError;
  constructor(error: RpcError) { super(error.message); this.error = error; }
}

export function readNativeDelivery(endpoint: string, credential: string, read: DeliveryRead, roots: string[], signal: AbortSignal, maxBytes = FILE_PREVIEW_LIMITS.fileBytes): Promise<DeliveryBytes> {
  validateDeliveryRead(read);
  return readNativeSource(endpoint, credential, read.target, { kind: 'session_file', locator: read.locator, delivery_index: read.delivery_index, agent_id: read.agent_id }, roots, signal, read.read, maxBytes) as Promise<DeliveryBytes>;
}
export function readNativeSource(endpoint: string, credential: string, target: AttachmentTarget, source: import('../shared/documents.ts').DocumentSource, roots: string[], signal: AbortSignal,
  read: FileReadOptions = { kind: 'bytes' }, maxBytes = FILE_PREVIEW_LIMITS.fileBytes): Promise<FileBytes & { file?: DeliveryBytes['file'] }> {
  signal.throwIfAborted();
  const url = new URL(endpoint);
  if (url.pathname !== '/' || url.search || url.hash || url.username || url.password) throw new Error('Invalid Product Host native endpoint');
  url.pathname = '/product-host/file-read';
  // Secret is handshake-only, never reflected in the selected subprotocol.
  const socket = new WebSocket(url, ['rustx.product-host.file-read.v5', `rustx-product-host.${credential}`]);
  socket.binaryType = 'arraybuffer';
  return new Promise((resolve, reject) => {
    let settled = false, dispatched = false, closing = false;
    let requestedError: Error | undefined;
    let metadata: Omit<FileBytes, 'data'> & { length: number; file?: DeliveryBytes['file'] } | undefined;
    const unknown = () => new WorkspaceHostError('Native file settlement could not be confirmed', 'file_settlement_unknown');
    const close = () => { if (!closing) { closing = true; socket.close(1000); } };
    const finish = (error?: unknown, bytes?: FileBytes & { file?: DeliveryBytes['file'] }) => {
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
      const payload = JSON.stringify({ target, source, roots, read: read.kind === 'bytes' ? { ...read, max_bytes: maxBytes } : read });
      if (Buffer.byteLength(payload) > 1_048_576) return finish(new Error('Product Host read coordinates exceed limit'));
      dispatched = true;
      try { socket.send(payload); } catch { failed(); }
    };
    const message = (event: MessageEvent) => {
      try {
        if (event.data instanceof ArrayBuffer) {
          if (!metadata || event.data.byteLength !== metadata.length) throw new Error('Invalid native file body');
          const { length: _length, ...value } = metadata;
          finish(requestedError, { ...value, data: new Uint8Array(event.data) });
          return;
        }
        if (typeof event.data !== 'string' || Buffer.byteLength(event.data) > 1_048_576) throw new Error('Invalid native file response');
        const response = JSON.parse(event.data);
        if (response?.jsonrpc !== '2.0' || response.id !== 0) throw new Error('Invalid native file response');
        if (response.error) {
          if (response.result !== undefined || typeof response.error.code !== 'number' || typeof response.error.message !== 'string') throw new Error('Invalid native file response');
          finish(requestedError ?? new NativeFileReadError(response.error));
          return;
        }
        const result = response.result;
        const limit = read.kind === 'text' || read.range ? FILE_PREVIEW_LIMITS.pageBytes : maxBytes;
        if (metadata || result?.type !== (source.kind === 'session_file' ? 'session_file_bytes' : 'artifact_bytes')
          || !Number.isSafeInteger(result.length) || result.length < 0 || result.length > limit
          || typeof result.version !== 'string' || !result.version
          || !Number.isSafeInteger(result.bytes) || result.bytes < 0 || !Number.isSafeInteger(result.offset) || result.offset < 0
          || typeof result.eof !== 'boolean'
          || (read.kind === 'text' && (!Number.isSafeInteger(result.lines) || result.lines < 0 || result.lines > read.limit || result.offset !== read.offset))) throw new Error('Invalid native file response');
        metadata = { file: result.file, version: result.version, bytes: result.bytes, offset: result.offset, eof: result.eof,
          length: result.length, ...(result.lines === undefined ? {} : { lines: result.lines }) };
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

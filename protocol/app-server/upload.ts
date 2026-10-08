import type { UploadDescriptor, UploadPolicy } from './v37.ts';

export function uploadOperationId(): string { return crypto.randomUUID().replaceAll('-', ''); }

/** Only the selected native origin, or an explicitly owned child's port. */
export function uploadUrl(transfer: UploadDescriptor, endpoint?: string): string {
  if (!/^\/session-upload\/[A-Za-z0-9_-]{43}$/.test(transfer.path)) throw new Error('Invalid upload capability');
  let origin: URL;
  if (endpoint !== undefined) {
    origin = new URL(endpoint);
    if (transfer.loopback_port != null || !['ws:', 'wss:'].includes(origin.protocol) || origin.username || origin.password || origin.pathname !== '/' || origin.search || origin.hash) throw new Error('Invalid upload origin');
  } else {
    if (!Number.isInteger(transfer.loopback_port) || transfer.loopback_port! < 1 || transfer.loopback_port! > 65535) throw new Error('Missing owned-child upload port');
    origin = new URL(`ws://127.0.0.1:${transfer.loopback_port}`);
  }
  return new URL(transfer.path, origin).href;
}

/** One chunk in flight, no queue, retries, redirects, base64 or storage staging.
 * "settled" only asks the caller to read the native outcome; it is not a receipt. */
export function transferUpload(transfer: UploadDescriptor, files: readonly Blob[], policy: UploadPolicy, endpoint?: string): Promise<void> {
  const url = uploadUrl(transfer, endpoint);
  return new Promise((resolve, reject) => {
    const socket = new WebSocket(url, 'rustx.session-upload.v1');
    let index = 0, offset = 0, reading = false, ended = false;
    const finish = (error?: unknown) => {
      if (ended) return;
      ended = true; socket.onmessage = null; socket.onerror = null; socket.onclose = null; socket.close();
      if (error) reject(error); else resolve();
    };
    socket.onerror = socket.onclose = () => finish(new Error('Upload carrier disconnected; check exact operation status'));
    socket.onmessage = event => {
      if (reading) { finish(new Error('Unexpected upload demand')); return; }
      if (event.data === 'settled' || event.data === 'check') { finish(); return; }
      while (index < files.length && offset === files[index]!.size) { index++; offset = 0; }
      if (event.data === 'finish' && index === files.length) { socket.send('finish'); return; }
      if (event.data !== 'next' || index >= files.length) { finish(new Error('Invalid upload framing')); return; }
      reading = true;
      const part = files[index]!.slice(offset, offset + policy.max_chunk_bytes);
      void part.arrayBuffer().then(bytes => {
        if (ended) return;
        offset += bytes.byteLength; reading = false; socket.send(bytes);
      }).catch(finish);
    };
  });
}

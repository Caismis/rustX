import { randomUUID } from 'node:crypto';
import type { ServerResponse } from 'node:http';

/** A fixed multipart boundary lets admission be acknowledged before the read.
 * The terminal metadata/body are written only after physical work has settled. */
export function acknowledgeFile(response: ServerResponse, kind: 'File' | 'Document' | 'Workbench', id: string): string {
  const boundary = `rustx-${randomUUID()}`;
  response.writeHead(200, { 'Content-Type': `multipart/form-data; boundary=${boundary}`,
    'Cache-Control': 'no-store', [`X-Rustx-${kind}-Operation`]: id });
  response.flushHeaders();
  return boundary;
}

export async function finishFile(response: ServerResponse, boundary: string, metadata: unknown, data?: Uint8Array): Promise<void> {
  if (response.destroyed) return;
  const settled = new Promise<void>((resolve, reject) => {
    const cleanup = () => { response.off('finish', done); response.off('close', done); response.off('error', failed); };
    const done = () => { cleanup(); resolve(); };
    const failed = (error: Error) => { cleanup(); reject(error); };
    response.once('finish', done); response.once('close', done); response.once('error', failed);
  });
  response.write(`--${boundary}\r\nContent-Disposition: form-data; name="metadata"\r\n\r\n${JSON.stringify(metadata)}\r\n`);
  if (data !== undefined) {
    response.write(`--${boundary}\r\nContent-Disposition: form-data; name="data"; filename="data"\r\nContent-Type: application/octet-stream\r\n\r\n`);
    response.write(data);
    response.write('\r\n');
  }
  response.end(`--${boundary}--\r\n`);
  await settled;
}

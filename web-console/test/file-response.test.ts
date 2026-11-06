// @vitest-environment node
import { expect, it, vi } from 'vitest';
import { fileResponse } from '../src/workspaces/file-response';

function multipart(metadata: unknown, data?: Uint8Array) {
  const boundary = 'file-response-test';
  const encode = (value: string) => new TextEncoder().encode(value);
  const parts: Uint8Array[] = [encode(`--${boundary}\r\nContent-Disposition: form-data; name="metadata"\r\n\r\n${JSON.stringify(metadata)}\r\n`)];
  if (data !== undefined) parts.push(encode(`--${boundary}\r\nContent-Disposition: form-data; name="data"; filename="data"\r\nContent-Type: application/octet-stream\r\n\r\n`), data, encode('\r\n'));
  parts.push(encode(`--${boundary}--\r\n`));
  const bytes = new Uint8Array(parts.reduce((size, part) => size + part.length, 0));
  let offset = 0;
  for (const part of parts) { bytes.set(part, offset); offset += part.length; }
  return { bytes, headers: { 'Content-Type': `multipart/form-data; boundary=${boundary}` } };
}
const success = { ok: true, attachment: 'data', value: { data: null } };
it.each([
  ['data', { data: null }],
  ['preview', { preview: { kind: 'pdf', data: null } }],
  ['workspace-preview', { preview: { data: null } }],
])('live %s attachments retain their bytes', async (attachment, value) => {
  const data = new Uint8Array([0, 1, 127, 255]);
  const { bytes, headers } = multipart({ ok: true, attachment, value }, data);
  const result = await fileResponse(new Response(bytes, { headers }), data.length);
  expect(result).toEqual({ ok: true, attachment, value: attachment === 'data' ? { data } : attachment === 'preview' ? { preview: { kind: 'pdf', data } } : { preview: { data } } });
});
it('canceled successful large payloads skip Blob materialization', async () => {
  const data = new Uint8Array(600 * 1024), abort = new AbortController(); abort.abort();
  const { bytes, headers } = multipart(success, data);
  const materialize = vi.spyOn(Blob.prototype, 'arrayBuffer');
  expect(await fileResponse(new Response(bytes, { headers }), data.length, abort.signal)).toEqual(success);
  expect(materialize).not.toHaveBeenCalled();
});
it('cancellation during multipart parsing still waits for the full physical terminal', async () => {
  const { bytes, headers } = multipart(success, new Uint8Array(600 * 1024));
  let controller!: ReadableStreamDefaultController<Uint8Array>;
  let entered!: () => void;
  const parsing = new Promise<void>(resolve => { entered = resolve; });
  const stream = new ReadableStream<Uint8Array>({ start(value) { controller = value; }, pull() { entered(); } });
  const abort = new AbortController(), materialize = vi.spyOn(Blob.prototype, 'arrayBuffer');
  const work = fileResponse(new Response(stream, { headers }), 600 * 1024, abort.signal);
  const terminal = vi.fn(); void work.then(terminal, terminal);
  controller.enqueue(bytes.subarray(0, bytes.length - 8));
  await parsing;
  abort.abort();
  expect(terminal).not.toHaveBeenCalled();
  controller.enqueue(bytes.subarray(bytes.length - 8)); controller.close();
  expect(await work).toEqual(success);
  expect(materialize).not.toHaveBeenCalled();
});
it.each([
  ['unknown attachment', { ...success, attachment: 'unknown' }, new Uint8Array([1]), 1],
  ['non-null placeholder', { ...success, value: { data: [] } }, new Uint8Array([1]), 1],
  ['invalid preview kind', { ok: true, attachment: 'preview', value: { preview: { kind: 'text', data: null } } }, new Uint8Array([1]), 1],
  ['invalid workspace preview shape', { ok: true, attachment: 'workspace-preview', value: {} }, new Uint8Array([1]), 1],
  ['missing body', success, undefined, 1],
  ['oversized body', success, new Uint8Array([1, 2]), 1],
  ['failure with attachment', { ...success, ok: false }, new Uint8Array([1]), 1],
  ['malformed envelope', { value: {} }, undefined, 1],
  ['unexpected body', { ok: true, value: {} }, new Uint8Array([1]), 1],
] as const)('canceled %s remains invalid', async (_name, metadata, data, limit) => {
  const abort = new AbortController(); abort.abort();
  const { bytes, headers } = multipart(metadata, data);
  const materialize = vi.spyOn(Blob.prototype, 'arrayBuffer');
  await expect(fileResponse(new Response(bytes, { headers }), limit, abort.signal)).rejects.toThrow();
  expect(materialize).not.toHaveBeenCalled();
});

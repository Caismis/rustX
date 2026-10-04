import { Worker } from 'node:worker_threads';
import { createHash } from 'node:crypto';
import { DOCUMENT_LIMITS, type DocumentRequest, type DocumentResult, type WorkbookPreview } from '../../src/client/document-types.ts';
import type { DeliveryBytes } from '../../src/workspaces/host.ts';
import { convertOffice } from './converter.ts';

type SourceBytes = { data: string; file?: DeliveryBytes['file'] };
const digest = (bytes: Buffer) => createHash('sha256').update(bytes).digest('hex');
export async function deriveDocument(request: DocumentRequest, read: () => Promise<SourceBytes>, signal: AbortSignal): Promise<DocumentResult> {
  const decode = (source: SourceBytes) => {
    const max = request.source.kind === 'artifact' ? 256 * 1024 : 512 * 1024;
    if (source.data.length > Math.ceil(max / 3) * 4) throw new Error('too_large');
    const bytes = Buffer.from(source.data, 'base64');
    if (bytes.length > max) throw new Error('too_large');
    if (digest(bytes) !== request.digest) throw new Error('source_changed');
    return bytes;
  };
  const source = await read();
  signal.throwIfAborted();
  const bytes = decode(source);
  const worker = new Worker(new URL('./worker.ts', import.meta.url), {
    execArgv: [], env: {},
    workerData: { bytes, extension: request.extension },
    resourceLimits: { maxOldGenerationSizeMb: 64, maxYoungGenerationSizeMb: 16, stackSizeMb: 2 },
  });
  let workbook: WorkbookPreview | undefined;
  let cleanup = () => {};
  try {
    workbook = await new Promise<WorkbookPreview | undefined>((resolve, reject) => {
      const abort = () => reject(new Error('obsolete'));
      const timer = setTimeout(() => reject(new Error('parser_timeout')), DOCUMENT_LIMITS.timeout);
      const finish = () => { clearTimeout(timer); signal.removeEventListener('abort', abort); };
      cleanup = finish;
      signal.addEventListener('abort', abort, { once: true });
      worker.once('message', value => { finish(); if (value.error) reject(new Error(value.error)); else resolve(value.result); });
      worker.once('error', () => { finish(); reject(new Error('parser_failure')); });
      worker.once('exit', () => { finish(); reject(new Error('parser_failure')); });
      if (signal.aborted) abort();
    });
  } finally { cleanup(); await worker.terminate(); worker.removeAllListeners(); }
  // Admission/parse never grants authority for conversion or publication. An
  // exact reread checks mutable bytes, mapping, root and attachment again.
  const reauthorize = async () => {
    signal.throwIfAborted();
    const current = await read(); decode(current);
    if (JSON.stringify(current.file) !== JSON.stringify(source.file)) throw new Error('source_changed');
    signal.throwIfAborted();
  };
  await reauthorize();
  if (request.extension === 'xlsx') {
    if (!workbook) throw new Error('malformed');
    return { digest: request.digest, file: source.file, preview: workbook };
  }
  const pdf = await convertOffice(bytes, request.extension, signal);
  await reauthorize();
  return { digest: request.digest, file: source.file, preview: { kind: 'pdf', data: pdf.toString('base64') } };
}

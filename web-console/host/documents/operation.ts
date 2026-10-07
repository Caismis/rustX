import { sameSessionFile } from '../../shared/session-file-identity.ts';
import { PARSER_TIMEOUT_MS } from './limits.ts';
import { Worker } from 'node:worker_threads';
import { createHash } from 'node:crypto';
import type { DocumentRequest, DocumentResult, WorkbookPreview } from '../../shared/documents.ts';
import { convertOffice } from './converter.ts';

type SourceBytes = { data: string; file?: DocumentResult['file'] };
const digest = (bytes: Buffer) => createHash('sha256').update(bytes).digest('hex');
export async function deriveDocument(request: DocumentRequest, read: () => Promise<SourceBytes>, signal: AbortSignal): Promise<DocumentResult> {
  return deriveBytes(request.extension, request.digest, read, request.source.kind === 'artifact' ? 256 * 1024 : 512 * 1024, signal);
}

/** Workspace paths are admitted by the Host; they never become retained native deliveries. */
export async function deriveWorkspaceOffice(extension: 'docx' | 'pptx', read: () => Promise<SourceBytes>, signal: AbortSignal): Promise<DocumentResult> {
  const first = await read();
  return deriveBytes(extension, digest(Buffer.from(first.data, 'base64')), read, 512 * 1024, signal);
}
async function deriveBytes(extension: DocumentRequest['extension'], sourceDigest: string, read: () => Promise<SourceBytes>, max: number, signal: AbortSignal): Promise<DocumentResult> {
  const decode = (source: SourceBytes) => {
    if (source.data.length > Math.ceil(max / 3) * 4) throw new Error('too_large');
    const bytes = Buffer.from(source.data, 'base64');
    if (bytes.length > max) throw new Error('too_large');
    if (digest(bytes) !== sourceDigest) throw new Error('source_changed');
    return bytes;
  };
  const source = await read();
  signal.throwIfAborted();
  const bytes = decode(source);
  const worker = new Worker(new URL('./worker.ts', import.meta.url), {
    execArgv: [], env: {},
    workerData: { bytes, extension },
    resourceLimits: { maxOldGenerationSizeMb: 64, maxYoungGenerationSizeMb: 16, stackSizeMb: 2 },
  });
  let workbook: WorkbookPreview | undefined;
  let cleanup = () => {};
  try {
    workbook = await new Promise<WorkbookPreview | undefined>((resolve, reject) => {
      const abort = () => reject(new Error('obsolete'));
      const timer = setTimeout(() => reject(new Error('parser_timeout')), PARSER_TIMEOUT_MS);
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
    if (!sameSessionFile(current.file, source.file)) throw new Error('source_changed');
    signal.throwIfAborted();
  };
  await reauthorize();
  if (extension === 'xlsx') {
    if (!workbook) throw new Error('malformed');
    return { digest: sourceDigest, file: source.file, preview: workbook };
  }
  const pdf = await convertOffice(bytes, extension, signal);
  await reauthorize();
  return { digest: sourceDigest, file: source.file, preview: { kind: 'pdf', data: pdf.toString('base64') } };
}

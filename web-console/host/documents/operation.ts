import { sameSessionFile } from '../../shared/session-file-identity.ts';
import { PARSER_TIMEOUT_MS } from './limits.ts';
import { Worker } from 'node:worker_threads';
import { createHash } from 'node:crypto';
import type { DocumentRequest, DocumentResult } from '../../shared/documents.ts';
import { convertOffice } from './converter.ts';
import { FILE_PREVIEW_LIMITS } from '../../shared/file-preview.ts';

type SourceBytes = { data: Uint8Array; file?: DocumentResult['file']; version?: string };
const digest = (bytes: Buffer) => createHash('sha256').update(bytes).digest('hex');
export async function deriveDocument(request: DocumentRequest, read: () => Promise<SourceBytes>, signal: AbortSignal): Promise<DocumentResult> {
  return deriveBytes(request.extension, read, request.source.kind === 'artifact' ? 256 * 1024 : FILE_PREVIEW_LIMITS.officeBytes, signal);
}

/** Workspace paths are admitted by the Host; they never become retained native deliveries. */
export async function deriveWorkspaceOffice(extension: 'docx' | 'pptx', read: () => Promise<SourceBytes>, signal: AbortSignal): Promise<DocumentResult> {
  return deriveBytes(extension, read, FILE_PREVIEW_LIMITS.officeBytes, signal);
}
async function deriveBytes(extension: DocumentRequest['extension'], read: () => Promise<SourceBytes>, max: number, signal: AbortSignal): Promise<DocumentResult> {
  const source = await read();
  signal.throwIfAborted();
  if (source.data.byteLength > max) throw new Error('too_large');
  const bytes = Buffer.from(source.data);
  const sourceDigest = digest(bytes);
  const decode = (source: SourceBytes) => {
    const bytes = Buffer.from(source.data);
    if (bytes.length > max) throw new Error('too_large');
    if (digest(bytes) !== sourceDigest) throw new Error('source_changed');
    return bytes;
  };
  const worker = new Worker(new URL('./worker.ts', import.meta.url), {
    execArgv: [], env: {},
    workerData: { bytes, extension },
    resourceLimits: { maxOldGenerationSizeMb: 64, maxYoungGenerationSizeMb: 16, stackSizeMb: 2 },
  });
  let cleanup = () => {};
  try {
    await new Promise<void>((resolve, reject) => {
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
    if (!sameSessionFile(current.file, source.file) || current.version !== source.version) throw new Error('source_changed');
    signal.throwIfAborted();
  };
  await reauthorize();
  const pdf = await convertOffice(bytes, extension, signal);
  await reauthorize();
  return { digest: sourceDigest, file: source.file, preview: { kind: 'pdf', data: Uint8Array.from(pdf) } };
}

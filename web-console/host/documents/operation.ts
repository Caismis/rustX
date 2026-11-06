import { sameSessionFile } from '../../shared/session-file-identity.ts';
import { PARSER_TIMEOUT_MS } from './limits.ts';
import { Worker } from 'node:worker_threads';
import { createHash } from 'node:crypto';
import type { DocumentRequest, DocumentResult } from '../../shared/documents.ts';
import { OfficeConverter } from './converter.ts';
import { FILE_PREVIEW_LIMITS } from '../../shared/file-preview.ts';
import { OfficeSettlementError } from './office-cgroup.ts';
import { OfficePreviewCache } from './cache.ts';

type SourceBytes = { data: Uint8Array; file?: DocumentResult['file']; version?: string };
const digest = (bytes: Buffer) => createHash('sha256').update(bytes).digest('hex');
/** One Host lifetime owns conversion reuse and its bounded successful PDFs. */
export class OfficeDocuments {
  private readonly cache = new OfficePreviewCache();
  private readonly converter = new OfficeConverter();
  private readonly lifetime = new AbortController();
  private readonly tasks = new Set<Promise<DocumentResult>>();
  private disposal?: Promise<void>;
  dispose(): Promise<void> {
    this.lifetime.abort(); this.cache.clear();
    return this.disposal ??= (async () => {
      const results = await Promise.allSettled([this.converter.dispose(), ...this.tasks]);
      const failure = results.find(result => result.status === 'rejected' && result.reason instanceof OfficeSettlementError);
      if (failure?.status === 'rejected') throw failure.reason;
    })();
  }
  private run(extension: DocumentRequest['extension'], read: () => Promise<SourceBytes>, max: number, signal: AbortSignal) {
    this.lifetime.signal.throwIfAborted();
    const work = this.deriveBytes(extension, read, max, AbortSignal.any([signal, this.lifetime.signal]));
    this.tasks.add(work); void work.finally(() => this.tasks.delete(work)).catch(() => {});
    return work;
  }
  async deriveDocument(request: DocumentRequest, read: () => Promise<SourceBytes>, signal: AbortSignal): Promise<DocumentResult> {
    return this.run(request.extension, read, request.source.kind === 'artifact' ? 256 * 1024 : FILE_PREVIEW_LIMITS.officeBytes, signal);
  }

  /** Workspace paths are admitted by the Host; they never become retained native deliveries. */
  async deriveWorkspaceOffice(extension: 'docx' | 'pptx', read: () => Promise<SourceBytes>, signal: AbortSignal): Promise<DocumentResult> {
    return this.run(extension, read, FILE_PREVIEW_LIMITS.officeBytes, signal);
  }
  private async deriveBytes(extension: DocumentRequest['extension'], read: () => Promise<SourceBytes>, max: number, signal: AbortSignal): Promise<DocumentResult> {
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
    const reauthorize = async () => {
      signal.throwIfAborted();
      const current = await read(); decode(current);
      if (!sameSessionFile(current.file, source.file) || current.version !== source.version) throw new Error('source_changed');
      signal.throwIfAborted();
    };
    const key = `${extension}:${sourceDigest}`;
    const cached = this.cache.get(key);
    if (cached) {
      await reauthorize();
      return { digest: sourceDigest, file: source.file, preview: { kind: 'pdf', data: Uint8Array.from(cached) } };
    }
    const warming = this.converter.warm(); void warming.catch(() => {});
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
    await warming;
    await reauthorize();
    const pdf = await this.converter.render(bytes, extension, signal);
    await reauthorize();
    this.cache.retain(key, pdf);
    return { digest: sourceDigest, file: source.file, preview: { kind: 'pdf', data: Uint8Array.from(pdf) } };
  }
}

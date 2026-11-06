import type { OfficeSandbox } from './office-sandbox.ts';
import { DERIVED_PDF_MAX_BYTES } from '../../shared/documents.ts';

/** A bounded binary stream, with a separate settlement trailer per document. */
export class OfficeChannel {
  private readonly source: AsyncIterator<Buffer>;
  private pending: Buffer = Buffer.alloc(0);
  private readonly sandbox: OfficeSandbox;
  constructor(sandbox: OfficeSandbox) { this.sandbox = sandbox; this.source = sandbox.output[Symbol.asyncIterator](); }
  private async more() {
    const next = await this.source.next();
    if (next.done) { await this.sandbox.closed; throw new Error('converter_failure'); }
    this.pending = this.pending.length ? Buffer.concat([this.pending, next.value]) : next.value;
  }
  private async line(): Promise<Record<string, unknown>> {
    for (;;) {
      const end = this.pending.indexOf(10);
      if (end >= 0) {
        if (end > 4096) throw new Error('converter_failure');
        const value: unknown = JSON.parse(this.pending.subarray(0, end).toString('utf8'));
        this.pending = this.pending.subarray(end + 1);
        if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('converter_failure');
        return value as Record<string, unknown>;
      }
      if (this.pending.length > 4096) throw new Error('converter_failure');
      await this.more();
    }
  }
  async ready(): Promise<number> {
    const result = await this.line();
    if (result.ready !== true || !Number.isSafeInteger(result.engine_pid) || Number(result.engine_pid) < 1) throw new Error('converter_failure');
    return Number(result.engine_pid);
  }
  async result(id: string, engine: number): Promise<Buffer> {
    const header = await this.line(), length = Number(header.length);
    if (header.id !== id || !Number.isSafeInteger(length) || length < 5 || length > DERIVED_PDF_MAX_BYTES) throw new Error('converter_failure');
    const chunks: Buffer[] = []; let remaining = length;
    while (remaining) {
      if (!this.pending.length) await this.more();
      const count = Math.min(this.pending.length, remaining);
      chunks.push(this.pending.subarray(0, count)); this.pending = this.pending.subarray(count); remaining -= count;
    }
    const trailer = await this.line();
    if (trailer.id !== id || trailer.settled !== true || trailer.engine_pid !== engine) throw new Error('converter_failure');
    const pdf = Buffer.concat(chunks, length);
    if (!pdf.subarray(0, 5).equals(Buffer.from('%PDF-'))) throw new Error('converter_failure');
    return pdf;
  }
}

import type { ContentChunk, ContentRef } from './v57.ts';
import { contentRefKey, sameContentRef } from './content-identity.ts';

/** Finite loaded byte ranges, independent of Applied Cursor and source lifetime. */
export class ContentCache {
  private static readonly RANGE_LIMIT = 64;
  private bytes = 0;
  private active = 0;
  private ranges = new Map<string, Uint8Array>();
  private fetch: (ref: ContentRef, offset: number, maxBytes: number, current: () => boolean) => Promise<ContentChunk>;
  private byteLimit: number;
  constructor(fetch: (ref: ContentRef, offset: number, maxBytes: number, current: () => boolean) => Promise<ContentChunk>, byteLimit = 4 * 1024 * 1024) { this.fetch = fetch; this.byteLimit = byteLimit; }
  async read(ref: ContentRef, offset: number, current: () => boolean): Promise<{ bytes: Uint8Array; next?: number }> {
    if (!Number.isSafeInteger(ref.readable_end) || ref.readable_end < 0
      || !Number.isSafeInteger(offset) || offset < 0 || offset > ref.readable_end) throw new Error('Invalid content range.');
    if (!current()) throw new Error('Content view was replaced.');
    const key = `${contentRefKey(ref)}:${offset}`, cached = this.ranges.get(key);
    if (cached) {
      this.ranges.delete(key); this.ranges.set(key, cached);
      const end = offset + cached.byteLength;
      return { bytes: cached.slice(), next: end < ref.readable_end ? end : undefined };
    }
    if (this.active >= 2) throw new Error('Content read capacity reached.');
    this.active++;
    try {
      const chunk = await this.fetch(ref, offset, 65_536, current);
      if (!current()) throw new Error('Content view was replaced.');
      if (chunk.data_base64.length > 87_384) throw new Error('Content chunk exceeds its decoded-byte bound.');
      if(chunk.data_base64.length>87_384 || chunk.data_base64.length%4!==0 || chunk.data_base64.length/4*3-(chunk.data_base64.endsWith('==')?2:chunk.data_base64.endsWith('=')?1:0)>65_536)throw new Error('Encoded native range exceeds the decoded byte bound.');
      const bytes = Uint8Array.from(atob(chunk.data_base64), value => value.charCodeAt(0));
      if (!sameContentRef(chunk.content_ref, ref) || chunk.offset !== offset || chunk.readable_end !== ref.readable_end || bytes.length > 65_536
        || chunk.end_offset !== offset + bytes.length || chunk.end_offset > ref.readable_end || offset < ref.readable_end && bytes.length === 0
        || chunk.complete !== (chunk.end_offset === ref.readable_end) || (chunk.next_offset ?? undefined) !== (chunk.complete ? undefined : chunk.end_offset)) throw new Error('Invalid authoritative content chunk.');
      // Concurrent reads of the same exact range replace one cache entry.
      this.bytes -= this.ranges.get(key)?.length ?? 0;
      this.ranges.delete(key); this.ranges.set(key, bytes); this.bytes += bytes.length;
      // Empty and tiny ranges still retain locator metadata and must have a
      // count bound independent of their decoded byte weight.
      while (this.bytes > this.byteLimit || this.ranges.size > ContentCache.RANGE_LIMIT) {
        const oldest = this.ranges.keys().next().value!;
        this.bytes -= this.ranges.get(oldest)!.length; this.ranges.delete(oldest);
      }
      return { bytes: bytes.slice(), next: chunk.next_offset ?? undefined };
    } finally { this.active--; }
  }
  clear() { this.ranges.clear(); this.bytes = 0; }
}

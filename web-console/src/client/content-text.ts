import type { ContentRef } from '../../../protocol/app-server/v57';
import type { ContentCache } from '../../../protocol/app-server/content-cache';

export async function readContentText(cache: ContentCache, reference: ContentRef, current: () => boolean) {
  const decoder = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }), parts: string[] = [];
  let offset: number | undefined = 0;
  do {
    if (!current()) throw Error('Content view was replaced.');
    const chunk = await cache.read(reference, offset, current);
    parts.push(reference.representation === 'binary_bytes' ? Array.from(chunk.bytes, byte => String.fromCharCode(byte)).join('') : decoder.decode(chunk.bytes, { stream: chunk.next !== undefined }));
    offset = chunk.next;
  } while (offset !== undefined);
  return reference.representation === 'binary_bytes' ? btoa(parts.join('')) : parts.join('');
}

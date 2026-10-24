import type { CheckpointPage, CheckpointPageItem, CheckpointValueChunk } from './v57.ts';
/** Typed u64 quantities arrive as decimal strings. Opaque integers that cannot
 * survive JavaScript parsing stay in the original byte view and cannot be edited
 * through a rounded object. No scalar coercion changes their source semantics. */
export function parseObservedValue(text: string): unknown {
  return JSON.parse(text, (_key, value: unknown) => {
    if (typeof value === 'number' && Number.isInteger(value) && !Number.isSafeInteger(value)) {
      throw new Error('Content requires an exact byte view; its integer cannot be safely edited.');
    }
    return value;
  });
}
export interface DomainReader {
  page(previous: CheckpointPage, offset: number): Promise<CheckpointPage>;
  value(page: CheckpointPage, item: CheckpointPageItem, offset: number): Promise<CheckpointValueChunk>;
  current(): boolean;
}
/** Read one selected original value, rather than its finite inline summary. */
async function originalValue(page: CheckpointPage, item: CheckpointPageItem, read: DomainReader, loaded: number) {
  let offset = 0, total: number | undefined, representation: 'utf8_text'|'canonical_json_v1'|undefined; const chunks: Uint8Array[] = [];
  while (total === undefined || offset < total) {
    const chunk = await read.value(page, item, offset);
    if (!read.current()) throw new Error('Domain observation was replaced.');
    if (JSON.stringify(chunk.cut) !== JSON.stringify(page.read_cut) || chunk.domain !== page.domain || chunk.item_offset !== Number(item.offset) || chunk.path.length !== 0 || chunk.offset !== offset || !Number.isSafeInteger(chunk.readable_end) || loaded + chunk.readable_end > 16 * 1024 * 1024 || (total !== undefined && total !== chunk.readable_end)) throw new Error('Domain range identity or frontier mismatch.');
    if(chunk.representation!=='utf8_text'&&chunk.representation!=='canonical_json_v1'||representation!==undefined&&representation!==chunk.representation)throw new Error('Domain representation changed.');
    representation=chunk.representation;
    const end = chunk.readable_end as number; total = end;
    if(chunk.bytes_base64.length>87_384 || chunk.bytes_base64.length%4!==0 || chunk.bytes_base64.length/4*3-(chunk.bytes_base64.endsWith('==')?2:chunk.bytes_base64.endsWith('=')?1:0)>65_536)throw new Error('Encoded native range exceeds the decoded byte bound.');
    const bytes = Uint8Array.from(atob(chunk.bytes_base64), value => value.charCodeAt(0));
    if (bytes.length > 65_536 || (offset < end && !bytes.length) || offset + bytes.length > end || (chunk.next_offset ?? null) !== (offset + bytes.length < end ? offset + bytes.length : null)) throw new Error('Domain range did not advance exactly.');
    offset += bytes.length; chunks.push(bytes);
  }
  const data = new Uint8Array(total!); let position = 0;
  for (const bytes of chunks) { data.set(bytes, position); position += bytes.length; }
  const text=new TextDecoder('utf-8',{fatal:true,ignoreBOM:true}).decode(data);
  const value = representation==='utf8_text'?text:parseObservedValue(text);
  // Pending summaries retain receipt-owned sequence/revision; the deferred
  // native value is the UserMessage, not a second inbound identity.
  return { value: page.domain === 'inbound.pending' && item.value != null
    ? { ...(item.value as object), message: value } : value, bytes: total! };
}
export async function domainOriginalValue<T>(page: CheckpointPage, read: DomainReader): Promise<T> {
  if (!read.current()) throw new Error('Domain observation was replaced.');
  if (page.items.length !== 1 || page.next_offset != null) throw new Error('Expected one authoritative domain value.');
  return (await originalValue(page, page.items[0]!, read, 0)).value as T;
}
/** One native collection page. Continuation replaces this finite cache page. */
export async function domainItems<T>(page: CheckpointPage, read: DomainReader): Promise<T[]> {
  if (!read.current()) throw new Error('Domain observation was replaced.');
  if (page.items.length > 32) throw new Error('Domain page exceeds its item bound.');
  const result: T[] = []; let loaded=0;
  for (const item of page.items) {
    if (item.value != null && !(page.domain === 'inbound.pending' && item.deferred)) { loaded+=new TextEncoder().encode(JSON.stringify(item.value)).length; if(loaded>16*1024*1024)throw new Error('Domain byte cache is full.'); result.push(item.value as T); continue; }
    const original = await originalValue(page, item, read, loaded);
    loaded += original.bytes;
    result.push(original.value as T);
  }
  return result;
}
export async function domainValue<T>(page: CheckpointPage, read: DomainReader): Promise<T> {
  const items = await domainItems<T>(page, read);
  if (items.length !== 1 || page.next_offset != null) throw new Error('Expected one authoritative domain value.');
  return items[0]!;
}

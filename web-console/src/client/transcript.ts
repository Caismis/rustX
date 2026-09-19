import type { RuntimeClientTranscriptEntry, RuntimeClientTranscriptPage } from '../../../protocol/app-server/v12';

export const HISTORY_LIMIT = 512;
export const HISTORY_MAX_BYTES = 8 * 1024 * 1024;
export const HISTORY_PAGE_SIZE = 64;
/** A contiguous, replaceable durable read window. Never contains live events. */
export interface TranscriptCache {
  page: RuntimeClientTranscriptPage;
  epoch: number;
  loading?: boolean;
  error?: string;
}
export function entryIdentity(entry: RuntimeClientTranscriptEntry): string {
  const item = entry.item;
  return item.type === 'message' ? `message:${item.message.id}`
    : item.type === 'publication_audit' ? `publication:${entry.cursor}` : `interaction:${item.event_id}`;
}
function merge(older: RuntimeClientTranscriptEntry[], newer: RuntimeClientTranscriptEntry[]) {
  const entries = new Map(older.map(entry => [entry.cursor, entry]));
  for (const entry of newer) entries.set(entry.cursor, entry);
  // Transcript positions are specified numeric durable order, never opaque IDs.
  return [...entries.values()].sort((a, b) => BigInt(a.cursor) < BigInt(b.cursor) ? -1 : 1);
}
export function replaceTranscript(page: RuntimeClientTranscriptPage, previous?: TranscriptCache): TranscriptCache {
  return { page, epoch: (previous?.epoch ?? 0) + 1 };
}
export function refreshTranscript(previous: TranscriptCache | undefined, page: RuntimeClientTranscriptPage): TranscriptCache {
  if (!previous) return replaceTranscript(page);
  const overlap = (previous.page.entries ?? []).some(old => (page.entries ?? []).some(entry => entry.cursor === old.cursor && entryIdentity(entry) === entryIdentity(old)));
  if (!overlap) return replaceTranscript(page, previous);
  // Mutable native Tool read projections cannot be retained indefinitely outside
  // the fresh page. Rebase rather than freeze a formerly assembled/running call
  // after its authoritative terminal record has moved beyond this window.
  const fresh = new Set((page.entries ?? []).map(entryIdentity));
  if ((previous.page.entries ?? []).some(entry => !fresh.has(entryIdentity(entry)) && (entry.response_pending || entry.tool_calls?.some(tool => tool.state.type !== 'settled')))) {
    return { ...replaceTranscript(page, previous), error: 'History was refreshed to reread unresolved native responses or Tools. Load earlier for their current results.' };
  }
  const entries = merge(previous.page.entries ?? [], page.entries ?? []);
  if (entries.length > HISTORY_LIMIT || JSON.stringify(entries).length * 2 > HISTORY_MAX_BYTES) return { ...replaceTranscript(page, previous), error: 'History read window reached its bound and was replaced with the current page.' };
  return { ...previous, page: { ...page, entries, next_cursor: previous.page.next_cursor } };
}
export function prependTranscript(cache: TranscriptCache, page: RuntimeClientTranscriptPage): TranscriptCache {
  const entries = merge(page.entries ?? [], cache.page.entries ?? []);
  if (entries.length > HISTORY_LIMIT || JSON.stringify(entries).length * 2 > HISTORY_MAX_BYTES) throw new Error('History window is full. Return to latest to load another window.');
  return { ...cache, loading: false, error: undefined, page: { ...cache.page, entries, next_cursor: page.next_cursor } };
}

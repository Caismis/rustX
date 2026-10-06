import type { RuntimeClientTranscriptEntry, RuntimeClientTranscriptPage } from '../../../protocol/app-server/v36';

export const HISTORY_PAGE_SIZE = 64;
/** One contiguous durable read window from its oldest loaded page through the
 * live tail, as Harness's Chat window: older pages only ever prepend. Never
 * contains live events. */
export interface TranscriptCache {
  page: RuntimeClientTranscriptPage;
  epoch: number;
  loading?: boolean;
  error?: string;
}
export function entryIdentity(entry: RuntimeClientTranscriptEntry): string {
  const item = entry.item;
  return item.type === 'message' ? `message:${item.message.id}`
    : item.type === 'attempt_terminal' ? `attempt:${item.turn.conversation_id}:${item.turn.attempt_id}`
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
  return { ...previous, page: { ...page, entries, next_cursor: previous.page.next_cursor } };
}
export const turnKey = (id: { conversation_id: string; attempt_id: string }) => JSON.stringify([id.conversation_id, id.attempt_id]);
export const turnAnchor = (id: { conversation_id: string; attempt_id: string }) => `turn:${turnKey(id)}`;
export function prependTranscript(cache: TranscriptCache, page: RuntimeClientTranscriptPage): TranscriptCache {
  const entries = merge(page.entries ?? [], cache.page.entries ?? []);
  return { ...cache, loading: false, error: undefined, page: { ...cache.page, entries, next_cursor: page.next_cursor } };
}

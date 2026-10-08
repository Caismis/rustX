import type { ConversationReadCut, ConversationWindow, RuntimeClientTranscriptEntry, RuntimeClientTranscriptPage } from '../../../protocol/app-server/v38';

export const HISTORY_PAGE_SIZE = 64;
export const HISTORY_LIMIT = 256;
export const HISTORY_MAX_BYTES = 8 * 1024 * 1024;
/** Finite presentation window. The runtime snapshot independently owns the live tail. */
export interface TranscriptCache {
  page: RuntimeClientTranscriptPage;
  epoch: number;
  window?: ConversationWindow;
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
  const entries = bounded(page.entries ?? [], false);
  return { page: { ...page, entries, next_cursor: entries.length < (page.entries?.length ?? 0) ? entries[0]?.cursor : page.next_cursor }, epoch: (previous?.epoch ?? 0) + 1 };
}
export function refreshTranscript(previous: TranscriptCache | undefined, page: RuntimeClientTranscriptPage): TranscriptCache {
  if (!previous) return replaceTranscript(page);
  if (previous.window) return previous;
  const overlap = (previous.page.entries ?? []).some(old => (page.entries ?? []).some(entry => entry.cursor === old.cursor && entryIdentity(entry) === entryIdentity(old)));
  if (!overlap) return replaceTranscript(page, previous);
  // Mutable native Tool read projections cannot be retained indefinitely outside
  // the fresh page. Rebase rather than freeze a formerly assembled/running call
  // after its authoritative terminal record has moved beyond this window.
  const fresh = new Set((page.entries ?? []).map(entryIdentity));
  if ((previous.page.entries ?? []).some(entry => !fresh.has(entryIdentity(entry)) && (entry.response_pending || entry.tool_calls?.some(tool => tool.state.type !== 'settled')))) {
    return { ...replaceTranscript(page, previous), error: 'History was refreshed to reread unresolved native responses or Tools. Load earlier for their current results.' };
  }
  const all = merge(previous.page.entries ?? [], page.entries ?? []);
  const entries = bounded(all, false);
  if (entries.length < all.length) return replaceTranscript({ ...page, entries, next_cursor: entries[0]?.cursor }, previous);
  return { ...previous, page: { ...page, entries, next_cursor: previous.page.next_cursor } };
}
export const turnKey = (id: { conversation_id: string; attempt_id: string }) => JSON.stringify([id.conversation_id, id.attempt_id]);
export const turnAnchor = (id: { conversation_id: string; attempt_id: string }) => `turn:${turnKey(id)}`;
export function prependTranscript(cache: TranscriptCache, page: RuntimeClientTranscriptPage): TranscriptCache {
  const entries = bounded(merge(page.entries ?? [], cache.page.entries ?? []), true);
  return { ...cache, loading: false, error: undefined, page: { ...cache.page, entries, next_cursor: page.next_cursor } };
}

/** Bound retained UTF-16 storage as well as entry count; never build a conversation-sized cache. */
function bounded(entries: RuntimeClientTranscriptEntry[], oldest: boolean) {
  let bytes = 0;
  const kept: RuntimeClientTranscriptEntry[] = [];
  for (let index = oldest ? 0 : entries.length - 1; index >= 0 && index < entries.length; index += oldest ? 1 : -1) {
    const entry = entries[index], size = JSON.stringify(entry).length * 2;
    if (kept.length === HISTORY_LIMIT || bytes + size > HISTORY_MAX_BYTES) break;
    kept.push(entry); bytes += size;
  }
  return oldest ? kept : kept.reverse();
}
export function installTranscriptWindow(window: ConversationWindow, previous?: TranscriptCache): TranscriptCache {
  const entries = window.page.entries ?? [];
  if (entries.length > HISTORY_PAGE_SIZE || JSON.stringify(entries).length * 2 > HISTORY_MAX_BYTES) throw new Error('Native history window exceeds the browser reading bound.');
  return { ...replaceTranscript(window.page, previous), window };
}

export function extendTranscriptWindow(window: ConversationWindow, previous: TranscriptCache, older: boolean): TranscriptCache {
  installTranscriptWindow(window); // Validate each native response before merging.
  const existing = (previous.page.entries ?? []).filter(entry => BigInt(entry.cursor) <= BigInt(window.cut.transcript));
  const all = merge(older ? window.page.entries ?? [] : existing, older ? existing : window.page.entries ?? []);
  const entries = bounded(all, older);
  const clipped = entries.length < all.length;
  const page = { ...window.page, entries, next_cursor: older ? window.page.next_cursor : clipped ? entries[0]?.cursor : previous.page.next_cursor };
  return { ...previous, page, loading: false, error: undefined, window: { ...window, page,
    newer_cursor: older ? clipped ? entries.at(-1)?.cursor : previous.window?.newer_cursor ?? null : window.newer_cursor,
  } };
}

/** Combine finite historical presentation and the independently current native tail.
 * Overlap is replaced by current native entries, never duplicated. */
export function transcriptPresentation(history: TranscriptCache | undefined, live: RuntimeClientTranscriptPage) {
  if (!history) return live.entries ?? [];
  return history.window ? merge(history.page.entries ?? [], live.entries ?? []) : history.page.entries ?? [];
}

export function sameReadCut(a: ConversationReadCut, b: ConversationReadCut) {
  return a.conversation_id === b.conversation_id && a.journal === b.journal
    && a.transcript === b.transcript && a.mutation_revision === b.mutation_revision;
}

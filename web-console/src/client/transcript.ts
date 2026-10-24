import { attemptOwnerKey } from '../../../protocol/app-server/attempt-identity';
import type { ContentBinding, ConversationReadCut, ConversationWindow, ObservedTurn, ObservedTurnPage, RuntimeClientTranscriptEntry, RuntimeClientTranscriptPage } from '../../../protocol/app-server/v57';
import { messageDescriptor } from '../../../protocol/app-server/message-descriptor';
import { contentRefKey } from '../../../protocol/app-server/content-identity';

export const HISTORY_PAGE_SIZE = 32;
// Native Turn directory pages have their own count bound.
export const TURN_PAGE_SIZE = 64;
/** A gesture retains the exact outline cut; ordinal zero is only a local live anchor. */
export type TurnSelection = { turn: ObservedTurn; cut?: ConversationReadCut } | number;
/** Wire counts remain exact; native navigation coordinates and viewport geometry are safe integers. */
export function turnPageCount(page: ObservedTurnPage): number {
  if(typeof page.total!=='string'||page.total.length>20||!/^(0|[1-9][0-9]*)$/.test(page.total))throw new Error('Native Turn directory total is unavailable.');
  const total = BigInt(page.total);
  if (total < 0n || total > BigInt(Number.MAX_SAFE_INTEGER)) throw new Error(`Native Turn directory is unavailable at its exact total ${page.total}.`);
  return Number(total);
}
export const HISTORY_PAGE_MAX_BYTES = 8 * 1024 * 1024;
/** Loaded reading history. Native requests, rather than the retained history, are bounded. */
export interface TranscriptCache {
  page: RuntimeClientTranscriptPage;
  epoch: number;
  window?: ConversationWindow;
  loading?: boolean;
  /** Passive initial hydration must preserve tail following, unlike a reader's prepend. */
  initialLoading?: boolean;
  error?: string;
}
export function entryIdentity(entry: RuntimeClientTranscriptEntry): string {
  const item = entry.item;
  return item.type === 'message' ? `message:${item.message.id}`
    : item.type === 'attempt_terminal' ? `attempt:${attemptOwnerKey(item.turn)}`
    : item.type === 'publication_audit' ? `publication:${entry.cursor}` : `interaction:${item.event_id}`;
}
function merge(older: RuntimeClientTranscriptEntry[], newer: RuntimeClientTranscriptEntry[]) {
  const entries = new Map(older.map(entry => [entry.cursor, entry]));
  for (const entry of newer) entries.set(entry.cursor, entry);
  // Transcript positions are specified numeric durable order, never opaque IDs.
  return [...entries.values()].sort((a, b) => BigInt(a.cursor) < BigInt(b.cursor) ? -1 : 1);
}
/** One native transcript position has exactly one inline or deferred representation. */
function pageRows(page: RuntimeClientTranscriptPage) {
  return new Map([
    ...(page.deferred_entries ?? []).map(item => [item.offset, messageDescriptor(item.value)?.identity != null
      ? `message:${messageDescriptor(item.value)!.identity}` : item.identity] as const),
    ...(page.entries ?? []).map(entry => [entry.cursor, entryIdentity(entry)] as const),
  ]);
}
function mergePages(older: RuntimeClientTranscriptPage, newer: RuntimeClientTranscriptPage): RuntimeClientTranscriptPage {
  const fresh = pageRows(newer);
  const entries = merge((older.entries ?? []).filter(entry => !fresh.has(entry.cursor)), newer.entries ?? []);
  const deferred = new Map((older.deferred_entries ?? []).filter(item => !fresh.has(item.offset)).map(item => [item.offset, item]));
  for (const item of newer.deferred_entries ?? []) deferred.set(item.offset, item);
  for (const entry of entries) deferred.delete(entry.cursor);
  const collections = new Map((older.collections ?? []).map(frontier => [frontier.domain, frontier]));
  for (const frontier of newer.collections ?? []) collections.set(frontier.domain, frontier);
  return { ...newer, entries, deferred_entries: [...deferred.values()].sort((a,b) => BigInt(a.offset) < BigInt(b.offset) ? -1 : 1),
    collections: [...collections.values()], content: pageBindings(entries, [older, newer]) };
}
/** Preserve exact source references while remapping only local loaded row paths. */
function pageBindings(entries: RuntimeClientTranscriptEntry[], pages: RuntimeClientTranscriptPage[]): ContentBinding[] {
  const bindings = new Map<string,ContentBinding>();
  for (const page of pages) for (const binding of page.content ?? []) {
    const path=[...binding.path],position=path.indexOf('entries');
    if(position>=0) {
      const row=page.entries?.[Number(path[position+1])];
      const loaded=row && entries.findIndex(entry=>entry.cursor===row.cursor && entryIdentity(entry)===entryIdentity(row));
      if(loaded===undefined || loaded<0)continue;
      path[position+1]=String(loaded);
    }
    bindings.set(contentRefKey(binding.content_ref),{...binding,path});
  }
  return [...bindings.values()];
}
const joinedContent=new WeakMap<ContentBinding[],WeakMap<ContentBinding[],ContentBinding[]>>();
const emptyContent:ContentBinding[]=[];
export function transcriptContent(history: TranscriptCache|undefined, current: ContentBinding[]) {
  const historical=history?.page.content??emptyContent;
  let byHistory=joinedContent.get(current);if(!byHistory){byHistory=new WeakMap();joinedContent.set(current,byHistory);}
  const retained=byHistory.get(historical);if(retained)return retained;
  const bindings=new Map((history?.page.content ?? []).map(binding=>[contentRefKey(binding.content_ref),binding]));
  for(const binding of current)bindings.set(contentRefKey(binding.content_ref),binding);
  const result=[...bindings.values()];byHistory.set(historical,result);return result;
}
export function replaceTranscript(page: RuntimeClientTranscriptPage, previous?: TranscriptCache): TranscriptCache {
  return { page, epoch: (previous?.epoch ?? 0) + 1 };
}
export function refreshTranscript(previous: TranscriptCache | undefined, page: RuntimeClientTranscriptPage): TranscriptCache {
  if (!previous) return replaceTranscript(page);
  if (previous.window) return previous;
  const fresh = pageRows(page);
  const overlap = [...pageRows(previous.page)].some(([cursor, identity]) => fresh.get(cursor) === identity);
  if (!overlap) return replaceTranscript(page, previous);
  // Keep the reader's rows in place. Mutable projections outside this head are
  // reread by identity in the background, never by discarding loaded history.
  return { ...previous, page: { ...mergePages(previous.page, page), next_cursor: previous.page.next_cursor } };
}
export function transcriptRepairs(cache: TranscriptCache | undefined, live: RuntimeClientTranscriptPage) {
  if (!cache || cache.window) return [];
  const fresh = new Set((live.entries ?? []).map(entryIdentity));
  return (cache.page.entries ?? []).filter(entry => entry.item.type === 'message' && !fresh.has(entryIdentity(entry))
    && (entry.response_pending || entry.tool_calls?.some(tool => tool.state.type !== 'settled')));
}
/** Replace only the requested row, retaining native source bindings and viewport identity. */
export function repairTranscript(cache: TranscriptCache, window: ConversationWindow, original: RuntimeClientTranscriptEntry): TranscriptCache {
  installTranscriptWindow(window);
  const entry = window.page.entries?.find(entry => entry.cursor === original.cursor && entryIdentity(entry) === entryIdentity(original));
  if (!entry || window.target_cursor !== original.cursor) throw new Error('Native transcript refresh returned a different message.');
  const entries = (cache.page.entries ?? []).map(row => entryIdentity(row) === entryIdentity(original) && row.cursor === original.cursor ? entry : row);
  const patch = { ...window.page, entries: [entry], content: pageBindings([entry], [window.page]) };
  return { ...cache, page: { ...cache.page, entries, content: pageBindings(entries, [cache.page, patch]) } };
}
export const turnKey = attemptOwnerKey;
export const turnAnchor = (id: Parameters<typeof attemptOwnerKey>[0]) => `turn:${turnKey(id)}`;
export function prependTranscript(cache: TranscriptCache, page: RuntimeClientTranscriptPage): TranscriptCache {
  return { ...cache, loading: false, error: undefined, page: { ...mergePages(page, cache.page), next_cursor: page.next_cursor } };
}

export function installTranscriptWindow(window: ConversationWindow, previous?: TranscriptCache): TranscriptCache {
  const entries = window.page.entries ?? [];
  if (entries.length > HISTORY_PAGE_SIZE || JSON.stringify(entries).length * 2 > HISTORY_PAGE_MAX_BYTES) throw new Error('Native history page exceeds the browser reading bound.');
  return { ...replaceTranscript(window.page, previous), window };
}

export function extendTranscriptWindow(window: ConversationWindow, previous: TranscriptCache, older: boolean): TranscriptCache {
  installTranscriptWindow(window); // Validate each native response before merging.
  const existing = { ...previous.page, entries: (previous.page.entries ?? []).filter(entry => BigInt(entry.cursor) <= BigInt(window.cut.transcript)),
    deferred_entries: (previous.page.deferred_entries ?? []).filter(item => BigInt(item.offset) <= BigInt(window.cut.transcript)) };
  const page = { ...mergePages(existing, window.page), next_cursor: older ? window.page.next_cursor : previous.page.next_cursor };
  return { ...previous, page, loading: false, error: undefined, window: { ...window, page,
    newer_cursor: older ? previous.window?.newer_cursor ?? null : window.newer_cursor,
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
    && a.transcript === b.transcript && a.mutation_revision === b.mutation_revision
    && a.ledger_through === b.ledger_through && a.surface_revision === b.surface_revision
    && a.transcript_total === b.transcript_total && a.pending_through === b.pending_through && a.pending_total === b.pending_total;
}

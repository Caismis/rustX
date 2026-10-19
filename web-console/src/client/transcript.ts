import { attemptOwnerKey } from '../../../protocol/app-server/attempt-identity';
import type { ContentBinding, ConversationReadCut, ConversationWindow, ObservedTurn, ObservedTurnPage, RuntimeClientTranscriptEntry, RuntimeClientTranscriptPage } from '../../../protocol/app-server/v51';
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
    : item.type === 'attempt_terminal' ? `attempt:${attemptOwnerKey(item.turn)}`
    : item.type === 'publication_audit' ? `publication:${entry.cursor}` : `interaction:${item.event_id}`;
}
function merge(older: RuntimeClientTranscriptEntry[], newer: RuntimeClientTranscriptEntry[]) {
  const entries = new Map(older.map(entry => [entry.cursor, entry]));
  for (const entry of newer) entries.set(entry.cursor, entry);
  // Transcript positions are specified numeric durable order, never opaque IDs.
  return [...entries.values()].sort((a, b) => BigInt(a.cursor) < BigInt(b.cursor) ? -1 : 1);
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
  const entries = bounded(page.entries ?? [], false);
  return { page: { ...page, entries, content:pageBindings(entries,[page]), next_cursor: entries.length < (page.entries?.length ?? 0) ? entries[0]?.cursor : page.next_cursor }, epoch: (previous?.epoch ?? 0) + 1 };
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
  return { ...previous, page: { ...page, entries, content:pageBindings(entries,[previous.page,page]), next_cursor: previous.page.next_cursor } };
}
export const turnKey = attemptOwnerKey;
export const turnAnchor = (id: Parameters<typeof attemptOwnerKey>[0]) => `turn:${turnKey(id)}`;
export function prependTranscript(cache: TranscriptCache, page: RuntimeClientTranscriptPage): TranscriptCache {
  const entries = bounded(merge(page.entries ?? [], cache.page.entries ?? []), true);
  return { ...cache, loading: false, error: undefined, page: { ...cache.page, entries, content:pageBindings(entries,[cache.page,page]), next_cursor: page.next_cursor } };
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
  const page = { ...window.page, entries, content:pageBindings(entries,[previous.page,window.page]), next_cursor: older ? window.page.next_cursor : clipped ? entries[0]?.cursor : previous.page.next_cursor };
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
    && a.transcript === b.transcript && a.mutation_revision === b.mutation_revision
    && a.ledger_through === b.ledger_through && a.surface_revision === b.surface_revision
    && a.transcript_total === b.transcript_total && a.pending_through === b.pending_through && a.pending_total === b.pending_total;
}

import type { TraceEntry, TracePage, TraceLifecycle } from '../../../protocol/app-server/v3';

export const TRACE_LIMIT = 512;
export const TRACE_MAX_BYTES = 4 * 1024 * 1024;
export const TRACE_PAGE_SIZE = 32;
/** Independent replaceable read domain. No events or execution decisions. */
export interface TraceCache { page: TracePage; epoch: number; selection?: TraceEntry; loading?: boolean; error?: string }
export function replaceTrace(page: TracePage, previous?: TraceCache): TraceCache {
  return { page, epoch: (previous?.epoch ?? 0) + 1 };
}
function merge(older: TraceEntry[], newer: TraceEntry[]) {
  const oldPositions = new Map(older.map((entry, index) => [entry.id, index]));
  const merged: TraceEntry[] = [];
  let pending: TraceEntry[] = [];
  let offset = 0;
  // Shared stable identities anchor the two server-ordered inputs. A fresh
  // page can reveal a prefix before the already-loaded tail (e.g. byte limits
  // changed); appending every unfamiliar row would put that prefix out of order.
  // No opaque cursor, Turn ID or Journal sequence is interpreted here.
  for (const entry of newer) {
    const anchor = oldPositions.get(entry.id);
    if (anchor === undefined) { pending.push(entry); continue; }
    merged.push(...older.slice(offset, anchor), ...pending, entry);
    pending = [];
    offset = anchor + 1;
  }
  return [...merged, ...older.slice(offset), ...pending];
}
function bounded(entries: TraceEntry[]) { return entries.length <= TRACE_LIMIT && JSON.stringify(entries).length * 2 <= TRACE_MAX_BYTES; }
export function traceInterests(cache?: TraceCache) {
  const entries = cache ? [...(cache.selection ? [cache.selection] : []), ...cache.page.entries] : [];
  return [...new Map(entries.map(entry => [entry.id, entry.position])).values()].slice(0, TRACE_LIMIT);
}
export function selectTrace(cache: TraceCache, id?: string): TraceCache {
  return { ...cache, selection: cache.page.entries.find(entry => entry.id === id) ?? (cache.selection?.id === id ? cache.selection : undefined) };
}
export function refreshTrace(previous: TraceCache | undefined, page: TracePage, updates: TraceLifecycle[] = []): TraceCache {
  if (!previous) return replaceTrace(page);
  const repairs = new Map(updates.map(update => [update.id, update]));
  const repair = (entry: TraceEntry) => {
    const update = repairs.get(entry.id);
    return update ? { ...entry, ...update, request: entry.request && update.request ? { ...entry.request, ...update.request } : entry.request } : entry;
  };
  const selection = previous.selection ? repair(page.entries.find(entry => entry.id === previous.selection!.id) ?? previous.selection) : undefined;
  if (previous.page.entries.length === 0) return { ...previous, page, selection };
  const overlap = new Set(previous.page.entries.map(entry => entry.id));
  // A flat ledger describes one server-proven interval. No shared anchor
  // means unknown intervening history, never permission to concatenate.
  if (!page.entries.some(entry => overlap.has(entry.id))) return { ...replaceTrace(page, previous), selection };
  const entries = merge(previous.page.entries.map(repair), page.entries);
  if (!bounded(entries)) return { ...replaceTrace(page, previous), selection, error: 'Trace window reached its bound; showing latest.' };
  return { ...previous, selection, page: { entries, next_cursor: previous.page.next_cursor } };
}
export function prependTrace(previous: TraceCache, page: TracePage): TraceCache {
  const entries = merge(page.entries, previous.page.entries);
  if (!bounded(entries)) throw new Error('Trace window is full. Return to latest first.');
  return { ...previous, loading: false, error: undefined, page: { entries, next_cursor: page.next_cursor } };
}

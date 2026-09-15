import type { TraceEntry, TracePage } from '../../../protocol/app-server/v3';

export const TRACE_LIMIT = 512;
export const TRACE_MAX_BYTES = 4 * 1024 * 1024;
export const TRACE_PAGE_SIZE = 32;
/** Independent replaceable read domain. No events or execution decisions. */
export interface TraceCache { page: TracePage; epoch: number; loading?: boolean; error?: string }
export function replaceTrace(page: TracePage, previous?: TraceCache): TraceCache {
  return { page, epoch: (previous?.epoch ?? 0) + 1 };
}
function merge(older: TraceEntry[], newer: TraceEntry[]) {
  const byId = new Map(older.map(entry => [entry.id, entry]));
  for (const entry of newer) byId.set(entry.id, entry);
  // Both pages are server-ordered contiguous slices. Never interpret an
  // opaque Trace cursor, native Turn ID, Request ID or event sequence.
  const ids = new Set(older.map(entry => entry.id));
  return [...older.map(entry => byId.get(entry.id)!), ...newer.filter(entry => !ids.has(entry.id))];
}
function bounded(entries: TraceEntry[]) { return entries.length <= TRACE_LIMIT && JSON.stringify(entries).length * 2 <= TRACE_MAX_BYTES; }
export function refreshTrace(previous: TraceCache | undefined, page: TracePage): TraceCache {
  if (!previous) return replaceTrace(page);
  const current = new Set(page.entries.map(entry => entry.id));
  const overlap = previous.page.entries.some(entry => current.has(entry.id));
  // An unresolved record outside the new tail could have settled. Retain no
  // stale certainty: replace the bounded window and fence pending history.
  const unresolvedOutside = previous.page.entries.some(entry => !current.has(entry.id) && ['running', 'incomplete', 'pending', 'cancelling', 'settling', 'waiting'].includes(entry.state));
  if (!overlap || unresolvedOutside) return replaceTrace(page, previous);
  const entries = merge(previous.page.entries, page.entries);
  if (!bounded(entries)) return { ...replaceTrace(page, previous), error: 'Trace window reached its bound; showing latest.' };
  return { ...previous, page: { entries, next_cursor: previous.page.next_cursor } };
}
export function prependTrace(previous: TraceCache, page: TracePage): TraceCache {
  const entries = merge(page.entries, previous.page.entries);
  if (!bounded(entries)) throw new Error('Trace window is full. Return to latest first.');
  return { ...previous, loading: false, error: undefined, page: { entries, next_cursor: page.next_cursor } };
}

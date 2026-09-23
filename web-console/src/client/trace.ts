import type { TraceDetail, TracePage, TraceRecord, TraceLifecycle } from '../../../protocol/app-server/v18';

/** Most summary records the browser retains for one Trace interval. */
export const TRACE_LIMIT = 512;
/** Estimated encoded UTF-16 ceiling of the retained summary window. */
export const TRACE_MAX_BYTES = 4 * 1024 * 1024;
/** Records requested per older page. */
export const TRACE_PAGE_SIZE = 32;
/** Most record details retained at once; selection is one, neighbours warm. */
export const TRACE_DETAIL_LIMIT = 8;

/**
 * One fetched detail, fenced by the identity and epoch it was read for.
 *
 * Fencing is the whole point of the wrapper: a detail read is asynchronous,
 * so a reply can arrive after the reader selected another record, after the
 * window rebased onto a new interval, or after the browser reattached to a
 * different Session. Recording what the reply was asked for lets every one of
 * those cases be rejected instead of attaching the wrong detail to a record.
 */
export interface TraceDetailEntry {
  readonly epoch: number;
  readonly detail?: TraceDetail;
  readonly loading?: boolean;
  readonly error?: string;
}

/**
 * The browser's replaceable Trace read domain.
 *
 * It holds a finite window of server-ordered summary records describing one
 * contiguous server-proven interval, the details fetched for inspected
 * records, and the current selection. It folds no events, derives no
 * execution semantics, and is discarded wholesale whenever the server says
 * the interval it described no longer applies.
 */
export interface TraceCache {
  page: TracePage;
  epoch: number;
  /** Retained separately so a rebase cannot close the open inspector. */
  selection?: TraceRecord;
  details: Readonly<Record<string, TraceDetailEntry>>;
  loading?: boolean;
  error?: string;
}

export function replaceTrace(page: TracePage, previous?: TraceCache): TraceCache {
  return { page, epoch: (previous?.epoch ?? 0) + 1, details: {} };
}

function merge(older: TraceRecord[], newer: TraceRecord[]) {
  const oldPositions = new Map(older.map((record, index) => [record.id, index]));
  const merged: TraceRecord[] = [];
  let pending: TraceRecord[] = [];
  let offset = 0;
  // Shared stable identities anchor the two server-ordered inputs. A fresh
  // page can reveal a prefix before the already-loaded tail (byte limits can
  // shrink a page), so appending every unfamiliar row would put that prefix
  // out of order. No opaque cursor, Turn ID or Journal sequence is read here.
  for (const record of newer) {
    const anchor = oldPositions.get(record.id);
    if (anchor === undefined) { pending.push(record); continue; }
    merged.push(...older.slice(offset, anchor), ...pending, record);
    pending = [];
    offset = anchor + 1;
  }
  return [...merged, ...older.slice(offset), ...pending];
}

function bounded(records: TraceRecord[]) {
  return records.length <= TRACE_LIMIT && JSON.stringify(records).length * 2 <= TRACE_MAX_BYTES;
}

/** Loaded identities the server should refresh at the next snapshot cut. */
export function traceInterests(cache?: TraceCache) {
  const records = cache ? [...(cache.selection ? [cache.selection] : []), ...cache.page.records] : [];
  return [...new Map(records.map(record => [record.id, record.position])).values()].slice(0, TRACE_LIMIT);
}

/**
 * Retains at most {@link TRACE_DETAIL_LIMIT} details, keeping the selected
 * one. Detail is heavy, so the browser caches a working set rather than the
 * whole window; the selected record is never the entry that gets evicted.
 */
function boundDetails(
  details: Readonly<Record<string, TraceDetailEntry>>,
  ...keep: (string | undefined)[]
): Readonly<Record<string, TraceDetailEntry>> {
  const ids = Object.keys(details);
  if (ids.length <= TRACE_DETAIL_LIMIT) return details;
  const protectedIds = keep.filter((id): id is string => id !== undefined && details[id] !== undefined);
  const retained = [
    ...new Set([...protectedIds, ...ids.filter(id => !protectedIds.includes(id))]),
  ].slice(0, TRACE_DETAIL_LIMIT);
  return Object.fromEntries(retained.map(id => [id, details[id]!]));
}

export function selectTrace(cache: TraceCache, id?: string): TraceCache {
  const selection = cache.page.records.find(record => record.id === id)
    ?? (cache.selection?.id === id ? cache.selection : undefined);
  return { ...cache, selection, details: boundDetails(cache.details, id) };
}

/** Records a detail request so a late or superseded reply can be rejected. */
export function beginTraceDetail(cache: TraceCache, id: string): TraceCache {
  return {
    ...cache,
    details: boundDetails(
      { ...cache.details, [id]: { epoch: cache.epoch, loading: true } },
      cache.selection?.id,
      id,
    ),
  };
}

/**
 * Applies a detail reply only when it still belongs to this cache epoch.
 *
 * A reply whose epoch no longer matches describes a Trace interval the
 * browser has already replaced, so it is dropped rather than attached to a
 * record that happens to share an identity.
 */
export function completeTraceDetail(
  cache: TraceCache,
  id: string,
  epoch: number,
  detail?: TraceDetail,
  error?: string,
): TraceCache {
  if (cache.epoch !== epoch) return cache;
  return {
    ...cache,
    details: boundDetails(
      { ...cache.details, [id]: { epoch, ...(detail ? { detail } : {}), ...(!detail ? { error: error ?? 'Record detail is unavailable at this read cut.' } : {}) } },
      cache.selection?.id,
      id,
    ),
  };
}

export function refreshTrace(previous: TraceCache | undefined, page: TracePage, updates: TraceLifecycle[] = []): TraceCache {
  if (!previous) return replaceTrace(page);
  const repairs = new Map(updates.map(update => [update.id, update]));
  const repair = (record: TraceRecord): TraceRecord => {
    const update = repairs.get(record.id);
    if (!update) return record;
    return {
      ...record,
      state: update.state,
      timing: update.timing,
      message_id: update.message_id,
      attachments: update.attachments,
      truncated: update.truncated,
      request: record.request && update.request ? { ...record.request, ...update.request } : record.request,
      tool: record.tool && update.tool ? { ...record.tool, ...update.tool } : record.tool,
    };
  };
  const selection = previous.selection
    ? repair(page.records.find(record => record.id === previous.selection!.id) ?? previous.selection)
    : undefined;
  if (previous.page.records.length === 0) return { ...previous, page, selection };
  const overlap = new Set(previous.page.records.map(record => record.id));
  // A flat ledger describes one server-proven interval. No shared anchor means
  // unknown intervening history, never permission to concatenate two ranges.
  if (!page.records.some(record => overlap.has(record.id))) {
    return { ...replaceTrace(page, previous), selection };
  }
  const records = merge(previous.page.records.map(repair), page.records);
  const latest = new Map(records.map(record => [record.id, record]));
  if (selection) latest.set(selection.id, selection);
  const old = new Map(previous.page.records.map(record => [record.id, record]));
  if (previous.selection) old.set(previous.selection.id, previous.selection);
  // A repaired record needs detail from the new native read cut.
  const details = Object.fromEntries(Object.entries(previous.details).filter(([id]) =>
    JSON.stringify(old.get(id)) === JSON.stringify(latest.get(id)),
  ));
  if (!bounded(records)) {
    return { ...replaceTrace(page, previous), selection, error: 'Trace window reached its bound; showing latest.' };
  }
  return { ...previous, selection, details, page: { records, next_cursor: previous.page.next_cursor } };
}

export function prependTrace(previous: TraceCache, page: TracePage): TraceCache {
  const records = merge(page.records, previous.page.records);
  if (!bounded(records)) throw new Error('Trace window is full. Return to latest first.');
  return { ...previous, loading: false, error: undefined, page: { records, next_cursor: page.next_cursor } };
}

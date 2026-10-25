export {traceInspectionHead} from '../../../protocol/app-server/trace-inspection';
import {traceInspectionHead} from '../../../protocol/app-server/trace-inspection';
import type {TraceValueSelector} from '../../../protocol/app-server/trace-field';
import type { TraceDetail, TracePage, TraceRecord, TraceLifecycle,CheckpointReadCut,CheckpointPage,CheckpointValueChunk,ContentRef,ContentChunk, Request1 } from '../../../protocol/app-server/v57';

/** Records requested per older page. */
export const TRACE_PAGE_SIZE = 32;
/** Most record details retained at once; selection is one, neighbours warm. */
export const TRACE_DETAIL_LIMIT = 8;
/** Aggregate UTF-8 encoded detail/cache evidence budget. */
export const TRACE_DETAIL_MAX_BYTES = 16 * 1024 * 1024;
const detailWeights = new WeakMap<TraceDetailEntry, number>();
function detailWeight(entry: TraceDetailEntry) {
  let bytes=detailWeights.get(entry);
  if(bytes===undefined){bytes=new TextEncoder().encode(JSON.stringify(entry)).length;detailWeights.set(entry,bytes);}
  return bytes;
}

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
  /** One in-flight read; lifecycle refreshes may mark its result stale without cancelling it. */
  readonly read?: object;
  readonly stale?: boolean;
  readonly content?:import('../../../protocol/app-server/v57').ContentBinding[];
  readonly readContent?:(ref:import('../../../protocol/app-server/v57').ContentRef,offset:number,current:()=>boolean)=>Promise<import('../../../protocol/app-server/v57').ContentChunk>;
  /** A positive finite descriptor is not a complete TraceDetail. */
  readonly page?: CheckpointPage;
  readonly reader?: TraceFieldReader;
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
export interface TraceFieldReader {
 value(cut:CheckpointReadCut,field:TraceValueSelector,offset:number,current:()=>boolean):Promise<CheckpointValueChunk>;
 content(cut:CheckpointReadCut,reference:ContentRef,offset:number,current:()=>boolean):Promise<ContentChunk>;
 page(cut:CheckpointReadCut,domain:string,offset:number,current:()=>boolean):Promise<CheckpointPage>;
}
export interface TraceCache {
  /** A native tool landing retains its historical interval until explicitly released. */
  located?: true;
  page: TracePage;
  /** Captured per represented row, never replaced with the latest checkpoint. */
  readCuts?:Readonly<Record<string,CheckpointReadCut>>;
  fields?:TraceFieldReader;
  epoch: number;
  /** Retained separately so a rebase cannot close the open inspector. */
  selection?: TraceRecord;
  details: Readonly<Record<string, TraceDetailEntry>>;
  loading?: boolean;
  error?: string;
}

function pageCuts(page:TracePage,outerCut?:CheckpointReadCut):Record<string,CheckpointReadCut> {
 const cut=page.read_cut??outerCut;
 return cut?Object.fromEntries(page.records.map(record=>[record.id,cut])):{};
}
function retainedCuts(cuts:Readonly<Record<string,CheckpointReadCut>>,records:readonly TraceRecord[],selection?:TraceRecord) {
 return Object.fromEntries([...records,...(selection?[selection]:[])].flatMap(record=>cuts[record.id]?[[record.id,cuts[record.id]]]:[]));
}
export function replaceTrace(page: TracePage, previous?: TraceCache,outerCut?:CheckpointReadCut): TraceCache {
  return { page,readCuts:pageCuts(page,outerCut),fields:previous?.fields, epoch: (previous?.epoch ?? 0) + 1, details: {} };
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

/** Standalone responses supply their own complete immutable reader scope. */
export function standaloneTraceCut(page:TracePage,conversationId:string,activation?:CheckpointReadCut['activation_id']) {
  const cut=page.read_cut;
  if(!cut||cut.conversation_id!==conversationId||JSON.stringify(cut.activation_id??null)!==JSON.stringify(activation??null))throw new Error('Original Trace page cut is unavailable.');
  return cut;
}

/** A detail header may itself be deferred; that never authorizes whole-item hydration. */
export function inlineTraceDetail(page:CheckpointPage,recordId:string):TraceDetail|undefined {
  const item=page.items[0];
  if(!item||item.deferred)return;
  const value=item.value;
  if(!value||typeof value!=='object'||Array.isArray(value)||!('id' in value)||value.id!==recordId)throw new Error('Trace detail header identity changed.');
  if((!('kind' in value)||value.kind!=='request')||!('request' in value)||value.request==null||'record' in value||'content' in value)throw new Error('Original non-Request Trace requires its finite inspection head.');
  return value as unknown as TraceDetail;
}

/** A deferred header remains read evidence, never an incomplete typed detail. */
export function retainedTraceDetailPage(page:CheckpointPage,id:string):CheckpointPage|undefined {
 const item=page.items[0];
 if(!item?.deferred)return;
 const sequence=/^trace:(0|[1-9][0-9]*)$/.exec(id)?.[1];
 if(!sequence||page.domain!==`trace.records.${sequence}.detail`||page.total!=='1'||page.cursor!==page.read_cut.revision||page.items.length!==1||page.next_offset!=null
  ||item.offset!=='0'||item.identity!==id||!item.value||typeof item.value!=='object'||Array.isArray(item.value)||!('id' in item.value)||item.value.id!==id)throw new Error('Original Trace detail descriptor changed.');
 traceInspectionHead(page,id);
 return page;
}
/** The selected page keeps its own owner and cut, independently of a later Trace head. */
export function bindTraceDetailReader(reader:TraceFieldReader,page:CheckpointPage,current:()=>boolean):TraceFieldReader {
 const valid=(cut:CheckpointReadCut,owned:()=>boolean)=>{
  if(!current()||!owned()||JSON.stringify(cut)!==JSON.stringify(page.read_cut))throw new Error('Original Trace detail view was replaced.');
 };
 return {
  value:async(cut,field,offset,owned)=>{valid(cut,owned);const chunk=await reader.value(cut,field,offset,()=>current()&&owned());valid(cut,owned);return chunk;},
  page:async(cut,domain,offset,owned)=>{valid(cut,owned);const result=await reader.page(cut,domain,offset,()=>current()&&owned());valid(cut,owned);return result;},
  content:async(cut,ref,offset,owned)=>{valid(cut,owned);const chunk=await reader.content(cut,ref,offset,()=>current()&&owned());valid(cut,owned);return chunk;},
 };
}

type TraceInterests = NonNullable<Extract<Request1, {method:'session/trace'}>['params']['records']>;

function appendInterest(batch: TraceInterests, value: string): TraceInterests {
  if (batch.length === 8) throw new Error('Trace refresh exceeds eight interests.');
  return [...batch, value];
}

/** Construct the schema-bounded tuple without widening the admission limit. */
function interestBatch(values: readonly string[]): TraceInterests {
  return values.reduce<TraceInterests>(appendInterest, []);
}

/** Loaded identities the server should refresh at the next snapshot cut. */
export function traceInterestBatches(cache?: TraceCache) {
  const records = cache ? [...(cache.selection ? [cache.selection] : []), ...cache.page.records] : [];
  const interests=[...new Map(records.map(record => [record.id, record.position])).values()];
  return Array.from({length:Math.max(1,Math.ceil(interests.length/8))},(_,index)=>interestBatch(interests.slice(index*8,index*8+8)));
}

export function traceInterests(cache?:TraceCache){return traceInterestBatches(cache)[0]!;}

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
  const protectedIds = keep.filter((id): id is string => id !== undefined && details[id] !== undefined);
  const retained = [
    ...new Set([...protectedIds, ...ids.filter(id => !protectedIds.includes(id))]),
  ];
  let bytes=2;
  const entries: [string, TraceDetailEntry][]=[];
  for(const id of retained){
    if(entries.length===TRACE_DETAIL_LIMIT)break;
    const entry=details[id]!, keyBytes=new TextEncoder().encode(JSON.stringify(id)).length+2, weight=detailWeight(entry)+keyBytes;
    if(weight>TRACE_DETAIL_MAX_BYTES){
      const refusal={epoch:entry.epoch,error:"Record detail exceeds the local observation cache budget."};
      const refusalBytes=detailWeight(refusal)+keyBytes;
      if(bytes+refusalBytes<=TRACE_DETAIL_MAX_BYTES){bytes+=refusalBytes;entries.push([id,refusal]);}
      continue;
    }
    if(bytes+weight>TRACE_DETAIL_MAX_BYTES)continue;
    bytes+=weight;entries.push([id,entry]);
  }
  return Object.fromEntries(entries);
}

export function selectTrace(cache: TraceCache, id?: string): TraceCache {
  const selection = cache.page.records.find(record => record.id === id)
    ?? (cache.selection?.id === id ? cache.selection : undefined);
  const details = { ...cache.details };
  // An explicit selection is a fresh read opportunity after a failed inspection.
  if (id && details[id]?.error) details[id] = { ...details[id], error: undefined, stale: true };
  return { ...cache, selection, readCuts:retainedCuts(cache.readCuts??{},cache.page.records,selection),details: boundDetails(details, id) };
}

/** Records a detail request so a late or superseded reply can be rejected. */
export function beginTraceDetail(cache: TraceCache, id: string): TraceCache {
  return {
    ...cache,
    details: boundDetails(
      { ...cache.details, [id]: { ...cache.details[id], epoch: cache.epoch, read: {}, stale: false, loading: true, error: undefined } },
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
  content?:TraceDetailEntry['content'],
  readContent?:TraceDetailEntry['readContent'],
  page?:CheckpointPage,
  reader?:TraceFieldReader,
): TraceCache {
  if (cache.epoch !== epoch) return cache;
  const previous = cache.details[id];
  const failure = !detail && !page ? error ?? 'Record detail is unavailable at this read cut.' : undefined;
  return {
    ...cache,
    details: boundDetails(
      { ...cache.details, [id]: failure
        ? { ...previous, epoch, read: undefined, loading: false, stale: false, error: failure }
        : { epoch, content, readContent, page, reader, detail, stale: previous?.stale } },
      cache.selection?.id,
      id,
    ),
  };
}

/** Only native content selectors change a detail's read source. Timing, usage,
 * status and Turn placement belong to the outer live row, not its frozen body. */
function contentIdentity(record: TraceRecord | undefined) {
  if (!record) return undefined;
  const request = record.request;
  return JSON.stringify([record.kind, record.has_detail, record.fields, record.collections, record.calls,
    record.message_id, record.attachments, record.tool, record.preview,
    request && [request.request_id, request.request_id_source, request.context_domain, request.context_total,
      request.context_next_offset, request.context_additions, request.system_prompt, request.tool_catalog]]);
}

export function refreshTrace(previous: TraceCache | undefined, page: TracePage, updates: TraceLifecycle[] = [],outerCut?:CheckpointReadCut,lifecycleOnly=false): TraceCache {
  if (!previous) return replaceTrace(page,undefined,outerCut);
  const cut=page.read_cut??outerCut;
  const readCuts={...previous.readCuts,...(lifecycleOnly||previous.located?{}:pageCuts(page,outerCut)),...(cut?Object.fromEntries(updates.map(update=>[update.id,cut])):{})};
  const repairs = new Map(updates.map(update => [update.id, update]));
  const repair = (record: TraceRecord): TraceRecord => {
    const update = repairs.get(record.id);
    if (!update) return record;
    return {
      ...record,
      location: update.location,
      fields:update.fields,collections:update.collections,
      state: update.state,
      timing: update.timing,
      message_id: update.message_id,
      attachments: update.attachments,
      truncated: update.truncated,
      request: record.request && update.request ? { ...record.request, ...update.request } : record.request,
      tool: record.tool && update.tool ? { ...record.tool, ...update.tool } : record.tool,
    };
  };
  if (previous.located||lifecycleOnly) page = { ...previous.page, records: previous.page.records.map(repair) };
  const selection = previous.selection
    ? repair(page.records.find(record => record.id === previous.selection!.id) ?? previous.selection)
    : undefined;
  if (previous.page.records.length === 0) return { ...previous, page, selection,readCuts:retainedCuts(readCuts,page.records,selection) };
  const overlap = new Set(previous.page.records.map(record => record.id));
  // A flat ledger describes one server-proven interval. No shared anchor means
  // unknown intervening history, never permission to concatenate two ranges.
  if (!page.records.some(record => overlap.has(record.id))) {
    return { ...replaceTrace(page, previous,outerCut), selection,readCuts:retainedCuts(readCuts,page.records,selection) };
  }
  const records = merge(previous.page.records.map(repair), page.records);
  const latest = new Map(records.map(record => [record.id, record]));
  if (selection) latest.set(selection.id, selection);
  const old = new Map(previous.page.records.map(record => [record.id, record]));
  if (previous.selection) old.set(previous.selection.id, previous.selection);
  // Keep the original reader while the same native content remains represented.
  // A changed body is refreshed in the background; it never blanks the inspector.
  const unchanged = (id: string) => old.get(id) === latest.get(id) || contentIdentity(old.get(id)) === contentIdentity(latest.get(id));
  for (const id of Object.keys(readCuts)) if (previous.readCuts?.[id] && unchanged(id)) readCuts[id] = previous.readCuts[id];
  const details = Object.fromEntries(Object.entries(previous.details).map(([id, entry]) =>
    [id, unchanged(id) && !(entry.error && old.get(id)?.state !== latest.get(id)?.state)
      ? entry : { ...entry, stale: true, error: undefined }],
  ));
  return { ...previous, selection, details, readCuts:retainedCuts(readCuts,records,selection),page: { read_cut:null,records, next_cursor: previous.page.next_cursor } };
}

export function prependTrace(previous: TraceCache, page: TracePage): TraceCache {
  const records = merge(page.records, previous.page.records);
  return { ...previous, loading: false, error: undefined,readCuts:retainedCuts({...pageCuts(page),...previous.readCuts},records,previous.selection),page: { read_cut:null,records, next_cursor: page.next_cursor } };
}

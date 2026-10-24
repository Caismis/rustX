import { validateObservationCut } from '../../../protocol/app-server/observation';
import { sameTarget } from '../../../protocol/app-server/attachment-identity';
import { standaloneTraceCut, inlineTraceDetail, retainedTraceDetailPage, bindTraceDetailReader, TRACE_PAGE_SIZE, beginTraceDetail, completeTraceDetail, prependTrace, refreshTrace, replaceTrace, selectTrace, traceInterestBatches, type TraceCache, type TraceFieldReader } from './trace';
import type { AttachmentTarget, CheckpointReadCut, ProjectionDetail, TraceRecord, TraceToolLocator } from '../../../protocol/app-server/v57';
import { traceWindowReady } from './history-policy';
import type { SessionLifecycles } from './session-lifecycle/system';
import type { ClientView, ConnectionRequest, OperationAdmission, SessionView } from './contracts';
import type { ColdReads } from './cold-reads';
import type { SessionReads } from './session-reads';

/** What Trace reads read from and write to their client. */
export interface TraceReadPort {
  request: ConnectionRequest;
  state(): Readonly<ClientView>;
  setSession(id: string, patch: Partial<Pick<SessionView, 'trace' | 'tracePreview'>>): void;
  observe: SessionLifecycles['observe'];
  admit(id: string, current: () => boolean): Promise<false | OperationAdmission>;
  coldScope: ColdReads['scope'];
  readContent: SessionReads['readContent'];
}

/** Trace reads of attached and opening Sessions: the live Trace domain
 * window and its coalesced refresh, older pages, exact Tool occurrence
 * location, on-demand record details, the cold read-only Trace preview, and
 * projection inspection. Trace authority is a per-Session generation of its
 * own, independent of Runtime Client cursors and cache interval epochs. */
export class TraceReads {
  constructor(private readonly port: TraceReadPort) {}
  /** Read-only trace is independent of slow runtime/resource initialization. */
  async readPreview(id: string, admitted: () => boolean, older = false) {
    const view = this.port.state().views[id], previous = older ? view?.tracePreview : undefined;
    if (older && (!previous || previous.cache.loading || previous.cache.page.next_cursor == null)) return;
    const scope = this.port.coldScope(id, admitted); if (!scope) return;
    const { node, conversation, current } = scope;
    if (previous && previous.conversationId !== conversation) return;
    const cache = previous?.cache ?? replaceTrace({ records: [], next_cursor: null });
    const limit = TRACE_PAGE_SIZE;
    if (!current()) return;
    if (previous) this.port.setSession(id, { tracePreview: { ...previous, cache: { ...cache, loading: true, error: undefined } } });
    try {
      const admission = older ? await this.port.admit(id, current) : current;
      if (!admission) return;
      const result = await this.port.request({ method: 'session/traceHistory', params: { session_id: id, node_id: node, before: older ? cache.page.next_cursor : null, limit } }, 'session_trace_history', undefined, admission);
      if (!current()) return;
      if (result.conversation_id !== conversation) throw new Error('Trace conversation changed.');
      standaloneTraceCut(result.page,conversation);
      this.port.setSession(id, { tracePreview: { conversationId: result.conversation_id, cache:{...(older ? prependTrace(this.port.state().views[id].tracePreview!.cache, result.page) : replaceTrace(result.page)),fields:this.coldFields(id,node,conversation,current)} } });
    } catch (error) {
      if (current()) this.port.setSession(id, { tracePreview: { conversationId: previous?.conversationId ?? '', cache: { ...(this.port.state().views[id]?.tracePreview?.cache ?? cache), loading: false, error: String(error) } } });
    }
  }
  private async readPreviewDetail(id: string, record: string) {
    const preview = this.port.state().views[id]?.tracePreview;
    if (!preview || preview.cache.details[record]?.loading || preview.cache.details[record]?.detail || preview.cache.details[record]?.page) return;
    const scope = this.port.coldScope(id); if (!scope || preview.conversationId !== scope.conversation) return;
    const node = scope.node;
    const pending = beginTraceDetail(preview.cache, record);
    const current = () => scope.current() && this.port.state().views[id]?.tracePreview?.conversationId === scope.conversation
      && this.port.state().views[id]?.tracePreview?.cache.details[record] === pending.details[record];
    this.port.setSession(id, { tracePreview: { ...preview, cache: pending } });
    try {
      const admission = await this.port.admit(id, current);
      if (!admission) return;
      const result = await this.port.request({ method: 'session/traceHistoryDetail', params: { session_id: id, node_id: node, record_id: record } }, 'session_trace_history_detail', undefined, admission);
      if (!current()) return;
      if (result.conversation_id !== preview.conversationId) throw new Error('Trace conversation changed.');
      const detail=inlineTraceDetail(result.page,record),page=retainedTraceDetailPage(result.page,record);
      if(page&&(page.read_cut.conversation_id!==scope.conversation||page.read_cut.activation_id!=null))throw new Error('Historical Trace detail source changed.');
      if(!current())return;
      const reader=page&&preview.cache.fields?bindTraceDetailReader(preview.cache.fields,page,()=>scope.current()&&this.port.state().views[id]?.tracePreview?.cache.epoch===pending.epoch&&this.port.state().views[id]?.tracePreview?.cache.details[record]?.page===page):undefined;
      this.port.setSession(id, { tracePreview: { ...preview, cache: completeTraceDetail(this.port.state().views[id].tracePreview!.cache,record,pending.epoch,detail,undefined,result.page.items.flatMap(item=>item.content),async(ref,offset,ownsView)=>{
        const valid=()=>ownsView()&&scope.current()&&this.port.state().views[id]?.tracePreview?.cache.epoch===pending.epoch;
        if(!valid())throw new Error('Trace content view was replaced.');
        return (await this.port.request({method:'session/traceContentRead',params:{session_id:id,node_id:node,cut:result.page.read_cut,content_ref:ref,offset,max_bytes:65536}},'content_chunk',undefined,valid)).chunk;
      },page,reader) } });
    } catch (error) {
      if (current()) this.port.setSession(id, { tracePreview: { ...preview, cache: completeTraceDetail(this.port.state().views[id].tracePreview!.cache, record, pending.epoch, undefined, String(error)) } });
    }
  }
  // Trace authority is independent of Runtime Client cursors and cache interval
  // epochs (an overlapping snapshot can preserve the latter).
  private readonly traceFieldScopes=new WeakMap<TraceFieldReader,{generation:number;target:AttachmentTarget}>();
  liveFields(target:AttachmentTarget):TraceFieldReader {
    const existing=this.port.state().views[target.session_id]?.trace?.fields,scope=existing&&this.traceFieldScopes.get(existing);
    if(existing&&scope?.generation===this.port.state().generation&&sameTarget(scope.target,target))return existing;
    const generation=this.port.state().generation;
    const current=()=>generation===this.port.state().generation&&sameTarget(this.port.observe(target.session_id)?.target,target);
    const valid=(cut:CheckpointReadCut,owns:()=>boolean)=>{
      if(!current()||!owns()||cut.conversation_id!==target.conversation_id||cut.activation_id!=null)throw new Error('Trace field source was replaced.');
    };
    const reader:TraceFieldReader={
      value:async(cut,field,offset,owns)=>{valid(cut,owns);const result=await this.port.request({method:'conversation/checkpointValueRead',params:{target,agent_id:null,cut,domain:field.domain,item_offset:field.item_offset,path:field.path,offset,max_bytes:65536}},'checkpoint_value_chunk',undefined,()=>current()&&owns());valid(cut,owns);return result.chunk;},
      content:async(cut,reference,offset,owns)=>{valid(cut,owns);if(reference.conversation_id!==cut.conversation_id)throw new Error('Trace content source changed.');return this.port.readContent(target,undefined,reference,offset,()=>current()&&owns());},
      page:async(cut,domain,offset,owns)=>{valid(cut,owns);const result=await this.port.request({method:'conversation/checkpointPage',params:{target,agent_id:null,cut,domain,offset,limit:32}},'checkpoint_page',undefined,()=>current()&&owns());valid(cut,owns);return result.page;},
    };
    this.traceFieldScopes.set(reader,{generation:this.port.state().generation,target});
    return reader;
  }
  private coldFields(id:string,node:string|null|undefined,conversation:string,current:()=>boolean):TraceFieldReader {
    const valid=(cut:CheckpointReadCut,owns:()=>boolean)=>{
      if(!current()||!owns()||cut.conversation_id!==conversation||cut.activation_id!=null)throw new Error('Historical Trace field source was replaced.');
    };
    return {
      value:async(cut,field,offset,owns)=>{valid(cut,owns);const result=await this.port.request({method:'session/traceValueRead',params:{session_id:id,node_id:node,cut,domain:field.domain,item_offset:field.item_offset,path:field.path,offset,max_bytes:65536}},'checkpoint_value_chunk',undefined,()=>current()&&owns());valid(cut,owns);return result.chunk;},
      content:async(cut,reference,offset,owns)=>{valid(cut,owns);if(reference.conversation_id!==conversation)throw new Error('Historical Trace content source changed.');return (await this.port.request({method:'session/traceContentRead',params:{session_id:id,node_id:node,cut,content_ref:reference,offset,max_bytes:65536}},'content_chunk',undefined,()=>current()&&owns())).chunk;},
      page:async(cut,domain,offset,owns)=>{valid(cut,owns);return (await this.port.request({method:'session/tracePage',params:{session_id:id,node_id:node,cut,domain,offset,limit:32}},'checkpoint_page',undefined,()=>current()&&owns())).page;},
    };
  }
  private traceAuthorities = new Map<string, number>();
  supersede(id: string, trace: TraceCache): TraceCache {
    this.traceAuthorities.set(id, (this.traceAuthorities.get(id) ?? 0) + 1);
    // Old paging/detail requests may no longer complete. Release their loading
    // markers and reader-owned evidence with the new authority so explicit reads remain available.
    return { ...trace, loading: false, details: Object.fromEntries(
      Object.entries(trace.details).filter(([, entry]) => !entry.loading&&!entry.page&&!entry.content?.length),
    ) };
  }
  private traceReads = new Map<string, { dirty: boolean; work: Promise<void> }>();
  refreshDomain(id: string): Promise<void> {
    const existing = this.traceReads.get(id);
    if (existing) { existing.dirty = true; return existing.work; }
    const observation = this.port.observe(id);
    if (!observation) return Promise.resolve();
    const { target, current } = observation;
    const read = { dirty: true, work: Promise.resolve() };
    read.work = (async () => {
      try {
        while (read.dirty && current()) {
          // Consume only the obligation admitting this iteration. A call made
          // while it awaits owns a separate, coalesced follow-up obligation.
          read.dirty = false;
          const authority = this.traceAuthorities.get(id);
          const observed = this.port.state().views[id]?.snapshot;
          const batches=traceInterestBatches(this.port.state().views[id]?.trace);
          for(const [index,records] of batches.entries()) {
            if(!current()||this.traceAuthorities.get(id)!==authority)break;
            let result;
            try {
              result=await this.port.request({method:'session/trace',params:{target,before:null,limit:index?1:TRACE_PAGE_SIZE,records}},'trace',undefined,current);
            }catch(error){if(!read.dirty||!current())throw error;break;}
            standaloneTraceCut(result.page,target.conversation_id);
            if(current()&&this.traceAuthorities.get(id)===authority) {
              const latest = this.port.state().views[id]?.snapshot;
              if (latest?.trace !== observed?.trace || latest?.trace_updates !== observed?.trace_updates) {
                // A newer native head already landed. Keep its rows and the
                // resident detail, then coalesce one read for the new head.
                read.dirty = true; break;
              }
              this.port.setSession(id,{trace:{...refreshTrace(this.port.state().views[id]?.trace,result.page,result.page.updates??[],undefined,index>0),fields:this.liveFields(target)}});
            }
          }
        }
      } finally {
        // Retire in the same continuation that decides to exit, so a new call
        // cannot coalesce onto a stopped worker before promise cleanup runs.
        if (this.traceReads.get(id) === read) this.traceReads.delete(id);
      }
    })();
    this.traceReads.set(id, read);
    return read.work;
  }
  /** Older reads are fenced by attachment, connection and read-window epoch.
   * Ordinary live refreshes preserve the epoch only with a durable overlap. */
  /** One Turn-aligned reading window, assembled from bounded native pages. */
  loadInitialTrace(id: string) {
    const trace = this.port.state().views[id]?.trace;
    if (!trace || trace.located || trace.loading || trace.error || trace.page.next_cursor == null || traceWindowReady(trace.page.records)) return Promise.resolve();
    return this.loadEarlierTrace(id, true);
  }
  async loadEarlierTrace(id: string, initial = false) {
    if (this.port.state().views[id]?.attachment === 'attaching') return this.readPreview(id, () => true, true);
    const observation = this.port.observe(id);
    if (!observation) return;
    const { target } = observation;
    const cache = this.port.state().views[id].trace;
    const authority = this.traceAuthorities.get(id);
    if (!cache || cache.loading || cache.page.next_cursor == null) return;
    const current = () => observation.current()
      && this.traceAuthorities.get(id) === authority
      && this.port.state().views[id]?.trace?.epoch === cache.epoch;
    this.port.setSession(id, { trace: { ...cache, loading: true, error: undefined } });
    try {
      let next = cache;
      const visited = new Set<string>();
      const added: TraceRecord[] = [];
      do {
        const before = next.page.next_cursor!;
        if (visited.has(before)) throw new Error('Native Trace history did not advance.');
        visited.add(before);
        const result = await this.port.request({ method: 'session/trace', params: { target, before, limit: TRACE_PAGE_SIZE } }, 'trace', undefined, current);
        if (!current()) return;
        standaloneTraceCut(result.page,target.conversation_id);
        if (result.page.next_cursor != null && result.page.next_cursor === before) throw new Error('Native Trace history did not advance.');
        added.push(...result.page.records);
        next = prependTrace(next, result.page);
      } while (next.page.next_cursor != null && !traceWindowReady(initial ? next.page.records : added));
      if (current()) {
        const live = this.port.state().views[id].trace!;
        this.port.setSession(id, { trace: { ...prependTrace(live, next.page), readCuts: { ...next.readCuts, ...live.readCuts }, loading: false } });
        // Include newly loaded identities in a repair even if their terminal
        // notification raced this pending historical read.
        await this.refreshDomain(id);
      }
    } catch (error) {
      if (current()) this.port.setSession(id, { trace: { ...this.port.state().views[id].trace!, loading: false, error: String(error) } });
      throw error;
    }
  }
  /** Locate the canonical Tool occurrence across the native Journal, never the loaded browser window. */
  async locateToolTrace(id: string, locator: TraceToolLocator): Promise<boolean> {
    const observation = this.port.observe(id);
    if (!observation) return false;
    const previous = this.port.state().views[id]?.trace;
    this.port.setSession(id, { trace: this.supersede(id, previous ?? replaceTrace({ records: [], next_cursor: null })) });
    const authority = this.traceAuthorities.get(id);
    const current = () => observation.current() && this.traceAuthorities.get(id) === authority;
    const result = await this.port.request({ method: 'session/traceLocateTool', params: { target: observation.target, locator } }, 'trace_tool_location', undefined, current);
    if (!current()) return false;
    if (!result.location) throw new Error('This tool occurrence has no native execution record.');
    standaloneTraceCut(result.location.page,observation.target.conversation_id);
    const cache = {...replaceTrace(result.location.page, this.port.state().views[id]?.trace),fields:this.liveFields(observation.target)};
    this.port.setSession(id, { trace: selectTrace({ ...cache, located: true }, result.location.record_id) });
    return true;
  }
  async returnToLatestTrace(id: string) {
    const trace = this.port.state().views[id]?.trace, observation = this.port.observe(id);
    if (!trace || !observation) return;
    this.port.setSession(id, { trace: this.supersede(id, replaceTrace({ records: [], next_cursor: null }, trace)) });
    const authority = this.traceAuthorities.get(id);
    try { await this.refreshDomain(id); }
    catch (cause) {
      if (observation.current() && this.traceAuthorities.get(id) === authority) this.port.setSession(id, { trace: { ...this.port.state().views[id].trace!, error: String(cause), loading: false } });
    }
  }
  selectTrace(id: string, record?: string) {
    const preview = this.port.state().views[id]?.tracePreview;
    if (this.port.state().views[id]?.attachment === 'attaching' && preview) { this.port.setSession(id, { tracePreview: { ...preview, cache: selectTrace(preview.cache, record) } }); return; }
    const trace = this.port.state().views[id]?.trace;
    if (trace) this.port.setSession(id, { trace: selectTrace(trace, record) });
  }
  /**
   * Fetches the heavy detail of one record on demand.
   *
   * Every relevant identity fences the reply: the connection generation, the
   * exact attachment target, Trace authority generation and cache interval.
   * A reply that survives those fences still belongs to the record it was asked
   * for; anything else is dropped rather than attached to a newer window.
   */
  async loadTraceDetail(id: string, record: string) {
    if (this.port.state().views[id]?.attachment === 'attaching') return this.readPreviewDetail(id, record);
    const observation = this.port.observe(id);
    if (!observation) return;
    const { target } = observation;
    const cache = this.port.state().views[id]?.trace;
    if (!cache) return;
    const epoch = cache.epoch;
    const authority = this.traceAuthorities.get(id);
    const existing = cache.details[record];
    if (existing?.loading || existing && !existing.stale && !existing.error && (existing.detail || existing.page)) return;
    const pending = beginTraceDetail(cache, record);
    const current = () => observation.current()
      && this.traceAuthorities.get(id) === authority
      && this.port.state().views[id]?.trace?.epoch === epoch
      && this.port.state().views[id]?.trace?.details[record]?.read === pending.details[record].read;
    this.port.setSession(id, { trace: pending });
    try {
      const result = await this.port.request({ method: 'session/traceDetail', params: { target, record_id: record } }, 'domain_page', undefined, current);
      if (!current()) return;
      const detail=inlineTraceDetail(result.page,record),page=retainedTraceDetailPage(result.page,record);
      if(page&&(page.read_cut.conversation_id!==target.conversation_id||page.read_cut.activation_id!=null))throw new Error('Trace detail source changed.');
      if(!current())return;
      const reader=page?bindTraceDetailReader(this.liveFields(target),page,()=>observation.current()&&this.traceAuthorities.get(id)===authority&&this.port.state().views[id]?.trace?.epoch===epoch&&this.port.state().views[id]?.trace?.details[record]?.page===page):undefined;
      this.port.setSession(id, { trace: completeTraceDetail(this.port.state().views[id].trace!,record,epoch,detail,undefined,result.page.items.flatMap(item=>item.content),(ref,offset,ownsView)=>this.port.readContent(target,undefined,ref,offset,()=>ownsView()&&observation.current()&&this.traceAuthorities.get(id)===authority&&this.port.state().views[id]?.trace?.epoch===epoch),page,reader) });
    } catch (error) {
      if (current()) this.port.setSession(id, { trace: completeTraceDetail(this.port.state().views[id].trace!, record, epoch, undefined, String(error)) });
    }
  }
  async projections(id: string, after = '0') {
    const observation = this.port.observe(id);
    if (!observation) throw new Error('Session is not attached.');
    return (await this.port.request({ method: 'session/projections', params: { target: observation.target, after, limit: 16 } }, 'projections', undefined, () => observation.current())).page;
  }
  private readonly projectionReaders = new WeakMap<ProjectionDetail, TraceFieldReader>();
  projectionDetailReader(detail: ProjectionDetail) { return this.projectionReaders.get(detail); }
  async projectionDetail(id: string, checkpointId: string, offset = 0) {
    const observation = this.port.observe(id);
    if (!observation) throw new Error('Session is not attached.');
    const detail = (await this.port.request({ method: 'session/projectionDetail', params: { target: observation.target, checkpoint_id: checkpointId, offset, limit: 16 } }, 'projection_detail', undefined, () => observation.current())).detail;
    validateObservationCut(detail.read_cut, observation.target.conversation_id, null, detail.read_cut.revision);
    this.projectionReaders.set(detail, this.liveFields(observation.target));
    return detail;
  }
  /** Retired observation work keeps no Trace read or authority. */
  retire(id: string) { this.traceReads.delete(id); this.traceAuthorities.delete(id); }
  /** A lost connection ends every Trace read and authority. */
  disconnect() { this.traceReads.clear(); this.traceAuthorities.clear(); }
}

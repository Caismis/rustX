import type { CheckpointReadCut,RuntimeClientAgent,TraceToolLocator } from '../../../protocol/app-server/v57';
import type { AppServerClient } from './app-server';
import type { Observation } from './session-lifecycle/port';
import { standaloneTraceCut,inlineTraceDetail,retainedTraceDetailPage,bindTraceDetailReader,TRACE_PAGE_SIZE, beginTraceDetail, completeTraceDetail, prependTrace, refreshTrace, replaceTrace, selectTrace, traceInterestBatches, type TraceFieldReader,type TraceCache } from './trace';
import { traceWindowReady } from './history-policy';

/** One child Conversation's finite, read-only Trace domain. Parent attachment
 * is the access proof, not the identity of the records being inspected. */
export class AgentTraceReader {
  private cache: TraceCache = { ...replaceTrace({ records: [], next_cursor: null }), loading: true };
  private listeners = new Set<() => void>();
  private live = true;
  private busy = false;
  private dirty = false;
  private paging?: object;
  private navigation = {};
  constructor(private client: AppServerClient, private sessionId: string, private proof: Observation, private scope:Pick<RuntimeClientAgent,'agent_id'|'activation_id'|'child_conversation_id'|'state'>) {}
  private get agentId(){return this.scope.agent_id;}
  private validateCut(cut:CheckpointReadCut){
    const activation=this.scope.activation_id;
    if(!this.current()||cut.conversation_id!==this.scope.child_conversation_id||(cut.activation_id??null)!==activation)throw new Error('Child Trace field scope was replaced.');
  }
  private fieldReader?:TraceFieldReader;
  private get fields():TraceFieldReader {
    if(this.fieldReader)return this.fieldReader;
    const valid=(cut:CheckpointReadCut,owns:()=>boolean)=>{this.validateCut(cut);if(!owns())throw new Error('Child Trace view was replaced.');};
    return this.fieldReader={
      value:async(cut,field,offset,owns)=>{valid(cut,owns);const result=await this.client.request({method:'conversation/checkpointValueRead',params:{target:this.proof.target,agent_id:this.agentId,cut,domain:field.domain,item_offset:field.item_offset,path:field.path,offset,max_bytes:65536}},'checkpoint_value_chunk',undefined,()=>this.current()&&owns());valid(cut,owns);return result.chunk;},
      page:async(cut,domain,offset,owns)=>{valid(cut,owns);const result=await this.client.request({method:'conversation/checkpointPage',params:{target:this.proof.target,agent_id:this.agentId,cut,domain,offset,limit:32}},'checkpoint_page',undefined,()=>this.current()&&owns());valid(cut,owns);return result.page;},
      content:async(cut,reference,offset,owns)=>{valid(cut,owns);if(reference.conversation_id!==cut.conversation_id)throw new Error('Child Trace content source changed.');return this.client.reads.readContent(this.proof.target,this.agentId,reference,offset,()=>this.current()&&owns());},
    };
  }
  private pageCut(page:TraceCache['page']){const cut=standaloneTraceCut(page,this.scope.child_conversation_id,this.scope.activation_id);this.validateCut(cut);}
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  snapshot = () => this.cache;
  private current = () => this.live && this.client.isAttachmentObservationCurrent(this.sessionId, this.proof);
  private publish(cache: TraceCache) { this.cache = cache; this.listeners.forEach(listener => listener()); }
  retire() { this.live = false; this.listeners.clear(); }
  select = (id?: string) => this.publish(selectTrace(this.cache, id));
  refresh = async (state = this.scope.state) => {
    this.scope = { ...this.scope, state };
    if (state === 'inactive' || state === 'unavailable') {
      const details = Object.fromEntries(Object.entries(this.cache.details).map(([id, entry]) =>
        [id, entry.error ? { ...entry, error: undefined, stale: true } : entry]));
      if (Object.values(this.cache.details).some(entry => entry.error)) this.publish({ ...this.cache, details });
    }
    this.dirty = true;
    if (this.busy || !this.current()) return;
    this.busy = true;
    try {
      while (this.dirty && this.current()) {
        this.dirty = false;
        const navigation = this.navigation;
        try {
          const batches=traceInterestBatches(this.cache);
          for(const [index,records] of batches.entries()) {
            if(!this.current()||this.navigation!==navigation)break;
            const result=await this.client.request({method:'agent/trace',params:{target:this.proof.target,agent_id:this.agentId,limit:index?1:TRACE_PAGE_SIZE,records}},'trace',undefined,()=>this.current()&&this.navigation===navigation);
            if(this.current()&&this.navigation===navigation) {
              this.pageCut(result.page);
              const next=refreshTrace(this.cache,result.page,result.page.updates,undefined,index>0);
              if(next.epoch!==this.cache.epoch)this.paging=undefined;
              this.publish({...next,fields:this.fields,error:undefined,loading:!!this.paging});
            }
          }
        } catch (cause) {
          if (this.current() && this.navigation === navigation) this.publish({ ...this.cache, error: String(cause), loading: !!this.paging });
        }
        // A newer navigation/activity request still owns its queued refresh,
        // even when the superseded read failed.
      }
    } finally { this.busy = false; }
  };
  locate = async (locator: TraceToolLocator): Promise<boolean> => {
    if (!this.current()) return false;
    const navigation = this.navigation = {};
    this.paging = undefined;
    // Retire pending paging/detail replies and their loading markers.
    this.publish({ ...this.cache, epoch: this.cache.epoch + 1, details: {}, loading: false });
    const current = () => this.current() && this.navigation === navigation;
    const result = await this.client.request({ method: 'agent/traceLocateTool', params: { target: this.proof.target, agent_id: this.agentId, locator } }, 'trace_tool_location', undefined, current);
    if (!current()) return false;
    if (!result.location) throw new Error('This tool occurrence has no native execution record.');
    this.pageCut(result.location.page);
    const cache = {...replaceTrace(result.location.page, this.cache),fields:this.fields};
    const repaired = refreshTrace({ ...cache, located: true }, result.location.page, result.location.page.updates);
    this.publish(selectTrace(repaired, result.location.record_id));
    return true;
  };
  latest = async () => {
    if (!this.current()) return;
    this.navigation = {}; this.paging = undefined;
    this.publish(replaceTrace({ records: [], next_cursor: null }, this.cache));
    await this.refresh();
  };
  initial = () => {
    if (this.cache.located || this.cache.error || traceWindowReady(this.cache.page.records)) return Promise.resolve();
    return this.earlier(true);
  };
  earlier = async (initial = false) => {
    const cache = this.cache;
    if (!this.current() || cache.loading || !cache.page.next_cursor) return;
    const operation = {};
    const current = () => this.current() && this.cache.epoch === cache.epoch && this.paging === operation;
    this.paging = operation;
    this.publish({ ...cache, loading: true, error: undefined });
    try {
      let next = cache;
      const visited = new Set<string>();
      const added: TraceCache['page']['records'] = [];
      do {
        const before = next.page.next_cursor!;
        if (visited.has(before)) throw new Error('Native child Trace history did not advance.');
        visited.add(before);
        const result = await this.client.request({ method: 'agent/trace', params: { target: this.proof.target, agent_id: this.agentId, before, limit: TRACE_PAGE_SIZE } }, 'trace', undefined, current);
        if (!current()) return;
        this.pageCut(result.page);
        if (result.page.next_cursor != null && result.page.next_cursor === before) throw new Error('Native child Trace history did not advance.');
        added.push(...result.page.records); next = prependTrace(next, result.page);
      } while (next.page.next_cursor != null && !traceWindowReady(initial ? next.page.records : added));
      if (current()) {
        const live = this.cache;
        this.paging = undefined;
        this.publish({ ...prependTrace(live, next.page), readCuts: { ...next.readCuts, ...live.readCuts } });
        await this.refresh();
      }
    } catch (cause) { if (current()) this.publish({ ...this.cache, loading: false, error: String(cause) }); }
    finally { if (this.paging === operation) this.paging = undefined; }
  };
  detail = async (id: string) => {
    const existing = this.cache.details[id];
    if (!this.current() || existing?.loading || existing && !existing.stale && !existing.error && (existing.detail || existing.page)) return;
    const pending = beginTraceDetail(this.cache, id), epoch = pending.epoch;
    const current = () => this.current() && this.cache.epoch === epoch && this.cache.details[id]?.read === pending.details[id].read;
    this.publish(pending);
    try {
      const result = await this.client.request({ method: 'agent/traceDetail', params: { target: this.proof.target, agent_id: this.agentId, record_id: id } }, 'domain_page', undefined, current);
      const detail=inlineTraceDetail(result.page,id),page=retainedTraceDetailPage(result.page,id);
      if(page)this.validateCut(page.read_cut);
      const reader=page?bindTraceDetailReader(this.fields,page,()=>this.current()&&this.cache.epoch===epoch&&this.cache.details[id]?.page===page):undefined;
      if (current()) this.publish(completeTraceDetail(this.cache,id,epoch,detail,undefined,result.page.items.flatMap(item=>item.content),(ref,offset,ownsView)=>this.client.reads.readContent(this.proof.target,this.agentId,ref,offset,()=>ownsView()&&this.current()&&this.cache.epoch===epoch),page,reader));
    } catch (cause) { if (current()) this.publish(completeTraceDetail(this.cache, id, epoch, undefined, String(cause))); }
  };
}

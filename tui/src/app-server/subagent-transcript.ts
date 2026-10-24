import {traceFields,traceFieldKey} from '../../../protocol/app-server/trace-identity.ts';
import {traceInspectionMessage,validateCanonicalInspectionPage} from '../../../protocol/app-server/trace-inspection.ts';
import type {TraceInspectionHead,TraceInspectionMessage,CheckpointPage,RuntimeClientAgent} from '../../../protocol/app-server/v57.ts';
import { attemptIdentitySourceKey } from '../../../protocol/app-server/attempt-identity.ts';
import { deferredAttemptIdentities } from '../presentation/message-descriptors.ts';
import { contentRefKey } from '../../../protocol/app-server/content-identity.ts';
import { deferredJournalFields as nativeJournalFields } from './journal-fields.ts';
/** Disposable Child observation and one bounded historical/content viewport. */
import { ChildObservation } from '../../../protocol/app-server/child-observation.ts';
import { ContentCache } from '../../../protocol/app-server/content-cache.ts';
import { replaceFromSnapshot } from '../presentation/projection.ts';
import type { AppServerSession } from './session.ts';
import type { RuntimeClientTranscriptPage } from '../protocol/app-server.ts';

type Inspection={generation:number;agent:RuntimeClientAgent;id:string;head?:TraceInspectionHead;page?:CheckpointPage;messages?:TraceInspectionMessage[];next?:number;messageIndex?:number;message?:TraceInspectionMessage;collection?:CheckpointPage;collectionIndex?:number};
type Session = Pick<AppServerSession, 'state' | 'agentTranscriptPage' | 'observeAgent' | 'readContent' | 'agentStatus' | 'onState' | 'checkpoint' | 'readChildJournalField' | 'readAttemptIdentity' | 'agentTraceInspection' | 'agentInspectionPage' | 'readAgentTraceField'>;
export class SubagentTranscript {
  readonly #session: Session;
  readonly selected: string;
  #inspection?:Inspection;
  #generation = 0;
  #contentRead = 0;
  #closed = false;
  #historical?: RuntimeClientTranscriptPage;
  #observation?: ChildObservation;
  #controller?: AbortController;
  #following?: Promise<void>;
  #opening?: Promise<void>;
  #agent?:import('../../../protocol/app-server/v57.ts').RuntimeClientAgent;
  #agentStamp?:string;
  #refreshing?:Promise<void>;
  #stopState:()=>void;

  #error?: string;
  #content?: { index: number; text: string; next?: number };
  #attemptCut?:{identity:string;generation:number;cut:import('../../../protocol/app-server/v57.ts').CheckpointReadCut};
  readonly #cache: ContentCache;
  #decoder = new TextDecoder('utf-8', {ignoreBOM: true});
  #decoding?: { identity: string; end: number };
  onChange?: () => void;
  constructor(session: Session, id: string) {
    this.#session = session; this.selected = id;
    this.#cache = new ContentCache((ref, offset) => session.readContent(ref, offset, id));
    this.#stopState=session.onState(()=>{
      const cut=session.checkpoint?.read_cut;
      const stamp=JSON.stringify(cut?.collection_cuts.agents??cut?.domain_revisions.agents);
      if(this.#closed||!this.#observation||stamp===this.#agentStamp)return;
      this.#agentStamp=stamp;
      if(!session.state.agents.some(agent=>agent.agent_id===id))void this.#refreshAgent();
    });

  }
  get agent(){return this.#session.state.agents.find(agent=>agent.agent_id===this.selected)??this.#agent;}
  async #refreshAgent():Promise<void> {
    if(this.#refreshing)return this.#refreshing;
    const generation=this.#generation,stamp=this.#agentStamp;
    this.#refreshing=(async()=>{
      const agent=await this.#session.agentStatus(this.selected);
      if(this.#closed||generation!==this.#generation)return;
      const previous=this.#agent;this.#agent=agent;
      if(this.#observation&&(agent.activation_id!==this.#observation.activationId||agent.child_conversation_id!==this.#observation.conversationId||previous?.state!==agent.state))void this.newest();
    })().catch(error=>{if(!this.#closed&&generation===this.#generation){this.#error=`Child source unavailable: ${String(error)}`;this.onChange?.();}}).finally(()=>{
      this.#refreshing=undefined;
      if(!this.#closed&&generation===this.#generation&&stamp!==this.#agentStamp)void this.#refreshAgent();
    });
    return this.#refreshing;
  }
  get page() { return this.#historical ?? this.#observation?.snapshot?.transcript; }
  get state() { const source = this.#observation; return !this.#historical && source?.snapshot ? replaceFromSnapshot(source.snapshot, source.cursor?.cursor ?? source.checkpoint!.cursor) : undefined; }
  get phase() { return this.#historical ? 'history' : this.#observation?.phase ?? 'checkpointLoading'; }
  get error() { return this.#error; }
  get content() { return this.#content; }
  get deferredJournalFields() { return nativeJournalFields(this.#observation?.snapshot?.attempt); }
  get deferredJournal() { return this.deferredJournalFields.map(field => field.reference); }
  get deferredAttemptIdentities(){const state=this.state;return state?deferredAttemptIdentities(state):[];}
  get deferredCount(){return this.deferred.length+this.deferredJournal.length+this.deferredAttemptIdentities.length+this.traceFields.length+this.collectionSources.length;}
  #ownsInspection(selected:Inspection) {
    const agent=this.agent;
    return !this.#closed&&selected.generation===this.#generation&&this.#inspection===selected&&agent?.agent_id===selected.agent.agent_id&&agent.child_conversation_id===selected.agent.child_conversation_id&&agent.activation_id===selected.agent.activation_id&&agent.state===selected.agent.state;
  }
  get inspection(){const selected=this.#inspection;return selected&&this.#ownsInspection(selected)?selected:undefined;}
  get traceRecords(){return this.#observation?.snapshot?.trace.records??[];}
  get traceFields(){
    const selected=this.inspection,head=selected?.head;if(!head)return [];
    const fields=traceFields(head.record);
    if(head.content.type==='tool'&&head.content.definition.type==='available')fields.push(head.content.definition.field);
    if(head.content.type==='messages')for(const message of selected.messages??head.content.page.items)fields.push(...message.fields);
    return [...new Map(fields.map(field=>[traceFieldKey(field),field])).values()];
  }
  get inspectionMessage(){const selected=this.inspection,head=selected?.head;if(!selected||head?.content.type!=='messages')return;return selected.message??(selected.messages??head.content.page.items)[selected.messageIndex??0];}
  get inspectionCollections(){const head=this.inspection?.head;return head?.content.type==='tool'?head.record.collections:this.inspectionMessage?.collections??[];}
  get collectionSources(){
    const page=this.inspection?.collection;if(!page)return [];
    return page.items.flatMap(item=>[
      {type:'inline' as const,item},
      ...item.content.map(binding=>({type:'content' as const,binding}))]);
  }
  selectTraceMessage():void {
    const selected=this.inspection,head=selected?.head;if(!selected||head?.content.type!=='messages')return;
    this.#contentRead++;this.#collectionOperation=undefined;this.#content=undefined;this.#error=undefined;
    if(selected.collection){selected.collection=undefined;selected.message=undefined;selected.messageIndex=0;}
    else selected.messageIndex=((selected.messageIndex??0)+1)%Math.max(1,(selected.messages??head.content.page.items).length);
    selected.collectionIndex=undefined;this.onChange?.();
  }
  async selectTraceCollection():Promise<void> {
    const selected=this.inspection,message=this.inspectionMessage,collections=this.inspectionCollections;if(!selected||!collections.length)return;
    const index=((selected.collectionIndex??-1)+1)%collections.length;
    await this.#loadTraceCollection(selected,message,collections[index]!.domain,0,index);
  }
  async #loadTraceCollection(selected:Inspection,message:TraceInspectionMessage|undefined,domain:string,offset:number,index:number):Promise<void> {
    const frontier=message?.collections[index]??(selected.head?.content.type==='tool'?selected.head.record.collections[index]:undefined);if(!selected.page||frontier?.domain!==domain)return;
    const advertisedContinuation=selected.collection?.domain===domain&&selected.collection.next_offset===String(offset)||frontier.next_offset===String(offset);
    this.#contentRead++;this.#content=undefined;
    // A new collection selection retires held pages before its physical response.
    const operation={};this.#collectionOperation=operation;
    const current=()=>this.#ownsInspection(selected)&&this.#collectionOperation===operation;
    try {const page=await this.#session.agentInspectionPage(selected.agent,selected.page.read_cut,domain,offset,current);
      if(!current())return;
      validateCanonicalInspectionPage(page,offset,frontier.total,advertisedContinuation);
      selected.message=message;selected.collection=page;selected.collectionIndex=index;selected.messages=undefined;this.#error=undefined;this.onChange?.();
    }catch(error){if(current()){this.#error=`Child original collection unavailable: ${String(error)}`;this.onChange?.();}}
  }
  #collectionOperation?:object;
  async inspectTrace(id:string):Promise<void> {
    this.#contentRead++;this.#collectionOperation=undefined;
    const agent=this.agent;if(!agent||this.#closed)return;
    const selected=this.#inspection={generation:this.#generation,agent:{...agent},id};
    const current=()=>this.#ownsInspection(selected);
    this.#content=undefined;this.#error=undefined;
    try {const result=await this.#session.agentTraceInspection(selected.agent,id,current);
      if(!current())return;Object.assign(selected,result);
      if(result.head.content.type==='messages')Object.assign(selected,{messages:result.head.content.page.items,next:result.head.content.page.next_offset??undefined});
      this.onChange?.();
    }catch(error){if(current()){this.#error=`Child Trace unavailable: ${String(error)}`;this.onChange?.();}}
  }
  async nextTraceMessages():Promise<void> {
    const selected=this.inspection,head=selected?.head;
    if(!selected?.page||!head)return;
    if(selected.collection){const next=selected.collection.next_offset;if(next!=null)await this.#loadTraceCollection(selected,selected.message,selected.collection.domain,Number(next),selected.collectionIndex!);return;}
    if(head.content.type!=='messages'||selected.next===undefined)return;
    this.#contentRead++;this.#collectionOperation=undefined;
    const operation={};this.#collectionOperation=operation;
    const offset=selected.next,current=()=>this.#ownsInspection(selected)&&this.#collectionOperation===operation;
    try {const page=await this.#session.agentInspectionPage(selected.agent,selected.page.read_cut,head.content.page.domain,offset,current);
      if(!current())return;
      if(page.total!==head.content.page.total||page.items.length>32||page.items.some((item,index)=>item.offset!==String(offset+index))||page.next_offset!=null&&page.next_offset!==String(offset+page.items.length)||page.next_offset!=null&&!page.items.length)throw new Error('Child Trace message continuation changed.');
      if(!/^(0|[1-9][0-9]{0,19})$/.test(page.total)||!Number.isSafeInteger(offset+page.items.length))throw new Error('Child Trace message coordinates unavailable.');
      const end=BigInt(offset+page.items.length),total=BigInt(page.total);
      if(total>18446744073709551615n||page.next_offset!=null&&(!Number.isSafeInteger(Number(page.next_offset))||Number(page.next_offset)<0))throw new Error('Child Trace message coordinates unavailable.');
      if(end>total||(page.next_offset!=null)!==(end<total))throw new Error('Child Trace message extent changed.');
      selected.messageIndex=0;selected.message=undefined;selected.messages=page.items.map(item=>traceInspectionMessage(item.value));selected.next=page.next_offset==null?undefined:Number(page.next_offset);this.#content=undefined;this.#error=undefined;this.onChange?.();
    }catch(error){if(current()){this.#error=`Child Trace messages unavailable: ${String(error)}`;this.onChange?.();}}
  }
  get deferred() { return this.#observation?.content.filter(binding => binding.deferred) ?? []; }
  /** Home resumes after the locally Applied cursor; it never jumps over a gap. */
  newest(): Promise<void> {
    this.#contentRead++;this.#collectionOperation=undefined;
    if(this.#closed)return Promise.resolve();
    if(this.#opening)return this.#opening;
    const opening=this.#newest().finally(()=>{if(this.#opening===opening)this.#opening=undefined;});
    this.#opening=opening;
    return opening;
  }
  async #newest(): Promise<void> {
    if (this.#closed) return;
    this.#controller?.abort(); await this.#following;
    if(this.#closed)return;
    const generation = ++this.#generation;
    this.#historical = undefined; this.#error = undefined;
    let agent=this.agent;
    if(!agent){try{agent=await this.#session.agentStatus(this.selected);}catch(error){if(!this.#closed&&generation===this.#generation){this.#error=`Child source unavailable: ${String(error)}`;this.onChange?.();}return;}}
    if(this.#closed||generation!==this.#generation)return;
    this.#agent=agent;
    this.#agentStamp=JSON.stringify(this.#session.checkpoint?.read_cut.collection_cuts.agents??this.#session.checkpoint?.read_cut.domain_revisions.agents);
    if (!agent) { this.#error = 'Child observation source is unavailable.'; this.onChange?.(); return; }
    const existing = this.#observation;
    if (!existing || existing.activationId !== agent.activation_id || existing.conversationId !== agent.child_conversation_id) {
      this.#observation = new ChildObservation(agent.agent_id, agent.child_conversation_id, agent.activation_id);
      this.#content=undefined;this.#decoding=undefined;this.#decoder=new TextDecoder('utf-8', {ignoreBOM: true});this.#cache.clear();
    }
    const controller = new AbortController(); this.#controller = controller;
    const current = () => !this.#closed && generation === this.#generation && !controller.signal.aborted;
    this.#following = (async () => {
      let recoveries = 0;
      while (current()) {
        const result = await this.#session.observeAgent(this.selected, this.#observation!.cursor, controller.signal);
        if (!current()) return;
        const active = this.agent;
        if (!active || active.activation_id !== this.#observation!.activationId || active.child_conversation_id !== this.#observation!.conversationId) throw new Error('Child activation was replaced. Reopen the view.');
        if(result.type==='resync_required'&&(result.activation_id!==this.#observation!.activationId||result.conversation_id!==this.#observation!.conversationId)){
          const refreshed=await this.#session.agentStatus(this.selected);
          if(!current())return;
          if(refreshed.activation_id!==result.activation_id||refreshed.child_conversation_id!==result.conversation_id)throw new Error('Child source changed during recovery.');
          this.#agent=refreshed;this.#observation=new ChildObservation(refreshed.agent_id,refreshed.child_conversation_id,refreshed.activation_id);this.onChange?.();continue;
        }
        this.#observation!.apply(result); this.onChange?.();
        if (this.#observation!.phase === 'durableOnly') return;
        if (this.#observation!.phase === 'resyncRequired' && ++recoveries > 2) throw new Error('Child observation requires an unavailable checkpoint.');
      }
    })().catch(error => { if (current()) { this.#error = `Child observation unavailable: ${error instanceof Error ? error.message : String(error)}`; this.onChange?.(); } });
    this.onChange?.();
  }
  async older(): Promise<void> {
    this.#contentRead++;this.#collectionOperation=undefined;
    const before = this.page?.next_cursor;
    if (this.#closed || before == null) return;
    this.#controller?.abort();
    const generation = ++this.#generation;
    try {
      const page = await this.#session.agentTranscriptPage(this.selected, before);
      if (!this.#closed && generation === this.#generation && page) { this.#historical = page; this.#error = undefined; this.onChange?.(); }
    } catch (error) { if (!this.#closed && generation === this.#generation) { this.#error = `Child history unavailable: ${String(error)}`; this.onChange?.(); } }
  }
  async loadContent(index = 0, offset = 0): Promise<void> {
    this.#collectionOperation=undefined;
    const token=++this.#contentRead,readCurrent=()=>token===this.#contentRead&&!this.#closed;
    const collectionIndex=index-this.deferred.length-this.deferredJournal.length-this.deferredAttemptIdentities.length-this.traceFields.length,collection=this.collectionSources[collectionIndex],inspection=this.inspection;
    if(collection&&inspection?.page){
      if(collection.type==='inline'){
        if(offset!==0){this.#error='Inline original metadata has no byte continuation.';this.onChange?.();return;}
        this.#content={index,text:JSON.stringify(collection.item.value)};this.#decoding=undefined;this.#error=undefined;this.onChange?.();return;
      }
      const identity=JSON.stringify(collection),current=()=>readCurrent()&&this.#ownsInspection(inspection)&&JSON.stringify(this.collectionSources[collectionIndex])===identity;
      try {
        if(collection.binding.content_ref.conversation_id!==inspection.page.read_cut.conversation_id)throw new Error('Child original content namespace changed.');
        const range=await this.#cache.read(collection.binding.content_ref,offset,current);
        if(!current())return;
        if(this.#decoding?.identity!==identity||this.#decoding.end!==offset)this.#decoder=new TextDecoder('utf-8',{ignoreBOM:true});
        const representation=collection.binding.content_ref.representation;
        this.#content={index,text:representation==='binary_bytes'?`Base64: ${Buffer.from(range.bytes).toString('base64')}`:this.#decoder.decode(range.bytes,{stream:range.next!==undefined}),next:range.next};this.#decoding={identity,end:offset+range.bytes.length};this.#error=undefined;this.onChange?.();
      }catch(error){if(current()){this.#error=`Child original collection bytes unavailable: ${String(error)}`;this.onChange?.();}}return;
    }
    const trace=this.traceFields[index-this.deferred.length-this.deferredJournal.length-this.deferredAttemptIdentities.length],selected=this.inspection;
    if(trace&&selected?.page){
      const identity=traceFieldKey(trace),current=()=>readCurrent()&&this.#ownsInspection(selected)&&this.traceFields.some(field=>traceFieldKey(field)===identity);
      try {const range=await this.#session.readAgentTraceField(selected.agent,selected.page.read_cut,trace,offset,current);
        if(!current())return;
        if(this.#decoding?.identity!==identity||this.#decoding.end!==offset)this.#decoder=new TextDecoder('utf-8',{ignoreBOM:true});
        this.#content={index,text:range.representation==='binary_bytes'?`Base64: ${Buffer.from(range.bytes).toString('base64')}`:this.#decoder.decode(range.bytes,{stream:range.next!==undefined}),next:range.next};
        this.#decoding={identity,end:offset+range.bytes.length};this.#error=undefined;this.onChange?.();
      }catch(error){if(current()){this.#error=`Child Trace field unavailable: ${String(error)}`;this.onChange?.();}}return;
    }
    const attempt=this.deferredAttemptIdentities[index-this.deferred.length-this.deferredJournal.length];
    if(attempt){
      const source=this.#observation,generation=this.#generation,identity=attemptIdentitySourceKey(attempt.source);
      if(this.#closed||!source)return;
      try{
        const previous=this.#attemptCut,cut=offset===0?source.checkpoint?.read_cut:previous?.identity===identity&&previous.generation===generation?previous.cut:undefined;
        if(!cut)throw new Error('Read the original Child owner identity from byte zero first.');
        const range=await this.#session.readAttemptIdentity(attempt.source,offset,cut,this.selected);
        const current=this.deferredAttemptIdentities[index-this.deferred.length-this.deferredJournal.length];
        if(!readCurrent()||generation!==this.#generation||this.#observation!==source||!current||attemptIdentitySourceKey(current.source)!==identity)return;
        if(this.#decoding?.identity!==identity||this.#decoding.end!==offset)this.#decoder=new TextDecoder('utf-8',{ignoreBOM:true});
        this.#attemptCut={identity,generation,cut};
        this.#content={index,text:this.#decoder.decode(range.bytes,{stream:range.next!==undefined}),next:range.next};this.#decoding={identity,end:offset+range.bytes.length};this.#error=undefined;this.onChange?.();
      }catch(error){if(readCurrent()&&generation===this.#generation){this.#error=`Child owner identity unavailable: ${String(error)}`;this.onChange?.();}}
      return;
    }
    const journal=this.deferredJournal[index-this.deferred.length];
    if(journal){
      const source=this.#observation,generation=this.#generation,identity=JSON.stringify(journal),cut=source?.checkpoint?.read_cut;
      if(this.#closed||!source||!cut)return;
      try{
        const range=await this.#session.readChildJournalField(this.selected,cut,journal,offset);
        if(!readCurrent()||generation!==this.#generation||this.#observation!==source||JSON.stringify(this.deferredJournal[index-this.deferred.length])!==identity)return;
        if(this.#decoding?.identity!==identity||this.#decoding.end!==offset)this.#decoder=new TextDecoder('utf-8', {ignoreBOM: true});
        this.#content={index,text:this.#decoder.decode(range.bytes,{stream:range.next!==undefined}),next:range.next};this.#decoding={identity,end:offset+range.bytes.length};this.#error=undefined;this.onChange?.();
      }catch(error){if(readCurrent()&&generation===this.#generation){this.#error=`Child native field unavailable: ${String(error)}`;this.onChange?.();}}
      return;
    }
    const binding = this.deferred[index]; if (!binding || this.#closed) return;
    const generation = this.#generation, identity = contentRefKey(binding.content_ref);
    const current = () => readCurrent() && generation === this.#generation && this.deferred[index]!=null && contentRefKey(this.deferred[index]!.content_ref) === identity;
    try {
      const range = await this.#cache.read(binding.content_ref, offset, current);
      if (current()) {
        if (this.#decoding?.identity !== identity || this.#decoding.end !== offset) this.#decoder = new TextDecoder('utf-8', {ignoreBOM: true});
        const text = binding.content_ref.representation === 'binary_bytes'
          ? `Base64: ${Buffer.from(range.bytes).toString('base64')}`
          : this.#decoder.decode(range.bytes, { stream: range.next !== undefined });
        this.#content = { index, text, next: range.next };
        this.#decoding = { identity, end: offset + range.bytes.length };
        this.#error = undefined; this.onChange?.();
      }
    } catch (error) { if (current()) { this.#error = `Deferred content unavailable: ${String(error)}`; this.onChange?.(); } }
  }
  dispose(): void {
    this.#closed = true; this.#contentRead++;this.#collectionOperation=undefined;this.#generation++; this.#controller?.abort();this.#stopState();
    this.#historical = undefined; this.#observation = undefined; this.#content = undefined; this.#cache.clear(); this.onChange = undefined;
  }
}

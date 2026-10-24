import { ContentCache } from '../../../protocol/app-server/content-cache.ts';
import {traceInspectionHead,traceInspectionMessage,validateCanonicalInspectionPage} from '../../../protocol/app-server/trace-inspection.ts';
import {traceFields,traceFieldKey} from '../../../protocol/app-server/trace-identity.ts';
import {readTraceSelectedWindow,readTraceFieldWindow} from '../../../protocol/app-server/trace-field.ts';
import { messageDescriptor } from '../../../protocol/app-server/message-descriptor.ts';
import { deliveryHead,deliveryItem,deliveryItemKey,deliveryPage as nativeDeliveryPage,readDeliveryMetadata as readOriginalDeliveryMetadata } from '../../../protocol/app-server/delivery-observation.ts';
import { attemptIdentityField, attemptIdentitySourceKey } from '../../../protocol/app-server/attempt-identity.ts';
import { deferredAttemptIdentities } from '../presentation/message-descriptors.ts';
import { deferredJournalFields as nativeJournalFields } from './journal-fields.ts';
import { domainItems, type DomainReader } from '../../../protocol/app-server/domain-observation.ts';
import { validateObservationCut } from '../../../protocol/app-server/observation.ts';
import { sameTarget } from "../../../protocol/app-server/attachment-identity.ts";
import type {
  AttachmentTarget, InteractionRef, MethodName, MethodParams, ResultType, ResultOf, RuntimeClientAgent,
  RuntimeClientTranscriptCursor, RuntimeClientTranscriptPage, SessionFileReference, AgentId,
} from "../protocol/app-server.ts";
import type { PresentationState } from "../presentation/state.ts";
import { pageDeliveries, type DeliveryLocation, type DeliveryPage, type DeliveryRecord } from "../presentation/deliveries.ts";
import type { AppServerClient } from "./client.ts";
import type { RuntimeClientCheckpoint } from "../../../protocol/app-server/v57.ts";
import type { SurfaceRevision, SessionUserMessageBoundaryObservation } from "../protocol/app-server.ts";



/** Native owner bound for one bounded page. */
export const SESSION_PROJECTION_PAGE_LIMIT = 32;
export const TRANSCRIPT_PROJECTION_PAGE_LIMIT = 32;

/** One bounded page of historical fork/branch boundaries. */
export interface BoundaryPage {
  /** The exact committed head the page was selected against. */
  surfaceRevision: SurfaceRevision;
  boundaries: SessionUserMessageBoundaryObservation[];
  nextOffset?: number;
}

/** Canonical evidence of a native read cut, independent of field order. */
export function readEvidence(value:unknown):string {
  return JSON.stringify(value,(_key,item)=>item&&typeof item==='object'&&!Array.isArray(item)?Object.fromEntries(Object.entries(item).filter(([,value])=>value!==undefined).sort(([left],[right])=>left.localeCompare(right))):item);
}

/** What one attached Session's readers read from their attachment owner. */
export interface SessionReadPort {
  client(): AppServerClient;
  /** Exact attachment calls; effects still meet the owner's control gate. */
  call<M extends Exclude<MethodName, 'delivery/cancel'>, T extends ResultType>(method: M, params: MethodParams<M>, expected: T): Promise<ResultOf<T>>;
  target(): AttachmentTarget;
  /** The owner's Session epoch; every read is fenced by the one it began in. */
  epoch(): number;
  released(): boolean;
  serverClosed(): boolean;
  checkpoint(): RuntimeClientCheckpoint | undefined;
  state(): PresentationState;
}

/**
 * The bounded native readers of one attached Session: Trace and projection
 * inspection viewports, deferred observation fields and original content
 * ranges, delivery history and bytes, child observation and transcript
 * pages, and finite domain pages. Every read is fenced by the owner's
 * Session epoch and attachment target, and none mutates the projection;
 * attachment lifecycle and the folded state stay with `AppServerSession`.
 */
export class SessionReaders {
  readonly #port: SessionReadPort;
  constructor(port: SessionReadPort) { this.#port = port; }
  // One explicit bounded inspection viewport, independent of projection/Applied.
  #traceInspection?: {epoch:number;id:string;owned:()=>boolean;head?:import('../../../protocol/app-server/v57.ts').TraceInspectionHead;page?:import('../../../protocol/app-server/v57.ts').CheckpointPage;messages?:import('../../../protocol/app-server/v57.ts').TraceInspectionMessage[];collection?:import('../../../protocol/app-server/v57.ts').CheckpointPage};
  #projectionInspection?: {epoch:number;owned:()=>boolean;detail?:import('../../../protocol/app-server/v57.ts').ProjectionDetail};
  #currentTraceInspection() {
    const selected=this.#traceInspection;
    return selected?.head&&selected.page&&!this.#port.released()&&!this.#port.serverClosed()&&selected.epoch===this.#port.epoch()&&selected.owned() ? selected : undefined;
  }
  async inspectTrace(recordId:string,owned:()=>boolean=()=>true) {
    if(typeof recordId!=='string'||!recordId||recordId.length>256)throw new Error('Invalid native Trace selection.');
    if(!owned())throw new Error('Trace inspection viewport was replaced.');
    this.#projectionInspection=undefined;
    const selected=this.#traceInspection={epoch:this.#port.epoch(),id:recordId,owned};
    const {page}=await this.#port.call('session/traceDetail',{target:this.#port.target(),record_id:recordId},'domain_page');
    if(this.#port.released()||this.#port.serverClosed()||selected.epoch!==this.#port.epoch()||this.#traceInspection!==selected||!owned())throw new Error('Trace inspection viewport was replaced.');
    validateObservationCut(page.read_cut,this.#port.target().conversation_id,null,page.cursor);
    const head=traceInspectionHead(page,recordId);
    if(!head)throw new Error('Selected Request fields are available through /content; this is not an inspection head.');
    Object.assign(selected,{head,page});
    return {head,page};
  }

  async agentTraceInspection(agent:RuntimeClientAgent,recordId:string,owned:()=>boolean) {
    const epoch=this.#port.epoch();
    const current=()=>!this.#port.released()&&!this.#port.serverClosed()&&epoch===this.#port.epoch()&&owned();
    if(!current()||!recordId||recordId.length>256)throw new Error('Child Trace view was replaced.');
    const {page}=await this.#port.call('agent/traceDetail',{target:this.#port.target(),agent_id:agent.agent_id,record_id:recordId},'domain_page');
    if(!current())throw new Error('Child Trace view was replaced.');
    validateObservationCut(page.read_cut,agent.child_conversation_id,agent.activation_id,page.cursor);
    const head=traceInspectionHead(page,recordId);
    if(!head)throw new Error('This selection is not a finite Trace inspection head.');
    return {head,page};
  }
  async agentInspectionPage(agent:RuntimeClientAgent,cut:import('../../../protocol/app-server/v57.ts').CheckpointReadCut,domain:string,offset:number,owned:()=>boolean) {
    const epoch=this.#port.epoch();
    const current=()=>!this.#port.released()&&!this.#port.serverClosed()&&epoch===this.#port.epoch()&&owned();
    validateObservationCut(cut,agent.child_conversation_id,agent.activation_id,cut.revision);
    if(!current())throw new Error('Child Trace view was replaced.');
    const {page}=await this.#port.call('conversation/checkpointPage',{target:this.#port.target(),agent_id:agent.agent_id,cut,domain,offset,limit:32},'checkpoint_page');
    if(!current()||page.domain!==domain||readEvidence(page.read_cut)!==readEvidence(cut)||page.cursor!==cut.revision)throw new Error('Child Trace page was replaced.');
    return page;
  }
  async readAgentTraceField(agent:RuntimeClientAgent,cut:import('../../../protocol/app-server/v57.ts').CheckpointReadCut,source:import('../../../protocol/app-server/v57.ts').TraceFieldSource,offset:number,owned:()=>boolean) {
    const epoch=this.#port.epoch(),target=this.#port.target();
    const current=()=>!this.#port.released()&&!this.#port.serverClosed()&&epoch===this.#port.epoch()&&owned();
    validateObservationCut(cut,agent.child_conversation_id,agent.activation_id,cut.revision);
    if(!current())throw new Error('Child Trace field was replaced.');
    if(source.type==='value') {
      const range=await readTraceFieldWindow(source,cut,offset,async at=>(await this.#port.call('conversation/checkpointValueRead',{target,agent_id:agent.agent_id,cut,domain:source.domain,item_offset:source.item_offset,path:source.path,offset:at,max_bytes:65536},'checkpoint_value_chunk')).chunk,current);
      return {...range,end:source.encoded_bytes,cut};
    }
    if(source.binding.content_ref.conversation_id!==cut.conversation_id)throw new Error('Child Trace content source was replaced.');
    const cache=new ContentCache((reference,at)=>this.readContent(reference,at,agent.agent_id));
    try {const range=await cache.read(source.binding.content_ref,offset,current);return {...range,end:source.binding.content_ref.readable_end,representation:source.binding.content_ref.representation,cut};}
    finally{cache.clear();}
  }

  get deferredJournalFields() {
    const fields = nativeJournalFields(this.#port.state().attempt), clock = this.#port.state().statistics?.latest_turn;
    if (clock?.attempt_id_deferred && clock.attempt_id_source) fields.push({ label: 'Turn attempt identity', reference: clock.attempt_id_source });
    return fields;
  }
  get deferredAttemptIdentities() {
    const fields=deferredAttemptIdentities(this.#port.state()),checkpoint=this.#port.checkpoint();
    if(checkpoint)for(const record of checkpoint.snapshot.trace.records){const source=record.location.attempt_id_source;if(source&&!fields.some(field=>attemptIdentitySourceKey(field.source)===attemptIdentitySourceKey(source)))fields.push({owner:`Trace ${record.id}`,source});}
    return fields;
  }
  get observationCollections() {
    const checkpoint=this.#port.checkpoint();
    if(!checkpoint)return [];
    const rows=new Map<string,{frontier:Pick<import('../../../protocol/app-server/v57.ts').CollectionFrontier,'domain'|'total'|'loaded'|'next_offset'>;cut:import('../../../protocol/app-server/v57.ts').CheckpointReadCut;inspection?:object}>((checkpoint.collections??[]).map(frontier=>[frontier.domain,{frontier,cut:checkpoint.read_cut}]));
    const cut=checkpoint.snapshot.trace.read_cut??checkpoint.read_cut;
    for(const record of checkpoint.snapshot.trace.records){
      for(const frontier of record.collections)rows.set(frontier.domain,{frontier,cut});
      const request=record.request;
      if(request?.context_next_offset!=null)rows.set(request.context_domain,{cut,frontier:{domain:request.context_domain,total:request.context_total,loaded:String(request.context_additions.length),next_offset:String(request.context_next_offset)}});
    }
    const selected=this.#currentTraceInspection();
    if(selected?.head&&selected.page){
      const cut=selected.page.read_cut,head=selected.head;
      for(const frontier of head.record.collections)rows.set(frontier.domain,{frontier,cut,inspection:selected});
      if(head.content.type==='messages'){
        const page=head.content.page;
        rows.set(page.domain,{cut,inspection:selected,frontier:{domain:page.domain,total:page.total,loaded:String(page.items.length),next_offset:page.next_offset==null?null:String(page.next_offset)}});
        for(const message of selected.messages??page.items)for(const frontier of message.collections)rows.set(frontier.domain,{frontier,cut,inspection:selected});
      }
    }
    return [...rows.values()];
  }
  get deferredTraceFields() {
    const checkpoint=this.#port.checkpoint();
    if(!checkpoint)return [];
    const fields=new Map<string,{record:string;source:import('../../../protocol/app-server/v57.ts').TraceFieldSource;cut:import('../../../protocol/app-server/v57.ts').CheckpointReadCut;inspection?:object;projection?:object}>();
    const cut=checkpoint.snapshot.trace.read_cut??checkpoint.read_cut;
    for(const record of checkpoint.snapshot.trace.records)for(const source of traceFields(record))fields.set(traceFieldKey(source),{record:record.id,source,cut});
    const selected=this.#currentTraceInspection();
    if(selected?.head&&selected.page){
      const head=selected.head,sources=traceFields(head.record);
      if(head.content.type==='tool'&&head.content.definition.type==='available')sources.push(head.content.definition.field);
      if(head.content.type==='messages')for(const message of selected.messages??head.content.page.items)sources.push(...message.fields);
      for(const source of sources)fields.set(traceFieldKey(source),{record:head.id,source,cut:selected.page.read_cut,inspection:selected});
    }
    const projection=this.#projectionInspection;
    if(projection?.detail&&!this.#port.released()&&!this.#port.serverClosed()&&projection.epoch===this.#port.epoch()&&projection.owned())for(const item of projection.detail.items){
      const source=item.content.provenance_source;
      if(source)fields.set(traceFieldKey(source),{record:`projection:${item.occurrence.checkpoint_id}/${item.occurrence.item_index}`,source,cut:projection.detail.read_cut,projection});
    }
    return [...fields.values()];
  }
  get observationContent() {
    const selected=this.#currentTraceInspection(),collection=selected?.collection;
    return selected&&collection?collection.items.flatMap(item=>item.content.map(binding=>({binding,cut:collection.read_cut,inspection:selected,page:collection,item:item.offset}))):[];
  }
  async readObservationContent(selected:SessionReaders['observationContent'][number],offset:number) {
    const epoch=this.#port.epoch(),current=()=>!this.#port.released()&&!this.#port.serverClosed()&&epoch===this.#port.epoch()&&this.#currentTraceInspection()===selected.inspection&&selected.inspection.collection===selected.page;
    const reference=selected.binding.content_ref;
    if(reference.conversation_id!==selected.cut.conversation_id||!current())throw new Error('Original observation content was replaced.');
    const cache=new ContentCache((ref,at)=>this.readContent(ref,at));
    try{return {...await cache.read(reference,offset,current),representation:reference.representation,end:reference.readable_end,cut:selected.cut};}finally{cache.clear();}
  }
  async readTraceField(selected:SessionReaders['deferredTraceFields'][number],offset:number) {
    const epoch=this.#port.epoch(),target=this.#port.target(),identity=traceFieldKey(selected.source),cut=selected.cut;
    const current=()=>!this.#port.released()&&!this.#port.serverClosed()&&epoch===this.#port.epoch()&&(!selected.inspection||this.#traceInspection===selected.inspection)&&(!selected.projection||this.#projectionInspection===selected.projection)&&this.deferredTraceFields.some(field=>traceFieldKey(field.source)===identity&&readEvidence(field.cut)===readEvidence(cut)&&field.inspection===selected.inspection&&field.projection===selected.projection);
    if(cut.conversation_id!==target.conversation_id||cut.activation_id!=null||!current())throw new Error('Trace field source was replaced.');
    const source=selected.source;
    if(source.type==='value') {
      const range=await readTraceFieldWindow(source,cut,offset,async at=>(await this.#port.call('conversation/checkpointValueRead',{target,agent_id:null,cut,domain:source.domain,item_offset:source.item_offset,path:source.path,offset:at,max_bytes:65536},'checkpoint_value_chunk')).chunk,current);
      return {...range,end:source.encoded_bytes,cut};
    }
    if(source.binding.content_ref.conversation_id!==cut.conversation_id)throw new Error('Trace content source was replaced.');
    const cache=new ContentCache((reference,at)=>this.readContent(reference,at));
    try {const range=await cache.read(source.binding.content_ref,offset,current);return {...range,end:source.binding.content_ref.readable_end,representation:source.binding.content_ref.representation,cut};}
    finally{cache.clear();}
  }
  async readAttemptIdentity(reference:import('../../../protocol/app-server/v57.ts').AttemptIdentitySource,offset:number,
    capturedCut?:import('../../../protocol/app-server/v57.ts').CheckpointReadCut,agentId?:string) {
    const epoch=this.#port.epoch(),target=this.#port.target(),cut=capturedCut??this.#port.checkpoint()?.read_cut;
    const {domain,path,encodedBytes}=attemptIdentityField(reference),identity=attemptIdentitySourceKey(reference);
    if(!cut||!Number.isSafeInteger(encodedBytes)||encodedBytes<0||!Number.isSafeInteger(offset)||offset<0||offset>encodedBytes)throw new Error('Invalid native owner identity range.');
    const owned=()=>!this.#port.released()&&epoch===this.#port.epoch()&&(agentId!=null||this.deferredAttemptIdentities.some(field=>attemptIdentitySourceKey(field.source)===identity));
    if(!owned())throw new Error('Native owner identity view was replaced.');
    const {chunk}=await this.#port.call('conversation/checkpointValueRead',{target,agent_id:agentId??null,cut,domain,item_offset:0,path,offset,max_bytes:65536},'checkpoint_value_chunk');
    if(!owned()||readEvidence(chunk.cut)!==readEvidence(cut)||chunk.domain!==domain||chunk.item_offset!==0||chunk.representation!=='canonical_json_v1'||JSON.stringify(chunk.path)!==JSON.stringify(path)||chunk.offset!==offset||chunk.readable_end!==encodedBytes)throw new Error('Native owner identity evidence changed.');
    if(chunk.bytes_base64.length>87_384||chunk.bytes_base64.length%4!==0||chunk.bytes_base64.length/4*3-(chunk.bytes_base64.endsWith('==')?2:chunk.bytes_base64.endsWith('=')?1:0)>65_536)throw new Error('Encoded native identity exceeds the decoded byte bound.');
    const bytes=Buffer.from(chunk.bytes_base64,'base64'),end=offset+bytes.length;
    if(!Number.isSafeInteger(end)||bytes.length>65536||end>encodedBytes||offset<encodedBytes&&!bytes.length||(chunk.next_offset??null)!==(end<encodedBytes?end:null))throw new Error('Native owner identity did not advance exactly.');
    return {bytes,next:chunk.next_offset??undefined,end:encodedBytes,cut};
  }
  async readJournalField(reference:import('../../../protocol/app-server/v57.ts').JournalFieldRef,offset:number) {
    const checkpoint=this.#port.checkpoint(),epoch=this.#port.epoch(),target=this.#port.target();
    if(!checkpoint || !Number.isSafeInteger(offset)||offset<0 ||offset>reference.encoded_bytes)throw new Error('Invalid deferred native field range.');
    const identity=JSON.stringify(reference);
    if(!this.deferredJournalFields.some(field=>JSON.stringify(field.reference)===identity))throw new Error('Native field view was replaced.');
    const domain=`journal.${reference.sequence}`;
    const {chunk}=await this.#port.call('conversation/checkpointValueRead',{target,agent_id:null,cut:checkpoint.read_cut,domain,item_offset:0,path:reference.path,offset,max_bytes:65536},'checkpoint_value_chunk');
    if(this.#port.released() ||epoch!==this.#port.epoch() ||!this.deferredJournalFields.some(field=>JSON.stringify(field.reference)===identity) ||readEvidence(chunk.cut)!==readEvidence(checkpoint.read_cut) ||chunk.domain!==domain ||chunk.item_offset!==0 ||chunk.representation!=='canonical_json_v1' ||JSON.stringify(chunk.path)!==JSON.stringify(reference.path) ||chunk.offset!==offset ||chunk.readable_end!==reference.encoded_bytes)throw new Error('Deferred native field evidence changed.');
    if(chunk.bytes_base64.length>87_384 || chunk.bytes_base64.length%4!==0 || chunk.bytes_base64.length/4*3-(chunk.bytes_base64.endsWith('==')?2:chunk.bytes_base64.endsWith('=')?1:0)>65_536)throw new Error('Encoded native range exceeds the decoded byte bound.');
    const bytes=Buffer.from(chunk.bytes_base64,'base64');
    if(bytes.length>65536 ||offset+bytes.length>chunk.readable_end ||offset<chunk.readable_end&&bytes.length===0 ||(chunk.next_offset==null?offset+bytes.length!==chunk.readable_end:chunk.next_offset!==offset+bytes.length||chunk.next_offset>=chunk.readable_end))throw new Error('Deferred native field did not advance exactly.');
    return {bytes,next:chunk.next_offset??undefined};
  }

  async readChildJournalField(agentId:string,cut:import('../../../protocol/app-server/v57.ts').CheckpointReadCut,reference:import('../../../protocol/app-server/v57.ts').JournalFieldRef,offset:number) {
    if(!Number.isSafeInteger(offset)||offset<0||offset>reference.encoded_bytes)throw new Error('Invalid Child result byte range.');
    const epoch=this.#port.epoch(),target=this.#port.target(),domain=`journal.${reference.sequence}`;
    const {chunk}=await this.#port.call('conversation/checkpointValueRead',{target,agent_id:agentId,cut,domain,item_offset:0,path:reference.path,offset,max_bytes:65536},'checkpoint_value_chunk');
    if(chunk.bytes_base64.length>87_384 || chunk.bytes_base64.length%4!==0 || chunk.bytes_base64.length/4*3-(chunk.bytes_base64.endsWith('==')?2:chunk.bytes_base64.endsWith('=')?1:0)>65_536)throw new Error('Encoded native range exceeds the decoded byte bound.');
    const bytes=Buffer.from(chunk.bytes_base64,'base64');
    if(this.#port.released()||epoch!==this.#port.epoch()||readEvidence(chunk.cut)!==readEvidence(cut)||chunk.domain!==domain||chunk.item_offset!==0||chunk.representation!=='canonical_json_v1'||JSON.stringify(chunk.path)!==JSON.stringify(reference.path)||chunk.offset!==offset||chunk.readable_end!==reference.encoded_bytes||bytes.length>65536||offset+bytes.length>chunk.readable_end||offset<chunk.readable_end&&bytes.length===0||(chunk.next_offset==null?offset+bytes.length!==chunk.readable_end:chunk.next_offset!==offset+bytes.length||chunk.next_offset>=chunk.readable_end))throw new Error('Child result evidence changed.');
    return {bytes,next:chunk.next_offset??undefined};
  }


  /**
   * Reads one bounded durable transcript page by its exclusive older boundary.
   * Callers pass the previous page's `next_cursor` unchanged.
   */
  async transcriptPage(
    beforeCursor?: RuntimeClientTranscriptCursor,
    limit = TRANSCRIPT_PROJECTION_PAGE_LIMIT,
  ): Promise<RuntimeClientTranscriptPage> {
    const page = await this.#port.call(
      "session/transcript",
      { target: this.#port.target(), at: beforeCursor ? { type: 'older', before: beforeCursor } : { type: 'latest' }, limit },
      "transcript_window",
    );
    return page.window.page;
  }

  /**
   * Whether this connection's transport was granted delivery access: the
   * owned stdio child, or the separate remote delivery credential.
   */
  get deliveryAccess(): boolean {
    return this.#port.client().capabilities?.delivery_access === true;
  }

  /** One bounded page of committed deliveries, by the same transcript paging. */
  async deliveryPage(before?: RuntimeClientTranscriptCursor,capturedCut?:import('../../../protocol/app-server/v57.ts').CheckpointReadCut): Promise<DeliveryPage> {
    const cut=capturedCut??this.#port.checkpoint()?.read_cut,epoch=this.#port.epoch();
    if(!cut)throw new Error('Load a native checkpoint first.');
    const offset=before==null?0:Number(BigInt(before));if(!Number.isSafeInteger(offset)||offset<0)throw new Error('Native delivery history coordinate is unavailable.');
    const {page}=await this.#port.call('conversation/checkpointPage',{target:this.#port.target(),agent_id:null,cut,domain:'transcript.entries',offset,limit:32},'checkpoint_page');
    if(this.#port.released()||epoch!==this.#port.epoch()||readEvidence(page.read_cut)!==readEvidence(cut)||page.domain!=='transcript.entries'||page.items.length>32)throw new Error('Native delivery history cut changed.');
    const descriptors=page.items.flatMap(item=>{const value=messageDescriptor(item.value);return value?[value]:[];});
    if(before==null&&readEvidence(this.#port.checkpoint()!.read_cut)===readEvidence(cut))descriptors.push(...this.#port.state().messageDescriptors??[]);
    const records:DeliveryRecord[]=[],collections:NonNullable<DeliveryPage['collections']>=[],covered=new Set(descriptors.filter(row=>row.deliveries&&row.identity!=null).map(row=>row.identity));
    for(const descriptor of descriptors)if(descriptor.deliveries){const head=deliveryHead(descriptor.deliveries);for(const item of head.items){deliveryItem(item);records.push({conversationId:cut.conversation_id,locator:item.locator,index:item.index,count:head.total,file:item.file??undefined,metadata:item.file?undefined:{item,cut}});}if(head.next_offset!=null&&!collections.some(row=>row.head.domain===head.domain))collections.push({head,cut});}
    for(const row of page.items){const value=row.value as import('../../../protocol/app-server/v57.ts').RuntimeClientTranscriptEntry|null;
      if(value?.item?.type==='message'&&!covered.has(value.item.message.id))records.push(...pageDeliveries({entries:[value]},cut.conversation_id).records);}
    const unique=new Map(records.map(record=>[deliveryItemKey(record),record]));
    return {records:[...unique.values()],cut,collections,next:page.next_offset==null?undefined:String(page.next_offset)};
  }
  async deliveryCollection(collection:NonNullable<DeliveryPage['collections']>[number]):Promise<DeliveryPage>{
    const {head,cut}=collection,epoch=this.#port.epoch();if(head.next_offset==null)throw new Error('Delivery collection is complete.');
    const {page}=await this.#port.call('conversation/checkpointPage',{target:this.#port.target(),agent_id:null,cut,domain:head.domain,offset:head.next_offset,limit:8},'checkpoint_page');
    if(this.#port.released()||epoch!==this.#port.epoch())throw new Error('Native delivery view changed.');const next=nativeDeliveryPage(head,page,cut,head.next_offset);
    return {cut,records:next.items.map(item=>({conversationId:cut.conversation_id,locator:item.locator,index:item.index,count:head.total,file:item.file??undefined,metadata:item.file?undefined:{item,cut}})),collections:next.next_offset==null?[]:[{head:next,cut}]};
  }
  async deliveryMetadata(record:DeliveryRecord,current:()=>boolean):Promise<SessionFileReference>{
    if(record.file)return record.file;const selected=record.metadata,epoch=this.#port.epoch();
    if(!selected||record.conversationId!==this.#port.target().conversation_id||selected.cut.conversation_id!==record.conversationId)throw new Error('Native delivery metadata source is unavailable.');
    const owned=()=>current()&&!this.#port.released()&&epoch===this.#port.epoch();
    return readOriginalDeliveryMetadata(selected.item,selected.cut,async offset=>(await this.#port.call('conversation/checkpointValueRead',{target:this.#port.target(),agent_id:null,cut:selected.cut,domain:selected.item.metadata_domain,item_offset:0,path:[],offset,max_bytes:65536},'checkpoint_value_chunk')).chunk,owned);
  }

  /**
   * Original bytes (≤ 512 KiB, base64) of one committed delivery, through
   * this attachment. Native authority resolves the address every time; no
   * model request, Agent, or Tool execution starts. Aborting `signal`
   * cancels this exact native request on the server; the call then settles
   * with the server's terminal outcome (bytes only if publication won).
   */
  async readDelivery(
    record: Pick<DeliveryRecord, "conversationId" | "locator" | "index">,
    signal: AbortSignal = new AbortController().signal,
  ): Promise<{ file: SessionFileReference; data: string }> {
    if(record.conversationId!==this.#port.target().conversation_id)throw new Error('Native delivery locator source changed.');
    const { file, data } = await this.#port.client().callDelivery(
      "delivery/read",
      { target: this.#port.target(), locator: record.locator, delivery_index: record.index },
      "session_file_bytes",
      signal,
    );
    return { file, data };
  }

  /**
   * The verified server-side path and leaf identity of one delivery.
   * Aborting `signal` cancels the native request (see `readDelivery`).
   */
  async locateDelivery(
    record: Pick<DeliveryRecord, "conversationId" | "locator" | "index">,
    signal: AbortSignal = new AbortController().signal,
  ): Promise<DeliveryLocation> {
    if(record.conversationId!==this.#port.target().conversation_id)throw new Error('Native delivery locator source changed.');
    const { file, path, device, inode } = await this.#port.client().callDelivery(
      "delivery/locate",
      { target: this.#port.target(), locator: record.locator, delivery_index: record.index },
      "session_file_location",
      signal,
    );
    return { file, path, device, inode };
  }

  async observeAgent(agentId: string, after: import('../../../protocol/app-server/v57.ts').AgentObservationCursor | undefined, signal: AbortSignal) {
    const epoch = this.#port.epoch();
    const result = await this.#port.client().observe({ target: this.#port.target(), agent_id: agentId, after: after ?? null }, signal);
    if (this.#port.released() || epoch !== this.#port.epoch() || signal.aborted) throw new Error('Child observation view was replaced.');
    return result.observation;
  }
  /** One original occupancy-model field range at its captured native cut. */
  async readOccupancyModel(offset: number, capturedCut?: import('../../../protocol/app-server/v57.ts').CheckpointReadCut) {
    const occupancy = this.#port.state().context.occupancy, domain = occupancy?.model_source;
    const cut = capturedCut ?? this.#port.checkpoint()?.read_cut, epoch = this.#port.epoch(), target = this.#port.target();
    if (!occupancy?.model_deferred || !domain || !cut || !Number.isSafeInteger(offset) || offset < 0)
      throw new Error('Original context model source unavailable.');
    const {chunk} = await this.#port.call('conversation/checkpointValueRead', {
      target, agent_id:null, cut, domain, item_offset:0, path:[], offset, max_bytes:65_536,
    }, 'checkpoint_value_chunk');
    if (this.#port.released() || epoch !== this.#port.epoch() || !sameTarget(target,this.#port.target())
      || this.#port.state().context.occupancy?.model_source !== domain || readEvidence(chunk.cut) !== readEvidence(cut)
      || chunk.domain !== domain || chunk.item_offset !== 0 || chunk.path.length !== 0
      || chunk.offset !== offset || chunk.representation !== 'canonical_json_v1')
      throw new Error('Original context model view was replaced.');
    const encoded = chunk.bytes_base64;
    if (encoded.length > 87_384 || encoded.length % 4 !== 0
      || encoded.length / 4 * 3 - (encoded.endsWith('==') ? 2 : encoded.endsWith('=') ? 1 : 0) > 65_536)
      throw new Error('Encoded native range exceeds the decoded byte bound.');
    const bytes = Buffer.from(encoded,'base64'), end = offset + bytes.length;
    if (!Number.isSafeInteger(chunk.readable_end) || !Number.isSafeInteger(end) || end > chunk.readable_end
      || bytes.length > 65_536 || offset < chunk.readable_end && bytes.length === 0
      || (chunk.next_offset ?? null) !== (end < chunk.readable_end ? end : null))
      throw new Error('Original context model range did not advance exactly.');
    return {bytes,next:chunk.next_offset ?? undefined,end:chunk.readable_end,cut};
  }
  async readContent(content_ref: import('../../../protocol/app-server/v57.ts').ContentRef, offset: number, agentId?: string) {
    const epoch = this.#port.epoch();
    const { chunk } = await this.#port.call('conversation/contentRead', { target: this.#port.target(), agent_id: agentId ?? null, content_ref, offset, max_bytes: 65_536 }, 'content_chunk');
    if (this.#port.released() || epoch !== this.#port.epoch()) throw new Error('Content view was replaced.');
    return chunk;
  }

  /** A read is valid only in the exact parent attachment/presentation epoch. */
  async agentTranscriptPage(
    agentId: AgentId,
    before?: RuntimeClientTranscriptCursor,
  ): Promise<RuntimeClientTranscriptPage | undefined> {
    if (this.#port.released() || this.#port.serverClosed()) return undefined;
    const epoch = this.#port.epoch();
    const target = this.#port.target();
    try {
      const result = await this.#port.call("agent/transcript", {
        target, agent_id: agentId, at: before == null ? { type: "latest" } : { type: "older", before, cut: null },
        limit: TRANSCRIPT_PROJECTION_PAGE_LIMIT,
      }, "transcript_window");
      if (epoch !== this.#port.epoch() || !sameTarget(target, this.#port.target())) return undefined;
      return result.window.page;
    } catch (error) {
      if (epoch !== this.#port.epoch() || !sameTarget(target, this.#port.target())) return undefined;
      throw error;
    }
  }

  /** Reads one bounded page of historical fork/branch boundaries. */
  async boundaries(
    offset = 0,
    limit = SESSION_PROJECTION_PAGE_LIMIT,
  ): Promise<BoundaryPage> {
    const page = await this.#port.call(
      "session/boundaries",
      { target: this.#port.target(), offset, limit },
      "boundaries",
    );
    return {
      surfaceRevision: page.surface_revision,
      boundaries: page.boundaries,
      nextOffset: page.next_offset ?? undefined,
    };
  }

  /** A domain reader fenced by the Session epoch it was created in. */
  domainReader(): DomainReader {
    const epoch = this.#port.epoch(), target = this.#port.target();
    return {current:()=>!this.#port.released() && !this.#port.serverClosed() && epoch===this.#port.epoch(),
      page:async(page,offset)=>(await this.#port.call('conversation/checkpointPage',{target,agent_id:null,cut:page.read_cut,domain:page.domain,offset,limit:32},'checkpoint_page')).page,
      value:async(page,item,offset)=>(await this.#port.call('conversation/checkpointValueRead',{target,agent_id:null,cut:page.read_cut,domain:page.domain,item_offset:Number(item.offset),path:[],offset,max_bytes:65536},'checkpoint_value_chunk')).chunk};
  }
  /** Read a finite native domain page without loading deferred item bodies. */
  async observationPage(domain:string,offset=0,previous?:import('../../../protocol/app-server/v57.ts').CheckpointPage,owned:()=>boolean=()=>true) {
    const source=this.observationCollections.find(row=>row.frontier.domain===domain),inspection=source?.inspection;
    const cut=previous?.read_cut??source?.cut??this.#port.checkpoint()?.read_cut,epoch=this.#port.epoch(),target=this.#port.target();
    if(!cut||!owned()||!Number.isSafeInteger(offset)||offset<0||previous&&previous.domain!==domain)throw new Error('Invalid observation page ownership.');
    const {page}=await this.#port.call('conversation/checkpointPage',{target,agent_id:null,cut,domain,offset,limit:32},'checkpoint_page');
    if(this.#port.released()||this.#port.serverClosed()||epoch!==this.#port.epoch()||!owned()||inspection&&this.#currentTraceInspection()!==inspection||page.domain!==domain||page.cursor!==cut.revision||readEvidence(page.read_cut)!==readEvidence(cut)||page.items.length>32)throw new Error('Observation page was replaced.');
    const selected=this.#currentTraceInspection(),content=selected?.head?.content;
    if(inspection&&selected&&source&&/^messages\..+\.(?:blocks|tools)$/.test(domain)){
      validateCanonicalInspectionPage(page,offset,source.frontier.total,previous?.domain===domain&&previous.next_offset===String(offset)||source.frontier.next_offset===String(offset));selected.collection=page;
    }
    if(inspection&&selected?.page&&content?.type==='messages'&&domain===content.page.domain){
      const next=BigInt(offset)+BigInt(page.items.length),total=BigInt(content.page.total);
      if(page.total!==content.page.total||next>total||page.items.some((item,index)=>item.offset!==String(BigInt(offset)+BigInt(index)))
        ||(page.next_offset??null)!==(next<total?String(next):null)||next<total&&!page.items.length)throw new Error('Original Trace message page failed to advance.');
      selected.collection=undefined;selected.messages=page.items.map(item=>traceInspectionMessage(item.value));
    }
    return page;
  }
  async observationValue(page:import('../../../protocol/app-server/v57.ts').CheckpointPage,itemOffset:number,path:string[],offset:number,owned:()=>boolean=()=>true,expectedEnd?:number) {
    if(/^messages\..+\.(?:blocks|tools)$/.test(page.domain))throw new Error('Canonical source bytes use their advertised content references.');
    if(!Number.isSafeInteger(itemOffset)||itemOffset<0||!page.items.some(item=>item.offset===String(itemOffset)))throw new Error('Invalid selected observation range.');
    const epoch=this.#port.epoch(),target=this.#port.target(),current=()=>!this.#port.released()&&!this.#port.serverClosed()&&epoch===this.#port.epoch()&&owned();
    validateObservationCut(page.read_cut,target.conversation_id,null,page.cursor);
    const range=await readTraceSelectedWindow({domain:page.domain,item_offset:itemOffset,path},page.read_cut,offset,async at=>(await this.#port.call('conversation/checkpointValueRead',{target,agent_id:null,cut:page.read_cut,domain:page.domain,item_offset:itemOffset,path,offset:at,max_bytes:65536},'checkpoint_value_chunk')).chunk,current,expectedEnd);
    return {...range,end:range.readable_end};
  }
  async checkpointPage(domain: string, offset=0, previous?: import('../../../protocol/app-server/v57.ts').CheckpointPage) {
    const cut=previous?.read_cut ?? this.#port.checkpoint()?.read_cut;
    if (!cut) throw new Error('Load a checkpoint first.');
    const page=(await this.#port.call('conversation/checkpointPage',{target:this.#port.target(),agent_id:null,cut,domain,offset,limit:32},'checkpoint_page')).page;
    return {page,items:await domainItems<unknown>(page,this.domainReader())};
  }
  async interactionValue(interaction: InteractionRef, itemOffset: number, page: import('../../../protocol/app-server/v57.ts').CheckpointPage | undefined, offset: number, path: string[] = []) {
    const hasIdentity=this.#port.state().pendingInteractions.some(value=>value.interaction.conversation_id===interaction.conversation_id && value.interaction.interaction_id===interaction.interaction_id && !('request' in value) && Number(value.source_offset)===itemOffset)
      || page?.items.some(value=>{const header=value.value as import('../../../protocol/app-server/v57.ts').RuntimeClientInteractionHeader | null;return Number(value.offset)===itemOffset && Number(header?.source_offset)===itemOffset && header?.interaction?.conversation_id===interaction.conversation_id && header.interaction.interaction_id===interaction.interaction_id;});
    if(!Number.isSafeInteger(itemOffset) || itemOffset<0 || !hasIdentity) throw new Error('Interaction view no longer owns this identity.');
    const cut=page?.read_cut ?? this.#port.checkpoint()?.read_cut;
    if(!cut) throw new Error('Load the native checkpoint first.');
    const epoch=this.#port.epoch(), target=this.#port.target();
    const {chunk}=await this.#port.call('conversation/checkpointValueRead',{target,agent_id:null,cut,domain:'pending_interactions',item_offset:itemOffset,path,offset,max_bytes:65536},'checkpoint_value_chunk');
    if(this.#port.released() || epoch!==this.#port.epoch() || !sameTarget(target,this.#port.target()) || JSON.stringify(chunk.cut)!==JSON.stringify(cut) || chunk.domain!=='pending_interactions' || chunk.item_offset!==itemOffset || chunk.offset!==offset || JSON.stringify(chunk.path)!==JSON.stringify(path)) throw new Error('Interaction body read authority changed.');
    return chunk;
  }
  /** One bounded capability page, preserving deferred fields and continuation. */
  async capabilitiesPage(): Promise<import('../../../protocol/app-server/v57.ts').CheckpointPage> {
    const target = this.#port.target();
    const { page } = await this.#port.call('resources/read', { target }, 'domain_page');
    if (this.#port.target() !== target) throw new Error('Capability observation was replaced.');
    return page;
  }

  async projections(after = "0") {
    return (await this.#port.client().call("session/projections", { target:this.#port.target(), after, limit:16 }, "projections")).page;
  }
  async projectionDetail(checkpointId:string, offset = 0, owned:()=>boolean=()=>true) {
    this.#traceInspection=undefined;
    const selected=this.#projectionInspection={epoch:this.#port.epoch(),owned};
    const detail=(await this.#port.call("session/projectionDetail", { target:this.#port.target(), checkpoint_id:checkpointId, offset, limit:16 }, "projection_detail")).detail;
    if(this.#port.released()||this.#port.serverClosed()||selected.epoch!==this.#port.epoch()||this.#projectionInspection!==selected||!owned())throw new Error('Projection inspection viewport was replaced.');
    validateObservationCut(detail.read_cut,this.#port.target().conversation_id,null,detail.read_cut.revision);
    if(detail.checkpoint.checkpoint_id!==checkpointId||detail.items.some(item=>item.occurrence.checkpoint_id!==checkpointId))throw new Error('Projection inspection checkpoint changed.');
    Object.assign(selected,{detail});
    return detail;
  }
}

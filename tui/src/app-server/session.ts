import { ContentCache } from '../../../protocol/app-server/content-cache.ts';
import {traceInspectionHead,traceInspectionMessage,validateCanonicalInspectionPage} from '../../../protocol/app-server/trace-inspection.ts';
import {traceFields,traceFieldKey} from '../../../protocol/app-server/trace-identity.ts';
import {readTraceSelectedWindow,readTraceFieldWindow} from '../../../protocol/app-server/trace-field.ts';
import { messageDescriptor } from '../../../protocol/app-server/message-descriptor.ts';
import { deliveryHead,deliveryItem,deliveryItemKey,deliveryPage as nativeDeliveryPage,readDeliveryMetadata as readOriginalDeliveryMetadata } from '../../../protocol/app-server/delivery-observation.ts';
import { attemptIdentityField, attemptIdentitySourceKey } from '../../../protocol/app-server/attempt-identity.ts';
import { deferredAttemptIdentities } from '../presentation/message-descriptors.ts';
import { deferredJournalFields as nativeJournalFields } from './journal-fields.ts';
import { materializeApplication } from '../../../protocol/app-server/application-observation.ts';
import { UncertainOutcomeError, AppServerRequestError } from './client.ts';
import { materializeEffective } from '../../../protocol/app-server/effective-observation.ts';
import { domainItems, domainValue, type DomainReader } from '../../../protocol/app-server/domain-observation.ts';
import { checkpointSnapshot } from '../../../protocol/app-server/projection.ts';
import { ObservationChain, validateObservationCut, validateRootCheckpoint, validateRootEvent } from '../../../protocol/app-server/observation.ts';
import { uploadOperationId } from '../../../protocol/app-server/upload.ts';
/**
 * One attached Session: its authoritative projection and its typed operations.
 *
 * ```text
 * session/attach -> bounded Target receipt -> checkpoint -> subscribe receipt
 *                -> fold consecutive registration-scoped events
 * ```
 *
 * The locally Applied Cursor advances only after a successful complete fold.
 * Recovery installs an exact checkpoint, then registers after that cut. Cache
 * eviction never changes execution facts or the Applied Cursor.
 *
 * # Identity and fencing
 *
 * An {@link AttachmentTarget} names four distinct domains at once: the Session,
 * the Conversation, the runtime incarnation, and this attachment. They are not
 * aliases of each other, and neither is the projection cursor or a JSON-RPC
 * request id. Every notification repeats the full target, and this object folds
 * an event only when all four match — so an event addressed to a previous
 * incarnation of the same Session cannot reach a replacement projection.
 *
 * Responses are fenced the same way, by epoch: an authoritative read issued
 * against one attachment can still be in flight when the attachment is replaced
 * or released, and installing its result afterwards would overwrite current
 * truth with stale truth. The epoch check makes that impossible without any
 * reliance on timing.
 *
 * # Resync
 *
 * `session/resyncRequired` means the incremental projection can no longer be
 * trusted. The response is never to guess at the gap:
 *
 * ```text
 * incremental state no longer trusted
 *   -> session/checkpoint
 *   -> replace the projection wholesale
 *   -> session/subscribe after the new cursor
 * ```
 *
 * Nothing is replayed from what the UI thought happened.
 */

import {
  compareExact,
  sameTarget,
  type ConfigurationApplication,
  type AvailableConfiguration,
  type AttachmentTarget,
  type MethodName,
  type MethodParams,
  type ResultType,
  type ResultOf,
  type GoalControl,
  type RuntimeClientGoalView,
  type InteractionRef,
  type InteractionResponse,
  type ModelCatalogView,
  type Notification,
  type RuntimeClientJob,
  type RuntimeClientContextView,
  type RuntimeClientCursor,
  type RuntimeClientSnapshot,
  type RuntimeClientAgent,
  type RuntimeClientTranscriptCursor,
  type RuntimeClientTranscriptPage,
  type SessionFileReference,
  type SessionModelConfig,
  type SessionModelView,
  type SessionNodeId,
  type SessionUserMessageBoundaryObservation,
  type AgentId,
  type SurfaceRevision,
  type ToolExecutionId,
  type UserInputBlock,
} from "../protocol/app-server.ts";
import {
  deferredContent,
  presentationCheckpoint,
  mergeTranscriptPage,
  reduce,
  replaceFromSnapshot,
} from "../presentation/projection.ts";
import type { PresentationState } from "../presentation/state.ts";
import {
  pageDeliveries,
  type DeliveryLocation,
  type DeliveryPage,
  type DeliveryRecord,
} from "../presentation/deliveries.ts";
import { AppServerClient, METHOD_RESPONSE_LOSS_CLASS, isResyncRequired, isUncertainOutcome } from "./client.ts";

/** Native owner bound for one bounded page. */
export const SESSION_PROJECTION_PAGE_LIMIT = 32;
export const TRANSCRIPT_PROJECTION_PAGE_LIMIT = 32;

type StateListener = (state: PresentationState) => void;
type SnapshotListener = () => void;
type ClosedListener = () => void;

/** One bounded page of historical fork/branch boundaries. */
export interface BoundaryPage {
  /** The exact committed head the page was selected against. */
  surfaceRevision: SurfaceRevision;
  boundaries: SessionUserMessageBoundaryObservation[];
  nextOffset?: number;
}

function readEvidence(value:unknown):string {
  return JSON.stringify(value,(_key,item)=>item&&typeof item==='object'&&!Array.isArray(item)?Object.fromEntries(Object.entries(item).filter(([,value])=>value!==undefined).sort(([left],[right])=>left.localeCompare(right))):item);
}

export class AppServerSession {
  readonly nodeId: SessionNodeId | undefined;
  readonly #client: AppServerClient;
  #target: AttachmentTarget;
  #state: PresentationState;
  #configuration?: ConfigurationApplication;
  get application(): ConfigurationApplication | undefined { return this.#configuration; }
  readonly #listeners = new Set<StateListener>();
  readonly #snapshotListeners = new Set<SnapshotListener>();
  readonly #closedListeners = new Set<ClosedListener>();
  #resyncCount = 0;
  #observationError?:string;
  get observationError(){return this.#observationError;}
  /** Advanced whenever authoritative ownership of this projection changes. */
  #epoch = 0;
  #released = false;
  #serverClosed = false;
  #modelMutationInFlight = false;
  /** Local evidence about unanswered requests, never authoritative Session state. */
  readonly #unknownModelSelections: Set<string>;
  /** Serializes repairs so two resyncs cannot interleave their installs. */
  #repair: Promise<void> | undefined;
  readonly #observation = new ObservationChain<Parameters<typeof reduce>[1]['event']>();
  #checkpoint?: import('../../../protocol/app-server/v50.ts').RuntimeClientCheckpoint;
  // One explicit bounded inspection viewport, independent of projection/Applied.
  #traceInspection?: {epoch:number;id:string;owned:()=>boolean;head?:import('../../../protocol/app-server/v50.ts').TraceInspectionHead;page?:import('../../../protocol/app-server/v50.ts').CheckpointPage;messages?:import('../../../protocol/app-server/v50.ts').TraceInspectionMessage[];collection?:import('../../../protocol/app-server/v50.ts').CheckpointPage};
  #projectionInspection?: {epoch:number;owned:()=>boolean;detail?:import('../../../protocol/app-server/v50.ts').ProjectionDetail};
  #currentTraceInspection() {
    const selected=this.#traceInspection;
    return selected?.head&&selected.page&&!this.#released&&!this.#serverClosed&&selected.epoch===this.#epoch&&selected.owned() ? selected : undefined;
  }
  async inspectTrace(recordId:string,owned:()=>boolean=()=>true) {
    if(typeof recordId!=='string'||!recordId||recordId.length>256)throw new Error('Invalid native Trace selection.');
    if(!owned())throw new Error('Trace inspection viewport was replaced.');
    this.#projectionInspection=undefined;
    const selected=this.#traceInspection={epoch:this.#epoch,id:recordId,owned};
    const {page}=await this.#call('session/traceDetail',{target:this.#target,record_id:recordId},'domain_page');
    if(this.#released||this.#serverClosed||selected.epoch!==this.#epoch||this.#traceInspection!==selected||!owned())throw new Error('Trace inspection viewport was replaced.');
    validateObservationCut(page.read_cut,this.#target.conversation_id,null,page.cursor);
    const head=traceInspectionHead(page,recordId);
    if(!head)throw new Error('Selected Request fields are available through /content; this is not an inspection head.');
    Object.assign(selected,{head,page});
    return {head,page};
  }

  async agentTraceInspection(agent:RuntimeClientAgent,recordId:string,owned:()=>boolean) {
    const epoch=this.#epoch;
    const current=()=>!this.#released&&!this.#serverClosed&&epoch===this.#epoch&&owned();
    if(!current()||!recordId||recordId.length>256)throw new Error('Child Trace view was replaced.');
    const {page}=await this.#call('agent/traceDetail',{target:this.#target,agent_id:agent.agent_id,record_id:recordId},'domain_page');
    if(!current())throw new Error('Child Trace view was replaced.');
    validateObservationCut(page.read_cut,agent.child_conversation_id,agent.activation_id,page.cursor);
    const head=traceInspectionHead(page,recordId);
    if(!head)throw new Error('This selection is not a finite Trace inspection head.');
    return {head,page};
  }
  async agentInspectionPage(agent:RuntimeClientAgent,cut:import('../../../protocol/app-server/v50.ts').CheckpointReadCut,domain:string,offset:number,owned:()=>boolean) {
    const epoch=this.#epoch;
    const current=()=>!this.#released&&!this.#serverClosed&&epoch===this.#epoch&&owned();
    validateObservationCut(cut,agent.child_conversation_id,agent.activation_id,cut.revision);
    if(!current())throw new Error('Child Trace view was replaced.');
    const {page}=await this.#call('conversation/checkpointPage',{target:this.#target,agent_id:agent.agent_id,cut,domain,offset,limit:32},'checkpoint_page');
    if(!current()||page.domain!==domain||readEvidence(page.read_cut)!==readEvidence(cut)||page.cursor!==cut.revision)throw new Error('Child Trace page was replaced.');
    return page;
  }
  async readAgentTraceField(agent:RuntimeClientAgent,cut:import('../../../protocol/app-server/v50.ts').CheckpointReadCut,source:import('../../../protocol/app-server/v50.ts').TraceFieldSource,offset:number,owned:()=>boolean) {
    const epoch=this.#epoch,target=this.#target;
    const current=()=>!this.#released&&!this.#serverClosed&&epoch===this.#epoch&&owned();
    validateObservationCut(cut,agent.child_conversation_id,agent.activation_id,cut.revision);
    if(!current())throw new Error('Child Trace field was replaced.');
    if(source.type==='value') {
      const range=await readTraceFieldWindow(source,cut,offset,async at=>(await this.#call('conversation/checkpointValueRead',{target,agent_id:agent.agent_id,cut,domain:source.domain,item_offset:source.item_offset,path:source.path,offset:at,max_bytes:65536},'checkpoint_value_chunk')).chunk,current);
      return {...range,end:source.encoded_bytes,cut};
    }
    if(source.binding.content_ref.conversation_id!==cut.conversation_id)throw new Error('Child Trace content source was replaced.');
    const cache=new ContentCache((reference,at)=>this.readContent(reference,at,agent.agent_id));
    try {const range=await cache.read(source.binding.content_ref,offset,current);return {...range,end:source.binding.content_ref.readable_end,representation:source.binding.content_ref.representation,cut};}
    finally{cache.clear();}
  }

  get observationPhase() { return this.#observation.phase; }
  get controlReady() { return !this.#released && !this.#serverClosed && this.#observation.phase === 'following'; }
  get checkpoint() { return this.#checkpoint; }

  #assertControlReady(): void {
    if (!this.controlReady) {
      throw new Error('Session observation is not following; request not sent.');
    }
  }

  /** New effects require the same accepted observation baseline as the editor. */
  #call<M extends Exclude<MethodName, 'delivery/cancel'>, T extends ResultType>(
    method: M, params: MethodParams<M>, expected: T,
  ): Promise<ResultOf<T>> {
    const goalRead = method === 'goal/control' && 'control' in params && params.control.action === 'show';
    if ('target' in params && 'session_id' in params.target
      && METHOD_RESPONSE_LOSS_CLASS[method] === 'side_effecting' && !goalRead
      && method !== 'session/detach' && method !== 'session/switchNode') {
      try { this.#assertControlReady(); } catch (error) { return Promise.reject(error); }
    }
    return this.#client.call(method, params, expected);
  }
  get deferredContent() { return deferredContent(this.#state); }
  get deferredJournalFields() {
    const fields = nativeJournalFields(this.#state.attempt), clock = this.#state.statistics?.latest_turn;
    if (clock?.attempt_id_deferred && clock.attempt_id_source) fields.push({ label: 'Turn attempt identity', reference: clock.attempt_id_source });
    return fields;
  }
  get deferredAttemptIdentities() {
    const fields=deferredAttemptIdentities(this.#state),checkpoint=this.#checkpoint;
    if(checkpoint)for(const record of checkpoint.snapshot.trace.records){const source=record.location.attempt_id_source;if(source&&!fields.some(field=>attemptIdentitySourceKey(field.source)===attemptIdentitySourceKey(source)))fields.push({owner:`Trace ${record.id}`,source});}
    return fields;
  }
  get observationCollections() {
    const checkpoint=this.#checkpoint;
    if(!checkpoint)return [];
    const rows=new Map<string,{frontier:Pick<import('../../../protocol/app-server/v50.ts').CollectionFrontier,'domain'|'total'|'loaded'|'next_offset'>;cut:import('../../../protocol/app-server/v50.ts').CheckpointReadCut;inspection?:object}>((checkpoint.collections??[]).map(frontier=>[frontier.domain,{frontier,cut:checkpoint.read_cut}]));
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
    const checkpoint=this.#checkpoint;
    if(!checkpoint)return [];
    const fields=new Map<string,{record:string;source:import('../../../protocol/app-server/v50.ts').TraceFieldSource;cut:import('../../../protocol/app-server/v50.ts').CheckpointReadCut;inspection?:object;projection?:object}>();
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
    if(projection?.detail&&!this.#released&&!this.#serverClosed&&projection.epoch===this.#epoch&&projection.owned())for(const item of projection.detail.items){
      const source=item.content.provenance_source;
      if(source)fields.set(traceFieldKey(source),{record:`projection:${item.occurrence.checkpoint_id}/${item.occurrence.item_index}`,source,cut:projection.detail.read_cut,projection});
    }
    return [...fields.values()];
  }
  get observationContent() {
    const selected=this.#currentTraceInspection(),collection=selected?.collection;
    return selected&&collection?collection.items.flatMap(item=>item.content.map(binding=>({binding,cut:collection.read_cut,inspection:selected,page:collection,item:item.offset}))):[];
  }
  async readObservationContent(selected:AppServerSession['observationContent'][number],offset:number) {
    const epoch=this.#epoch,current=()=>!this.#released&&!this.#serverClosed&&epoch===this.#epoch&&this.#currentTraceInspection()===selected.inspection&&selected.inspection.collection===selected.page;
    const reference=selected.binding.content_ref;
    if(reference.conversation_id!==selected.cut.conversation_id||!current())throw new Error('Original observation content was replaced.');
    const cache=new ContentCache((ref,at)=>this.readContent(ref,at));
    try{return {...await cache.read(reference,offset,current),representation:reference.representation,end:reference.readable_end,cut:selected.cut};}finally{cache.clear();}
  }
  async readTraceField(selected:AppServerSession['deferredTraceFields'][number],offset:number) {
    const epoch=this.#epoch,target=this.#target,identity=traceFieldKey(selected.source),cut=selected.cut;
    const current=()=>!this.#released&&!this.#serverClosed&&epoch===this.#epoch&&(!selected.inspection||this.#traceInspection===selected.inspection)&&(!selected.projection||this.#projectionInspection===selected.projection)&&this.deferredTraceFields.some(field=>traceFieldKey(field.source)===identity&&readEvidence(field.cut)===readEvidence(cut)&&field.inspection===selected.inspection&&field.projection===selected.projection);
    if(cut.conversation_id!==target.conversation_id||cut.activation_id!=null||!current())throw new Error('Trace field source was replaced.');
    const source=selected.source;
    if(source.type==='value') {
      const range=await readTraceFieldWindow(source,cut,offset,async at=>(await this.#call('conversation/checkpointValueRead',{target,agent_id:null,cut,domain:source.domain,item_offset:source.item_offset,path:source.path,offset:at,max_bytes:65536},'checkpoint_value_chunk')).chunk,current);
      return {...range,end:source.encoded_bytes,cut};
    }
    if(source.binding.content_ref.conversation_id!==cut.conversation_id)throw new Error('Trace content source was replaced.');
    const cache=new ContentCache((reference,at)=>this.readContent(reference,at));
    try {const range=await cache.read(source.binding.content_ref,offset,current);return {...range,end:source.binding.content_ref.readable_end,representation:source.binding.content_ref.representation,cut};}
    finally{cache.clear();}
  }
  async readAttemptIdentity(reference:import('../../../protocol/app-server/v50.ts').AttemptIdentitySource,offset:number,
    capturedCut?:import('../../../protocol/app-server/v50.ts').CheckpointReadCut,agentId?:string) {
    const epoch=this.#epoch,target=this.#target,cut=capturedCut??this.#checkpoint?.read_cut;
    const {domain,path,encodedBytes}=attemptIdentityField(reference),identity=attemptIdentitySourceKey(reference);
    if(!cut||!Number.isSafeInteger(encodedBytes)||encodedBytes<0||!Number.isSafeInteger(offset)||offset<0||offset>encodedBytes)throw new Error('Invalid native owner identity range.');
    const owned=()=>!this.#released&&epoch===this.#epoch&&(agentId!=null||this.deferredAttemptIdentities.some(field=>attemptIdentitySourceKey(field.source)===identity));
    if(!owned())throw new Error('Native owner identity view was replaced.');
    const {chunk}=await this.#call('conversation/checkpointValueRead',{target,agent_id:agentId??null,cut,domain,item_offset:0,path,offset,max_bytes:65536},'checkpoint_value_chunk');
    if(!owned()||readEvidence(chunk.cut)!==readEvidence(cut)||chunk.domain!==domain||chunk.item_offset!==0||chunk.representation!=='canonical_json_v1'||JSON.stringify(chunk.path)!==JSON.stringify(path)||chunk.offset!==offset||chunk.readable_end!==encodedBytes)throw new Error('Native owner identity evidence changed.');
    if(chunk.bytes_base64.length>87_384||chunk.bytes_base64.length%4!==0||chunk.bytes_base64.length/4*3-(chunk.bytes_base64.endsWith('==')?2:chunk.bytes_base64.endsWith('=')?1:0)>65_536)throw new Error('Encoded native identity exceeds the decoded byte bound.');
    const bytes=Buffer.from(chunk.bytes_base64,'base64'),end=offset+bytes.length;
    if(!Number.isSafeInteger(end)||bytes.length>65536||end>encodedBytes||offset<encodedBytes&&!bytes.length||(chunk.next_offset??null)!==(end<encodedBytes?end:null))throw new Error('Native owner identity did not advance exactly.');
    return {bytes,next:chunk.next_offset??undefined,end:encodedBytes,cut};
  }
  async readJournalField(reference:import('../../../protocol/app-server/v50.ts').JournalFieldRef,offset:number) {
    const checkpoint=this.#checkpoint,epoch=this.#epoch,target=this.#target;
    if(!checkpoint || !Number.isSafeInteger(offset)||offset<0 ||offset>reference.encoded_bytes)throw new Error('Invalid deferred native field range.');
    const identity=JSON.stringify(reference);
    if(!this.deferredJournalFields.some(field=>JSON.stringify(field.reference)===identity))throw new Error('Native field view was replaced.');
    const domain=`journal.${reference.sequence}`;
    const {chunk}=await this.#call('conversation/checkpointValueRead',{target,agent_id:null,cut:checkpoint.read_cut,domain,item_offset:0,path:reference.path,offset,max_bytes:65536},'checkpoint_value_chunk');
    if(this.#released ||epoch!==this.#epoch ||!this.deferredJournalFields.some(field=>JSON.stringify(field.reference)===identity) ||readEvidence(chunk.cut)!==readEvidence(checkpoint.read_cut) ||chunk.domain!==domain ||chunk.item_offset!==0 ||chunk.representation!=='canonical_json_v1' ||JSON.stringify(chunk.path)!==JSON.stringify(reference.path) ||chunk.offset!==offset ||chunk.readable_end!==reference.encoded_bytes)throw new Error('Deferred native field evidence changed.');
    if(chunk.bytes_base64.length>87_384 || chunk.bytes_base64.length%4!==0 || chunk.bytes_base64.length/4*3-(chunk.bytes_base64.endsWith('==')?2:chunk.bytes_base64.endsWith('=')?1:0)>65_536)throw new Error('Encoded native range exceeds the decoded byte bound.');
    const bytes=Buffer.from(chunk.bytes_base64,'base64');
    if(bytes.length>65536 ||offset+bytes.length>chunk.readable_end ||offset<chunk.readable_end&&bytes.length===0 ||(chunk.next_offset==null?offset+bytes.length!==chunk.readable_end:chunk.next_offset!==offset+bytes.length||chunk.next_offset>=chunk.readable_end))throw new Error('Deferred native field did not advance exactly.');
    return {bytes,next:chunk.next_offset??undefined};
  }

  async readChildJournalField(agentId:string,cut:import('../../../protocol/app-server/v50.ts').CheckpointReadCut,reference:import('../../../protocol/app-server/v50.ts').JournalFieldRef,offset:number) {
    if(!Number.isSafeInteger(offset)||offset<0||offset>reference.encoded_bytes)throw new Error('Invalid Child result byte range.');
    const epoch=this.#epoch,target=this.#target,domain=`journal.${reference.sequence}`;
    const {chunk}=await this.#call('conversation/checkpointValueRead',{target,agent_id:agentId,cut,domain,item_offset:0,path:reference.path,offset,max_bytes:65536},'checkpoint_value_chunk');
    if(chunk.bytes_base64.length>87_384 || chunk.bytes_base64.length%4!==0 || chunk.bytes_base64.length/4*3-(chunk.bytes_base64.endsWith('==')?2:chunk.bytes_base64.endsWith('=')?1:0)>65_536)throw new Error('Encoded native range exceeds the decoded byte bound.');
    const bytes=Buffer.from(chunk.bytes_base64,'base64');
    if(this.#released||epoch!==this.#epoch||readEvidence(chunk.cut)!==readEvidence(cut)||chunk.domain!==domain||chunk.item_offset!==0||chunk.representation!=='canonical_json_v1'||JSON.stringify(chunk.path)!==JSON.stringify(reference.path)||chunk.offset!==offset||chunk.readable_end!==reference.encoded_bytes||bytes.length>65536||offset+bytes.length>chunk.readable_end||offset<chunk.readable_end&&bytes.length===0||(chunk.next_offset==null?offset+bytes.length!==chunk.readable_end:chunk.next_offset!==offset+bytes.length||chunk.next_offset>=chunk.readable_end))throw new Error('Child result evidence changed.');
    return {bytes,next:chunk.next_offset??undefined};
  }


  private constructor(
    client: AppServerClient,
    target: AttachmentTarget,
    state: PresentationState,
    nodeId?: SessionNodeId,
    unknownModelSelections = new Set<string>(),
  ) {
    this.nodeId = nodeId;
    this.#client = client;
    this.#target = target;
    this.#state = state;
    this.#unknownModelSelections = unknownModelSelections;
    this.#observation.installCheckpoint(state.cursor);
    client.onClose(() => {
      this.#released = true;
      this.#epoch += 1;
    });
  }

  /**
   * Attaches to a durable Session and installs its authoritative projection.
   *
   * Attach loads or reuses the runtime and acquires this connection's external
   * control of it. It is not a process operation: attaching to a second Session
   * never replaces the first, and both stay live in the same App Server.
   */
  static async attach(
    client: AppServerClient,
    sessionId: string,
    nodeId?: SessionNodeId,
    prepared?: (session: AppServerSession) => void,
    previous?: AppServerSession,
    unknownModelSelections = new Set<string>(),
  ): Promise<AppServerSession> {
    let attached: Extract<import('../../../protocol/app-server/v50.ts').MethodResult,{type:'attached'}>;
    try {
      attached = await client.call('session/attach', {session_id:sessionId,node_id:nodeId??null}, 'attached');
    } catch (error) {
      if (client.closed || !(error instanceof UncertainOutcomeError || error instanceof AppServerRequestError && error.kind === 'outcome_unknown')) throw error;
      const status = await client.call('session/attachmentStatus',{session_id:sessionId},'attachment_status');
      if (!status.target || status.target.session_id !== sessionId) throw error;
      const node = nodeId == null
        ? (await client.call('session/read',{session_id:sessionId},'session')).session.active_node : nodeId;
      const found = await client.call('session/node',{session_id:sessionId,node_id:node},'node');
      if (found.node.conversation_id !== status.target.conversation_id) throw error;
      attached = {type:'attached',target:status.target,configuration_revision:status.configuration_revision,attachment_kind:'controller',observation_source:'live'};
    }
    const applied=previous ? previous.#observation.applied : undefined;
    const resume=previous && client.authorityId!==undefined && client.authorityId===previous.#client.authorityId && applied!==undefined && previous.#state.cursor===applied && previous.#checkpoint?.cursor===applied
      && previous.#target.session_id===attached.target.session_id && previous.#target.conversation_id===attached.target.conversation_id
      && previous.#target.runtime_incarnation===attached.target.runtime_incarnation ? previous : undefined;
    const checkpoint=(resume ? resume.#checkpoint : undefined) ?? (await client.call('session/checkpoint',{target:attached.target,trace_records:[]},'checkpoint')).checkpoint;
    validateRootCheckpoint(attached.target, checkpoint, resume ? resume.#observation.applied : undefined);
    const session = new AppServerSession(client, attached.target,
      (resume ? resume.#state : undefined) ?? replaceFromSnapshot(checkpointSnapshot(checkpoint), checkpoint.cursor), nodeId,unknownModelSelections);
    session.#checkpoint = checkpoint;
    prepared?.(session);
    const stopTemporaryRouting = prepared ? () => {} : client.onNotification(notification => session.applyNotification(notification));
    try {
      if (session.#released || session.#serverClosed) return session;
      const epoch = session.#epoch, chain = session.#observation;
      session.#observation.beginSubscription(checkpoint.cursor);
      try {
        const receipt = await client.call('session/subscribe', { target: attached.target, after_cursor: checkpoint.cursor }, 'subscribed');
        if (epoch !== session.#epoch || session.#released || session.#serverClosed || session.#observation !== chain) return session;
        if (!sameTarget(receipt.target, attached.target) || !session.#observation.acceptReceipt(receipt.registration_id, receipt.after_cursor)) {
          session.#observationError ??= `Subscription receipt could not join Applied ${session.#observation.applied}; receipt cursor ${receipt.after_cursor}, phase ${session.#observation.phase}`;
          session.#publish();
          if (session.#observation.phase !== 'unavailable') await session.resync();
        }
        session.#publish();
      } catch(error) {if (session.#released || session.#serverClosed) return session;if(!isResyncRequired(error))throw error;await session.resync();}
      if (session.#released || session.#serverClosed) return session;
      const configuration = await client.call('session/configuration', { session_id: sessionId }, 'session_configuration');
      const application=await materializeApplication(configuration.projection,async(reference,offset)=>(await client.call('session/configurationValueRead',{session_id:sessionId,reference,offset,max_bytes:65536},'session_application_value_chunk')).chunk,()=>!session.#released);
      if(application)session.#installConfiguration(application);
    } finally { stopTemporaryRouting(); }
    return session;
  }

  /** The full four-domain identity of this attachment. */
  get target(): AttachmentTarget {
    return this.#target;
  }

  get sessionId(): string {
    return this.#target.session_id;
  }

  /** The current presentation state. Always a projection, never authority. */
  get state(): PresentationState {
    return this.#state;
  }

  /** How many authoritative repairs this attachment has performed. */
  get resyncCount(): number {
    return this.#resyncCount;
  }

  /** Whether the server reported this attachment closed. */
  get serverClosed(): boolean {
    return this.#serverClosed;
  }

  /** Whether this client has released the attachment. */
  get released(): boolean {
    return this.#released;
  }

  /** Subscribes to presentation state changes. */
  onState(listener: StateListener): () => void {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  }

  /**
   * Subscribes to authoritative projection replacement, including resync.
   *
   * The controlling TUI uses this signal to invalidate attachment-local
   * presentation leases. It carries no runtime semantics.
   */
  onSnapshot(listener: SnapshotListener): () => void {
    this.#snapshotListeners.add(listener);
    return () => this.#snapshotListeners.delete(listener);
  }

  /** Subscribes to the server reporting this attachment closed. */
  onClosed(listener: ClosedListener): () => void {
    this.#closedListeners.add(listener);
    return () => this.#closedListeners.delete(listener);
  }

  // -------------------------------------------------------------------------
  // Notification routing
  // -------------------------------------------------------------------------

  /**
   * Folds one routed notification, if it addresses exactly this attachment.
   *
   * Returns whether the notification belonged here. A notification for another
   * Session, or for a superseded incarnation or attachment of this Session, is
   * declined rather than applied: a stale target must never mutate the
   * projection that replaced it.
   */
  applyNotification(notification: Notification): boolean {
    if (notification.method === "configuration/changed") {
      const application = notification.params.application;
      if (this.#released || application.scope !== this.sessionId) return false;
      this.#observeConfiguration(application);
      return true;
    }
    if (notification.method === "session/summaryInvalidated" || notification.method === "session/ownershipRetired") {
      // Both explicit catalog_changed values are valid. The TUI holds
      // no live catalog/summary cache: `/resume` reads the catalog afresh every
      // time it opens, so there is nothing here to repair. The notification is
      // accepted and declined — never folded into the conversation projection,
      // and never treated as invalid protocol input.
      return false;
    }
    if (notification.method === "configuration/sourceRepairChanged") {
      // Source-scoped (Issue #464): it addresses a configuration source, never
      // a Session, and is never folded into conversation presentation.
      return false;
    }
    if (this.#released || this.#serverClosed || !sameTarget(notification.params.target, this.#target)) {
      return false;
    }
    switch (notification.method) {
      case "session/event": {
        const result = this.#observation.deliver({ registrationId: notification.params.registration_id, cursor: notification.params.cursor, event: notification.params.event },
          (event, cursor) => this.#applyEvent(cursor, event));
        if (result === 'recovery') {
          this.#observationError ??= `Observation continuity lost at cursor ${notification.params.cursor}; Applied ${this.#observation.applied}`;
          this.#publish();
          this.#enqueueRepair();
        }
        return true;
      }
      case "session/observationUnavailable": {
        const result = this.#observation.deliver({ registrationId: notification.params.registration_id, unavailable: true }, (event, cursor) => this.#applyEvent(cursor, event));
        if (result === 'unavailable') {
          this.#observationError = `Observation source unavailable: ${notification.params.message}`.slice(0, 512);
          this.#publish();
        }
        return true;
      }
      case "session/resyncRequired": {
        const result = this.#observation.deliver({ registrationId: notification.params.registration_id, resync: true }, (event, cursor) => this.#applyEvent(cursor, event));
        if (result === 'recovery') {
          this.#observationError = `Observation recovery required: ${notification.params.reason}`.slice(0,512);
          this.#publish();
          this.#enqueueRepair();
        }
        return true;
      }
      case "session/closed":
        this.#serverClosed = true;
        this.#observation.retire();
        this.#epoch += 1;
        // Residency ended. That is a statement about this attachment's
        // observability, not a runtime outcome: nothing here fabricates a
        // settled attempt, an answered interaction, or a completed tool.
        for (const listener of [...this.#closedListeners]) {
          listener();
        }
        return true;
      default: {
        const exhaustive: never = notification;
        void exhaustive;
        return false;
      }
    }
  }

  // -------------------------------------------------------------------------
  // Turn and interaction
  // -------------------------------------------------------------------------

  /** Submits one inbound message. Acceptance, never completion. */
  async submitInbound(
    content: UserInputBlock[],
  ): Promise<{ messageId: string; sequence: string }> {
    const accepted = await this.#call(
      "turn/start",
      { target: this.#target, content },
      "inbound_accepted",
    );
    return {
      messageId: accepted.message_id,
      sequence: accepted.inbound_sequence,
    };
  }

  /** Steering and queue admission stay native; neither retains a client queue. */
  async steer(content: UserInputBlock[]): Promise<void> {
    await this.#call("turn/steer", { target: this.#target, content }, "inbound_accepted");
  }

  async editPending(expected: import("../protocol/app-server.ts").MethodParams<"inbound/edit">["expected"], text: string): Promise<void> {
    const result = await this.#call("inbound/edit", { target: this.#target, expected, text }, "inbound_mutation");
    if (result.outcome.status !== "applied") throw new Error(`Pending edit: ${result.outcome.status}. Not retried.`);
  }

  async removePending(expected: import("../protocol/app-server.ts").MethodParams<"inbound/remove">["expected"]): Promise<void> {
    const result = await this.#call("inbound/remove", { target: this.#target, expected }, "inbound_mutation");
    if (result.outcome.status !== "applied") throw new Error(`Pending removal: ${result.outcome.status}. Not retried.`);
  }

  get uploadPolicy() { return this.#client.capabilities?.upload_policy; }
  async upload(name: string, bytes: Uint8Array): Promise<UserInputBlock[]> {
    this.#assertControlReady();
    const operation = uploadOperationId();
    const epoch = this.#epoch, target = this.#target;
    const current = () => !this.#released && !this.#serverClosed && epoch === this.#epoch && sameTarget(target, this.#target);
    const check = () => { if (!current()) throw new Error(`Upload authority changed; reconcile operation ${operation}`); };
    check();
    const policy = this.uploadPolicy;
    if (!policy) throw new Error('Upload policy unavailable');
    const { transfer } = await this.#call("session/uploadPrepare", {
      target, operation_id: operation, files: [{ name, size: bytes.byteLength }],
    }, "upload_prepared");
    check();
    try { await this.#client.uploadCarrier(transfer, [new Blob([Uint8Array.from(bytes)])], policy, this.#client.uploadEndpoint); } catch { /* Repair the exact operation; never resend bytes. */ }
    check();
    const { outcome } = await this.#call("session/uploadStatus", { target, operation_id: operation }, "upload_status");
    check();
    if (outcome.state !== 'ready') throw new Error(`Upload ${operation}: ${outcome.state}. No bytes replayed.`);
    return outcome.files.map(({ receipt }) => ({ type: "upload", ...receipt }));
  }

  /** Requests cancellation of the current attempt. Acceptance, not settlement. */
  async cancelCurrentAttempt(): Promise<string> {
    const attempt=this.#state.attempt;
    if(!attempt?.attemptId||!['admitted','running'].includes(attempt.phase.type))throw new Error('Exact live Attempt identity is unavailable for cancellation.');
    const accepted = await this.#call(
      "turn/cancel",
      { target: this.#target },
      "cancellation_accepted",
    );
    return accepted.attempt_id;
  }

  /** Answers one runtime-owned interaction. */
  async respondInteraction(
    interaction: InteractionRef,
    response: InteractionResponse,
  ): Promise<void> {
    await this.#call(
      "interaction/respond",
      { target: this.#target, interaction, response },
      "interaction_settled",
    );
  }

  /** Cancels one pending interaction through its originating coordinator. */
  async cancelInteraction(interaction: InteractionRef): Promise<void> {
    await this.#call(
      "interaction/cancel",
      { target: this.#target, interaction },
      "interaction_settled",
    );
  }

  // -------------------------------------------------------------------------
  // Authoritative reads
  // -------------------------------------------------------------------------

  /**
   * Reads one bounded durable transcript page by its exclusive older boundary.
   * Callers pass the previous page's `next_cursor` unchanged.
   */
  async transcriptPage(
    beforeCursor?: RuntimeClientTranscriptCursor,
    limit = TRANSCRIPT_PROJECTION_PAGE_LIMIT,
  ): Promise<RuntimeClientTranscriptPage> {
    const page = await this.#call(
      "session/transcript",
      { target: this.#target, at: beforeCursor ? { type: 'older', before: beforeCursor } : { type: 'latest' }, limit },
      "transcript_window",
    );
    return page.window.page;
  }

  /**
   * Whether this connection's transport was granted delivery access: the
   * owned stdio child, or the separate remote delivery credential.
   */
  get deliveryAccess(): boolean {
    return this.#client.capabilities?.delivery_access === true;
  }

  /** One bounded page of committed deliveries, by the same transcript paging. */
  async deliveryPage(before?: RuntimeClientTranscriptCursor,capturedCut?:import('../../../protocol/app-server/v50.ts').CheckpointReadCut): Promise<DeliveryPage> {
    const cut=capturedCut??this.#checkpoint?.read_cut,epoch=this.#epoch;
    if(!cut)throw new Error('Load a native checkpoint first.');
    const offset=before==null?0:Number(BigInt(before));if(!Number.isSafeInteger(offset)||offset<0)throw new Error('Native delivery history coordinate is unavailable.');
    const {page}=await this.#call('conversation/checkpointPage',{target:this.#target,agent_id:null,cut,domain:'transcript.entries',offset,limit:32},'checkpoint_page');
    if(this.#released||epoch!==this.#epoch||readEvidence(page.read_cut)!==readEvidence(cut)||page.domain!=='transcript.entries'||page.items.length>32)throw new Error('Native delivery history cut changed.');
    const descriptors=page.items.flatMap(item=>{const value=messageDescriptor(item.value);return value?[value]:[];});
    if(before==null&&readEvidence(this.#checkpoint!.read_cut)===readEvidence(cut))descriptors.push(...this.#state.messageDescriptors??[]);
    const records:DeliveryRecord[]=[],collections:NonNullable<DeliveryPage['collections']>=[],covered=new Set(descriptors.filter(row=>row.deliveries&&row.identity!=null).map(row=>row.identity));
    for(const descriptor of descriptors)if(descriptor.deliveries){const head=deliveryHead(descriptor.deliveries);for(const item of head.items){deliveryItem(item);records.push({conversationId:cut.conversation_id,locator:item.locator,index:item.index,count:head.total,file:item.file??undefined,metadata:item.file?undefined:{item,cut}});}if(head.next_offset!=null&&!collections.some(row=>row.head.domain===head.domain))collections.push({head,cut});}
    for(const row of page.items){const value=row.value as import('../../../protocol/app-server/v50.ts').RuntimeClientTranscriptEntry|null;
      if(value?.item?.type==='message'&&!covered.has(value.item.message.id))records.push(...pageDeliveries({entries:[value]},cut.conversation_id).records);}
    const unique=new Map(records.map(record=>[deliveryItemKey(record),record]));
    return {records:[...unique.values()],cut,collections,next:page.next_offset==null?undefined:String(page.next_offset)};
  }
  async deliveryCollection(collection:NonNullable<DeliveryPage['collections']>[number]):Promise<DeliveryPage>{
    const {head,cut}=collection,epoch=this.#epoch;if(head.next_offset==null)throw new Error('Delivery collection is complete.');
    const {page}=await this.#call('conversation/checkpointPage',{target:this.#target,agent_id:null,cut,domain:head.domain,offset:head.next_offset,limit:8},'checkpoint_page');
    if(this.#released||epoch!==this.#epoch)throw new Error('Native delivery view changed.');const next=nativeDeliveryPage(head,page,cut,head.next_offset);
    return {cut,records:next.items.map(item=>({conversationId:cut.conversation_id,locator:item.locator,index:item.index,count:head.total,file:item.file??undefined,metadata:item.file?undefined:{item,cut}})),collections:next.next_offset==null?[]:[{head:next,cut}]};
  }
  async deliveryMetadata(record:DeliveryRecord,current:()=>boolean):Promise<SessionFileReference>{
    if(record.file)return record.file;const selected=record.metadata,epoch=this.#epoch;
    if(!selected||record.conversationId!==this.#target.conversation_id||selected.cut.conversation_id!==record.conversationId)throw new Error('Native delivery metadata source is unavailable.');
    const owned=()=>current()&&!this.#released&&epoch===this.#epoch;
    return readOriginalDeliveryMetadata(selected.item,selected.cut,async offset=>(await this.#call('conversation/checkpointValueRead',{target:this.#target,agent_id:null,cut:selected.cut,domain:selected.item.metadata_domain,item_offset:0,path:[],offset,max_bytes:65536},'checkpoint_value_chunk')).chunk,owned);
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
    if(record.conversationId!==this.#target.conversation_id)throw new Error('Native delivery locator source changed.');
    const { file, data } = await this.#client.callDelivery(
      "delivery/read",
      { target: this.#target, locator: record.locator, delivery_index: record.index },
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
    if(record.conversationId!==this.#target.conversation_id)throw new Error('Native delivery locator source changed.');
    const { file, path, device, inode } = await this.#client.callDelivery(
      "delivery/locate",
      { target: this.#target, locator: record.locator, delivery_index: record.index },
      "session_file_location",
      signal,
    );
    return { file, path, device, inode };
  }

  async observeAgent(agentId: string, after: import('../../../protocol/app-server/v50.ts').AgentObservationCursor | undefined, signal: AbortSignal) {
    const epoch = this.#epoch;
    const result = await this.#client.observe({ target: this.#target, agent_id: agentId, after: after ?? null }, signal);
    if (this.#released || epoch !== this.#epoch || signal.aborted) throw new Error('Child observation view was replaced.');
    return result.observation;
  }
  /** One original occupancy-model field range at its captured native cut. */
  async readOccupancyModel(offset: number, capturedCut?: import('../../../protocol/app-server/v50.ts').CheckpointReadCut) {
    const occupancy = this.#state.context.occupancy, domain = occupancy?.model_source;
    const cut = capturedCut ?? this.#checkpoint?.read_cut, epoch = this.#epoch, target = this.#target;
    if (!occupancy?.model_deferred || !domain || !cut || !Number.isSafeInteger(offset) || offset < 0)
      throw new Error('Original context model source unavailable.');
    const {chunk} = await this.#call('conversation/checkpointValueRead', {
      target, agent_id:null, cut, domain, item_offset:0, path:[], offset, max_bytes:65_536,
    }, 'checkpoint_value_chunk');
    if (this.#released || epoch !== this.#epoch || !sameTarget(target,this.#target)
      || this.#state.context.occupancy?.model_source !== domain || readEvidence(chunk.cut) !== readEvidence(cut)
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
  async readContent(content_ref: import('../../../protocol/app-server/v50.ts').ContentRef, offset: number, agentId?: string) {
    const epoch = this.#epoch;
    const { chunk } = await this.#call('conversation/contentRead', { target: this.#target, agent_id: agentId ?? null, content_ref, offset, max_bytes: 65_536 }, 'content_chunk');
    if (this.#released || epoch !== this.#epoch) throw new Error('Content view was replaced.');
    return chunk;
  }

  /** A read is valid only in the exact parent attachment/presentation epoch. */
  async agentTranscriptPage(
    agentId: AgentId,
    before?: RuntimeClientTranscriptCursor,
  ): Promise<RuntimeClientTranscriptPage | undefined> {
    if (this.#released || this.#serverClosed) return undefined;
    const epoch = this.#epoch;
    const target = this.#target;
    try {
      const result = await this.#call("agent/transcript", {
        target, agent_id: agentId, at: before == null ? { type: "latest" } : { type: "older", before, cut: null },
        limit: TRANSCRIPT_PROJECTION_PAGE_LIMIT,
      }, "transcript_window");
      if (epoch !== this.#epoch || !sameTarget(target, this.#target)) return undefined;
      return result.window.page;
    } catch (error) {
      if (epoch !== this.#epoch || !sameTarget(target, this.#target)) return undefined;
      throw error;
    }
  }

  /** Loads the next older page without changing the live event cursor. */
  async loadOlderTranscript(
    limit = TRANSCRIPT_PROJECTION_PAGE_LIMIT,
  ): Promise<boolean> {
    const beforeCursor = this.#state.transcriptNextCursor;
    if (beforeCursor === undefined || beforeCursor === null) {
      return false;
    }
    const epoch = this.#epoch;
    const page = await this.transcriptPage(beforeCursor, limit);
    if (epoch !== this.#epoch || this.#state.transcriptNextCursor !== beforeCursor) {
      // A page belongs to the exact boundary requested. A live refresh can
      // replace that window without replacing attachment ownership. Neither
      // that stale page nor its next cursor may be spliced into the new window.
      return false;
    }
    this.#state = mergeTranscriptPage(this.#state, page);
    this.#publish();
    return (page.entries ?? []).length > 0;
  }

  /** Reads one bounded page of historical fork/branch boundaries. */
  async boundaries(
    offset = 0,
    limit = SESSION_PROJECTION_PAGE_LIMIT,
  ): Promise<BoundaryPage> {
    const page = await this.#call(
      "session/boundaries",
      { target: this.#target, offset, limit },
      "boundaries",
    );
    return {
      surfaceRevision: page.surface_revision,
      boundaries: page.boundaries,
      nextOffset: page.next_offset ?? undefined,
    };
  }

  #domainReader(): DomainReader {
    const epoch = this.#epoch, target = this.#target;
    return {current:()=>!this.#released && !this.#serverClosed && epoch===this.#epoch,
      page:async(page,offset)=>(await this.#call('conversation/checkpointPage',{target,agent_id:null,cut:page.read_cut,domain:page.domain,offset,limit:32},'checkpoint_page')).page,
      value:async(page,item,offset)=>(await this.#call('conversation/checkpointValueRead',{target,agent_id:null,cut:page.read_cut,domain:page.domain,item_offset:Number(item.offset),path:[],offset,max_bytes:65536},'checkpoint_value_chunk')).chunk};
  }
  /** Read a finite native domain page without loading deferred item bodies. */
  async observationPage(domain:string,offset=0,previous?:import('../../../protocol/app-server/v50.ts').CheckpointPage,owned:()=>boolean=()=>true) {
    const source=this.observationCollections.find(row=>row.frontier.domain===domain),inspection=source?.inspection;
    const cut=previous?.read_cut??source?.cut??this.#checkpoint?.read_cut,epoch=this.#epoch,target=this.#target;
    if(!cut||!owned()||!Number.isSafeInteger(offset)||offset<0||previous&&previous.domain!==domain)throw new Error('Invalid observation page ownership.');
    const {page}=await this.#call('conversation/checkpointPage',{target,agent_id:null,cut,domain,offset,limit:32},'checkpoint_page');
    if(this.#released||this.#serverClosed||epoch!==this.#epoch||!owned()||inspection&&this.#currentTraceInspection()!==inspection||page.domain!==domain||page.cursor!==cut.revision||readEvidence(page.read_cut)!==readEvidence(cut)||page.items.length>32)throw new Error('Observation page was replaced.');
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
  async observationValue(page:import('../../../protocol/app-server/v50.ts').CheckpointPage,itemOffset:number,path:string[],offset:number,owned:()=>boolean=()=>true,expectedEnd?:number) {
    if(/^messages\..+\.(?:blocks|tools)$/.test(page.domain))throw new Error('Canonical source bytes use their advertised content references.');
    if(!Number.isSafeInteger(itemOffset)||itemOffset<0||!page.items.some(item=>item.offset===String(itemOffset)))throw new Error('Invalid selected observation range.');
    const epoch=this.#epoch,target=this.#target,current=()=>!this.#released&&!this.#serverClosed&&epoch===this.#epoch&&owned();
    validateObservationCut(page.read_cut,target.conversation_id,null,page.cursor);
    const range=await readTraceSelectedWindow({domain:page.domain,item_offset:itemOffset,path},page.read_cut,offset,async at=>(await this.#call('conversation/checkpointValueRead',{target,agent_id:null,cut:page.read_cut,domain:page.domain,item_offset:itemOffset,path,offset:at,max_bytes:65536},'checkpoint_value_chunk')).chunk,current,expectedEnd);
    return {...range,end:range.readable_end};
  }
  async checkpointPage(domain: string, offset=0, previous?: import('../../../protocol/app-server/v50.ts').CheckpointPage) {
    const cut=previous?.read_cut ?? this.#checkpoint?.read_cut;
    if (!cut) throw new Error('Load a checkpoint first.');
    const page=(await this.#call('conversation/checkpointPage',{target:this.#target,agent_id:null,cut,domain,offset,limit:32},'checkpoint_page')).page;
    return {page,items:await domainItems<unknown>(page,this.#domainReader())};
  }
  async interactionValue(interaction: InteractionRef, itemOffset: number, page: import('../../../protocol/app-server/v50.ts').CheckpointPage | undefined, offset: number, path: string[] = []) {
    const hasIdentity=this.#state.pendingInteractions.some(value=>value.interaction.conversation_id===interaction.conversation_id && value.interaction.interaction_id===interaction.interaction_id && !('request' in value) && Number(value.source_offset)===itemOffset)
      || page?.items.some(value=>{const header=value.value as import('../../../protocol/app-server/v50.ts').RuntimeClientInteractionHeader | null;return Number(value.offset)===itemOffset && Number(header?.source_offset)===itemOffset && header?.interaction?.conversation_id===interaction.conversation_id && header.interaction.interaction_id===interaction.interaction_id;});
    if(!Number.isSafeInteger(itemOffset) || itemOffset<0 || !hasIdentity) throw new Error('Interaction view no longer owns this identity.');
    const cut=page?.read_cut ?? this.#checkpoint?.read_cut;
    if(!cut) throw new Error('Load the native checkpoint first.');
    const epoch=this.#epoch, target=this.#target;
    const {chunk}=await this.#call('conversation/checkpointValueRead',{target,agent_id:null,cut,domain:'pending_interactions',item_offset:itemOffset,path,offset,max_bytes:65536},'checkpoint_value_chunk');
    if(this.#released || epoch!==this.#epoch || !sameTarget(target,this.#target) || JSON.stringify(chunk.cut)!==JSON.stringify(cut) || chunk.domain!=='pending_interactions' || chunk.item_offset!==itemOffset || chunk.offset!==offset || JSON.stringify(chunk.path)!==JSON.stringify(path)) throw new Error('Interaction body read authority changed.');
    return chunk;
  }
  get catalogDeferredNotice(): string | undefined {
    const page = this.#modelCatalogPage;
    if (!page) return undefined;
    const deferred = page.items.filter(item => item.value == null || item.deferred
      || (item.value as {profilesDeferred?: boolean}).profilesDeferred === true);
    if (!deferred.length) return undefined;
    return `Binding ${this.#state.modelStatus?.binding_revision ?? 'unavailable'}: deferred model/profile choices at native positions ${deferred.map(item => item.offset).join(', ')}. Read /observation model_catalog.models; /model-position selects native defaults at the displayed binding revision.`;
  }
  #modelCatalogPage?: import('../../../protocol/app-server/v50.ts').CheckpointPage;
  get nextModelOffset() { return this.#modelCatalogPage?.next_offset == null ? undefined : Number(this.#modelCatalogPage.next_offset); }
  /** One bounded capability page, preserving deferred fields and continuation. */
  async capabilitiesPage(): Promise<import('../../../protocol/app-server/v50.ts').CheckpointPage> {
    const target = this.#target;
    const { page } = await this.#call('resources/read', { target }, 'domain_page');
    if (this.#target !== target) throw new Error('Capability observation was replaced.');
    return page;
  }

  // -------------------------------------------------------------------------
  // Maintenance and settings
  // -------------------------------------------------------------------------

  async projections(after = "0") {
    return (await this.#client.call("session/projections", { target:this.#target, after, limit:16 }, "projections")).page;
  }
  async projectionDetail(checkpointId:string, offset = 0, owned:()=>boolean=()=>true) {
    this.#traceInspection=undefined;
    const selected=this.#projectionInspection={epoch:this.#epoch,owned};
    const detail=(await this.#call("session/projectionDetail", { target:this.#target, checkpoint_id:checkpointId, offset, limit:16 }, "projection_detail")).detail;
    if(this.#released||this.#serverClosed||selected.epoch!==this.#epoch||this.#projectionInspection!==selected||!owned())throw new Error('Projection inspection viewport was replaced.');
    validateObservationCut(detail.read_cut,this.#target.conversation_id,null,detail.read_cut.revision);
    if(detail.checkpoint.checkpoint_id!==checkpointId||detail.items.some(item=>item.occurrence.checkpoint_id!==checkpointId))throw new Error('Projection inspection checkpoint changed.');
    Object.assign(selected,{detail});
    return detail;
  }
  /** Runs one manual idle compaction to its durable terminal result. */
  async compactContext(): Promise<RuntimeClientContextView> {
    await this.#call(
      "context/compact",
      { target: this.#target, request_id: globalThis.crypto.randomUUID() },
      "context_compacted",
    );
    const {checkpoint}=await this.#call('session/checkpoint',{target:this.#target,trace_records:[]},'checkpoint');
    const {page}=await this.#call('conversation/checkpointPage',{target:this.#target,agent_id:null,cut:checkpoint.read_cut,domain:'context',offset:0,limit:32},'checkpoint_page');
    return domainValue<RuntimeClientContextView>(page,this.#domainReader());
  }

  /** Reads or mutates the root Goal through its native owner. */
  async goal(control: GoalControl): Promise<RuntimeClientGoalView> {
    if (control.action !== 'show') await this.#call('goal/control',{target:this.#target,control},'goal_mutation');
    const {page}=await this.#call('goal/control',{target:this.#target,control:{action:'show'}},'domain_page');
    return domainValue<RuntimeClientGoalView>(page,this.#domainReader());
  }

  #configurationCut?:import('../../../protocol/app-server/v50.ts').BoundedSessionApplication;
  #configurationRead?:Promise<void>;
  #configurationObservationError?:string;
  get configurationObservationError(){return this.#configurationObservationError;}
  get deferredConfiguration(){return this.#configurationRead?this.#configurationCut:undefined;}
  #observeConfiguration(cut:import('../../../protocol/app-server/v50.ts').BoundedSessionApplication) {
    if(this.#configurationCut && BigInt(cut.version)<=BigInt(this.#configurationCut.version))return;
    this.#configurationCut=cut;
    if(cut.inline){this.#configurationObservationError=undefined;this.#installConfiguration(cut.inline);return;}
    this.#configuration=undefined;this.#configurationObservationError=undefined;this.#publish();
    if(this.#configurationRead)return;
    const epoch=this.#epoch;
    const work=materializeApplication(cut,async(reference,offset)=>(await this.#call('session/configurationValueRead',{session_id:this.sessionId,reference,offset,max_bytes:65536},'session_application_value_chunk')).chunk,()=>!this.#released&&epoch===this.#epoch&&this.#configurationCut===cut)
      .then(application=>{if(application){this.#configurationObservationError=undefined;this.#installConfiguration(application);}}).catch(error=>{if(this.#configurationCut===cut){this.#configurationObservationError=String(error).slice(0,512);this.#publish();}}).finally(()=>{
        if(this.#configurationRead!==work)return;this.#configurationRead=undefined;
        const latest=this.#configurationCut;if(latest!==cut&&latest){this.#configurationCut=undefined;this.#observeConfiguration(latest);}
      });
    this.#configurationRead=work;this.#publish();
  }
  #installConfiguration(application: ConfigurationApplication): void {
    if (this.#configuration && compareExact(application.version, this.#configuration.version) < 0) return;
    this.#configuration = application;
    this.#publish();
  }

  async readConfiguration() {
    const result = await this.#call("session/configuration", { session_id: this.sessionId }, "session_configuration");
    const epoch=this.#epoch;
    const application=await materializeApplication(result.projection,async(reference,offset)=>(await this.#call('session/configurationValueRead',{session_id:this.sessionId,reference,offset,max_bytes:65536},'session_application_value_chunk')).chunk,()=>!this.#released&&epoch===this.#epoch);
    if(application)this.#installConfiguration(application);
    return application;
  }

  async adoptConfiguration(candidate: AvailableConfiguration): Promise<ConfigurationApplication> {
    await this.#call("session/adoptConfiguration", {
      session_id: this.sessionId, candidate: candidate.identity, expected_binding: candidate.expected_binding,
    }, "configuration_receipt");
    // Acknowledgement is not a new observation. The caller rereads native authority.
    const application = await this.readConfiguration();
    if (!application) throw new Error("Configuration source unavailable.");
    return application;
  }

  /** The safe public catalog. This is why the client never reads rustx.toml. */
  async modelCatalog(offset?: number): Promise<ModelCatalogView> {
    const epoch = this.#epoch;
    const binding = this.#state.modelStatus?.binding_revision;
    const page=offset===undefined ? (await this.#call('session/models',{target:this.#target},'domain_page')).page
      : this.#modelCatalogPage ? await this.#domainReader().page(this.#modelCatalogPage,offset) : undefined;
    if(!page) throw new Error('Load the catalog before its continuation.');
    if (this.#released || this.#serverClosed || epoch !== this.#epoch
      || binding !== this.#state.modelStatus?.binding_revision) {
      throw new Error('Catalog read belongs to a retired model binding or attachment.');
    }
    this.#modelCatalogPage=page;
    // Browsing reads finite native headers only. Giant identities and incomplete
    // profile heads remain explicit source descriptors, never automatic reads.
    const models: NonNullable<ModelCatalogView['models']> = [];
    for (const item of page.items) {
      const value = item.value as (NonNullable<ModelCatalogView['models']>[number] & {profilesDeferred?: boolean}) | null;
      if (!value || item.deferred || value.profilesDeferred === true
        || typeof value.model !== 'string' || new TextEncoder().encode(value.model).length > 256
        || value.defaultProfile != null && new TextEncoder().encode(value.defaultProfile).length > 256
        || (value.profiles ?? []).some(profile => new TextEncoder().encode(profile.id).length > 256)) continue;
      models.push(value);
    }
    return {models};
  }
  async modelGet(): Promise<SessionModelView> {
    if (this.#repair) await this.#repair;
    const epoch=this.#epoch;
    const {page}=await this.#call('session/model',{target:this.#target},'domain_page');
    const model=await domainValue<SessionModelView>(page,this.#domainReader());
    if(this.#released||this.#serverClosed||epoch!==this.#epoch)throw new Error('Model read belongs to a retired attachment; reread native authority.');
    return model;
  }

  /** A snapshot cannot establish the terminal outcome of an unanswered request. */
  get modelSelectionOutcomeUnknown(): boolean {
    return this.#unknownModelSelections.has(this.sessionId);
  }

  /**
   * Replaces the authoritative session model configuration.
   *
   * A whole-state replacement, never a patch: callers send back the complete
   * configuration they read. The update affects future admissions only; an
   * already-admitted attempt keeps the model it froze.
   *
   * A new deliberate request is allowed after a lost response. Native revision
   * checks fence older prepared work when the new selection changes authority;
   * a NoOp acknowledgement cannot settle an earlier unanswered request.
   */
  /** Selects native catalog defaults by finite ordinals at the displayed source revision. */
  async modelSelect(choice:import('../../../protocol/app-server/v50.ts').CatalogModelSelection) {
    if(!Number.isSafeInteger(choice.model_position)||choice.model_position<0 ||choice.profile_position!=null&&(!Number.isSafeInteger(choice.profile_position)||choice.profile_position<0))throw new Error('Invalid native model selection ordinal.');
    if(this.#released||this.#serverClosed)throw new Error('Session attachment is unavailable.');
    if(this.#modelMutationInFlight)throw new Error('A model selection request is still in flight; wait for its response or connection recovery.');
    this.#assertControlReady();
    const epoch=this.#epoch;
    this.#modelMutationInFlight=true;
    try {
      const receipt=await this.#call('session/selectModel',{target:this.#target,choice},'model_set');
      if(this.#released||this.#serverClosed||epoch!==this.#epoch)throw new Error('Model acknowledgement belongs to a retired observation; reread native authority. Not replayed.');
      // Confirmation never requires materializing a growing model identity or its defaults.
      return receipt;
    }catch(error){
      if(isUncertainOutcome(error))this.#unknownModelSelections.add(this.sessionId);
      if(!this.#released&&!this.#serverClosed)await this.resync().catch(()=>{});
      throw error;
    }finally{this.#modelMutationInFlight=false;}
  }

  async modelSet(config: SessionModelConfig): Promise<SessionModelView> {
    if (this.#released || this.#serverClosed) throw new Error("Session attachment is unavailable.");
    if (this.#modelMutationInFlight) throw new Error("A model selection request is still in flight; wait for its response or connection recovery.");
    this.#assertControlReady();
    const epoch = this.#epoch;
    this.#modelMutationInFlight = true;
    try {
      await this.#call(
        "session/setModel",
        { target: this.#target, config },
        "model_set",
      );
      if (this.#released || this.#serverClosed || epoch !== this.#epoch) {
        throw new Error("Model acknowledgement belongs to a retired observation; reread native authority. Not replayed.");
      }
      // A response confirms this operation, but is not an ordered observation.
      // Only native events and snapshots update the Session or Attempt view.
      return await this.modelGet();
    } catch (error) {
      if (isUncertainOutcome(error)) this.#unknownModelSelections.add(this.sessionId);
      // Readback repairs the projection, not the outcome of an unanswered
      // request. Native work can survive its waiter and commit after a snapshot.
      if (!this.#released && !this.#serverClosed) await this.resync().catch(() => {});
      throw error;
    } finally {
      this.#modelMutationInFlight = false;
    }
  }

  /** One native read of the published immutable configuration. */
  async configuration(): Promise<import("../protocol/app-server.ts").EffectiveConfiguration> {
    const result = await this.#call("session/effectiveConfiguration", { target: this.#target }, "effective_configuration");
    const epoch=this.#epoch;
    return materializeEffective(result.projection,async(reference,offset)=>(await this.#call('session/effectiveConfigurationValueRead',{target:this.#target,reference,offset,max_bytes:65536},'configuration_value_chunk')).chunk,()=>!this.#released&&epoch===this.#epoch);
  }

  // -------------------------------------------------------------------------
  // Finite Jobs and durable Agents
  // -------------------------------------------------------------------------

  /** Commit native cancellation intent; lifecycle and physical settlement remain observed facts. */
  async cancelJob(jobId: ToolExecutionId) {
    const receipt = await this.#call("job/cancel", { target: this.#target, job_id: jobId }, "job_cancellation_accepted");
    if (receipt.job_id !== jobId) throw new Error('Job cancellation receipt named another native execution.');
    return receipt;
  }

  async jobStatus(jobId: ToolExecutionId): Promise<RuntimeClientJob> {
    const {page}=await this.#call('job/status',{target:this.#target,job_id:jobId},'domain_page');
    return domainValue<RuntimeClientJob>(page,this.#domainReader());
  }

  async listJobs() {
    const {page}=await this.#call('job/list',{target:this.#target},'domain_page');
    const jobs=await domainItems<RuntimeClientJob>(page,this.#domainReader());
    return {jobs,matched:Number(page.total),returned:jobs.length,limit:32,truncated:page.next_offset!=null};
  }

  async waitJob(jobId: ToolExecutionId): Promise<RuntimeClientJob> {
    await this.#call("job/wait", { target: this.#target, job_id: jobId }, "job_outcome");
    return this.jobStatus(jobId);
  }

  /** One native bounded Agent viewport; continuations retain its exact cut. */
  async agentPage(offset = 0, previous?: import('../../../protocol/app-server/v50.ts').CheckpointPage) {
    const epoch=this.#epoch;
    const page=previous
      ? await this.#domainReader().page(previous,offset)
      : (await this.#call('agent/list',{target:this.#target},'domain_page')).page;
    if(this.#released||epoch!==this.#epoch)throw new Error('Agent viewport was replaced.');
    const agents=await domainItems<RuntimeClientAgent>(page,this.#domainReader());
    if(this.#released||epoch!==this.#epoch)throw new Error('Agent viewport was replaced.');
    return {page,agents};
  }

  async agentStatus(agentId: AgentId): Promise<RuntimeClientAgent> {
    const {page}=await this.#call('agent/status',{target:this.#target,agent_id:agentId},'domain_page');
    return domainValue<RuntimeClientAgent>(page,this.#domainReader());
  }

  async sendMessage(agentId: AgentId, message: string, attachments: import("../../../protocol/app-server/v50.js").UploadReceipt[] = []) {
    return this.#call("agent/sendMessage", { target: this.#target, agent_id: agentId, message, attachments }, "agent_message");
  }

  async waitAgent(agentId: AgentId) {
    return this.#call("agent/wait", { target: this.#target, agent_id: agentId }, "agent_wait");
  }

  async interruptAgent(agentId: AgentId) {
    const receipt = await this.#call("agent/interrupt", { target: this.#target, agent_id: agentId }, "agent_interruption_accepted");
    if (receipt.agent_id !== agentId) throw new Error('Agent interruption receipt named another native owner.');
    return receipt;
  }

  // -------------------------------------------------------------------------
  // Repair and release
  // -------------------------------------------------------------------------

  /**
   * Takes a fresh authoritative snapshot and replaces the projection.
   *
   * The snapshot and the re-subscription are two requests, so the cursor is
   * what joins them: the new registration starts exactly where the snapshot
   * ends, and any observation in between is either already described by the
   * snapshot or arrives after that cursor.
   */
  resync(): Promise<void> {
    if (this.#repair) return this.#repair;
    if (this.#released || this.#serverClosed) return Promise.resolve();
    this.#observation.requireRecovery();
    let epoch = ++this.#epoch;
    const target = this.#target;
    const work = (async () => {
      for (let attempt = 0; attempt < 3; attempt++) {
        this.#observation.beginCheckpointLoading();
        const { checkpoint } = await this.#call("session/checkpoint", { target, trace_records: [] }, "checkpoint");
        if (epoch !== this.#epoch || this.#released || this.#serverClosed) return;
        validateRootCheckpoint(target, checkpoint, this.#observation.applied);
        this.#resyncCount += 1;
        this.#checkpoint = checkpoint;
        this.#install(checkpointSnapshot(checkpoint), checkpoint.cursor);
        this.#observation.installCheckpoint(checkpoint.cursor);
        epoch = this.#epoch;
        try {
          const chain = this.#observation;
          chain.beginSubscription(checkpoint.cursor);
          const receipt = await this.#call("session/subscribe", { target, after_cursor: checkpoint.cursor }, "subscribed");
          if (epoch !== this.#epoch || this.#released || this.#serverClosed || this.#observation !== chain) return;
          if (!sameTarget(receipt.target, target) || !chain.acceptReceipt(receipt.registration_id, receipt.after_cursor)) throw new Error(chain.phase === 'unavailable' ? "Observation source unavailable for the accepted registration." : "Subscription recovery required.");
          this.#publish();
          return;
        } catch (error) {
          if (epoch !== this.#epoch || this.#released || this.#serverClosed) return;
          if (!isResyncRequired(error) || attempt === 2) throw error;
        }
      }
    })().catch(error => {
      if (epoch === this.#epoch && !this.#released && !this.#serverClosed) {
        if (error instanceof AppServerRequestError && error.kind === 'observation_unavailable' || this.#observation.phase === 'unavailable') this.#observation.markUnavailable();
        else this.#observation.requireRecovery();
        this.#observationError = `Observation source unavailable: ${String(error)}`.slice(0, 512);
        this.#publish();
      }
      throw error;
    }).finally(() => {
      if (this.#repair === work) this.#repair = undefined;
    });
    this.#repair = work;
    this.#publish();
    return work;
  }

  /**
   * Releases this attachment.
   *
   * Detach removes exactly this connection's control relationship. It does not
   * cancel a turn, settle an interaction, unload the runtime, or shut anything
   * down, and the Session keeps running in the App Server afterwards.
   */
  async detach(): Promise<void> {
    if (this.#released) {
      return;
    }
    this.#released = true;
    this.#epoch += 1;
    this.#observation.retire();
    await this.#call(
      "session/detach",
      { target: this.#target },
      "detached",
    );
  }

  /** Switch the durable branch; native retirement remains manager-owned. */
  async switchNode(nodeId: string): Promise<void> {
    this.#released = true;
    this.#epoch += 1;
    this.#observation.retire();
    await this.#call("session/switchNode", { target: this.#target, node_id: nodeId }, "session");
  }

  /** Applies a pure local transformation of transient client state. */
  updateState(update: (state: PresentationState) => PresentationState): void {
    this.#state = update(this.#state);
    this.#publish();
  }

  // -------------------------------------------------------------------------
  // Internals
  // -------------------------------------------------------------------------

  #enqueueRepair(): void {
    void this.resync().catch(() => {});
  }

  #applyEvent(
    cursor: RuntimeClientCursor,
    event: Parameters<typeof reduce>[1]["event"],
  ): void {
    validateRootEvent(this.#target, event, cursor);
    // An event at or before the installed cursor is already described by the
    // snapshot; folding it again would double-apply a fact. Cursors are exact
    // u64 decimal text, so this is a numeric comparison and never a string one.
    if (compareExact(cursor, this.#state.cursor) <= 0) {
      return;
    }
    if (BigInt(cursor) !== BigInt(this.#state.cursor) + 1n) throw new Error("Non-contiguous observation.");
    try { this.#state = reduce(this.#state, { cursor, event }); }
    catch(error) {
      this.#observationError=`${event.type} at ${cursor}: ${String(error)}`.slice(0,512);
      this.#publish();
      throw error;
    }
    this.#observationError=undefined;
    this.#checkpoint = presentationCheckpoint(this.#state) ?? this.#checkpoint;
    this.#publish();

  }

  #install(snapshot: RuntimeClientSnapshot, cursor: RuntimeClientCursor): void {
    this.#epoch += 1;
    this.#state = replaceFromSnapshot(snapshot, cursor);
    this.#observationError = undefined;
    for (const listener of [...this.#snapshotListeners]) {
      listener();
    }
    this.#publish();
  }

  #publish(): void {
    for (const listener of [...this.#listeners]) {
      listener(this.#state);
    }
  }
}

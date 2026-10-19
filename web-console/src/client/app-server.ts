import {interactionBody,readInteractionBody} from '../../../protocol/app-server/interaction';
import { ObservationChain, validateRootCheckpoint, validateRootEvent } from '../../../protocol/app-server/observation';
import { SessionLifecycles } from './session-lifecycle/system';
import { emptyFacts, type Observation, type LifecycleFacts, type SessionLifecyclePort } from './session-lifecycle/port';
import { lifecycleAvailability, type LifecycleContext } from './session-lifecycle/machine';
import { AgentMeters } from './agent-meters';
import { UploadFailure, AttachmentIntakes } from './uploads';
import { checkpointSnapshot, foldRuntimeEvent, observationCheckpoint, transitionFacts } from '../../../protocol/app-server/projection';
import { hasResponseDeadline, requestEffect, requestLane } from '../../../protocol/app-server/method-policy';
import { SourceRepairObserver } from './source-repairs';
import { SessionCatalog } from './session-catalog';
import { SessionReads } from './session-reads';
import { ReadingNavigation } from './reading';
import { ColdReads } from './cold-reads';
import { TraceReads } from './trace-reads';
import { ConfigurationObservations } from './configuration-observations';
import { sameTarget } from '../../../protocol/app-server/attachment-identity';
import { SessionExportController } from "./session-export";
import { refreshTrace, replaceTrace, traceInterestBatches, traceInterests } from './trace';
import type {
  TraceToolLocator, PendingInboundRef, AttachmentTarget, GoalMutation, GoalRef, InteractionRef, InteractionResponse, MethodResult, Notification,
  Request, Request1, Response, RuntimeClientSnapshot, RuntimeClientEvent,
  UserInputBlock, UploadReceipt, UploadedFile,
} from '../../../protocol/app-server/v50';
import { transferUpload, uploadOperationId } from '../../../protocol/app-server/upload';
import { refreshTranscript, replaceTranscript, type TurnSelection } from './transcript';
import { ProtocolLog, type WireContext } from './protocol-log';
import { OutcomeUncertain, RequestAdmissionDeferred, RequestNotDispatched, RpcFailure, isOutcomeUncertain } from './errors';
import { interactionKey, type CancellationTarget, type ClientView, type CompactionRequestEvidence, type ConnectionState, type GoalControlOutcome, type InboundControlOutcome, type OperationAdmission, type SessionView, type Submission } from './contracts';


export interface Socket {
  readonly readyState: number;
  onopen: ((event: Event) => unknown) | null;
  onmessage: ((event: MessageEvent) => unknown) | null;
  onclose: ((event: CloseEvent) => unknown) | null;
  onerror: ((event: Event) => unknown) | null;
  send(data: string): void;
  close(): void;
}
export type SocketFactory = (url: string, protocols: string[]) => Socket;
type SessionAdmission = (id: string, current: () => boolean) => Promise<false | OperationAdmission>;
/** One installation lifetime of an attachment-admission owner. Identity is the
 * object, never the callback: reinstalling the same callback is a new owner. */
interface SessionAdmissionOwner { readonly admit: SessionAdmission }
interface Pending {
  /** Pure actor authority, evaluated after caller-owned admission callbacks. */
  authority?: () => boolean;
  dispatchCurrent?: (() => boolean) | OperationAdmission;
  /** Final validation holding an RPC slot; `timer` is its deadline until send. */
  validation?: AbortController;
  acknowledged?: (result: MethodResult) => void;
  request: Request;
  context: WireContext;
  mutation: boolean;
  sent: boolean;
  expected: MethodResult['type'];
  resolve: (result: MethodResult) => void;
  reject: (error: Error) => void;
  timer?: ReturnType<typeof setTimeout>;
}
/** GoalDomain serializes its bounded rejection into the error message. Only the
 * reason is displayed; its embedded `current` is never adopted as authority. */
function goalRefusal(error: unknown) {
  if (!(error instanceof RpcFailure)) return error instanceof Error ? error.message : String(error);
  try {
    const rejection: unknown = JSON.parse(error.error.message);
    if (rejection && typeof rejection === 'object' && 'reason' in rejection && typeof rejection.reason === 'string') return rejection.reason;
  } catch { /* Non-Goal refusal: show the transport message. */ }
  return error.message;
}
/** Shared method semantics; this transport keeps its own queue, backpressure
 * and lifetime implementation. Only a side-effecting request may have
 * committed when its response is lost. */
const mayCommit = (operation: Request1) => requestEffect(operation) === 'side_effecting';
const DOMAIN_CAPACITY = { control: 2, observation: 8 } as const;
const RPC_CAPACITY = 6;
/** Operation-admission validations reserve ordinary RPC slots so the final check
 * stays adjacent to send, but at most two at once: a stalled Product Host read
 * can never take the remaining slots needed to cancel or control native work. */
const VALIDATION_CAPACITY = 2;

const dispatchCurrent = ({ dispatchCurrent: proof }: Pending) => !proof || (typeof proof === 'function' ? proof() : proof.current());


/** One native rustX connection. All retained snapshots are replaceable read caches.
 * Runtime events fold below React; snapshots initialize or repair exact attachments. */
export class AppServerClient {
  readonly agentMeters = new AgentMeters(async (target, id, current) => {
    let acknowledged = false;
    try {
      const result = await this.request({ method: 'agent/statistics', params: { target, agent_id: id } }, 'agent_statistics', () => { acknowledged = true; }, current);
      return { settled: true, metrics: result.metrics };
    } catch (error) {
      if (error instanceof RequestAdmissionDeferred) return { settled: true, deferred: error.revision };
      // A correlated native rejection settles the read; an unsent refusal
      // started none. Timeout/disconnect only retires the browser waiter.
      return { settled: acknowledged || error instanceof RpcFailure || error instanceof RequestNotDispatched,
        error: error instanceof Error ? error.message : String(error) };
    }
  });
  readonly attachmentIntakes = new AttachmentIntakes();
  readonly log = new ProtocolLog();
  /** Final client lifetime ends only explicitly, never on a component unmount. */
  dispose() { this.attachmentIntakes.dispose(); for (const listener of [...this.disposeListeners]) listener(); this.disposeListeners.clear(); return this.disconnect(); }
  private disposeListeners = new Set<() => void>();
  private authorityListeners = new Set<() => void>();
  /** Application owners whose local intent lives exactly as long as this client. */
  onDispose = (listener: () => void) => { this.disposeListeners.add(listener); return () => { this.disposeListeners.delete(listener); }; };
  /** Application owners that must fence work of a replaced App Server authority. */
  onAuthorityRetired = (listener: () => void) => { this.authorityListeners.add(listener); return () => { this.authorityListeners.delete(listener); }; };
  private socket?: Socket;
  private initialized = false;
  private readonly sourceRepairs = new SourceRepairObserver({
    request: (...args) => this.request(...args),
    connection: () => ({ generation: this.state.generation, authorityId: this.state.authorityId }),
    current: (generation, authorityId) => this.current(generation) && this.initialized && (authorityId === undefined || this.state.authorityId === authorityId),
    held: () => this.state.sourceRepairs ?? {},
    publish: sourceRepairs => this.publish({ sourceRepairs }),
  });
  private nextId = 0;
  private listeners = new Set<() => void>();
  private pending = new Map<string, Pending>();
  private refreshes = new Map<string, Promise<void>>();
  private dirty = new Set<string>();
  private acquiring = new Set<string>();
  private resubscribe = new Set<string>();
  private observationChains = new Map<string, ObservationChain<RuntimeClientEvent>>();
  private resumeSeeds = new Map<string,{target:AttachmentTarget;snapshot:RuntimeClientSnapshot;checkpoint:import('../../../protocol/app-server/v50').RuntimeClientCheckpoint;cursor:string;bytes:number}>();
  private retainAppliedObservations() {
    for(const [id,chain] of this.observationChains) {
      const view=this.state.views[id];
      if(!view?.target||!view.snapshot||!view.checkpoint||!chain.applied||view.cursor!==chain.applied) {this.resumeSeeds.delete(id);continue;}
      const bytes=new TextEncoder().encode(JSON.stringify(view.snapshot)).length;
      if(bytes>1_048_576){this.resumeSeeds.delete(id);continue;}
      this.resumeSeeds.delete(id);
      this.resumeSeeds.set(id,{target:view.target,snapshot:view.snapshot,checkpoint:view.checkpoint,cursor:chain.applied,bytes});
    }
    let bytes=Array.from(this.resumeSeeds.values()).reduce((sum,value)=>sum+value.bytes,0);
    while(this.resumeSeeds.size>32||bytes>16*1024*1024){const key=this.resumeSeeds.keys().next().value!;bytes-=this.resumeSeeds.get(key)!.bytes;this.resumeSeeds.delete(key);}
  }
  private readonly lifecycles = new SessionLifecycles({
    transport: () => ({ generation: this.state.generation, connected: this.initialized }),
    port: id => this.lifecyclePort(id),
    project: (id, current, previous) => this.projectLifecycle(id, current, previous),
  });
  private state: ClientView = {
    connection: 'disconnected', generation: 0, sessions: [], views: {}, uncertain: [], interactionOperations: {},
  };
  private admissionRevision = 0;
  private admissionNotification = false;
  private capacityReleased() {
    ++this.admissionRevision;
    if (this.admissionNotification) return;
    this.admissionNotification = true;
    // Notify after pending-map mutation/pumping has committed. A readiness cut
    // also prevents a completion racing the refusal continuation from being lost.
    queueMicrotask(() => {
      this.admissionNotification = false;
      if (this.socket && this.initialized && this.pending.size < 64) this.agentMeters.admissionAvailable(this.admissionRevision);
    });
  }
  constructor(private readonly socketFactory: SocketFactory = (url, protocols) => new WebSocket(url, protocols), private readonly timeoutMs = 30_000, private readonly uploadCarrier = transferUpload) {
    // Navigation retires admission proofs; release their reservations outside the caller's stack.
  }
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  getSnapshot = () => this.state;
  isAttachmentObservationCurrent(id: string, admission: SessionView['attachmentObservation']): boolean {
    return this.lifecycles.observes(id, admission);
  }
  isAttachmentControlCurrent(id: string, admission: SessionView['attachmentObservation']): boolean {
    return !!this.state.views[id]?.snapshot && this.observationChains.get(id)?.phase === 'following'
      && this.lifecycles.controls(id, admission);
  }
  private publish(patch: Partial<ClientView>) {
    this.state = { ...this.state, ...patch };
    for (const listener of this.listeners) listener();
  }
  private setSession(id: string, patch: Partial<Omit<SessionView, keyof LifecycleFacts | 'lifecycle'>> & { error?: string }) {
    const view = this.state.views[id] ?? { id, summary: this.state.sessions.find(row => row.id === id), ...emptyFacts };
    this.publish({ views: { ...this.state.views, [id]: { ...view, ...patch } } });
  }
  /** One-way actor projection. Ordinary presentation writes cannot assign these facts. */
  private projectLifecycle(id: string, context: LifecycleContext, previous?: LifecycleContext) {
    const view = this.state.views[id] ?? { id, summary: this.state.sessions.find(row => row.id === id), ...emptyFacts };
    const facts = context.facts;
    const changedNode=previous&&facts.nodeId!==undefined&&previous.facts.nodeId!==undefined&&facts.nodeId!==previous.facts.nodeId;
    const opening = facts.attachment === 'attaching' && previous?.epoch !== context.epoch;
    const revoked = !!previous?.facts.attachmentObservation && facts.attachmentObservation !== previous.facts.attachmentObservation;
    if (revoked) this.retireObservationWork(id);
    const retiredClaim = !!previous?.facts.target && !facts.target;
    const attached = context.attachmentResult && context.attachmentResult !== previous?.attachmentResult && facts.attachmentObservation;
    const result = context.attachmentResult;
    const projection: SessionView = { ...view, ...facts, lifecycle: lifecycleAvailability(context),
      ...(revoked ? {
        history:view.history?.loading?{...view.history,loading:false}:view.history,
        trace: view.trace && { ...view.trace, loading: false, details: Object.fromEntries(Object.entries(view.trace.details).filter(([, detail]) => !detail.loading)) },
        turnOutline: view.turnOutline && { ...view.turnOutline, loading: false },
        turnNavigation: view.turnNavigation && { intent: view.turnNavigation.intent },
      } : {}),
      ...(changedNode ? { snapshot: undefined, cursor: undefined, history: undefined, trace: undefined, preview: undefined, statisticsPreview: undefined, tracePreview: undefined } : {}),
      ...(opening ? { modelIntent: undefined, cancellation: undefined, preview: undefined, statisticsPreview: undefined, tracePreview: undefined } : {}),
      ...(facts.attachmentIntent === 'released' ? { modelIntent: undefined, cancellation: undefined } : {}),
      ...(attached && result ? { snapshot: undefined, cursor: undefined, observationState: 'attached' as const, preview: undefined, statisticsPreview: undefined, tracePreview: undefined,
        turnOutline: undefined, turnNavigation: undefined } : {}),
    };
    if (context.deleted) {
      this.retireAttachmentWork(id);
      const views = { ...this.state.views }; delete views[id];
      this.publish({ views, sessions: this.state.sessions.filter(row => row.id !== id) });
      this.deletionListeners.forEach(listener => listener(id));
      return;
    }
    this.publish({ views: { ...this.state.views, [id]: projection } });
    if (previous?.epoch !== context.epoch) this.catalog.epochChanged(id);
    if (retiredClaim || facts.deletionCommitted && facts.deletionCommitted !== previous?.facts.deletionCommitted) this.retireAttachmentWork(id);
    if (facts.deletionCommitted && facts.deletionCommitted !== previous?.facts.deletionCommitted) {
      this.publish({ sessions: this.state.sessions.filter(row => row.id !== id) });
      this.deletionListeners.forEach(listener => listener(id));
      this.setSession(id, { snapshot: undefined });
    }
  }
  private readonly lifecycleAdmissions = new WeakSet<OperationAdmission>();
  private lifecyclePort(id: string): SessionLifecyclePort {
    const native = <T extends MethodResult['type']>(operation: Request1, expected: T, admission: OperationAdmission) => {
      this.lifecycleAdmissions.add(admission);
      return this.request(operation, expected, undefined, admission);
    };
    return {
      resolveNode: async current => {
        const result = await this.request({ method: 'session/read', params: { session_id: id } }, 'session', undefined, current);
        if (result.session.id !== id) throw new Error('Mismatched Session identity.');
        return { node: result.session.active_node, conversation: result.session.active_conversation_id };
      },
      conversation: async (node, current) => {
        const { node: found } = await this.request({ method: 'session/node', params: { session_id: id, node_id: node } }, 'node', undefined, current);
        if (found.id !== node) throw new Error('Mismatched native Node identity.');
        return found.conversation_id;
      },
      admit: current => this.admitSessionOperation(id, current),
      attach: async (node_id, admission) => {
        const generation = this.state.generation;
        try { return await native({ method: 'session/attach', params: { session_id: id, node_id } }, 'attached', admission); }
        catch (error) {
          // A bounded receipt can be lost after commitment. Reconcile the
          // existing connection claim; never replay Attach to infer its outcome.
          if (!isOutcomeUncertain(error) || generation !== this.state.generation || this.state.connection !== 'connected') throw error;
          const status = await this.request({method:'session/attachmentStatus',params:{session_id:id}},'attachment_status',undefined,admission.current);
          if (!status.target || status.target.session_id !== id) throw error;
          const {node} = await this.request({method:'session/node',params:{session_id:id,node_id}},'node',undefined,admission.current);
          if (node.id !== node_id || node.conversation_id !== status.target.conversation_id) throw error;
          return {type:'attached' as const,target:status.target,configuration_revision:status.configuration_revision,attachment_kind:'controller' as const,observation_source:'live' as const};
        }
      },
      detach: async (target, admission) => { await native({ method: 'session/detach', params: { target } }, 'detached', admission); },
      switchNode: async (target, node_id, admission) => (await native({ method: 'session/switchNode', params: { target, node_id } }, 'session', admission)).session,
      delete: async (expected_target_revision, admission) => (await native({ method: 'session/delete', params: { session_id: id, expected_target_revision } }, 'deletion', admission)).result,
      recover: async admission => (await native({ method: 'session/recoverDeletion', params: { session_id: id } }, 'deletion', admission)).result,
      inspectDeletion: async current => (await this.request({ method: 'session/deletePreview', params: { session_id: id } }, 'deletion', undefined, current)).result,
      observeAttached: async (result, current) => {
        if (!current()) return;
        const previous=this.resumeSeeds.get(id);
        this.resumeSeeds.delete(id);
        const resumable=previous&&previous.target.conversation_id===result.target.conversation_id
          &&previous.target.runtime_incarnation===result.target.runtime_incarnation;
        let checkpoint:import('../../../protocol/app-server/v50').RuntimeClientCheckpoint;
        let observed:RuntimeClientSnapshot;
        if(resumable){checkpoint={...previous.checkpoint,cursor:previous.cursor};observed=previous.snapshot;}
        else {
          this.setSession(id,{observationState:'checkpointLoading'});
          checkpoint=(await this.request({method:'session/checkpoint',params:{target:result.target,trace_records:[]}},'checkpoint',undefined,current)).checkpoint;
          validateRootCheckpoint(result.target, checkpoint);
          observed=checkpointSnapshot(checkpoint);
        }
        if(!current())return;
        validateRootCheckpoint(result.target, checkpoint);
        const chain=new ObservationChain<RuntimeClientEvent>();
        chain.installCheckpoint(checkpoint.cursor);
        this.observationChains.set(id,chain);
        this.setSession(id,{snapshot:observed,checkpoint,cursor:checkpoint.cursor,observationState:'subscribePending',
          ...(!resumable&&this.state.views[id]?.modelMutation?.generation!==this.state.generation?{modelMutation:undefined}:{}),
          history:replaceTranscript(observed.transcript,this.state.views[id]?.history),
          trace:this.traces.supersede(id,({...replaceTrace(observed.trace,this.state.views[id]?.trace,checkpoint.read_cut),fields:this.traces.liveFields(result.target)}))});
        chain.beginSubscription(checkpoint.cursor);
        try {
          const receipt=await this.request({method:'session/subscribe',params:{target:result.target,after_cursor:chain.applied!}},'subscribed',undefined,current);
          if(!current()||this.observationChains.get(id)!==chain)return;
          if(!sameTarget(receipt.target,result.target)||!chain.acceptReceipt(receipt.registration_id,receipt.after_cursor))throw new Error('Subscription recovery required.');
        } catch(error) {
          if(!current())return;
          if (error instanceof RpcFailure && error.error.data?.kind === 'observation_unavailable' || chain.phase === 'unavailable') {
            chain.markUnavailable(); this.setSession(id, {observationState:'unavailable'});
          } else { chain.requireRecovery(); this.setSession(id,{observationState:'resyncRequired'}); }
          if(error instanceof RpcFailure&&error.error.data?.kind==='resync_required'){
            this.resubscribe.add(id);await this.refresh(id);return;
          }
          throw error;
        }
        this.setSession(id, { observationState: chain.phase });
        this.reconcileInteractions(id); this.settleSubmissions(id);
        const settings=await this.reads.readSettings(id,current);
        if(!current())return;
        this.setSession(id,{settings:settings.settings});
        this.sourceRepairs.visit(settings.settings.cwd);
        void this.catalog.refreshDisplaySummary(id).catch(() => {});
      },
      refresh: () => this.refresh(id),
      cold: current => { void this.cold.readHistoryPreview(id, current); void this.cold.readMetadata(id, current); void this.traces.readPreview(id, current); },
      classify: error => error instanceof RequestNotDispatched ? 'unsent' : isOutcomeUncertain(error) ? 'uncertain'
        : error instanceof RpcFailure ? error.error.data?.kind === 'stale_attachment' ? 'stale-route' : 'refused' : 'local',
    };
  }
  // Product policy is injected by the Web owner, not interpreted by this transport.
  // Fail closed when there is no admission owner (including after its disposal).
  private sessionAdmission?: SessionAdmissionOwner;
  setSessionAdmission(admit: SessionAdmission) {
    // Every installation is a new incarnation, so retired proofs stay retired
    // even when the same callback is installed again.
    const owner: SessionAdmissionOwner = { admit };
    this.replaceSessionAdmission(owner);
    // Cleanup removes only its own incarnation; a stale one never disturbs a newer owner or its proofs.
    return () => { if (this.sessionAdmission === owner) this.replaceSessionAdmission(undefined); };
  }
  private replaceSessionAdmission(owner?: SessionAdmissionOwner) {
    this.sessionAdmission = owner;
    // Owner change retires its proofs; release their reservations outside the caller's stack, as navigation does.
    queueMicrotask(() => this.pump());
  }
  async admitSessionOperation(id: string, current: () => boolean = () => true): Promise<false | OperationAdmission> {
    const generation = this.state.generation;
    const owner = this.sessionAdmission;
    const valid = () => current() && this.current(generation) && owner === this.sessionAdmission;
    if (!valid()) return false;
    if (!owner) throw new Error('No Web attachment admission owner.');
    const allowed = await owner.admit(id, valid);
    return allowed && valid() && { current: () => allowed.current() && valid(), validate: allowed.validate };
  }
  restoreViews(ids: readonly string[]) {
    for (const id of ids.slice(0, 32)) if (!this.state.views[id]) this.lifecycles.restore(id);
  }
  private retireAuthority() {
    this.resumeSeeds.clear();
    this.attachmentIntakes.retireAll();
    for (const listener of [...this.authorityListeners]) listener();
    const sessions = Object.values(this.state.views).filter(view => view.deleting || view.error || view.modelMutation || view.cancellation)
      .map(({ id, error, modelMutation, cancellation, deleting, deletionCommitted }) => ({ id, error, modelMutation, cancellation,
        ...(deleting ? { deletion: deletionCommitted ?? 'uncertain' as const } : {}) }));
    // Admission reserved capacity for close-time evidence before synchronous fencing.
    // Generation-guarded continuations cannot create new Session diagnostics;
    // model/cancellation continuations only update already-reserved Session rows.
    const detached = [...(this.state.detached ?? [])];
    if (this.state.uncertain.length || sessions.length) detached.push({ authority: this.state.endpoint!, authorityId: this.state.authorityId, operations: this.state.uncertain, sessions });
    this.lifecycles.replaceAuthority(); this.reading.retireAuthority(); this.catalog.retireAuthority();
    this.log.clear();
    this.publish({ authorityId: undefined, views: {}, sessions: [], nextOffset: undefined, uncertain: [], interactionOperations: {}, detached,
      authorityRevision: (this.state.authorityRevision ?? 0) + 1 });
  }
  // Browser transport authority, not a durable server identity. Reconnect alone
  // can restore wanted Session intent; replacement must retire it after fencing.
  isSameAuthority(endpoint: string) { return !this.state.endpoint || new URL(endpoint).href === this.state.endpoint; }
  async connect(endpoint: string, token: string, transition: 'reconnect' | 'replace-authority' = 'reconnect', committed?: () => void) {
    const url = new URL(endpoint);
    if (!['ws:', 'wss:'].includes(url.protocol) || url.username || url.password || url.search || url.hash || url.pathname !== '/') {
      throw new Error('Use a ws:// or wss:// endpoint at / with no credentials, query, or fragment.');
    }
    if (!/^[A-Za-z0-9_-]{43,128}$/.test(token)) throw new Error('Enter the dedicated 43–128 character App Server transport token.');
    const replacing = !this.isSameAuthority(url.href);
    if (replacing && transition !== 'replace-authority') throw new Error('Different App Server authority requires explicit replacement.');
    if (replacing) this.admitAuthorityReplacement();
    const attempt = ++this.connectionAttempt;
    if (this.socket) this.endConnection('disconnected');
    const generation = this.state.generation;
    if (this.closing) await this.closing;
    if (generation !== this.state.generation || attempt !== this.connectionAttempt) return;
    if (replacing) {
      this.retireAuthority();
    }
    this.configurations.connect();
    this.sourceRepairs.connect();
    this.publish({ configuration: {}, configurationObservation: {}, sourceRepairs: {}, endpoint: url.href, connection: transition === 'reconnect' ? 'reconnecting' : 'connecting', capabilities: undefined, error: undefined });
    // Ownership commits after close/retirement, before attempting the new transport.
    committed?.();
    try {
      const socket = this.socketFactory(url.href, ['rustx.app-server.v50', `rustx-token.${token}`]);
      this.socket = socket;
      await new Promise<void>((resolve, reject) => {
        const fail = (message: string) => {
          reject(new Error(message));
          if (this.current(generation)) {
            const connecting = !this.initialized;
            this.lose(generation);
            this.publish({ connection: connecting ? 'error' : 'stale', error: message });
          }
        };
        const timer = setTimeout(() => fail('WebSocket connection timed out.'), this.timeoutMs);
        socket.onopen = () => { clearTimeout(timer); if (this.current(generation)) resolve(); else reject(new Error('Obsolete connection.')); };
        socket.onmessage = event => { if (this.current(generation)) this.receive(event.data, generation); };
        socket.onclose = () => { this.closedSockets.add(socket); clearTimeout(timer); fail('WebSocket closed. Check endpoint and transport token.'); };
        socket.onerror = () => { clearTimeout(timer); fail('WebSocket failed. Check endpoint and transport token.'); };
      });
      const hello = await this.request({ method: 'initialize', params: {
        protocol_version: 50, client: { name: 'rustx-web-console', version: '0.1.0' },
        presentation: { images: true, questionnaires: true, reviews: true },
      } }, 'initialized');
      if (!this.current(generation)) return;
      if (!hello.authority_id || hello.protocol_version !== 50 || !hello.capabilities.multi_session || !hello.capabilities.headless_interactions || !hello.capabilities.single_writable_controller) {
        throw new Error('Incompatible App Server protocol or capabilities. Protocol v50 with native multi-Session, headless interactions, and single-controller admission is required.');
      }
      if (this.state.authorityId && this.state.authorityId !== hello.authority_id) {
        try { this.admitAuthorityReplacement(); }
        catch (error) {
          // Keep unresolved operation diagnostics under their old authority, but
          // retire presentation now. No summaries or attachments from the new
          // process may be read until replacement can safely commit.
          this.publish({ sessions: [], authorityRevision: (this.state.authorityRevision ?? 0) + 1 });
          throw error;
        }
        this.retireAuthority();
      }
      this.publish({ authorityId: hello.authority_id });
      this.initialized = true;
      this.sourceRepairs.attribute(hello.authority_id);
      this.lifecycles.transport(generation, true);
      this.publish({ capabilities: hello.capabilities, connection: 'resynchronizing' });
      await this.catalog.list();
      // Each actor decides inspection or renewed admission from its retained intent and native outcome.
      await this.lifecycles.reconnect();
      if (this.current(generation)) this.publish({ connection: 'connected' });
    } catch (error) {
      if (!this.current(generation)) return;
      const incompatible = (error instanceof RpcFailure && error.error.data?.kind === 'unsupported_version') || String(error).includes('Incompatible');
      this.lose(generation);
      this.publish({ connection: incompatible ? 'incompatible' : 'error', error: String(error) });
      throw error;
    }
  }
  /** Side-effect-free policy, called immediately before fencing with no intervening await.
   * One replacement detaches at most one authority batch. pump transmits at most
   * sixteen requests across bounded lanes; only sent mutations become uncertain, each exactly once.
   * request admission already bounds uncertain + pending to 64 for mutations.
   * Reserve Session rows for pending continuations too, including unsent work.
   */
  private admitAuthorityReplacement() {
    const views = Object.values(this.state.views);
    if ((this.state.detached?.length ?? 0) >= 8) throw new Error('Review and acknowledge detached authority diagnostics before replacing another App Server.');
    const sessions = new Set(views.filter(view => view.deleting || view.error || view.modelMutation || view.cancellation).map(view => view.id));
    for (const pending of this.pending.values()) if (pending.context.sessionId) sessions.add(pending.context.sessionId);
    if (sessions.size > 64) throw new Error('Too many unresolved Session diagnostics. Review the current authority before replacing it.');
  }
  private current(generation: number) { return generation === this.state.generation && !!this.socket; }
  /** Explicit transport loss only; never dispatches semantic cancellation or unload. */
  private closing?: Promise<void>;
  private connectionAttempt = 0;
  private closedSockets = new WeakSet<Socket>();
  disconnect() { ++this.connectionAttempt; this.endConnection('disconnected'); return this.closing; }
  acknowledgeDetached(index: number) { this.publish({ detached: this.state.detached?.filter((_, at) => at !== index) }); }
  /** Browser evidence only. No native settlement, deletion notification or RPC. */
  acknowledgeSessionDiagnostic(id: string) {
    if (this.socket || this.pending.size) throw new Error('Disconnect before acknowledging current Session evidence.');
    const view = this.state.views[id];
    if (!view || !(view.deleting || view.error || view.modelMutation || view.cancellation)) return;
    this.lifecycles.forget(id);
    const views = { ...this.state.views }; delete views[id];
    this.publish({ views });
  }
  private lose(generation: number) {
    if (this.current(generation)) this.endConnection('stale');
  }
  private endConnection(connection: ConnectionState) {
    const oldSocket = this.socket;
    this.retainAppliedObservations();
    this.socket = undefined;
    this.initialized = false;
    this.sourceRepairs.disconnect();
    const uncertain = [...this.state.uncertain];
    for (const [id, pending] of this.pending) {
      clearTimeout(pending.timer); pending.validation?.abort(); pending.validation = undefined;
      if (pending.sent && pending.mutation) {
        const params = pending.request.params;
        uncertain.push({ id, method: pending.request.method, sessionId: pending.context.sessionId,
          generation: this.state.generation,
          ...(pending.request.method === 'session/uploadPrepare' ? { uploadOperationId: pending.request.params.operation_id } : {}),
          ...(pending.request.method === 'context/compact' ? { compactionRequestId: pending.request.params.request_id } : {}),
          ...('interaction' in params ? { interactionKey: interactionKey(params.interaction) } : {}),
        });
        pending.reject(new OutcomeUncertain());
      } else pending.reject(pending.sent
        ? new Error('Disconnected before a response.')
        : new RequestNotDispatched('Disconnected before a response. Unsent operations were discarded.'));
    }
    this.pending.clear();
    for (const pending of this.modelPreparations.values()) pending.retire();
    this.modelPreparations.clear();
    for (const chain of this.observationChains.values()) chain.retire(); this.observationChains.clear();
    this.refreshes.clear(); this.traces.disconnect(); this.acquiring.clear(); this.dirty.clear(); this.resubscribe.clear();
    const operations = { ...this.state.interactionOperations };
    for (const [key, operation] of Object.entries(operations)) if (operation.status === 'in-flight') delete operations[key];
    for (const item of uncertain) if (item.interactionKey) operations[item.interactionKey] = { sessionId: item.sessionId!, status: 'uncertain' };
    this.lifecycles.transport(this.state.generation + 1, false);
    this.publish({ connection, generation: this.state.generation + 1, uncertain, interactionOperations: operations,
      views: Object.fromEntries(Object.entries(this.state.views).map(([id, view]) => [id, {
        ...view,modelIntent:undefined,history:view.history,turnOutline: undefined, turnNavigation: undefined, submissions: undefined, inboundRequests: undefined,
      }])),
    });
    if (oldSocket && !this.closedSockets.has(oldSocket)) {
      const previousClose = oldSocket.onclose;
      this.closing = new Promise<void>((resolve, reject) => {
        const deadline = setTimeout(() => reject(new Error('Previous WebSocket did not close. Reload before reconnecting.')), this.timeoutMs);
        oldSocket.onclose = event => { clearTimeout(deadline); this.closedSockets.add(oldSocket); previousClose?.(event); resolve(); };
        oldSocket.close();
      });
      void this.closing.catch(() => {});
    }
  }
  /** Correlation only. No call is ever retried. Every payload is a generated union. */
  async request<T extends MethodResult['type']>(operation: Request1, expected: T, acknowledged?: (result: Extract<MethodResult, { type: T }>) => void, dispatchCurrent?: (() => boolean) | OperationAdmission, observationSignal?: AbortSignal): Promise<Extract<MethodResult, { type: T }>> {
    if (!this.socket || (!this.initialized && operation.method !== 'initialize')) throw new RequestNotDispatched('Connect and initialize first.');
    if (['session/attach', 'session/detach', 'session/switchNode', 'session/delete', 'session/recoverDeletion'].includes(operation.method)
      && (typeof dispatchCurrent !== 'object' || !this.lifecycleAdmissions.delete(dispatchCurrent))) throw new RequestNotDispatched('Session lifecycle operations require actor admission.');
    // The Session actor owns control admission. Retained targets authorize only
    // lifecycle settlement; live reads and new effects require the exact committed
    // attachment proof, captured once and checked again adjacent to socket send.
    let authority: (() => boolean) | undefined;
    if ('target' in operation.params && 'session_id' in operation.params.target
      && mayCommit(operation)
      && operation.method !== 'session/detach' && operation.method !== 'session/switchNode') {
      const target = operation.params.target, id = target.session_id;
      const admission = this.state.views[id]?.attachmentObservation;
      const current = () => (operation.method === 'turn/cancel' || operation.method === 'context/compact'
        ? this.lifecycles.observes(id, admission)
        : this.isAttachmentControlCurrent(id, admission)) && sameTarget(admission?.target, target);
      if (!current()) throw new RequestNotDispatched('Attachment control authority was revoked.');
      authority = current;
    }
    if ((operation.method.startsWith('artifact/') || operation.method === 'agent/artifactRead') && [...this.pending.values()].filter(item => (item.request.method.startsWith('artifact/') || item.request.method === 'agent/artifactRead')).length >= 2) throw new RequestNotDispatched('Artifact transfer capacity reached. Retry after current transfers finish.');
    const lane = requestLane(operation.method);
    if (lane !== 'rpc' && lane !== 'observation' && [...this.pending.values()].filter(item => requestLane(item.request.method) === lane).length >= DOMAIN_CAPACITY[lane]) {
      throw new RequestNotDispatched(`Client ${lane} capacity reached. Inspect current operations before issuing another.`);
    }
    if (this.pending.size >= 64) throw new RequestAdmissionDeferred(this.admissionRevision);
    // Keep uncertain diagnostics finite without silently forgetting unresolved mutations.
    if (mayCommit(operation) && this.state.uncertain.length + this.pending.size >= 64) throw new RequestNotDispatched('Uncertain-operation capacity reached. Inspect and acknowledge diagnostics first.');
    const generation = this.state.generation;
    const id = `${generation}:${++this.nextId}`;
    const request: Request = { jsonrpc: '2.0', id, ...operation };
    try {
      if (new TextEncoder().encode(JSON.stringify(request)).length > 1_048_576) throw new Error('Request exceeds the App Server 1 MiB limit.');
    } catch (cause) { throw new RequestNotDispatched(cause); }
    let abortObservation = () => {};
    const work = new Promise<MethodResult>((resolve, reject) => {
      const params = operation.params;
      const context = { method: operation.method,
        sessionId: 'target' in params && 'session_id' in params.target ? params.target.session_id : 'session_id' in params ? params.session_id : undefined };
      this.pending.set(id, { request, context, mutation: mayCommit(operation), sent: false, expected, resolve, reject, dispatchCurrent, authority, acknowledged: result => acknowledged?.(result as Extract<MethodResult, { type: T }>) });
      abortObservation = () => {
        if (operation.method !== 'agent/observe') return;
        const pending = this.pending.get(id);
        if (!pending) return;
        if (!pending.sent) { this.refuse(pending, new Error('Observation retired before dispatch')); this.pump(); }
        else void this.request({method:'agent/observeCancel',params:{request_id:id}},'agent_observe_cancelled').catch(() => {});
      };
      observationSignal?.addEventListener('abort', abortObservation, {once:true});
      if (observationSignal?.aborted) abortObservation();

      if (operation.method === 'turn/start' || operation.method === 'turn/steer') this.publishInbound(operation.params.target.session_id);
      this.pump();
    });
    const result = observationSignal ? await work.finally(() => observationSignal.removeEventListener('abort', abortObservation)) : await work;
    if (!this.current(generation)) throw new Error('Obsolete connection response; inspect the current authoritative state.');
    if (result.type !== expected) {
      this.lose(this.state.generation);
      throw new Error(`Incompatible response: expected ${expected}, received ${result.type}.`);
    }
    return result as Extract<MethodResult, { type: T }>;
  }
  private dispatchAllowed(pending: Pending): boolean {
    try {
      return dispatchCurrent(pending) && (!pending.authority || pending.authority())
        && !!this.socket && this.pending.get(String(pending.request.id)) === pending && !pending.sent;
    }
    catch (cause) { this.refuse(pending, cause); return false; }
  }
  /** A cancelled read remains the native admission owner until its correlated
   * terminal response arrives. Replacements wait in the existing bounded queue;
   * cancellation and reads of other exact targets retain their own lanes. */
  private observeOccupied(pending: Pending): boolean {
    const request = pending.request;
    if (request.method !== 'agent/observe') return false;
    return [...this.pending.values()].some(other => other !== pending && other.sent
      && other.request.method === 'agent/observe'
      && other.request.params.agent_id === request.params.agent_id
      && sameTarget(other.request.params.target, request.params.target));
  }
  private pump() {
    // An obsolete proof releases its reservation now, never after its Host read.
    for (const pending of this.pending.values()) if (pending.validation && !this.dispatchAllowed(pending)) this.refuse(pending);
    let occupied = [...this.pending.values()].filter(p => (p.sent || p.validation) && requestLane(p.request.method) === 'rpc').length;
    let observing = [...this.pending.values()].filter(p => p.sent && requestLane(p.request.method) === 'observation').length;
    let validating = [...this.pending.values()].filter(p => p.validation).length;
    for (const pending of this.pending.values()) {
      if (!this.socket) break;
      const lane = requestLane(pending.request.method);
      if (pending.sent || pending.validation || (lane === 'rpc' && occupied >= RPC_CAPACITY) || (lane === 'observation' && observing >= DOMAIN_CAPACITY.observation)) continue;
      if (!this.dispatchAllowed(pending)) { this.refuse(pending); continue; }
      if (this.observeOccupied(pending)) continue;
      const proof = typeof pending.dispatchCurrent === 'object' ? pending.dispatchCurrent : undefined;
      // A waiting validation holds nothing, so later requests are never blocked behind it.
      if (proof && validating >= VALIDATION_CAPACITY) continue;
      if (lane === 'rpc') occupied++;
      if (lane === 'observation') observing++;
      if (proof) {
        validating++;
        // Reserve the same bounded RPC slot while the operation owner revalidates.
        // No request is sent or marked uncertain during this read. The deadline
        // starts here, not at send; retirement aborts the read without awaiting it.
        const validation = new AbortController();
        pending.validation = validation;
        pending.timer = setTimeout(() => {
          this.refuse(pending, new Error('Operation admission validation timed out. No operation was sent.')); this.pump();
        }, this.timeoutMs);
        // Only the settlement that still owns the reservation may act on it.
        let checked: Promise<boolean>;
        try { checked = proof.validate(validation.signal); }
        catch (cause) { this.refuse(pending, cause); occupied--; validating--; continue; }
        void checked.then(allowed => {
          if (pending.validation !== validation) return;
          if (!allowed || !this.dispatchAllowed(pending)) this.refuse(pending);
          else { pending.validation = undefined; clearTimeout(pending.timer); this.sendPending(pending); }
        }, cause => {
          if (pending.validation === validation) this.refuse(pending, cause);
        }).finally(() => this.pump());
      } else this.sendPending(pending);
    }
  }
  private refuse(pending: Pending, cause: unknown = new Error('Authority changed before dispatch. No operation was sent.')) {
    if (pending.sent || !this.pending.delete(String(pending.request.id))) return;
    this.capacityReleased();
    clearTimeout(pending.timer); pending.validation?.abort(); pending.validation = undefined;
    pending.reject(new RequestNotDispatched(cause));
    if (pending.request.method === 'turn/start' || pending.request.method === 'turn/steer') this.publishInbound(pending.request.params.target.session_id);
  }
  private sendPending(pending: Pending) {
    const socket = this.socket, generation = this.state.generation;
    let raw: string;
    try { raw = JSON.stringify(pending.request); }
    catch (cause) { this.refuse(pending, cause); return; }
    if (!this.dispatchAllowed(pending)) { this.refuse(pending); return; }
    if (!socket || this.socket !== socket || !this.current(generation)
      || this.pending.get(String(pending.request.id)) !== pending || pending.sent) return;
    if (this.observeOccupied(pending)) return;
    if (socket.readyState !== 1) { this.lose(generation); return; }
    // No external observer runs between this final proof and send invocation.
    // sent means transmission attempted, never native acceptance.
    pending.sent = true;
    this.log.observe('out', generation, raw, pending.context);
    if (requestLane(pending.request.method) === 'rpc' && hasResponseDeadline(pending.request.method)) pending.timer = setTimeout(() => this.lose(generation), this.timeoutMs);
    try { socket.send(raw); } catch { this.lose(generation); }
    try { if (typeof pending.dispatchCurrent === 'object') pending.dispatchCurrent.sent?.(); }
    catch (error) { console.error('App Server dispatch observer failed', error); }
  }
  private receive(data: unknown, generation: number) {
    if (typeof data !== 'string' || data.length > 1_048_576 || new TextEncoder().encode(data).length > 1_048_576) { this.lose(generation); return; }
    let parsed: unknown;
    try { parsed = JSON.parse(data); } catch {
      this.log.observe('in', generation, data); this.lose(generation); return;
    }
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
      this.log.observe('in', generation, data); this.lose(generation); return;
    }
    const value = parsed as Response | Notification;
    const pending = 'id' in value && value.id !== null ? this.pending.get(String(value.id)) : undefined;
    this.log.observe('in', generation, data, pending?.context);
    if (value.jsonrpc !== '2.0') { this.lose(generation); return; }
    if ('result' in value || 'error' in value || 'id' in value) {
      if (!pending || !pending.sent) return;
      if (('result' in value) === ('error' in value) ||
          ('result' in value && (!value.result || value.result.type !== pending.expected))) {
        // Leave the transmitted request registered while closing, so a malformed
        // acknowledgement cannot erase its uncertain mutation diagnostic.
        this.lose(generation); return;
      }
      this.pending.delete(String(value.id)); clearTimeout(pending.timer);
      this.capacityReleased();
      if ('result' in value && value.result?.type === 'session_transition' && value.result.durability_diagnostic) {
        this.publish({ uncertain: [...this.state.uncertain, { id: String(value.id), method: pending.request.method, sessionId: pending.context.sessionId, generation,
          committedNode: { sessionId: value.result.session.id, nodeId: value.result.session.active_node }, diagnostic: value.result.durability_diagnostic }] });
      }
      // Evidence observers run at decode time, before continuation authority
      // checks. Their failures are local diagnostics, never a native RPC result.
      try {
        if ('result' in value) pending.acknowledged?.(value.result);
      } catch (error) {
        console.error('App Server acknowledgement observer failed', error);
      }
      try {
        const operation = pending.request;
        if (operation.method === 'turn/start' || operation.method === 'turn/steer') {
          const target = operation.params.target;
          const accepted = 'result' in value && value.result.type === 'inbound_accepted' && sameTarget(this.state.views[target.session_id]?.target, target)
            ? { messageId: value.result.message_id, content: operation.params.content } : undefined;
          // One publication hands request ownership to exact acknowledged identity.
          // Never publish a zero count before publishing the accepted MessageId.
          this.publishInbound(target.session_id, accepted);
          this.settleSubmissions(target.session_id);
        }
      } catch (error) {
        console.error('App Server response presentation failed', error);
      } finally {
        // The correlated wire outcome alone settles the RPC. Presentation
        // subscribers cannot strand it or consume a request capacity slot.
        if ('error' in value) pending.reject(new RpcFailure(value.error));
        else if ('result' in value) pending.resolve(value.result);
        this.pump();
      }
      return;
    }
    if (value.method === 'configuration/changed') { this.configurations.changed(value.params.application, generation); return; }
    if (value.method === 'configuration/sourceRepairChanged') {
      // Source-scoped (Issue #464): it addresses a configuration source, never
      // a Session, and never changes runtime application or conversation state.
      const { target } = value.params;
      if (target?.kind !== 'user' && (target?.kind !== 'workspace' || typeof target.directory !== 'string')) { this.lose(generation); return; }
      const receipt = value.params.receipt ?? undefined;
      const sequence = value.params.sequence;
      if (typeof sequence !== 'string' || !/^\d+$/.test(sequence)) { this.lose(generation); return; }
      this.sourceRepairs.notice(target, receipt, sequence, this.initialized ? this.state.authorityId : undefined);
      return;
    }
    if (value.method === 'session/ownershipRetired') return; // Product Host owns execution retirement.
    if (value.method === 'session/summaryInvalidated') {
      // Addressed by Session identity alone: no attachment target, so a branch
      // view — or a row this client only lists — converges without attaching a
      // runtime, and Session A can never update Session B.
      const sessionId = value.params?.session_id;
      if (typeof sessionId !== 'string' || typeof value.params.catalog_changed !== 'boolean') { this.lose(generation); return; }
      this.catalog.invalidateSummary(sessionId, generation);
      if (value.params.catalog_changed) this.catalog.invalidateCatalog(generation);
      return;
    }
    if (!['session/event', 'session/resyncRequired', 'session/observationUnavailable', 'session/closed'].includes(value.method) || !value.params?.target) {
      this.lose(generation); return;
    }
    const target = value.params.target;
    const view = this.state.views[target.session_id];
    // An attach response may be interleaved after its first notification.
    if (view?.attachment === 'attaching' && !view?.target) { this.dirty.add(target.session_id); if (value.method === 'session/resyncRequired') this.resubscribe.add(target.session_id); return; }
    if (!sameTarget(view?.target, target)) return;
    if (value.method === 'session/closed') {
      this.lifecycles.event(target.session_id, { type: 'ROUTE_CLOSED', target });
    } else {
      const observation = this.lifecycles.observe(target.session_id);
      if (!observation || !sameTarget(observation.target, target)) return;
      const { proof } = observation;
      const chain = this.observationChains.get(target.session_id);
      if (!chain) return;
      if (value.method === 'session/observationUnavailable') {
        const result = chain.deliver({ registrationId: value.params.registration_id, unavailable: true },
          (event, cursor) => this.applyObservation(target.session_id, target, event, cursor));
        if (result === 'unavailable' && observation.current() && this.observationChains.get(target.session_id) === chain) {
          this.reading.invalidateReading(target.session_id); this.reading.retireOutline(target.session_id);
          this.resubscribe.delete(target.session_id); this.dirty.delete(target.session_id);
          this.setSession(target.session_id, { observationState: 'unavailable', error: value.params.message.slice(0, 512) });
          this.lifecycles.event(target.session_id, { type: 'OBSERVATION', proof, status: 'stale', error: value.params.message.slice(0, 512) });
        }
        return;
      }
      const repairResync=value.method==='session/resyncRequired'&&chain.phase==='resyncRequired'&&chain.registrationId===value.params.registration_id;
      const result = chain.deliver(value.method === 'session/resyncRequired'
        ? { registrationId: value.params.registration_id, resync: true }
        : { registrationId: value.params.registration_id, cursor: value.params.cursor, event: value.params.event },
      (event, cursor) => this.applyObservation(target.session_id, target, event, cursor));
      if ((result === 'recovery'||repairResync) && observation.current() && this.observationChains.get(target.session_id)===chain) {
        this.reading.invalidateReading(target.session_id);this.reading.retireOutline(target.session_id);
        if(!observation.current())return;
        this.lifecycles.event(target.session_id, { type: 'OBSERVATION', proof, status: 'resynchronizing' });
        this.setSession(target.session_id, { observationState: 'resyncRequired' });
        this.resubscribe.add(target.session_id);
        // The active checkpoint already has a fresh subscription handoff.
        // If its cut is too old, Subscribe returns typed replay unavailability.
        if(!this.acquiring.has(target.session_id))void this.refresh(target.session_id).catch(() => {});
      }
    }
  }
  private applyObservation(id: string, target: AttachmentTarget, event: RuntimeClientEvent, cursor: string) {
    const view = this.state.views[id], observation = this.lifecycles.observe(id);
    if (!observation?.current() || !sameTarget(observation.proof.target, target) || !view?.snapshot) throw new Error('Observation view was replaced.');
    validateRootEvent(target, event, cursor);
    if (transitionFacts(event).some(fact=>fact.type==='pending_inbound_changed')) {
      this.reading.invalidateReading(id);
      if(!observation.current())throw new Error('Observation view was replaced during invalidation.');
      this.reading.retireOutline(id);
      const error = 'History changed. Reload the Turn outline to navigate again.';
      this.setSession(id, { turnOutline: { paging: view.turnOutline?.paging ?? { type: 'latest' }, error },
        ...(view.history?.window ? { history: { ...replaceTranscript(view.snapshot.transcript, view.history), error } } : {}) });
    }
    // Presentation subscribers may retire this exact observation synchronously.
    if(!observation.current()||!sameTarget(this.state.views[id]?.target,target))throw new Error('Observation view was replaced during publication.');
    const snapshot = foldRuntimeEvent(view.snapshot, event);
    const checkpointAdvanced = transitionFacts(event).some(fact => fact.type === 'checkpoint_advanced');
    const traceAdvanced = checkpointAdvanced || transitionFacts(event).some(fact => fact.type === 'read_domains_updated');
    this.setSession(id, { snapshot, ...(traceAdvanced ? {trace:this.traces.supersede(id,refreshTrace(view.trace,snapshot.trace,snapshot.trace_updates,observationCheckpoint(snapshot,cursor)?.read_cut))} : {}), checkpoint: observationCheckpoint(snapshot, cursor) ?? view.checkpoint, cursor, observationState: 'following',
      history: snapshot.transcript === view.snapshot.transcript ? this.state.views[id].history : refreshTranscript(this.state.views[id].history, snapshot.transcript) });
    const settled=transitionFacts(event).filter(fact=>fact.type==='interaction_settled'||fact.type==='interaction_removed');
    if(settled.length){
      const keys=new Set(settled.map(fact=>interactionKey((fact as Extract<RuntimeClientEvent,{type:'interaction_settled'|'interaction_removed'}>).interaction)));
      this.publish({interactionOperations:Object.fromEntries(Object.entries(this.state.interactionOperations).filter(([key])=>!keys.has(key))),
        uncertain:this.state.uncertain.filter(value=>!value.interactionKey||!keys.has(value.interactionKey))});
    }
    this.reconcileInteractions(id); this.settleSubmissions(id);
    if (checkpointAdvanced) void this.catalog.refreshDisplaySummary(id);
    if (transitionFacts(event).some(fact=>fact.type==='trace_changed')) void this.traces.refreshDomain(id).catch(() => {});
  }

  /** Bounded native Session, model, settings and content reads. */
  readonly reads = new SessionReads((...args) => this.request(...args));
  private readonly configurations = new ConfigurationObservations({
    request: (...args) => this.request(...args),
    state: () => this.state,
    current: generation => this.current(generation),
    publish: patch => this.publish(patch),
  });
  private readonly reading = new ReadingNavigation({
    request: (...args) => this.request(...args),
    state: () => this.state,
    setSession: (id, patch) => this.setSession(id, patch),
    observe: id => this.lifecycles.observe(id),
    epoch: id => this.lifecycles.epoch(id),
  });
  private readonly cold = new ColdReads({
    request: (...args) => this.request(...args),
    state: () => this.state,
    current: generation => this.current(generation),
    epoch: id => this.lifecycles.epoch(id),
    admit: (id, current) => this.admitSessionOperation(id, current),
    readSettings: (id, current) => this.reads.readSettings(id, current),
    setSession: (id, patch) => this.setSession(id, patch),
  });
  private readonly traces = new TraceReads({
    request: (...args) => this.request(...args),
    state: () => this.state,
    setSession: (id, patch) => this.setSession(id, patch),
    observe: id => this.lifecycles.observe(id),
    admit: (id, current) => this.admitSessionOperation(id, current),
    coldScope: (id, admitted) => this.cold.scope(id, admitted),
    readContent: (...args) => this.reads.readContent(...args),
  });
  loadEarlierTrace(id: string) { return this.traces.loadEarlierTrace(id); }
  locateToolTrace(id: string, locator: TraceToolLocator) { return this.traces.locateToolTrace(id, locator); }
  returnToLatestTrace(id: string) { return this.traces.returnToLatestTrace(id); }
  selectTrace(id: string, record?: string) { this.traces.selectTrace(id, record); }
  loadTraceDetail(id: string, record: string) { return this.traces.loadTraceDetail(id, record); }
  projections(id: string, after?: string) { return this.traces.projections(id, after); }
  projectionDetail(id: string, checkpointId: string, offset?: number) { return this.traces.projectionDetail(id, checkpointId, offset); }
  projectionDetailReader(detail: import('../../../protocol/app-server/v50').ProjectionDetail) { return this.traces.projectionDetailReader(detail); }
  /** A new navigation or an explicit reload retires the previous jump's landing. */
  invalidateReading(id: string) { return this.reading.invalidateReading(id); }
  refreshTurns(id: string) { return this.reading.refreshTurns(id); }
  readTurns(id: string, offset?: number, userCurrent?: () => boolean) { return this.reading.readTurns(id, offset, userCurrent); }
  previewTurns(id: string, offset: number, current: () => boolean) { return this.reading.previewTurns(id, offset, current); }
  navigateTurn(id: string, selection: TurnSelection, userCurrent?: () => boolean) { return this.reading.navigateTurn(id, selection, userCurrent); }
  navigateMessage(id: string, messageId: string, userCurrent?: () => boolean) { return this.reading.navigateMessage(id, messageId, userCurrent); }
  returnToLatest(id: string) { this.reading.returnToLatest(id); }
  loadLater(id: string) { return this.reading.loadLater(id); }
  loadEarlier(id: string) { return this.reading.loadEarlier(id); }
  loadEarlierPreview(id: string) { return this.cold.loadEarlierPreview(id); }
  private readonly catalog = new SessionCatalog({
    request: (...args) => this.request(...args),
    state: () => this.state,
    current: generation => this.current(generation),
    epoch: id => this.lifecycles.epoch(id),
    publish: patch => this.publish(patch),
  });
  listSessions(offset?: number, query?: string, current?: () => boolean) { return this.catalog.list(offset, query, current); }
  readSessionSummary(id: string) { return this.catalog.readSummary(id); }
  renameSession(id: string, name: string) { return this.catalog.rename(id, name); }
  private exports = new SessionExportController(async id => {
    const endpoint = this.state.endpoint;
    const generation = this.state.generation;
    if (!endpoint) throw new Error('No App Server connected');
    const { download } = await this.request({ method: 'session/exportPrepare', params: { session_id: id } }, 'session_archive');
    if (!this.current(generation)) throw new Error('Archive preparation belongs to a disconnected App Server');
    return { download, endpoint };
  });
  exportSession(id: string): Promise<void> { return this.exports.download(id); }

  async deleteSession(id: string, expectedRevision: string) {
    const result = await this.lifecycles.command(id, { kind: 'delete', revision: expectedRevision, current: () => true });
    await this.catalog.list(); return result;
  }
  async recoverSessionDeletion(id: string) {
    const result = await this.lifecycles.command(id, { kind: 'recover', current: () => true });
    await this.catalog.list(); return result;
  }
  private deletionListeners = new Set<(id: string) => void>();
  subscribeSessionDeletion = (listener: (id: string) => void) => {
    this.deletionListeners.add(listener);
    return () => { this.deletionListeners.delete(listener); };
  };
  /** Wait for admitted work without manufacturing another Open. */
  async waitForAttachment(id: string, current: () => boolean): Promise<void> {
    await this.lifecycles.waitForOpen(id);
    if (!current() || !this.lifecycles.controls(id, this.state.views[id]?.attachmentObservation)) throw new Error(this.state.views[id]?.error ?? 'Conversation connection changed. Your input was not sent.');
  }
  async attach(id: string, nodeId?: string, navigationCurrent: () => boolean = () => true, attached?: (target: AttachmentTarget) => void): Promise<void> {
    if (!navigationCurrent()) return;
    await this.lifecycles.command(id, { kind: 'open', node: nodeId, current: navigationCurrent, attached });
  }
  /** Explicit reconciliation/recovery only. Native bounded replay owns overlap. */
  refresh(id: string): Promise<void> {
    const observation = this.lifecycles.observe(id);
    if (!observation) return Promise.resolve();
    this.observationChains.get(id)?.requireRecovery();
    this.lifecycles.event(id, { type: 'OBSERVATION', proof: observation.proof, status: 'resynchronizing' });
    this.dirty.add(id);
    const existing = this.refreshes.get(id);
    if (existing) return existing;
    const work = this.performRefresh(id, observation.proof);
    this.refreshes.set(id, work);
    void work.finally(() => { if (this.refreshes.get(id) === work) this.refreshes.delete(id); }).catch(() => {});
    return work;
  }
  private async performRefresh(id: string, proof: Observation) {
    let replayRepairs = 0;
    const target = proof.target, current = () => this.lifecycles.observes(id, proof);
    try {
      while (this.dirty.has(id) && current()) {
        this.dirty.delete(id);
        const resync = this.resubscribe.delete(id);
        if (resync) {
          this.reading.retireOutline(id);
          this.lifecycles.event(id, { type: 'OBSERVATION', proof, status: 'resynchronizing' });
          if (!current()) return;
          this.setSession(id, {
            trace: this.traces.supersede(id, replaceTrace({ records: [], next_cursor: null }, this.state.views[id]?.trace)),
            history: replaceTranscript({ entries: [] }, this.state.views[id]?.history), turnOutline: undefined, turnNavigation: undefined,
          });
        }
        if (!current()) return;
        this.acquiring.add(id);
        const recoveringChain = this.observationChains.get(id) ?? new ObservationChain<RuntimeClientEvent>();
        recoveringChain.beginCheckpointLoading();
        this.observationChains.set(id, recoveringChain);
        this.setSession(id, { observationState: 'checkpointLoading' });
        const { checkpoint } = await this.request({ method: 'session/checkpoint', params: { target, trace_records: traceInterests(this.state.views[id]?.trace) } }, 'checkpoint', undefined, current);
        if (!current()) return;
        validateRootCheckpoint(target, checkpoint, recoveringChain.applied);
        const chain = this.observationChains.get(id) ?? new ObservationChain<RuntimeClientEvent>();
        chain.installCheckpoint(checkpoint.cursor);
        this.observationChains.set(id, chain);
        this.setSession(id, { snapshot: checkpointSnapshot(checkpoint), checkpoint, preview: undefined, statisticsPreview: undefined, tracePreview: undefined,
          cursor:checkpoint.cursor,observationState:'subscribePending',
          ...(this.state.views[id]?.modelMutation?.generation!==this.state.generation?{modelMutation:undefined}:{}),history: refreshTranscript(this.state.views[id]?.history, checkpoint.snapshot.transcript),
          trace: this.traces.supersede(id, ({...refreshTrace(this.state.views[id]?.trace, checkpoint.snapshot.trace, checkpoint.snapshot.trace_updates,checkpoint.read_cut),fields:this.traces.liveFields(target)})), error: undefined });
        this.reconcileInteractions(id); this.settleSubmissions(id);
        this.acquiring.delete(id);
        this.resubscribe.delete(id);
        chain.beginSubscription(checkpoint.cursor);
        try {
          const receipt = await this.request({ method: 'session/subscribe', params: { target, after_cursor: checkpoint.cursor } }, 'subscribed', undefined, current);
          if (!current() || this.observationChains.get(id) !== chain) return;
          if (!sameTarget(receipt.target, target) || !chain.acceptReceipt(receipt.registration_id, receipt.after_cursor)) throw new Error('Subscription recovery required.');
          this.setSession(id, { observationState: chain.phase });
        } catch (error) {
          if (current() && error instanceof RpcFailure && error.error.data?.kind === 'resync_required' && ++replayRepairs < 2) {
            this.resubscribe.add(id); this.dirty.add(id);
          } else throw error;
        }
        if (current() && !this.dirty.has(id)) {this.lifecycles.event(id, { type: 'OBSERVATION', proof, status: 'attached' });void this.catalog.refreshDisplaySummary(id);
          if(traceInterestBatches(this.state.views[id]?.trace).length>1)void this.traces.refreshDomain(id).catch(error=>{if(current()&&this.state.views[id]?.trace)this.setSession(id,{trace:{...this.state.views[id].trace!,error:String(error)}});});
        }
      }
    } catch (error) {
      if (current()) {
        this.acquiring.delete(id);
        const chain = this.observationChains.get(id);
        if (error instanceof RpcFailure && error.error.data?.kind === 'observation_unavailable' || chain?.phase === 'unavailable') {
          chain?.markUnavailable(); this.resubscribe.delete(id); this.dirty.delete(id); this.setSession(id, {observationState:'unavailable'});
        } else this.resubscribe.add(id);
        this.lifecycles.event(id, { type: 'OBSERVATION', proof, status: 'stale', error: String(error) });
      }
      throw error;
    }
  }
  private reconcileInteractions(id: string) {
    if (!this.lifecycles.observe(id)) return;
    const snapshot = this.state.views[id].snapshot!;
    const request = this.state.views[id].compactionRequest;
    const target = this.state.views[id].target;
    if (request) {
      if (request.authorityId !== this.state.authorityId || (request.generation === this.state.generation && !sameTarget(request.target, target)) || request.target.conversation_id !== target?.conversation_id
        || request.target.runtime_incarnation !== target?.runtime_incarnation) {
        this.setSession(id, { compactionRequest: undefined });
      } else if (snapshot.context?.compaction_in_progress && ['succeeded', 'failed'].includes(request.status)) {
        this.setSession(id, { compactionRequest: undefined });
      } else {
        const evidence = snapshot.context?.manual_compaction;
        if (evidence?.request_id === request.requestId && evidence.released) {
          // Native release ends gesture ownership independently of the RPC reply.
          if (this.compactionRequests.get(id)?.requestId === request.requestId) this.compactionRequests.delete(id);
          if (request.status !== 'succeeded') this.setSession(id, { compactionRequest: { ...request, status: evidence.error ? 'failed' : 'succeeded', diagnostic: evidence.error ?? undefined } });
          if (this.state.uncertain.some(item => item.compactionRequestId === request.requestId && item.sessionId === id)) {
            this.publish({ uncertain: this.state.uncertain.filter(item => item.compactionRequestId !== request.requestId || item.sessionId !== id) });
          }
        }
      }
    }
    const mutation = this.state.views[id].modelMutation;
    if (mutation && mutation.generation !== this.state.generation) this.setSession(id, { modelMutation: undefined });
    const cancellation = this.state.views[id].cancellation;
    // Until acknowledgement the retained display Attempt is not the operation's
    // native target, so even its observed settlement cannot resolve this request.
    if (cancellation?.status === 'acknowledged' && cancellation.attemptId !== undefined
      && snapshot.attempt?.attempt_id === cancellation.attemptId && snapshot.attempt.phase.type === 'settled') {
      this.setSession(id, { cancellation: undefined });
    }
    const frontier=observationCheckpoint(snapshot,this.state.views[id].cursor??'0')?.collections.find(value=>value.domain==='pending_interactions');
    // A bounded head cannot establish absence from the complete native set.
    if(frontier&&BigInt(frontier.total)>BigInt(snapshot.pending_interactions?.length??0))return;
    const pending = new Set(snapshot.pending_interactions?.map(item => interactionKey(item.interaction)));
    const operations = { ...this.state.interactionOperations };
    // Only an authoritative fresh snapshot can establish absence. Include routed
    // children by using the recorded Session route, not just root ConversationId.
    const resolved = this.state.uncertain.filter(item => item.sessionId === id && item.interactionKey && !pending.has(item.interactionKey));
    for (const item of resolved) delete operations[item.interactionKey!];
    for (const key of Object.keys(operations)) {
      if (operations[key].sessionId === id && !pending.has(key)) delete operations[key];
    }
    this.publish({ uncertain: this.state.uncertain.filter(item => !resolved.includes(item)), interactionOperations: operations });
  }
  target(id: string) {
    return this.lifecycles.target(id);
  }
  /** Validate command/tree evidence against the acquired Node. Observation
   * never changes desired identity; switching belongs to its explicit lifecycle. */
  assertAttachmentNode(target: AttachmentTarget, nodeId: string) {
    const view = this.state.views[target.session_id];
    if (!sameTarget(view?.target, target) || view?.attachmentNodeId !== nodeId) throw new Error('Node does not own the current attachment.');
  }
  async uploadStatus(id: string, operationId: string) {
    const target = this.target(id), generation = this.state.generation;
    const { outcome } = await this.request({ method: 'session/uploadStatus', params: { target, operation_id: operationId } }, 'upload_status');
    if (outcome.state !== 'unresolved' && this.current(generation) && sameTarget(this.state.views[id]?.target, target)
      && this.state.uncertain.some(item => item.sessionId === id && item.uploadOperationId === operationId)) {
      this.publish({ uncertain: this.state.uncertain.filter(item => item.sessionId !== id || item.uploadOperationId !== operationId) });
    }
    return outcome;
  }
  async upload(id: string, files: readonly File[], evidence?: { current: () => boolean; acknowledged: (files: UploadedFile[]) => void }, operationId = uploadOperationId()): Promise<UploadedFile[]> {
    let target: AttachmentTarget;
    try { target = this.target(id); } catch (error) { throw new UploadFailure('failed', error); }
    const policy = this.state.capabilities?.upload_policy;
    if (!policy) throw new UploadFailure('failed', 'Upload policy unavailable');
    if (!files.length || files.length > policy.max_files_per_transfer || files.some(file => file.size > policy.max_file_bytes) || files.reduce((sum, file) => sum + file.size, 0) > policy.max_transfer_bytes) throw new UploadFailure('failed', 'Selection exceeds native upload policy');
    const admission = this.state.views[id]?.attachmentObservation;
    const current = () => (!evidence || evidence.current()) && this.isAttachmentControlCurrent(id, admission) && sameTarget(admission?.target, target);
    if (!current()) throw new UploadFailure('failed', 'Upload authority changed');
    const read = async () => {
      const outcome = await this.uploadStatus(id, operationId).catch(error => { throw new UploadFailure('uncertain', error); });
      if (outcome.state !== 'ready') throw new UploadFailure(outcome.state === 'unresolved' ? 'uncertain' : 'failed');
      evidence?.acknowledged(outcome.files);
      if (!current()) throw new UploadFailure('uncertain', 'Upload belongs to a retired view');
      return outcome.files;
    };
    let transfer;
    try {
      ({ transfer } = await this.request({ method: 'session/uploadPrepare', params: { target, operation_id: operationId, files: files.map(file => ({ name: file.name, size: file.size })) } }, 'upload_prepared', undefined, current));
    } catch (error) {
      if (error instanceof RequestNotDispatched) throw new UploadFailure('failed', error);
      // A server refusal published no new preparation, but the exact operation
      // may already exist. Read its authority before permitting a fresh Retry.
      if (error instanceof RpcFailure && !isOutcomeUncertain(error) && current()) return read();
      throw new UploadFailure('uncertain', error);
    }
    try {
      if (!current()) throw new Error('Upload authority changed');
      await this.uploadCarrier(transfer, files, policy, this.state.endpoint);
    } catch {
      // No retransmission: the authoritative read below repairs carrier loss.
    }
    if (!current()) throw new UploadFailure('uncertain', 'Upload authority changed');
    return read();
  }

  async send(id: string, text: string, receipts: readonly UploadReceipt[] = [], delivery: 'send' | 'steer' = 'send', acknowledged?: (result: Extract<MethodResult, { type: 'inbound_accepted' }>) => void, dispatchCurrent?: () => boolean) {
    if (receipts.some(receipt => receipt.session_id !== id)) throw new Error('Invalid Session upload receipts.');
    const content: UserInputBlock[] = [
      ...receipts.map(receipt => ({ type: 'upload' as const, ...receipt })),
      ...(text ? [{ type: 'text' as const, text }] : []),
    ];
    return this.sendContent(id, content, delivery, acknowledged, dispatchCurrent);
  }
  async sendContent(id: string, content: UserInputBlock[], delivery: 'send' | 'steer' = 'send', acknowledged?: (result: Extract<MethodResult, { type: 'inbound_accepted' }>) => void, dispatchCurrent?: () => boolean) {
    const generation = this.state.generation;
    const preparation = this.modelPreparations.get(id);
    if (preparation) await preparation.work;
    if (!this.current(generation)) throw new Error('Connection changed before sending.');
    if (this.state.views[id]?.modelIntent) throw new Error(this.state.views[id].modelIntent?.error ?? 'Model selection is not ready.');
    if (this.state.views[id].modelMutation) throw new Error('Reread native model state before sending.');
    const target = this.target(id);
    // `turn/start` and `turn/steer` share one native inbound owner: an idle runtime
    // admits a fresh attempt, a running one drains the mailbox at a safe boundary.
    // The request pipeline owns unresolved transport and its acknowledgement
    // handoff. No MessageId or queue identity is invented before acceptance.
    return this.request({ method: delivery === 'steer' ? 'turn/steer' : 'turn/start', params: { target, content } }, 'inbound_accepted', acknowledged, dispatchCurrent);
  }
  private publishInbound(id: string, accepted?: Submission) {
    const view = this.state.views[id];
    if (!view) return;
    const inboundRequests = [...this.pending.values()].filter(item =>
      (item.request.method === 'turn/start' || item.request.method === 'turn/steer') && item.context.sessionId === id).length;
    const submissions = view.submissions ?? [];
    this.setSession(id, { inboundRequests, submissions: accepted && !submissions.some(item => item.messageId === accepted.messageId)
      ? [...submissions, accepted] : submissions });
  }
  /** An accepted submission settles only when an authoritative snapshot names its
   * exact MessageId: pending in the mailbox (the native row replaces it) or adopted
   * into canonical messages. No text, order or queue-length matching. */
  private settleSubmissions(id: string) {
    if (!this.lifecycles.observe(id)) return;
    const view = this.state.views[id];
    if (!view?.submissions?.length || !view.snapshot) return;
    const observed = new Set([
      ...(view.snapshot.inbound.pending ?? []).map(item => item.message.id),
      ...view.snapshot.messages.map(message => message.id),
      ...(view.snapshot.transcript.entries ?? []).flatMap(entry => entry.item.type === 'message' ? [entry.item.message.id] : []),
    ]);
    const remaining = view.submissions.filter(item => !observed.has(item.messageId));
    if (remaining.length !== view.submissions.length) this.setSession(id, { submissions: remaining });
  }
  /** One CAS-bound Goal control. `expected` is the authoritative GoalRef the caller
   * rendered. A known outcome (applied or refused) is not projection convergence:
   * `observed` is true only when a snapshot read requested after the outcome
   * succeeded for this attachment. A lost response stays uncertain. Nothing is
   * retried and no newer revision is ever substituted. */
  /** Load the original typed Goal objective at one exact native read cut. */
  async goalObjective(id: string, expected: GoalRef): Promise<string> {
    const view = this.state.views[id], target = view?.target, cut = view?.checkpoint?.read_cut, generation = this.state.generation;
    const current = () => this.current(generation) && sameTarget(this.state.views[id]?.target, target)
      && this.state.views[id]?.snapshot?.goal?.current?.reference.id === expected.id
      && this.state.views[id]?.snapshot?.goal?.current?.reference.revision === expected.revision;
    if (!target || !cut || !current()) throw new Error('Goal observation was replaced.');
    const path = ['current', 'objective'];
    const { chunk } = await this.request({ method: 'conversation/checkpointValueRead', params: {
      target, agent_id: null, cut, domain: 'goal', item_offset: 0, path, offset: 0, max_bytes: 65_536,
    } }, 'checkpoint_value_chunk', undefined, current);
    if (!current()) throw new Error('Goal observation was replaced.');
    if(chunk.bytes_base64.length>87_384 || chunk.bytes_base64.length%4!==0 || chunk.bytes_base64.length/4*3-(chunk.bytes_base64.endsWith('==')?2:chunk.bytes_base64.endsWith('=')?1:0)>65_536)throw new Error('Encoded native range exceeds the decoded byte bound.');
    const bytes = Uint8Array.from(atob(chunk.bytes_base64), value => value.charCodeAt(0));
    if (JSON.stringify(chunk.cut) !== JSON.stringify(cut) || chunk.domain !== 'goal' || chunk.item_offset !== 0
      || JSON.stringify(chunk.path) !== JSON.stringify(path) || chunk.offset !== 0 || chunk.next_offset != null
      || !Number.isSafeInteger(chunk.readable_end) || chunk.readable_end !== bytes.length || bytes.length > 65_536) throw new Error('Goal objective range changed.');
    const text = new TextDecoder('utf-8', { fatal: true }).decode(bytes);
    const original: unknown = chunk.representation === 'utf8_text' ? text : chunk.representation === 'canonical_json_v1' ? JSON.parse(text) : undefined;
    if (typeof original !== 'string' || new TextEncoder().encode(original).length > 8192) throw new Error('Goal objective exceeds its native bound.');
    return original;
  }
  async controlGoal(id: string, expected: GoalRef, mutation: GoalMutation): Promise<GoalControlOutcome> {
    const generation = this.state.generation;
    let target: AttachmentTarget;
    try { target = this.target(id); } catch (error) { return { status: 'rejected', reason: error instanceof Error ? error.message : String(error), observed: false }; }
    const current = () => this.current(generation) && sameTarget(this.state.views[id]?.target, target);
    try {
      await this.request({ method: 'goal/control', params: { target, control: { action: 'mutate', expected, mutation } } }, 'goal_mutation');
    } catch (error) {
      if (error instanceof OutcomeUncertain) return { status: 'uncertain' };
      if (!current()) return { status: 'obsolete' };
      return { status: 'rejected', reason: goalRefusal(error), observed: await this.reread(id, current) };
    }
    if (!current()) return { status: 'obsolete' };
    return { status: 'applied', observed: await this.reread(id, current) };
  }
  /** `refresh` re-marks the view dirty, so even a coalesced in-flight refresh
   * completes a snapshot request issued after this call before resolving. */
  private async reread(id: string, current: () => boolean) {
    const observation = this.lifecycles.observe(id);
    if (!observation || !current()) return false;
    try { await this.refresh(id); } catch { return false; }
    return current() && observation.current() && this.state.views[id]?.attachment === 'attached';
  }
  async editInbound(id: string, expected: PendingInboundRef, text: string): Promise<InboundControlOutcome> {
    return this.controlInbound(id, target => ({ method: 'inbound/edit', params: { target, expected, text } }));
  }
  async removeInbound(id: string, expected: PendingInboundRef): Promise<InboundControlOutcome> {
    return this.controlInbound(id, target => ({ method: 'inbound/remove', params: { target, expected } }));
  }
  private async controlInbound(id: string, operation: (target: AttachmentTarget) => Request1): Promise<InboundControlOutcome> {
    const generation = this.state.generation;
    let target: AttachmentTarget;
    try { target = this.target(id); } catch (error) { return { status: 'rejected', reason: String(error), observed: false }; }
    const current = () => this.current(generation) && sameTarget(this.state.views[id]?.target, target);
    try {
      const result = await this.request(operation(target), 'inbound_mutation');
      if (!current()) return { status: 'obsolete' };
      if (result.outcome.status === 'durability_uncertain') return { status: 'uncertain' };
      return { status: 'known', outcome: result.outcome, observed: await this.rereadPending(id, current) };
    } catch (error) {
      if (!current()) return isOutcomeUncertain(error) ? { status: 'uncertain' } : { status: 'obsolete' };
      if (isOutcomeUncertain(error)) return { status: 'uncertain' };
      return { status: 'rejected', reason: String(error), observed: await this.rereadPending(id, current) };
    }
  }
  /** Pending edits/removals invalidate historical windows too. Replace after
   * the final coalesced read, so an older in-flight response cannot restore a
   * removed row through the normal append-only transcript merge. */
  private async rereadPending(id: string, current: () => boolean) {
    const observation = this.lifecycles.observe(id);
    const observed = await this.reread(id, current) && !!observation?.current();
    const view = this.state.views[id];
    if (observed && view?.snapshot) this.setSession(id, { history: replaceTranscript(view.snapshot.transcript, view.history) });
    return observed;
  }
  private modelPreparations = new Map<string, { work: Promise<import('../../../protocol/app-server/v50').SessionModelConfig>; retire: () => void }>();
  /** Client-owned selection while native resources initialize. Last unsent choice
   * wins; the existing native mutation and authoritative reread still own apply. */
  prepareAgentModel(id: string, config: import('../../../protocol/app-server/v50').SessionModelConfig) {
    const view = this.state.views[id];
    if (view?.attachment !== 'attaching' || view.attachmentIntent !== 'wanted' || view.deleting)
      return Promise.reject(new Error('Conversation is no longer connecting.'));
    this.setSession(id, { modelIntent: { config, phase: 'waiting' } });
    const previous = this.modelPreparations.get(id);
    if (previous) return previous.work;
    let live = true;
    const generation = this.state.generation, epoch = this.lifecycles.epoch(id);
    const current = () => live && this.current(generation) && this.lifecycles.epoch(id) === epoch
      && this.state.views[id]?.attachmentIntent === 'wanted' && !this.state.views[id]?.deleting;
    const work = (async () => {
      try {
        await this.waitForAttachment(id, current);
        if (!current()) throw new Error('Model selection retired with its connection.');
        const selection = this.state.views[id].modelIntent!.config;
        this.setSession(id, { modelIntent: { config: selection, phase: 'applying' } });
        await this.setAgentModel(id, selection);
        if (!current()) throw new Error('Model selection retired with its connection.');
        await this.repairAgentModel(id);
        if (!current()) throw new Error('Model selection retired with its connection.');
        const observed = this.state.views[id].snapshot?.model?.configured;
        if (observed?.model !== selection.model || (observed.profile ?? undefined) !== (selection.profile ?? undefined))
          throw new Error('Selected model has not been confirmed. Reread model configuration before sending.');
        this.setSession(id, { modelIntent: undefined });
        return selection;
      } catch (error) {
        const intent = this.state.views[id]?.modelIntent;
        if (current() && intent) this.setSession(id, { modelIntent: { ...intent, phase: 'failed', error: String(error) } });
        throw error;
      }
    })();
    this.modelPreparations.set(id, { work, retire: () => { live = false; } });
    void work.finally(() => { if (this.modelPreparations.get(id)?.work === work) this.modelPreparations.delete(id); }).catch(() => {});
    return work;
  }
  /** Native catalog defaults at the exact generation whose ordinals were read. */
  async selectCatalogModel(id:string,choice:import('../../../protocol/app-server/v50').CatalogModelSelection,owned:()=>boolean=()=>true) {
    const target=this.target(id),generation=this.state.generation;
    if(this.state.views[id].modelMutation)throw new Error('Reread native model state before another mutation.');
    const current=()=>owned()&&this.current(generation)&&sameTarget(this.state.views[id]?.target,target);
    if(!current())return;
    const operation={generation,status:'in-flight' as const};this.setSession(id,{modelMutation:operation});
    try {
      await this.request({method:'session/selectModel',params:{target,choice}},'model_set',undefined,current);
      if(this.current(generation)&&sameTarget(this.state.views[id]?.target,target))this.setSession(id,{modelMutation:{generation,status:'acknowledged'}});
    }catch(error){
      if(this.state.views[id]?.modelMutation===operation)this.setSession(id,{modelMutation:error instanceof RequestNotDispatched?undefined:{generation,status:isOutcomeUncertain(error)?'uncertain':'acknowledged'}});
      throw error;
    }
  }

  /** Transport continuation guard, not Session model authority. A successful
   * mutation response alone cannot enable a dependent Send. */
  async setAgentModel(id: string, config: import('../../../protocol/app-server/v50').SessionModelConfig) {
    const target = this.target(id), generation = this.state.generation;
    if (this.state.views[id].modelMutation) throw new Error('Reread native model state before another mutation.');
    const current = () => this.current(generation) && sameTarget(this.state.views[id]?.target, target);
    const operation = { generation, status: 'in-flight' as const };
    this.setSession(id, { modelMutation: operation });
    try {
      await this.request({ method: 'session/setModel', params: { target, config } }, 'model_set');
      if (!current()) return;
      this.setSession(id, { modelMutation: { generation, status: 'acknowledged' }, ...(this.state.views[id]?.modelIntent?.phase === 'failed' ? { modelIntent: undefined } : {}) });
    } catch (error) {
      if (this.getSnapshot().views[id]?.modelMutation === operation) this.setSession(id, { modelMutation: { generation, status: isOutcomeUncertain(error) ? 'uncertain' : 'acknowledged' } });
      throw error;
    }
  }
  private compactionRequests = new Map<string, CompactionRequestEvidence>();
  /** Claims the local gesture synchronously. Native maintenance owns admission. */
  compact(id: string, consume: () => void = () => {}): boolean {
    const attachment = this.lifecycles.observe(id);
    if (!attachment) throw new RequestNotDispatched('Attachment control authority was revoked.');
    const target = attachment.target, generation = this.state.generation;
    const pending = this.compactionRequests.get(id);
    if (pending?.generation === generation && sameTarget(pending.target, target)) return false;
    const operation: CompactionRequestEvidence = { baselineCount: this.state.views[id]?.snapshot?.context?.compaction_count ?? 0, generation, requestId: crypto.randomUUID(), authorityId: this.state.authorityId, target, status: 'submitting' };
    this.compactionRequests.set(id, operation);
    this.setSession(id, { compactionRequest: operation });
    consume();
    const current = () => this.current(generation) && attachment.current() && sameTarget(this.state.views[id]?.target, target)
      && this.state.views[id]?.compactionRequest?.requestId === operation.requestId;
    const release = () => { if (this.compactionRequests.get(id) === operation) this.compactionRequests.delete(id); };
    void this.request({ method: 'context/compact', params: { target, request_id: operation.requestId } }, 'context_compacted', undefined, current)
      .then(result => {
        release();
        if (!current()) return;
        // The response follows native release. Read failure cannot erase this fact.
        this.setSession(id, { compactionRequest: { ...operation, status: 'succeeded', receipt: result.compaction } });
      }, error => {
        release();
        if (this.state.views[id]?.compactionRequest !== operation) return;
        if (!current() && (this.state.views[id]?.target || this.state.authorityId !== operation.authorityId)) return;
        this.setSession(id, { compactionRequest: { ...operation, status: isOutcomeUncertain(error) ? 'uncertain' : 'failed', diagnostic: String(error) } });
      });
    return true;
  }

  async repairAgentModel(id: string) {
    const view = this.state.views[id], target = view?.target;
    if (!target || !this.lifecycles.observe(id)) return;
    const generation = this.state.generation, operation = view.modelMutation, intent = view.modelIntent;
    if (operation?.status === 'in-flight') return;
    await this.refresh(id);
    const latest = this.state.views[id];
    // Recovery installs a fresh observation proof. Fence this reconciliation
    // by its connection, attachment and exact operation, rather than the proof
    // that the authoritative checkpoint necessarily replaced.
    if (!this.current(generation) || !sameTarget(latest?.target, target)
      || latest.modelMutation !== operation || latest.modelIntent !== intent
      || latest.observationState !== 'following') return;
    const observed = latest.snapshot?.model?.configured;
    const confirmed = intent?.phase === 'failed' && observed?.model === intent.config.model
      && (observed.profile ?? undefined) === (intent.config.profile ?? undefined);
    if (operation || confirmed) this.setSession(id, { modelMutation: undefined, ...(confirmed ? { modelIntent: undefined } : {}) });
  }
  cancellationTarget(id: string): CancellationTarget | undefined {
    const view = this.state.views[id], attempt = view?.snapshot?.attempt;
    // This proof owns the live attachment even when its observation source is
    // unavailable. The coordinator selects the current Attempt at dispatch.
    if (!this.isAttachmentObservationCurrent(id, view?.attachmentObservation) || !this.initialized || this.state.connection !== 'connected' || view?.attachmentIntent !== 'wanted'
      || !view.target || view.deleting || this.cancellationPending(id)) return;
    if (this.observationChains.get(id)?.phase === 'following' && (!attempt || attempt.attempt_id == null || !['admitted','running'].includes(attempt.phase.type)
      || view.snapshot?.shutting_down || view.snapshot?.durability_failure || view.snapshot?.pending_interactions?.length)) return;
    return { generation: this.state.generation, target: view.target, attemptId: attempt?.attempt_id ?? undefined, attachment: view.attachmentObservation };
  }
  private cancellationPending(id: string): boolean {
    const view = this.state.views[id], cancellation = view?.cancellation;
    // An acknowledgement cannot establish that its Attempt is still current
    // while observation is unavailable. A later explicit gesture selects anew.
    return !!cancellation && (cancellation.status !== 'acknowledged'
      || this.observationChains.get(id)?.phase === 'following' && cancellation.attemptId === view.snapshot?.attempt?.attempt_id);
  }
  async cancelTurn(expected: CancellationTarget) {
    const { target, generation, attemptId, attachment } = expected, id = target.session_id;
    const current = () => this.current(generation) && this.initialized && this.state.connection === 'connected'
      && this.lifecycles.observes(id, attachment) && sameTarget(this.state.views[id]?.target, target);
    if (!current() || this.cancellationPending(id)) return;
    const operation = { attemptId, status: 'in-flight' as const };
    this.setSession(id, { cancellation: operation });
    try {
      const accepted = await this.request({ method: 'turn/cancel', params: { target } }, 'cancellation_accepted', undefined,
        () => current() && this.state.views[id]?.cancellation === operation);
      if (current() && this.state.views[id].cancellation === operation) {
        this.setSession(id, { cancellation: { attemptId: accepted.attempt_id, status: 'acknowledged' } });
      }
    } catch (error) {
      // The exact operation owns cleanup even if its unsent transport was retired.
      // Acknowledgement or a successor operation replaces this marker.
      if (this.state.views[id]?.cancellation === operation) {
        this.setSession(id, { cancellation: isOutcomeUncertain(error) ? { attemptId, status: 'uncertain' } : undefined });
      }
      throw error;
    }
  }
  pendingInteractions(id:string) {
    const view=this.state.views[id],loaded=view?.interactionPage;
    if(loaded&&sameTarget(view.target,loaded.target)&&this.interactionCutCurrent(view,loaded.page.read_cut))return loaded.rows;
    return view?.snapshot?.pending_interactions??[];
  }
  interactionBody(id:string,header:import('../../../protocol/app-server/v50').RuntimeClientInteractionHeader) {
    const view=this.state.views[id],detail=view?.interactionDetail;
    if(detail&&sameTarget(view.target,detail.target)&&this.interactionCutCurrent(view,detail.cut)&&interactionKey(detail.body.interaction)===interactionKey(header.interaction))return detail.body;
    return interactionBody(header);
  }
  async loadInteractionBody(id:string,index:number,ownsView:()=>boolean=()=>true) {
    const view=this.state.views[id],observation=this.lifecycles.observe(id),header=this.pendingInteractions(id)[index];
    if(!observation||!header||!view.checkpoint)throw new Error('Interaction observation is unavailable.');
    const viewport=view.interactionPage;
    const selected=viewport&&sameTarget(view.target,viewport.target)&&this.interactionCutCurrent(view,viewport.page.read_cut)?viewport:undefined;
    const cut=selected?.page.read_cut??view.checkpoint.read_cut,target=observation.target;
    const offset=Number(header.source_offset);
    if(!Number.isSafeInteger(offset)||offset<0)throw new Error("Invalid native interaction source offset.");
    const current=()=>ownsView()&&observation.current()&&!!this.state.views[id]&&this.interactionCutCurrent(this.state.views[id],cut)
      &&this.pendingInteractions(id)[index]?.request_id===header.request_id;
    const {page}=await this.request({method:'conversation/checkpointPage',params:{target,agent_id:null,cut,domain:'pending_interactions',offset,limit:1}},'checkpoint_page',undefined,current);
    if(page.items.length!==1)throw new Error('Interaction body is unavailable.');
    const body=await readInteractionBody(page,page.items[0]!,header,this.reads.domainReader(target,current));
    if(current())this.setSession(id,{interactionDetail:{target,cut,body}});
  }
  async interactionValueChunk(id:string,index:number,path:string[],offset:number,ownsView:()=>boolean) {
    const view=this.state.views[id],observation=this.lifecycles.observe(id),header=this.pendingInteractions(id)[index];
    if(!observation||!header||!view.checkpoint)throw new Error('Interaction observation is unavailable.');
    const selected=view.interactionPage&&sameTarget(view.target,view.interactionPage.target)&&this.interactionCutCurrent(view,view.interactionPage.page.read_cut)?view.interactionPage:undefined;
    const cut=selected?.page.read_cut??view.checkpoint.read_cut,item_offset=Number(header.source_offset);
    if(!Number.isSafeInteger(item_offset)||item_offset<0)throw new Error("Invalid native interaction source offset.");
    const current=()=>ownsView()&&observation.current()&&!!this.state.views[id]&&this.interactionCutCurrent(this.state.views[id],cut)&&this.pendingInteractions(id)[index]?.request_id===header.request_id;
    const {chunk}=await this.request({method:'conversation/checkpointValueRead',params:{target:observation.target,agent_id:null,cut,domain:'pending_interactions',item_offset,path,offset,max_bytes:65536}},'checkpoint_value_chunk',undefined,current);
    if(!current()||JSON.stringify(chunk.cut)!==JSON.stringify(cut)||chunk.domain!=='pending_interactions'||chunk.item_offset!==item_offset||JSON.stringify(chunk.path)!==JSON.stringify(path)||chunk.offset!==offset)throw new Error('Interaction range identity changed.');
    if(chunk.bytes_base64.length>87_384 || chunk.bytes_base64.length%4!==0 || chunk.bytes_base64.length/4*3-(chunk.bytes_base64.endsWith('==')?2:chunk.bytes_base64.endsWith('=')?1:0)>65_536)throw new Error('Encoded native range exceeds the decoded byte bound.');
    const bytes=Uint8Array.from(atob(chunk.bytes_base64),value=>value.charCodeAt(0));
    if(bytes.length>65536||!Number.isSafeInteger(chunk.readable_end)||offset+bytes.length>chunk.readable_end||offset<chunk.readable_end&&!bytes.length||(chunk.next_offset??null)!==(offset+bytes.length<chunk.readable_end?offset+bytes.length:null))throw new Error('Interaction range failed to advance exactly.');
    return {bytes,next:chunk.next_offset??undefined,readableEnd:chunk.readable_end};
  }
  private interactionCutCurrent(view:SessionView,cut:import('../../../protocol/app-server/v50').CheckpointReadCut) {
    const current=view.checkpoint?.read_cut;
    return !!current&&current.conversation_id===cut.conversation_id&&current.activation_id===cut.activation_id
      &&(cut.domain_revisions.pending_interactions!==undefined
        ?current.domain_revisions.pending_interactions===cut.domain_revisions.pending_interactions:current.revision===cut.revision);
  }
  async interactionPage(id:string,offset:number) {
    const view=this.state.views[id],observation=this.lifecycles.observe(id),checkpoint=view?.checkpoint;
    if(!observation||!checkpoint||!Number.isSafeInteger(offset)||offset<0)throw new Error('Interaction observation is unavailable.');
    const cut=checkpoint.read_cut,target=observation.target;
    const current=()=>observation.current()&&!!this.state.views[id]&&this.interactionCutCurrent(this.state.views[id],cut);
    const {page}=await this.request({method:'conversation/checkpointPage',params:{target,agent_id:null,cut,domain:'pending_interactions',offset,limit:32}},'checkpoint_page',undefined,current);
    if(page.items.length>32)throw new Error('Interaction page exceeds its item bound.');
    const rows=page.items.map(item=>{if(item.value==null)throw new Error('Interaction descriptor is missing.');return item.value as import('../../../protocol/app-server/v50').RuntimeClientInteractionHeader;});
    if(!current())throw new Error('Interaction observation was replaced.');
    this.setSession(id,{interactionPage:{target,page,rows,offset}});
  }
  async answer(id: string, interaction: InteractionRef, response?: InteractionResponse) {
    const key = interactionKey(interaction);
    if (this.state.interactionOperations[key]) throw new Error('Response already in flight or uncertain. Refresh authoritative state.');
    const target = this.target(id);
    if (!this.pendingInteractions(id).some(item => interactionKey(item.interaction) === key)) throw new Error('Interaction is no longer pending.');
    const generation = this.state.generation;
    this.publish({ interactionOperations: { ...this.state.interactionOperations, [key]: { sessionId: id, status: 'in-flight' } } });
    let acknowledged = false;
    try {
      await this.request(response ? { method: 'interaction/respond', params: { target, interaction, response } }
        : { method: 'interaction/cancel', params: { target, interaction } }, 'interaction_settled');
      if (!this.current(generation) || !sameTarget(this.state.views[id]?.target, target)) return;
      acknowledged = true;
      this.publish({ interactionOperations: { ...this.state.interactionOperations, [key]: { sessionId: id, status: 'acknowledged' } } });
      await this.refresh(id);
    } catch (error) {
      if (this.current(generation) && !(error instanceof OutcomeUncertain) && !acknowledged) {
        const operations = { ...this.state.interactionOperations }; delete operations[key];
        this.publish({ interactionOperations: operations });
        await this.refresh(id).catch(() => {});
      }
      throw error;
    }
  }
  /** Intent events are synchronously admitted and projected by the Session actor. */
  async release(id: string): Promise<void> {
    this.modelPreparations.get(id)?.retire(); this.modelPreparations.delete(id);
    await this.lifecycles.command(id, { kind: 'release', current: () => true });
  }
  switchNode(id: string, nodeId: string, current: () => boolean = () => true): Promise<void> {
    return this.lifecycles.command(id, { kind: 'switch', node: nodeId, current }, true).then(() => {});
  }
  private retireObservationWork(id: string) {
    this.acquiring.delete(id);
    this.reading.retire(id);
    this.traces.retire(id);
    this.refreshes.delete(id); this.dirty.delete(id); this.resubscribe.delete(id);
  }
  private retireAttachmentWork(id: string) {
    this.retireObservationWork(id);
    this.catalog.forgetAttachment(id);
    this.setSession(id,{history:this.initialized?undefined:this.state.views[id]?.history,submissions:undefined,turnOutline:undefined,turnNavigation:undefined});
  }
  /** Resample admission proofs after their browser owner retires. No native
   * transaction is cancelled after dispatch. */
  recheckAdmissions() { queueMicrotask(() => this.pump()); }
  clearError() { this.publish({ error: undefined }); }
  acknowledgeDiagnostic(id: string) {
    // This acknowledges only the notice. Uncertain interaction controls remain
    // disabled until native state resolves them; no RPC is emitted.
    this.publish({ uncertain: this.state.uncertain.filter(item => item.id !== id) });
  }
}

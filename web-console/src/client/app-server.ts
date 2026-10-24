import { AuxiliaryQuestions } from './auxiliary';
import { ObservationChain, validateRootCheckpoint, validateRootEvent } from '../../../protocol/app-server/observation';
import { SessionLifecycles } from './session-lifecycle/system';
import { emptyFacts, type Observation, type LifecycleFacts, type SessionLifecyclePort } from './session-lifecycle/port';
import { lifecycleAvailability, type LifecycleContext } from './session-lifecycle/machine';
import { AgentMeters } from './agent-meters';
import { publicationHeads } from './publication-heads';
import { AttachmentIntakes } from './uploads';
import { checkpointSnapshot, foldRuntimeEvent, observationCheckpoint, transitionFacts } from '../../../protocol/app-server/projection';
import { SourceRepairObserver } from './source-repairs';
import { SessionCatalog } from './session-catalog';
import { SessionReads } from './session-reads';
import { ReadingNavigation } from './reading';
import { ColdReads } from './cold-reads';
import { TraceReads } from './trace-reads';
import { SessionControls } from './session-controls';
import { ModelSelection } from './model-selection';
import { Interactions } from './interactions';
import { SessionUploads } from './session-uploads';
import { ConfigurationObservations } from './configuration-observations';
import { sameTarget } from '../../../protocol/app-server/attachment-identity';
import { SessionExportController } from "./session-export";
import { refreshTrace, replaceTrace, traceInterestBatches, traceInterests } from './trace';
import type {
  TraceToolLocator, PendingInboundRef, AttachmentTarget, GoalMutation, GoalRef, InteractionRef, InteractionResponse, MethodResult, Notification,
  Request1, Response, RuntimeClientSnapshot, RuntimeClientEvent,
  UserInputBlock, UploadReceipt, UploadedFile, SessionModelConfig, RuntimeClientInteractionHeader,
} from '../../../protocol/app-server/v57';
import { transferUpload } from '../../../protocol/app-server/upload';
import { refreshTranscript, replaceTranscript, type TurnSelection } from './transcript';
import { RequestPipeline } from './request-pipeline';
import { RequestAdmissionDeferred, RequestNotDispatched, RpcFailure, isOutcomeUncertain } from './errors';
import { interactionKey, type CancellationTarget, type ClientView, type ConnectionState, type OperationAdmission, type SessionView, type Socket, type SocketFactory, type Submission } from './contracts';


type SessionAdmission = (id: string, current: () => boolean) => Promise<false | OperationAdmission>;
/** One installation lifetime of an attachment-admission owner. Identity is the
 * object, never the callback: reinstalling the same callback is a new owner. */
interface SessionAdmissionOwner { readonly admit: SessionAdmission }
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
  private listeners = new Set<() => void>();
  private readonly pipeline = new RequestPipeline({
    timeoutMs: () => this.timeoutMs,
    socket: () => this.socket,
    initialized: () => this.initialized,
    state: () => this.state,
    current: generation => this.current(generation),
    lose: generation => this.lose(generation),
    claimLifecycleAdmission: admission => this.lifecycleAdmissions.delete(admission),
    observes: (id, admission) => this.lifecycles.observes(id, admission),
    controls: (id, admission) => this.isAttachmentControlCurrent(id, admission),
    inboundChanged: id => this.publishInbound(id),
    inboundAnswered: (id, accepted) => { this.publishInbound(id, accepted); this.settleSubmissions(id); },
    uncertainCommit: operation => this.publish({ uncertain: [...this.state.uncertain, operation] }),
    admissionAvailable: revision => this.agentMeters.admissionAvailable(revision),
  });
  /** Correlation only. No call is ever retried. Every payload is a generated union. */
  request<T extends MethodResult['type']>(operation: Request1, expected: T, acknowledged?: (result: Extract<MethodResult, { type: T }>) => void, dispatchCurrent?: (() => boolean) | OperationAdmission, observationSignal?: AbortSignal): Promise<Extract<MethodResult, { type: T }>> {
    return this.pipeline.request(operation, expected, acknowledged, dispatchCurrent, observationSignal);
  }
  private refreshes = new Map<string, Promise<void>>();
  private dirty = new Set<string>();
  private acquiring = new Set<string>();
  private resubscribe = new Set<string>();
  private observationChains = new Map<string, ObservationChain<RuntimeClientEvent>>();
  private resumeSeeds = new Map<string,{target:AttachmentTarget;snapshot:RuntimeClientSnapshot;checkpoint:import('../../../protocol/app-server/v57').RuntimeClientCheckpoint;cursor:string;bytes:number}>();
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
  constructor(private readonly socketFactory: SocketFactory = (url, protocols) => new WebSocket(url, protocols), private readonly timeoutMs = 30_000, private readonly uploadCarrier = transferUpload) {
    // Navigation retires admission proofs; release their reservations outside the caller's stack.
  }
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  getSnapshot = () => this.state;
  readonly auxiliary = new AuxiliaryQuestions(this);
  hasAuxiliaryAuthority(id: string): boolean {
    return this.initialized && this.state.capabilities?.delivery_access === true && this.state.connection === "connected" && this.lifecycles.observes(id, this.state.views[id]?.attachmentObservation);
  }
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
      ...(changedNode ? { snapshot: undefined, publicationHeads: undefined, cursor: undefined, history: undefined, trace: undefined, preview: undefined, statisticsPreview: undefined, tracePreview: undefined } : {}),
      ...(opening ? { modelIntent: undefined, cancellation: undefined, preview: undefined, statisticsPreview: undefined, tracePreview: undefined } : {}),
      ...(facts.attachmentIntent === 'released' ? { modelIntent: undefined, cancellation: undefined } : {}),
      ...(attached && result ? { snapshot: undefined, publicationHeads: undefined, cursor: undefined, observationState: 'attached' as const, preview: undefined, statisticsPreview: undefined, tracePreview: undefined,
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
        let checkpoint:import('../../../protocol/app-server/v57').RuntimeClientCheckpoint;
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
        this.reconcile(id); this.settleSubmissions(id);
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
    queueMicrotask(() => this.pipeline.pump());
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
    this.publish({ authorityId: undefined, views: {}, sessions: [], nextOffset: undefined, uncertain: [], interactionOperations: {}, detached,
      authorityRevision: (this.state.authorityRevision ?? 0) + 1 });
  }
  // Browser transport authority, not a durable server identity. Reconnect alone
  // can restore wanted Session intent; replacement must retire it after fencing.
  isSameAuthority(endpoint: string) { return !this.state.endpoint || new URL(endpoint).href === this.state.endpoint; }
  async connect(endpoint: string, token: string, transition: 'reconnect' | 'replace-authority' = 'reconnect', committed?: () => void, deliveryToken?: string) {
    const url = new URL(endpoint);
    if (!['ws:', 'wss:'].includes(url.protocol) || url.username || url.password || url.search || url.hash || url.pathname !== '/') {
      throw new Error('Use a ws:// or wss:// endpoint at / with no credentials, query, or fragment.');
    }
    if (!/^[A-Za-z0-9_-]{43,128}$/.test(token)) throw new Error('Enter the dedicated 43–128 character App Server transport token.');
    if (deliveryToken && (!/^[A-Za-z0-9_-]{43,128}$/.test(deliveryToken) || deliveryToken === token)) throw new Error('Enter a separate 43–128 character Delivery Access token.');
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
      const socket = this.socketFactory(url.href, ['rustx.app-server.v57', `rustx-token.${token}`, ...(deliveryToken ? [`rustx-delivery-access.${deliveryToken}`] : [])]);
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
        protocol_version: 57, client: { name: 'rustx-web-console', version: '0.1.0' },
        presentation: { images: true, questionnaires: true, reviews: true },
      } }, 'initialized');
      if (!this.current(generation)) return;
      if (!hello.authority_id || hello.protocol_version !== 57 || !hello.capabilities.multi_session || !hello.capabilities.headless_interactions || !hello.capabilities.single_writable_controller) {
        throw new Error('Incompatible App Server protocol or capabilities. Protocol v57 with native multi-Session, headless interactions, and single-controller admission is required.');
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
    for (const id of this.pipeline.sessions()) sessions.add(id);
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
    if (this.socket || this.pipeline.size) throw new Error('Disconnect before acknowledging current Session evidence.');
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
    uncertain.push(...this.pipeline.disconnect(this.state.generation));
    this.models.disconnect();
    for (const chain of this.observationChains.values()) chain.retire(); this.observationChains.clear();
    this.refreshes.clear(); this.traces.disconnect(); this.reading.disconnect(); this.acquiring.clear(); this.dirty.clear(); this.resubscribe.clear();
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
  private receive(data: unknown, generation: number) {
    if (typeof data !== 'string' || data.length > 1_048_576 || new TextEncoder().encode(data).length > 1_048_576) { this.lose(generation); return; }
    let parsed: unknown;
    try { parsed = JSON.parse(data); } catch {
      this.lose(generation); return;
    }
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
      this.lose(generation); return;
    }
    const value = parsed as Response | Notification;
    if (value.jsonrpc !== '2.0') { this.lose(generation); return; }
    if ('result' in value || 'error' in value || 'id' in value) { this.pipeline.response(value as Response, generation); return; }
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
    if (value.method === 'auxiliary/segment') {
      const { target, registration_id, segment } = value.params;
      const chain = this.observationChains.get(target.session_id);
      if (this.hasAuxiliaryAuthority(target.session_id) && sameTarget(this.state.views[target.session_id]?.target, target) && chain?.phase === 'following' && chain.registrationId === registration_id) this.auxiliary.segment(target, segment);
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
    const trace = traceAdvanced ? refreshTrace(view.trace, snapshot.trace, snapshot.trace_updates, observationCheckpoint(snapshot, cursor)?.read_cut) : undefined;
    this.setSession(id, { snapshot, publicationHeads: publicationHeads(view.publicationHeads, event, snapshot), ...(trace ? { trace: trace.epoch === view.trace?.epoch ? trace : this.traces.supersede(id, trace) } : {}), checkpoint: observationCheckpoint(snapshot, cursor) ?? view.checkpoint, cursor, observationState: 'following',
      history: snapshot.transcript === view.snapshot.transcript ? this.state.views[id].history : refreshTranscript(this.state.views[id].history, snapshot.transcript) });
    const settled=transitionFacts(event).filter(fact=>fact.type==='interaction_settled'||fact.type==='interaction_removed');
    if(settled.length){
      const keys=new Set(settled.map(fact=>interactionKey((fact as Extract<RuntimeClientEvent,{type:'interaction_settled'|'interaction_removed'}>).interaction)));
      this.interactions.settled(keys);
    }
    this.reconcile(id); this.settleSubmissions(id);
    if (snapshot.transcript !== view.snapshot.transcript) void this.reading.refreshTranscriptDomain(id);
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
  loadInitialTrace(id: string) { return this.traces.loadInitialTrace(id); }
  locateToolTrace(id: string, locator: TraceToolLocator) { return this.traces.locateToolTrace(id, locator); }
  returnToLatestTrace(id: string) { return this.traces.returnToLatestTrace(id); }
  selectTrace(id: string, record?: string) { this.traces.selectTrace(id, record); }
  loadTraceDetail(id: string, record: string) { return this.traces.loadTraceDetail(id, record); }
  projections(id: string, after?: string) { return this.traces.projections(id, after); }
  projectionDetail(id: string, checkpointId: string, offset?: number) { return this.traces.projectionDetail(id, checkpointId, offset); }
  projectionDetailReader(detail: import('../../../protocol/app-server/v57').ProjectionDetail) { return this.traces.projectionDetailReader(detail); }
  private readonly controls = new SessionControls({
    request: (...args) => this.request(...args),
    state: () => this.state,
    current: generation => this.current(generation),
    controlReady: () => this.initialized && this.state.connection === 'connected',
    setSession: (id, patch) => this.setSession(id, patch),
    publish: patch => this.publish(patch),
    target: id => this.target(id),
    observe: id => this.lifecycles.observe(id),
    observes: (id, admission) => this.lifecycles.observes(id, admission),
    observesAttachment: (id, admission) => this.isAttachmentObservationCurrent(id, admission),
    observationPhase: id => this.observationChains.get(id)?.phase,
    refresh: id => this.refresh(id),
  });
  goalObjective(id: string, expected: GoalRef) { return this.controls.goalObjective(id, expected); }
  controlGoal(id: string, expected: GoalRef, mutation: GoalMutation) { return this.controls.controlGoal(id, expected, mutation); }
  editInbound(id: string, expected: PendingInboundRef, text: string) { return this.controls.editInbound(id, expected, text); }
  removeInbound(id: string, expected: PendingInboundRef) { return this.controls.removeInbound(id, expected); }
  /** Claims the local gesture synchronously. Native maintenance owns admission. */
  compact(id: string, consume?: () => void) { return this.controls.compact(id, consume); }
  cancellationTarget(id: string) { return this.controls.cancellationTarget(id); }
  cancelTurn(expected: CancellationTarget) { return this.controls.cancelTurn(expected); }
  private readonly models = new ModelSelection({
    request: (...args) => this.request(...args),
    state: () => this.state,
    current: generation => this.current(generation),
    setSession: (id, patch) => this.setSession(id, patch),
    target: id => this.target(id),
    observe: id => this.lifecycles.observe(id),
    epoch: id => this.lifecycles.epoch(id),
    waitForAttachment: (id, current) => this.waitForAttachment(id, current),
    refresh: id => this.refresh(id),
  });
  prepareAgentModel(id: string, config: SessionModelConfig) { return this.models.prepareAgentModel(id, config); }
  setAgentModel(id: string, config: SessionModelConfig) { return this.models.setAgentModel(id, config); }
  repairAgentModel(id: string) { return this.models.repairAgentModel(id); }
  private readonly interactions = new Interactions({
    request: (...args) => this.request(...args),
    state: () => this.state,
    current: generation => this.current(generation),
    setSession: (id, patch) => this.setSession(id, patch),
    publish: patch => this.publish(patch),
    target: id => this.target(id),
    observe: id => this.lifecycles.observe(id),
    domainReader: (...args) => this.reads.domainReader(...args),
    refresh: id => this.refresh(id),
  });
  pendingInteractions(id: string) { return this.interactions.pendingInteractions(id); }
  interactionBody(id: string, header: RuntimeClientInteractionHeader) { return this.interactions.interactionBody(id, header); }
  loadInteractionBody(id: string, index: number, ownsView?: () => boolean) { return this.interactions.loadInteractionBody(id, index, ownsView); }
  interactionValueChunk(id: string, index: number, path: string[], offset: number, ownsView: () => boolean) { return this.interactions.interactionValueChunk(id, index, path, offset, ownsView); }
  interactionPage(id: string, offset: number) { return this.interactions.interactionPage(id, offset); }
  answer(id: string, interaction: InteractionRef, response?: InteractionResponse) { return this.interactions.answer(id, interaction, response); }
  private readonly uploads = new SessionUploads({
    request: (...args) => this.request(...args),
    state: () => this.state,
    current: generation => this.current(generation),
    publish: patch => this.publish(patch),
    target: id => this.target(id),
    controls: (id, admission) => this.isAttachmentControlCurrent(id, admission),
    carrier: (...args) => this.uploadCarrier(...args),
  });
  uploadStatus(id: string, operationId: string) { return this.uploads.uploadStatus(id, operationId); }
  upload(id: string, files: readonly File[], evidence?: { current: () => boolean; acknowledged: (files: UploadedFile[]) => void }, operationId?: string) { return this.uploads.upload(id, files, evidence, operationId); }
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
  loadInitialHistory(id: string) { return this.reading.loadInitialHistory(id); }
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
        this.reconcile(id); this.settleSubmissions(id);
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
        if (current() && !this.dirty.has(id)) {this.lifecycles.event(id, { type: 'OBSERVATION', proof, status: 'attached' });void this.catalog.refreshDisplaySummary(id);void this.reading.refreshTranscriptDomain(id);
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
  /** One authoritative snapshot reconciles every local operation it can settle. */
  private reconcile(id: string) {
    if (!this.lifecycles.observe(id)) return;
    this.controls.reconcileCompaction(id);
    this.models.reconcile(id);
    this.controls.reconcileCancellation(id);
    this.interactions.reconcile(id);
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
    const preparation = this.models.preparation(id);
    if (preparation) await preparation;
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
    const inboundRequests = this.pipeline.inboundRequests(id);
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
  /** Intent events are synchronously admitted and projected by the Session actor. */
  async release(id: string): Promise<void> {
    this.models.release(id);
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
  recheckAdmissions() { queueMicrotask(() => this.pipeline.pump()); }
  clearError() { this.publish({ error: undefined }); }
  acknowledgeDiagnostic(id: string) {
    // This acknowledges only the notice. Uncertain interaction controls remain
    // disabled until native state resolves them; no RPC is emitted.
    this.publish({ uncertain: this.state.uncertain.filter(item => item.id !== id) });
  }
}

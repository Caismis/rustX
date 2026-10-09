import { SessionLifecycles } from './session-lifecycle/system';
import { emptyFacts, type Observation, type LifecycleFacts, type SessionLifecyclePort } from './session-lifecycle/port';
import { lifecycleAvailability, type LifecycleContext } from './session-lifecycle/machine';
import { AgentMeters } from './agent-meters';
import { UploadFailure, AttachmentIntakes } from './uploads';
import { foldRuntimeEvent } from '../../../protocol/app-server/projection';
import { NavigationEpoch } from './navigation';
import { FirstSubmissions } from '../app/new-conversation/first-submit';
import { SessionExportController } from "./session-export";
import { TRACE_LIMIT, TRACE_PAGE_SIZE, beginTraceDetail, completeTraceDetail, prependTrace, refreshTrace, replaceTrace, selectTrace, traceInterests, type TraceCache } from './trace';
import type {
  ConfigurationApplication, PendingInboundRef, PendingMutationOutcome, AttachmentTarget, GoalMutation, GoalRef, InteractionRef, InteractionResponse, MethodResult, Notification,
  Request, Request1, Response, RuntimeClientCursor, RuntimeClientSnapshot,
  SessionPersistentState, SessionSummary, ServerCapabilities, UserInputBlock, UploadReceipt, UploadedFile,
} from '../../../protocol/app-server/v38';
import { transferUpload, uploadOperationId } from '../../../protocol/app-server/upload';
import { HISTORY_PAGE_SIZE, sameReadCut, extendTranscriptWindow, installTranscriptWindow, prependTranscript, refreshTranscript, replaceTranscript, turnKey, type TranscriptCache } from './transcript';
import type { ConversationTurn, ConversationTurnPage } from '../../../protocol/app-server/v38';
import { ProtocolLog, type WireContext } from './protocol-log';

interface OutlineDemand {
  offset?: number;
  paging: OutlinePagingIntent;
  automatic: boolean;
  current: () => boolean;
  work: Promise<ConversationTurnPage | undefined>;
  resolve: (page?: ConversationTurnPage) => void;
}
interface OutlineRead {
  authority: () => boolean;
  active: OutlineDemand;
  pending?: OutlineDemand;
  refresh: boolean;
}

/** Expected observed cancellation identity; never substituted with a successor Attempt. */
export interface CancellationTarget {
  readonly generation: number;
  readonly target: AttachmentTarget;
  readonly attemptId: string;
}
export type InboundControlOutcome =
  | { status: 'known'; outcome: PendingMutationOutcome; observed: boolean }
  | { status: 'uncertain' }
  | { status: 'obsolete' }
  | { status: 'rejected'; reason: string; observed: boolean };

export type ConnectionState = 'disconnected' | 'connecting' | 'connected' | 'reconnecting' | 'resynchronizing' | 'stale' | 'incompatible' | 'error';
export type OutlinePagingIntent = { type: 'latest' } | { type: 'page'; offset: number };
export interface CompactionRequestEvidence {
  /** Presentation ordering only; never resolves an uncertain request. */
  baselineCount: number;
  generation: number;
  requestId: string;
  authorityId?: string;
  target: AttachmentTarget;
  status: 'submitting' | 'uncertain' | 'succeeded' | 'failed';
  diagnostic?: string;
}
export interface SessionView extends Omit<LifecycleFacts, 'error' | 'attachmentIntentRevision'> {
  /** Readonly actor projection; absent on catalog-only presentation rows. */
  readonly attachmentIntentRevision?: number;
  readonly lifecycle?: ReturnType<typeof lifecycleAvailability>;
  compactionRequest?: CompactionRequestEvidence;
  id: string;
  /** Last native catalog row, retained when the Sidebar reads a different page. */
  summary?: SessionSummary;
  snapshot?: RuntimeClientSnapshot;
  trace?: TraceCache;
  cursor?: RuntimeClientCursor;
  history?: TranscriptCache;
  /** Read-only durable history, never an execution snapshot or control claim. */
  preview?: { conversationId: string; history: TranscriptCache };
  statisticsPreview?: { conversationId: string; statistics: import('../../../protocol/app-server/v38').ConversationStatistics; occupancy?: import('../../../protocol/app-server/v38').ContextOccupancy | null };
  tracePreview?: { conversationId: string; cache: TraceCache };
  turnOutline?: { paging: OutlinePagingIntent; page?: ConversationTurnPage; loading?: boolean; error?: string };
  turnNavigation?: { intent: number; pending?: string; error?: string };
  settings?: SessionPersistentState;
  /** Exact acknowledged MessageIds awaiting projection reconciliation, not queue authority. */
  submissions?: readonly Submission[];
  /** Current-generation turn/start or turn/steer requests awaiting an outcome.
   * Transport ownership only, including unsent requests in the bounded pipeline. */
  inboundRequests?: number;
  modelIntent?: { config: import('../../../protocol/app-server/v38').SessionModelConfig; phase: 'waiting' | 'applying' | 'failed'; error?: string };
  modelMutation?: { generation: number; status: 'in-flight' | 'acknowledged' | 'uncertain' };
  cancellation?: { attemptId: string; status: 'in-flight' | 'acknowledged' | 'uncertain' };
  error?: string;
}
/** Exists only after `inbound_accepted` names the server MessageId, which is its
 * sole identity; settles when an authoritative snapshot contains that MessageId. */
export interface Submission {
  messageId: string;
  content: readonly UserInputBlock[];
}
/** Settled local outcome of one CAS-bound native Goal control. A known outcome is
 * not projection convergence: `observed` reports whether an authoritative snapshot
 * read after the outcome succeeded. `uncertain` means the response was lost;
 * `obsolete` means the connection or attachment changed and nothing may apply. */
export type GoalControlOutcome =
  | { status: 'applied'; observed: boolean }
  | { status: 'rejected'; reason: string; observed: boolean }
  | { status: 'uncertain' }
  | { status: 'obsolete' };
export interface UncertainOperation {
  uploadOperationId?: string;
  compactionRequestId?: string;
  id: string;
  method: Request1['method'];
  sessionId?: string;
  interactionKey?: string;
  generation: number;
}
export interface ClientView {
  configuration?: Readonly<Record<string, ConfigurationApplication>>;
  authorityRevision?: number;
  /** Native AppServerHost identity; unlike generation, stable across reconnect. */
  authorityId?: string;
  detached?: readonly DetachedEvidence[];
  endpoint?: string;
  connection: ConnectionState;
  generation: number;
  capabilities?: ServerCapabilities;
  error?: string;
  sessions: readonly SessionSummary[];
  nextOffset?: number | null;
  views: Readonly<Record<string, SessionView>>;
  uncertain: readonly UncertainOperation[];
  interactionOperations: Readonly<Record<string, { sessionId: string; status: 'in-flight' | 'uncertain' | 'acknowledged' }>>;
}
/** Read-only historical evidence. Never consulted by attachment/control admission. */
export interface DetachedEvidence {
  authority: string;
  authorityId?: string;
  operations: readonly UncertainOperation[];
  sessions: readonly (Pick<SessionView, 'id' | 'error' | 'modelMutation' | 'cancellation'> & { deletion?: 'uncertain' | SessionView['deletionRecovery'] })[];
}
export interface Socket {
  onopen: ((event: Event) => unknown) | null;
  onmessage: ((event: MessageEvent) => unknown) | null;
  onclose: ((event: CloseEvent) => unknown) | null;
  onerror: ((event: Event) => unknown) | null;
  send(data: string): void;
  close(): void;
}
export type SocketFactory = (url: string, protocols: string[]) => Socket;
/** An operation-owned proof, re-observed after transport backpressure and checked
 * synchronously again before send. The transport never interprets Host policy.
 * `validate` must stop its reads when `signal` aborts; the transport never waits for it. */
export interface OperationAdmission {
  /** Transport evidence only; called exactly when native dispatch starts. */
  sent?: () => void;
  current: () => boolean;
  validate: (signal: AbortSignal) => Promise<boolean>;
}
type AttachmentAdmission = (id: string, current: () => boolean) => Promise<false | OperationAdmission>;
/** One installation lifetime of an attachment-admission owner. Identity is the
 * object, never the callback: reinstalling the same callback is a new owner. */
interface AttachmentAdmissionOwner { readonly admit: AttachmentAdmission }
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
/** The request owner proves that transport dispatch never began. */
export class RequestNotDispatched extends Error {
  constructor(cause: unknown) {
    super(cause instanceof Error ? cause.message : String(cause), { cause });
  }
}
/** Capacity refusal carries the exact readiness cut at which nothing was sent. */
class RequestAdmissionDeferred extends RequestNotDispatched {
  constructor(readonly revision: number) { super('Client request capacity reached.'); }
}
export class OutcomeUncertain extends Error {
  constructor() { super('Response lost after transmission. Outcome uncertain; the request was not replayed. Reconnect and inspect authoritative state.'); }
}
export function isOutcomeUncertain(error: unknown): boolean {
  return error instanceof OutcomeUncertain || (error instanceof RpcFailure && error.error.data?.kind === "committed_durability_uncertain");
}
export class RpcFailure extends Error {
  constructor(readonly error: Extract<Response, { error: unknown }>['error']) {
    const data = error.data;
    let message = `${error.message} (${error.code})${data ? `: ${JSON.stringify(data)}` : ''}`;
    switch (data?.kind) {
      case 'agent_not_delivered':
        message = `Agent ${data.agent_id} input was not delivered`;
        break;
      case 'agent_delivery_unknown':
        message = `Agent ${data.agent_id} input acceptance was not acknowledged; delivery is unknown, do not replay automatically`;
        break;
      case 'job_publication_abandoned':
        message = `Job ${data.job_id} terminal publication was abandoned; no durable terminal result is available`;
        break;
      case 'agent_settlement':
        message = `Agent ${data.agent_id} is unavailable; physical settlement, publication, or workspace authority remains unresolved`;
        break;
      case 'archive_preparation_failed':
        message = error.message;
    }
    super(message);
  }
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
const READS = new Set<Request1['method']>([
  'session/turns', 'session/uploadStatus',
  'artifact/read', 'initialize', 'server/info', 'session/list', 'session/read', 'session/summary', 'session/tree', 'session/deletePreview',
  'session/history', 'session/statistics', 'session/traceHistory', 'session/traceHistoryDetail', 'session/configuration', 'session/snapshot', 'session/transcript', 'session/trace', 'session/traceDetail', 'session/settings', 'session/model', 'session/models',
  'configuration/sourcesRead', 'session/effectiveConfiguration', 'resources/read', 'job/status', 'job/list', 'job/wait', 'agent/status', 'agent/list', 'agent/wait', 'agent/transcript', 'agent/statistics', 'session/boundaries',
]);
/** Domain settlement has no RPC response deadline. Separate bounded lanes keep
 * observation/admission from occupying the slots needed to stop or inspect work. */
function requestLane(method: Request1['method']): 'wait' | 'admission' | 'control' | 'rpc' {
  switch (method) {
    // Compaction awaits native summary generation and release, just like other
    // long-lived domain waits. It must not expire the shared socket's RPC clock.
    case 'context/compact': case 'agent/wait': case 'job/wait': return 'wait';
    case 'agent/sendMessage': return 'admission';
    case 'agent/interrupt': case 'job/cancel': case 'turn/cancel': return 'control';
    default: return 'rpc';
  }
}
const DOMAIN_CAPACITY = { wait: 4, admission: 2, control: 2 } as const;
const RPC_CAPACITY = 8;
/** Operation-admission validations reserve ordinary RPC slots so the final check
 * stays adjacent to send, but at most two at once: a stalled Product Host read
 * can never take the remaining slots needed to cancel or control native work. */
const VALIDATION_CAPACITY = 2;

const dispatchCurrent = ({ dispatchCurrent: proof }: Pending) => !proof || (typeof proof === 'function' ? proof() : proof.current());
export const interactionKey = (ref: InteractionRef) => JSON.stringify([ref.conversation_id, ref.interaction_id]);
export const sameTarget = (a?: AttachmentTarget, b?: AttachmentTarget) => !!a && !!b &&
  a.session_id === b.session_id && a.conversation_id === b.conversation_id &&
  a.runtime_incarnation === b.runtime_incarnation && a.attachment_id === b.attachment_id;

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
  readonly navigation = new NavigationEpoch();
  readonly firstSubmissions = new FirstSubmissions();
  /** Final client lifetime ends only explicitly, never on a component unmount. */
  dispose() { this.attachmentIntakes.dispose(); this.firstSubmissions.dispose(); return this.disconnect(); }
  private socket?: Socket;
  private initialized = false;
  private nextId = 0;
  private listeners = new Set<() => void>();
  private pending = new Map<string, Pending>();
  private refreshes = new Map<string, Promise<void>>();
  private dirty = new Set<string>();
  private acquiring = new Set<string>();
  private resubscribe = new Set<string>();
  private readonly lifecycles = new SessionLifecycles({
    transport: () => ({ generation: this.state.generation, connected: this.initialized }),
    port: id => this.lifecyclePort(id),
    project: (id, current, previous) => this.projectLifecycle(id, current, previous),
  });
  private readingIntents = new Map<string, number>();
  private readingAuthorities = new Map<string, number>();
  private outlineReads = new Map<string, OutlineRead>();
  // Native metadata invalidation (Issue #386). One monotonic clock orders every
  // metadata observation this client makes: `summaryReadSequence` is ticked at
  // the *start* of each catalog list and each exact summary read — its causal
  // start cut — and again for each `session/summaryInvalidated` notification.
  // `summaryInvalidations` holds the ticket of the latest invalidation observed
  // for a Session, so an observation discharges it only when it started later.
  // `summarySettled` records that a causally later read completed, so it is
  // evidence about a read, never a permanent conclusion drawn from canonical
  // history. A newer invalidation always outranks an older settlement.
  private summaryInvalidations = new Map<string, number>();
  private summarySettled = new Set<string>();
  private summaryObservedEpoch = new Map<string, number>();
  private summaryInFlight = new Map<string, { generation: number; invalidations: number; work: Promise<void> }>();
  private summaryReadSequence = 0;
  // Start cuts of the metadata observations still outstanding on this connection.
  private summaryObservations = new Set<number>();
  // Minimum acceptable metadata sequence: last observation or a committed rename read floor.
  private summaryReads = new Map<string, number>();
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
    this.navigation.subscribe(() => queueMicrotask(() => this.pump()));
  }
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  getSnapshot = () => this.state;
  isAttachmentObservationCurrent(id: string, admission: SessionView['attachmentObservation']): boolean {
    return this.lifecycles.observes(id, admission);
  }
  isAttachmentControlCurrent(id: string, admission: SessionView['attachmentObservation']): boolean {
    return this.lifecycles.controls(id, admission);
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
    const changedNode = previous && facts.nodeId !== previous.facts.nodeId;
    const opening = facts.attachment === 'attaching' && previous?.epoch !== context.epoch;
    const revoked = !!previous?.facts.attachmentObservation && facts.attachmentObservation !== previous.facts.attachmentObservation;
    if (revoked) this.retireObservationWork(id);
    const retiredClaim = !!previous?.facts.target && !facts.target;
    const attached = context.attachmentResult && context.attachmentResult !== previous?.attachmentResult && facts.attachmentObservation;
    const result = context.attachmentResult;
    const projection: SessionView = { ...view, ...facts, lifecycle: lifecycleAvailability(context),
      ...(revoked ? {
        history: view.history && { ...view.history, loading: false },
        trace: view.trace && { ...view.trace, loading: false, details: Object.fromEntries(Object.entries(view.trace.details).filter(([, detail]) => !detail.loading)) },
        turnOutline: view.turnOutline && { ...view.turnOutline, loading: false },
        turnNavigation: view.turnNavigation && { intent: view.turnNavigation.intent },
      } : {}),
      ...(changedNode ? { snapshot: undefined, cursor: undefined, history: undefined, trace: undefined, preview: undefined, statisticsPreview: undefined, tracePreview: undefined } : {}),
      ...(opening ? { modelIntent: undefined, preview: undefined, statisticsPreview: undefined, tracePreview: undefined } : {}),
      ...(facts.attachmentIntent === 'released' ? { modelIntent: undefined } : {}),
      ...(attached && result ? { snapshot: result.snapshot, cursor: result.cursor, preview: undefined, statisticsPreview: undefined, tracePreview: undefined,
        history: replaceTranscript(result.snapshot.transcript, view.history), trace: this.supersedeTrace(id, replaceTrace(result.snapshot.trace, view.trace)), turnOutline: undefined, turnNavigation: undefined } : {}),
    };
    if (context.deleted) {
      this.retireAttachmentWork(id);
      const views = { ...this.state.views }; delete views[id];
      this.publish({ views, sessions: this.state.sessions.filter(row => row.id !== id) });
      this.deletionListeners.forEach(listener => listener(id));
      return;
    }
    this.publish({ views: { ...this.state.views, [id]: projection } });
    if (previous?.epoch !== context.epoch) this.summarySettled.delete(id);
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
        let offset: number | null | undefined = 0;
        while (offset != null) {
          const result: Extract<MethodResult, { type: 'tree' }> = await this.request({ method: 'session/tree', params: { session_id: id, offset, limit: 32 } }, 'tree', undefined, current);
          const found = result.nodes.find(row => row.id === node); if (found) return found.conversation_id;
          if (result.next_offset != null && result.next_offset <= offset) throw new Error('Invalid Session tree page.');
          offset = result.next_offset;
        }
        throw new Error('Open Node is absent from the native Session tree.');
      },
      admit: current => this.admitAttachment(id, current),
      attach: (node_id, admission) => native({ method: 'session/attach', params: { session_id: id, node_id } }, 'attached', admission),
      detach: async (target, admission) => { await native({ method: 'session/detach', params: { target } }, 'detached', admission); },
      switchNode: async (target, node_id, admission) => (await native({ method: 'session/switchNode', params: { target, node_id } }, 'session', admission)).session,
      delete: async (expected_target_revision, admission) => (await native({ method: 'session/delete', params: { session_id: id, expected_target_revision } }, 'deletion', admission)).result,
      recover: async admission => (await native({ method: 'session/recoverDeletion', params: { session_id: id } }, 'deletion', admission)).result,
      inspectDeletion: async current => (await this.request({ method: 'session/deletePreview', params: { session_id: id } }, 'deletion', undefined, current)).result,
      observeAttached: async (result, current) => {
        if (!current()) return;
        if (result.configuration) this.publish({ configuration: { ...this.state.configuration, [id]: result.configuration } });
        this.reconcileInteractions(id); this.settleSubmissions(id);
        const settings = await this.request({ method: 'session/settings', params: { session_id: id } }, 'settings', undefined, current);
        if (!current()) return;
        this.setSession(id, { settings: settings.settings });
        void this.refreshDisplaySummary(id).catch(() => {});
        if (this.dirty.delete(id)) {
          if (this.resubscribe.has(id)) await this.refresh(id);
          else {
            try { await this.request({ method: 'session/subscribe', params: { target: result.target, after_cursor: result.cursor } }, 'subscribed', undefined, current); }
            catch (error) {
              if (!current()) return;
              if (!(error instanceof RpcFailure) || error.error.data?.kind !== 'resync_required') throw error;
              this.resubscribe.add(id); await this.refresh(id);
            }
          }
        }
      },
      refresh: () => this.refresh(id),
      cold: current => { void this.readHistoryPreview(id, current); void this.readColdMetadata(id, current); void this.readTracePreview(id, current); },
      classify: error => error instanceof RequestNotDispatched ? 'unsent' : isOutcomeUncertain(error) ? 'uncertain'
        : error instanceof RpcFailure ? error.error.data?.kind === 'stale_attachment' ? 'stale-route' : 'refused' : 'local',
    };
  }
  // Product policy is injected by the Web owner, not interpreted by this transport.
  // Fail closed when there is no admission owner (including after its disposal).
  private attachmentAdmission?: AttachmentAdmissionOwner;
  setAttachmentAdmission(admit: AttachmentAdmission) {
    // Every installation is a new incarnation, so retired proofs stay retired
    // even when the same callback is installed again.
    const owner: AttachmentAdmissionOwner = { admit };
    this.replaceAttachmentAdmission(owner);
    // Cleanup removes only its own incarnation; a stale one never disturbs a newer owner or its proofs.
    return () => { if (this.attachmentAdmission === owner) this.replaceAttachmentAdmission(undefined); };
  }
  private replaceAttachmentAdmission(owner?: AttachmentAdmissionOwner) {
    this.attachmentAdmission = owner;
    // Owner change retires its proofs; release their reservations outside the caller's stack, as navigation does.
    queueMicrotask(() => this.pump());
  }
  async admitAttachment(id: string, current: () => boolean = () => true): Promise<false | OperationAdmission> {
    const generation = this.state.generation;
    const owner = this.attachmentAdmission;
    const valid = () => current() && this.current(generation) && owner === this.attachmentAdmission;
    if (!valid()) return false;
    if (!owner) throw new Error('No Web attachment admission owner.');
    const allowed = await owner.admit(id, valid);
    return allowed && valid() && { current: () => allowed.current() && valid(), validate: allowed.validate };
  }
  restoreViews(ids: readonly string[]) {
    for (const id of ids.slice(0, 32)) if (!this.state.views[id]) this.lifecycles.restore(id);
  }
  private retireAuthority() {
    this.navigation.invalidate();
    this.attachmentIntakes.retireAll();
    this.firstSubmissions.retireAuthority();
    const sessions = Object.values(this.state.views).filter(view => view.deleting || view.error || view.modelMutation || view.cancellation)
      .map(({ id, error, modelMutation, cancellation, deleting, deletionCommitted }) => ({ id, error, modelMutation, cancellation,
        ...(deleting ? { deletion: deletionCommitted ?? 'uncertain' as const } : {}) }));
    // Admission reserved capacity for close-time evidence before synchronous fencing.
    // Generation-guarded continuations cannot create new Session diagnostics;
    // model/cancellation continuations only update already-reserved Session rows.
    const detached = [...(this.state.detached ?? [])];
    if (this.state.uncertain.length || sessions.length) detached.push({ authority: this.state.endpoint!, authorityId: this.state.authorityId, operations: this.state.uncertain, sessions });
    this.lifecycles.replaceAuthority(); this.readingAuthorities.clear(); for (const id of this.outlineReads.keys()) this.retireOutline(id); this.readingIntents.clear(); this.summarySettled.clear(); this.summaryInvalidations.clear(); this.summaryObservedEpoch.clear(); this.summaryInFlight.clear(); this.summaryReads.clear(); this.summaryObservations.clear();
    this.listEpoch++; this.listOffset = 0; this.listQuery = '';
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
    this.publish({ configuration: {}, endpoint: url.href, connection: transition === 'reconnect' ? 'reconnecting' : 'connecting', capabilities: undefined, error: undefined });
    // Ownership commits after close/retirement, before attempting the new transport.
    committed?.();
    try {
      const socket = this.socketFactory(url.href, ['rustx.app-server.v38', `rustx-token.${token}`]);
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
        protocol_version: 38, client: { name: 'rustx-web-console', version: '0.1.0' },
        presentation: { images: true, questionnaires: true, reviews: true },
      } }, 'initialized');
      if (!this.current(generation)) return;
      if (!hello.authority_id || hello.protocol_version !== 38 || !hello.capabilities.multi_session || !hello.capabilities.headless_interactions || !hello.capabilities.single_writable_controller) {
        throw new Error('Incompatible App Server protocol or capabilities. Protocol v38 with native multi-Session, headless interactions, and single-controller admission is required.');
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
      this.lifecycles.transport(generation, true);
      this.publish({ capabilities: hello.capabilities, connection: 'resynchronizing' });
      await this.listSessions();
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
    this.socket = undefined;
    this.initialized = false;
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
    this.refreshes.clear(); this.traceReads.clear(); this.traceAuthorities.clear(); this.acquiring.clear(); this.dirty.clear(); this.resubscribe.clear();
    const operations = { ...this.state.interactionOperations };
    for (const [key, operation] of Object.entries(operations)) if (operation.status === 'in-flight') delete operations[key];
    for (const item of uncertain) if (item.interactionKey) operations[item.interactionKey] = { sessionId: item.sessionId!, status: 'uncertain' };
    this.lifecycles.transport(this.state.generation + 1, false);
    this.publish({ connection, generation: this.state.generation + 1, uncertain, interactionOperations: operations,
      views: Object.fromEntries(Object.entries(this.state.views).map(([id, view]) => [id, {
        ...view, modelIntent: undefined, history: undefined, turnOutline: undefined, turnNavigation: undefined, submissions: undefined, inboundRequests: undefined,
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
  async request<T extends MethodResult['type']>(operation: Request1, expected: T, acknowledged?: (result: Extract<MethodResult, { type: T }>) => void, dispatchCurrent?: (() => boolean) | OperationAdmission): Promise<Extract<MethodResult, { type: T }>> {
    if (!this.socket || (!this.initialized && operation.method !== 'initialize')) throw new RequestNotDispatched('Connect and initialize first.');
    if (['session/attach', 'session/detach', 'session/switchNode', 'session/delete', 'session/recoverDeletion'].includes(operation.method)
      && (typeof dispatchCurrent !== 'object' || !this.lifecycleAdmissions.delete(dispatchCurrent))) throw new RequestNotDispatched('Session lifecycle operations require actor admission.');
    // The Session actor owns control admission. Retained targets authorize only
    // lifecycle settlement; live reads and new effects require the exact committed
    // attachment proof, captured once and checked again adjacent to socket send.
    let authority: (() => boolean) | undefined;
    if ('target' in operation.params && 'session_id' in operation.params.target
      && !READS.has(operation.method) && !(operation.method === 'goal/control' && operation.params.control.action === 'show')
      && operation.method !== 'session/detach' && operation.method !== 'session/switchNode' && operation.method !== 'session/subscribe') {
      const target = operation.params.target, id = target.session_id;
      const admission = this.state.views[id]?.attachmentObservation;
      const current = () => this.isAttachmentControlCurrent(id, admission) && sameTarget(admission?.target, target);
      if (!current()) throw new RequestNotDispatched('Attachment control authority was revoked.');
      authority = current;
    }
    if (operation.method.startsWith('artifact/') && [...this.pending.values()].filter(item => item.request.method.startsWith('artifact/')).length >= 2) throw new RequestNotDispatched('Artifact transfer capacity reached. Retry after current transfers finish.');
    const lane = requestLane(operation.method);
    if (lane !== 'rpc' && [...this.pending.values()].filter(item => requestLane(item.request.method) === lane).length >= DOMAIN_CAPACITY[lane]) {
      throw new RequestNotDispatched(`Client ${lane} capacity reached. Inspect current operations before issuing another.`);
    }
    if (this.pending.size >= 64) throw new RequestAdmissionDeferred(this.admissionRevision);
    // Keep uncertain diagnostics finite without silently forgetting unresolved mutations.
    if (!READS.has(operation.method) && this.state.uncertain.length + this.pending.size >= 64) throw new RequestNotDispatched('Uncertain-operation capacity reached. Inspect and acknowledge diagnostics first.');
    const generation = this.state.generation;
    const id = `${generation}:${++this.nextId}`;
    const request: Request = { jsonrpc: '2.0', id, ...operation };
    try {
      if (new TextEncoder().encode(JSON.stringify(request)).length > 1_048_576) throw new Error('Request exceeds the App Server 1 MiB limit.');
    } catch (cause) { throw new RequestNotDispatched(cause); }
    const result = await new Promise<MethodResult>((resolve, reject) => {
      const params = operation.params;
      const context = { method: operation.method,
        sessionId: 'target' in params && 'session_id' in params.target ? params.target.session_id : 'session_id' in params ? params.session_id : undefined };
      this.pending.set(id, { request, context, mutation: !READS.has(operation.method), sent: false, expected, resolve, reject, dispatchCurrent, authority, acknowledged: result => acknowledged?.(result as Extract<MethodResult, { type: T }>) });
      if (operation.method === 'turn/start' || operation.method === 'turn/steer') this.publishInbound(operation.params.target.session_id);
      this.pump();
    });
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
  private pump() {
    // An obsolete proof releases its reservation now, never after its Host read.
    for (const pending of this.pending.values()) if (pending.validation && !this.dispatchAllowed(pending)) this.refuse(pending);
    let occupied = [...this.pending.values()].filter(p => (p.sent || p.validation) && requestLane(p.request.method) === 'rpc').length;
    let validating = [...this.pending.values()].filter(p => p.validation).length;
    for (const pending of this.pending.values()) {
      if (!this.socket) break;
      const lane = requestLane(pending.request.method);
      if (pending.sent || pending.validation || (lane === 'rpc' && occupied >= RPC_CAPACITY)) continue;
      if (!this.dispatchAllowed(pending)) { this.refuse(pending); continue; }
      const proof = typeof pending.dispatchCurrent === 'object' ? pending.dispatchCurrent : undefined;
      // A waiting validation holds nothing, so later requests are never blocked behind it.
      if (proof && validating >= VALIDATION_CAPACITY) continue;
      if (lane === 'rpc') occupied++;
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
    // No external observer runs between this final proof and send invocation.
    // sent means transmission attempted, never native acceptance.
    pending.sent = true;
    this.log.observe('out', generation, raw, pending.context);
    if (requestLane(pending.request.method) === 'rpc') pending.timer = setTimeout(() => this.lose(generation), this.timeoutMs);
    try { socket.send(raw); } catch { this.lose(generation); }
    try { if (typeof pending.dispatchCurrent === 'object') pending.dispatchCurrent.sent?.(); }
    catch (error) { console.error('App Server dispatch observer failed', error); }
  }
  private receive(data: unknown, generation: number) {
    if (typeof data !== 'string') { this.lose(generation); return; }
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
    if (value.method === 'configuration/changed') {
      const application = value.params.application;
      const previous = this.state.configuration?.[application.scope];
      if (!previous || BigInt(application.version) > BigInt(previous.version)) {
        this.publish({ configuration: { ...this.state.configuration, [application.scope]: application } });
      }
      return;
    }
    if (value.method === 'session/ownershipRetired') return; // Product Host owns execution retirement.
    if (value.method === 'session/summaryInvalidated') {
      // Addressed by Session identity alone: no attachment target, so a branch
      // view — or a row this client only lists — converges without attaching a
      // runtime, and Session A can never update Session B.
      const sessionId = value.params?.session_id;
      if (typeof sessionId !== 'string' || typeof value.params.catalog_changed !== 'boolean') { this.lose(generation); return; }
      this.invalidateSummary(sessionId, generation);
      if (value.params.catalog_changed) this.invalidateCatalog(generation);
      return;
    }
    if (!['session/event', 'session/resyncRequired', 'session/closed'].includes(value.method) || !value.params?.target) {
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
      if (value.method === 'session/resyncRequired') {
        this.lifecycles.event(target.session_id, { type: 'OBSERVATION', proof, status: 'resynchronizing' });
        if (!observation.current()) return;
        this.readingAuthorities.set(target.session_id, (this.readingAuthorities.get(target.session_id) ?? 0) + 1);
        this.retireOutline(target.session_id);
        this.invalidateReading(target.session_id);
        if (!observation.current()) return;
        this.setSession(target.session_id, { turnOutline: undefined });
        if (!observation.current()) return;
        this.resubscribe.add(target.session_id);
        if (this.acquiring.has(target.session_id)) return;
        void this.refresh(target.session_id).catch(() => {});
        return;
      }
      if (this.acquiring.has(target.session_id)) { this.resubscribe.add(target.session_id); return; } // Server replay owns overlap.
      // Native registration may publish replay before its RPC ACK. After the
      // acquired cut N, an exhausted old registration cannot emit N+1: only
      // the replacement registration can advance that immutable cursor stream.
      // Consume its contiguous replay while controls remain resynchronizing.
      const replaying = view.attachment === 'resynchronizing' && this.refreshes.has(target.session_id) && !this.resubscribe.has(target.session_id);
      if (!view.snapshot || view.cursor === undefined || (view.attachment !== 'attached' && !replaying)) return;
      const cursor = BigInt(value.params.cursor), previous = BigInt(view.cursor);
      if (cursor <= previous) return;
      if (cursor !== previous + 1n) {
        this.resubscribe.add(target.session_id);
        void this.refresh(target.session_id).catch(() => {});
        return;
      }
      if (value.params.event.type === 'pending_inbound_changed') {
        this.invalidateReading(target.session_id);
        if (!observation.current()) return;
        this.retireOutline(target.session_id);
        const error = 'History changed. Reload the Turn outline to navigate again.';
        this.setSession(target.session_id, { turnOutline: { paging: view.turnOutline?.paging ?? { type: 'latest' }, error },
          ...(view.history?.window ? { history: { ...replaceTranscript(view.snapshot.transcript, view.history), error } } : {}) });
      }
      if (!observation.current()) return;
      const snapshot = foldRuntimeEvent(view.snapshot, value.params.event);
      this.setSession(target.session_id, { snapshot, cursor: value.params.cursor,
        history: snapshot.transcript === view.snapshot.transcript ? this.state.views[target.session_id].history : refreshTranscript(this.state.views[target.session_id].history, snapshot.transcript) });
      this.reconcileInteractions(target.session_id); this.settleSubmissions(target.session_id);
      if (value.params.event.type === 'trace_changed') {
        void this.refreshTraceDomain(target.session_id).catch(() => {});
      }
    }
  }
  private listEpoch = 0;
  private listOffset = 0;
  private listQuery = '';
  async listSessions(offset = this.listOffset, query = this.listQuery, current: () => boolean = () => true) {
    const epoch = ++this.listEpoch;
    // The causal start cut of this page: the server captured its rows no earlier.
    const summaryRead = ++this.summaryReadSequence;
    this.summaryObservations.add(summaryRead);
    this.listOffset = offset; this.listQuery = query;
    const generation = this.state.generation;
    try {
      const result = await this.request({ method: 'session/list', params: { offset, limit: 32, query } }, 'sessions');
      if (this.current(generation) && epoch === this.listEpoch && current()) {
        const views = { ...this.state.views };
        const stale = new Set<string>();
        const sessions = result.sessions.map(row => {
          const fence = this.summaryReads.get(row.id) ?? 0;
          const cached = views[row.id]?.summary ?? this.state.sessions.find(item => item.id === row.id);
          const summary = fence > summaryRead && cached ? cached : row;
          // Accepting a row never discharges an invalidation observed after this
          // request's start cut — including for a Session this page introduces,
          // which no exact read could have been issued for while it was uncached.
          if (summary === row) {
            if (fence > summaryRead) stale.add(row.id); else this.summaryReads.set(row.id, summaryRead);
          }
          if ((this.summaryInvalidations.get(row.id) ?? 0) > (this.summaryReads.get(row.id) ?? 0)) stale.add(row.id);
          if (views[row.id]) views[row.id] = { ...views[row.id], summary };
          return summary;
        });
        // Retain ordering fences only for metadata actually cached by this client.
        for (const id of this.summaryReads.keys()) if (!views[id] && !sessions.some(row => row.id === id)) this.summaryReads.delete(id);
        this.publish({ sessions, nextOffset: result.next_offset, views });
        // Exact metadata repair only: never page membership, ordering, or query.
        for (const id of stale) void this.readSessionSummary(id).catch(() => {});
      }
    } finally {
      this.summaryObservations.delete(summaryRead);
      this.retireInvalidations();
    }
  }
  /** Invalidation evidence stays bounded by cached state and by the observations
   * that could still reintroduce stale metadata. It is retired for a Session
   * this client does not cache once no older observation remains outstanding. */
  private retireInvalidations() {
    let oldest = Number.POSITIVE_INFINITY;
    for (const start of this.summaryObservations) oldest = Math.min(oldest, start);
    for (const [id, at] of this.summaryInvalidations) {
      if (at >= oldest || this.summaryInFlight.has(id)) continue;
      if (!this.state.views[id] && !this.state.sessions.some(row => row.id === id)) this.summaryInvalidations.delete(id);
    }
  }
  /** Cached values remain renderable across transport loss. Each attachment
   * establishes fresh exact metadata independently of its first-message check.
   * A legitimate null projection settles here and is never polled; the native
   * `session/summaryInvalidated` notification is what reopens it. */
  private async refreshDisplaySummary(id: string) {
    const generation = this.state.generation, epoch = this.lifecycles.epoch(id);
    const current = () => this.current(generation) && this.lifecycles.epoch(id) === epoch
      && this.state.views[id]?.attachmentIntent === 'wanted';
    if (!current()) return;
    const existing = this.summaryInFlight.get(id);
    if (existing?.generation === generation) {
      await existing.work.catch(() => {});
      if (!current()) return;
    }
    const summary = this.state.sessions.find(row => row.id === id) ?? this.state.views[id]?.summary;
    if (this.summaryObservedEpoch.get(id) === epoch && summary
      && (summary.name || summary.preview || this.summarySettled.has(id) || !this.hasCanonicalUser(id))) return;
    await this.readSessionSummary(id).catch(() => {});
  }
  private hasCanonicalUser(id: string) {
    return !!this.state.views[id]?.snapshot?.transcript.entries?.some(entry => entry.item.type === 'message' && entry.item.message.role === 'user');
  }
  /** Exact native metadata observation, never a catalog search or membership change.
   * Coalesce within a connection and within one invalidation epoch. Canonical
   * user history is not by itself proof that the native display projection has
   * been published: publication is a separate, later catalog commit, so a read
   * settles the first-message check only when it was also causally after every
   * invalidation observed for this Session. */
  readSessionSummary(id: string): Promise<void> {
    const invalidations = this.summaryInvalidations.get(id) ?? 0;
    const existing = this.summaryInFlight.get(id);
    // Coalesce only reads that are causally equivalent. A read begun before an
    // invalidation can neither satisfy nor clear that newer invalidation, so it
    // never stands in for the causally later read it requires.
    if (existing?.generation === this.state.generation && existing.invalidations === invalidations) return existing.work;
    const generation = this.state.generation, summaryRead = ++this.summaryReadSequence;
    this.summaryObservations.add(summaryRead);
    const epoch = this.lifecycles.epoch(id), canonicalUser = this.hasCanonicalUser(id);
    const work = (async () => {
      const { summary } = await this.request({ method: 'session/summary', params: { session_id: id } }, 'session_summary');
      if (!this.current(generation) || this.lifecycles.epoch(id) !== epoch) throw new Error('Obsolete Session summary read.');
      if (summary.id !== id) throw new Error('Mismatched Session summary identity.');
      if ((this.summaryReads.get(id) ?? 0) <= summaryRead) {
        this.summaryReads.set(id, summaryRead);
        this.publish({ sessions: this.state.sessions.map(row => row.id === id ? summary : row),
          views: this.state.views[id] ? { ...this.state.views, [id]: { ...this.state.views[id], summary } } : this.state.views });
        if (epoch !== undefined && this.state.views[id]?.attachmentIntent === 'wanted') {
          this.summaryObservedEpoch.set(id, epoch);
          // Settle only when this read was causally after every invalidation
          // observed so far. One that arrived mid-flight keeps the Session
          // dirty and is answered by the read `invalidateSummary` started.
          if (canonicalUser && (this.summaryInvalidations.get(id) ?? 0) === invalidations) this.summarySettled.add(id);
        }
      }
    })();
    const read = { generation, invalidations, work };
    this.summaryInFlight.set(id, read);
    void work.finally(() => {
      if (this.summaryInFlight.get(id) === read) this.summaryInFlight.delete(id);
      this.summaryObservations.delete(summaryRead);
      this.retireInvalidations();
    }).catch(() => {});
    return work;
  }
  private catalogRefresh?: { generation: number; dirty: boolean; work: Promise<void> };
  /** Coalesced native membership invalidation. Never awaited by attach or send.
   * A failure settles this read; another invalidation/reconnect may reread. */
  private invalidateCatalog(generation: number) {
    const existing = this.catalogRefresh;
    if (existing?.generation === generation) { existing.dirty = true; return; }
    const refresh = { generation, dirty: true, work: Promise.resolve() };
    this.catalogRefresh = refresh;
    refresh.work = (async () => {
      while (refresh.dirty && this.current(generation)) {
        refresh.dirty = false;
        await this.listSessions();
      }
    })();
    void refresh.work.catch(() => {}).finally(() => { if (this.catalogRefresh === refresh) this.catalogRefresh = undefined; });
  }
  /** Native post-commit Session metadata invalidation (Issue #386).
   * Authoritative rereading, not a value: it overrides any earlier cached-null
   * check, and a read already in flight cannot answer it. A cached Session is
   * reread now; an uncached one is reread only if a list request older than this
   * invalidation later introduces its row, so an invalidation is never a poll. */
  private invalidateSummary(id: string, generation: number) {
    if (!this.current(generation)) return;
    this.summaryInvalidations.set(id, ++this.summaryReadSequence);
    this.summarySettled.delete(id);
    // Evidence is retained even for a Session this client does not cache: an
    // older list request still in flight can introduce that row, and accepting
    // it must not lose this invalidation.
    if (!this.state.views[id] && !this.state.sessions.some(row => row.id === id)) { this.retireInvalidations(); return; }
    void this.readSessionSummary(id).catch(() => {});
  }
  /** A committed rename requires a read started after its acknowledgement.
   * Ordinary observers still coalesce; the old read cannot publish past this floor. */
  async renameSession(id: string, name: string): Promise<void> {
    const generation = this.state.generation;
    await this.request({ method: 'session/name', params: { session_id: id, name } }, 'session');
    if (!this.current(generation)) return;
    await this.readFreshSessionSummary(id);
  }
  private readFreshSessionSummary(id: string): Promise<void> {
    this.summaryReads.set(id, ++this.summaryReadSequence);
    this.summaryObservedEpoch.delete(id);
    this.summaryInFlight.delete(id);
    return this.readSessionSummary(id);
  }
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
    await this.listSessions(); return result;
  }
  async recoverSessionDeletion(id: string) {
    const result = await this.lifecycles.command(id, { kind: 'recover', current: () => true });
    await this.listSessions(); return result;
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
  /** One observation scope for all cold durable reads. The Conversation is
   * native tree evidence, never inferred from whichever response arrives first. */
  private coldReadScope(id: string, admitted: () => boolean = () => true) {
    const view = this.state.views[id];
    if (!view?.nodeId || !view.nodeConversationId) return;
    const node = view.nodeId, conversation = view.nodeConversationId;
    const generation = this.state.generation, epoch = this.lifecycles.epoch(id), revision = view.attachmentIntentRevision ?? 0;
    const current = () => admitted() && this.current(generation) && this.lifecycles.epoch(id) === epoch
      && this.state.views[id]?.attachment === 'attaching' && this.state.views[id]?.attachmentIntent === 'wanted'
      && (this.state.views[id]?.attachmentIntentRevision ?? 0) === revision
      && this.state.views[id]?.nodeId === node && this.state.views[id]?.nodeConversationId === conversation;
    return { node, conversation, current };
  }
  private async readHistoryPreview(id: string, admitted: () => boolean) {
    const scope = this.coldReadScope(id, admitted); if (!scope) return;
    const { node, conversation, current } = scope;
    try {
      const result = await this.request({ method: 'session/history', params: { session_id: id, node_id: node, at: { type: 'latest' }, limit: HISTORY_PAGE_SIZE } }, 'session_history', undefined, current);
      if (current() && result.conversation_id === conversation && result.window.cut.conversation_id === conversation) this.setSession(id, { preview: { conversationId: conversation, history: replaceTranscript(result.window.page) } });
    } catch { /* Attachment recovery owns errors; a late read cannot replace the live view. */ }
  }
  private async readColdMetadata(id: string, admitted: () => boolean) {
    const scope = this.coldReadScope(id, admitted); if (!scope) return;
    const { node, conversation, current } = scope;
    await Promise.allSettled([
      this.request({ method: 'session/statistics', params: { session_id: id, node_id: node } }, 'session_statistics', undefined, current)
        .then(result => { if (current() && result.conversation_id === conversation) this.setSession(id, { statisticsPreview: { conversationId: result.conversation_id, statistics: result.statistics, occupancy: result.occupancy } }); }),
      this.request({ method: 'session/settings', params: { session_id: id } }, 'settings', undefined, current)
        .then(result => { if (current()) this.setSession(id, { settings: result.settings }); }),
    ]);
  }
  /** Read-only trace is independent of slow runtime/resource initialization. */
  private async readTracePreview(id: string, admitted: () => boolean, older = false) {
    const view = this.state.views[id], previous = older ? view?.tracePreview : undefined;
    if (older && (!previous || previous.cache.loading || previous.cache.page.next_cursor == null)) return;
    const scope = this.coldReadScope(id, admitted); if (!scope) return;
    const { node, conversation, current } = scope;
    if (previous && previous.conversationId !== conversation) return;
    const cache = previous?.cache ?? replaceTrace({ records: [], next_cursor: null });
    const limit = Math.min(TRACE_PAGE_SIZE, TRACE_LIMIT - cache.page.records.length);
    if (limit < 1 || !current()) return;
    if (previous) this.setSession(id, { tracePreview: { ...previous, cache: { ...cache, loading: true, error: undefined } } });
    try {
      const admission = older ? await this.admitAttachment(id, current) : current;
      if (!admission) return;
      const result = await this.request({ method: 'session/traceHistory', params: { session_id: id, node_id: node, before: older ? cache.page.next_cursor : null, limit } }, 'session_trace_history', undefined, admission);
      if (!current()) return;
      if (result.conversation_id !== conversation) throw new Error('Trace conversation changed.');
      this.setSession(id, { tracePreview: { conversationId: result.conversation_id, cache: older ? prependTrace(this.state.views[id].tracePreview!.cache, result.page) : replaceTrace(result.page) } });
    } catch (error) {
      if (current()) this.setSession(id, { tracePreview: { conversationId: previous?.conversationId ?? '', cache: { ...(this.state.views[id]?.tracePreview?.cache ?? cache), loading: false, error: String(error) } } });
    }
  }
  private async readTracePreviewDetail(id: string, record: string) {
    const preview = this.state.views[id]?.tracePreview;
    if (!preview || preview.cache.details[record]?.loading || preview.cache.details[record]?.detail) return;
    const scope = this.coldReadScope(id); if (!scope || preview.conversationId !== scope.conversation) return;
    const node = scope.node;
    const pending = beginTraceDetail(preview.cache, record);
    const current = () => scope.current() && this.state.views[id]?.tracePreview?.conversationId === scope.conversation
      && this.state.views[id]?.tracePreview?.cache.details[record] === pending.details[record];
    this.setSession(id, { tracePreview: { ...preview, cache: pending } });
    try {
      const admission = await this.admitAttachment(id, current);
      if (!admission) return;
      const result = await this.request({ method: 'session/traceHistoryDetail', params: { session_id: id, node_id: node, record_id: record } }, 'session_trace_history_detail', undefined, admission);
      if (!current()) return;
      if (result.conversation_id !== preview.conversationId) throw new Error('Trace conversation changed.');
      this.setSession(id, { tracePreview: { ...preview, cache: completeTraceDetail(this.state.views[id].tracePreview!.cache, record, pending.epoch, result.detail ?? undefined) } });
    } catch (error) {
      if (current()) this.setSession(id, { tracePreview: { ...preview, cache: completeTraceDetail(this.state.views[id].tracePreview!.cache, record, pending.epoch, undefined, String(error)) } });
    }
  }
  async loadEarlierPreview(id: string): Promise<void> {
    const view = this.state.views[id], preview = view?.preview, before = preview?.history.page.next_cursor;
    if (!preview || preview.history.loading || before == null || view.attachment !== 'attaching') return;
    const scope = this.coldReadScope(id); if (!scope || preview.conversationId !== scope.conversation) return;
    const epoch = preview.history.epoch, node = scope.node;
    const current = () => scope.current() && this.state.views[id]?.preview?.conversationId === scope.conversation
      && this.state.views[id]?.preview?.history.epoch === epoch;
    this.setSession(id, { preview: { ...preview, history: { ...preview.history, loading: true, error: undefined } } });
    try {
      const admission = await this.admitAttachment(id, current);
      if (!admission) return;
      const result = await this.request({ method: 'session/history', params: { session_id: id, node_id: node, at: { type: 'older', before }, limit: HISTORY_PAGE_SIZE } }, 'session_history', undefined, admission);
      if (!current()) return;
      if (result.conversation_id !== preview.conversationId || result.window.cut.conversation_id !== preview.conversationId || result.window.page.next_cursor != null && BigInt(result.window.page.next_cursor) >= BigInt(before)) throw new Error('Invalid history page.');
      this.setSession(id, { preview: { ...preview, history: prependTranscript(preview.history, result.window.page) } });
    } catch (error) {
      if (current()) this.setSession(id, { preview: { ...preview, history: { ...preview.history, error: String(error) } } });
    } finally {
      const latest = this.state.views[id]?.preview;
      if (current() && latest) this.setSession(id, { preview: { ...latest, history: { ...latest.history, loading: false } } });
    }
  }
  /** Explicit reconciliation/recovery only. Native bounded replay owns overlap. */
  refresh(id: string): Promise<void> {
    const observation = this.lifecycles.observe(id);
    if (!observation) return Promise.resolve();
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
          this.retireOutline(id);
          this.lifecycles.event(id, { type: 'OBSERVATION', proof, status: 'resynchronizing' });
          if (!current()) return;
          this.setSession(id, {
            trace: this.supersedeTrace(id, replaceTrace({ records: [], next_cursor: null }, this.state.views[id]?.trace)),
            history: replaceTranscript({ entries: [] }, this.state.views[id]?.history), turnOutline: undefined, turnNavigation: undefined,
          });
        }
        if (!current()) return;
        this.acquiring.add(id);
        const result = await this.request({ method: 'session/snapshot', params: { target, trace_records: traceInterests(this.state.views[id]?.trace) } }, 'snapshot', undefined, current);
        if (!current()) return;
        if (result.snapshot.conversation_id !== target.conversation_id) throw new Error('Mismatched snapshot conversation.');
        if (BigInt(result.cursor) >= BigInt(this.state.views[id].cursor ?? '0')) {
          this.setSession(id, { snapshot: result.snapshot, preview: undefined, statisticsPreview: undefined, tracePreview: undefined, cursor: result.cursor, history: refreshTranscript(this.state.views[id]?.history, result.snapshot.transcript), trace: this.supersedeTrace(id, refreshTrace(this.state.views[id]?.trace, result.snapshot.trace, result.snapshot.trace_updates)), error: undefined });
          this.reconcileInteractions(id); this.settleSubmissions(id);
          await this.refreshDisplaySummary(id);
          if (!current()) return;
        }
        this.acquiring.delete(id);
        const replay = this.resubscribe.delete(id);
        if (resync || replay) {
          try { await this.request({ method: 'session/subscribe', params: { target, after_cursor: result.cursor } }, 'subscribed', undefined, current); }
          catch (error) {
            if (current() && error instanceof RpcFailure && error.error.data?.kind === 'resync_required' && ++replayRepairs < 3) {
              this.resubscribe.add(id); this.dirty.add(id);
            } else throw error;
          }
        }
        if (current() && !this.dirty.has(id)) this.lifecycles.event(id, { type: 'OBSERVATION', proof, status: 'attached' });
      }
    } catch (error) {
      if (current()) { this.acquiring.delete(id); this.resubscribe.add(id); this.lifecycles.event(id, { type: 'OBSERVATION', proof, status: 'stale', error: String(error) }); }
      throw error;
    }
  }
  // Trace authority is independent of Runtime Client cursors and cache interval
  // epochs (an overlapping snapshot can preserve the latter).
  private traceAuthorities = new Map<string, number>();
  private supersedeTrace(id: string, trace: TraceCache): TraceCache {
    this.traceAuthorities.set(id, (this.traceAuthorities.get(id) ?? 0) + 1);
    // Old paging/detail requests may no longer complete. Release their loading
    // markers with the new authority so explicit reads remain available.
    return { ...trace, loading: false, details: Object.fromEntries(
      Object.entries(trace.details).filter(([, entry]) => !entry.loading),
    ) };
  }
  private traceReads = new Map<string, { dirty: boolean; work: Promise<void> }>();
  private refreshTraceDomain(id: string): Promise<void> {
    const existing = this.traceReads.get(id);
    if (existing) { existing.dirty = true; return existing.work; }
    const observation = this.lifecycles.observe(id);
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
          let result;
          try {
            result = await this.request({ method: 'session/trace', params: { target, before: null, limit: TRACE_PAGE_SIZE, records: traceInterests(this.state.views[id]?.trace) } }, 'trace', undefined, current);
          } catch (error) {
            // Failure cannot create work or erase work already owed. Callers
            // share the final owed read's outcome, not an earlier failure.
            if (!read.dirty || !current()) throw error;
            continue;
          }
          if (current() && this.traceAuthorities.get(id) === authority) this.setSession(id, { trace: refreshTrace(this.state.views[id]?.trace, result.page, result.page.updates ?? []) });
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
  /** One older page, as Harness's loadOlder. */
  loadEarlier(id: string) { return this.pageOlder(id); }
  async loadEarlierTrace(id: string) {
    if (this.state.views[id]?.attachment === 'attaching') return this.readTracePreview(id, () => true, true);
    const observation = this.lifecycles.observe(id);
    if (!observation) return;
    const { target } = observation;
    const cache = this.state.views[id].trace;
    const authority = this.traceAuthorities.get(id);
    if (!cache || cache.loading || cache.page.next_cursor == null) return;
    const limit = Math.min(TRACE_PAGE_SIZE, TRACE_LIMIT - cache.page.records.length);
    if (limit < 1) throw new Error('Trace window is full. Return to latest first.');
    const current = () => observation.current()
      && this.traceAuthorities.get(id) === authority
      && this.state.views[id]?.trace?.epoch === cache.epoch;
    this.setSession(id, { trace: { ...cache, loading: true, error: undefined } });
    try {
      const result = await this.request({ method: 'session/trace', params: { target, before: cache.page.next_cursor, limit } }, 'trace', undefined, current);
      if (current()) {
        this.setSession(id, { trace: prependTrace(this.state.views[id].trace!, result.page) });
        // Include newly loaded identities in a repair even if their terminal
        // notification raced this pending historical read.
        await this.refreshTraceDomain(id);
      }
    } catch (error) {
      if (current()) this.setSession(id, { trace: { ...this.state.views[id].trace!, loading: false, error: String(error) } });
      throw error;
    }
  }
  selectTrace(id: string, record?: string) {
    const preview = this.state.views[id]?.tracePreview;
    if (this.state.views[id]?.attachment === 'attaching' && preview) { this.setSession(id, { tracePreview: { ...preview, cache: selectTrace(preview.cache, record) } }); return; }
    const trace = this.state.views[id]?.trace;
    if (trace) this.setSession(id, { trace: selectTrace(trace, record) });
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
    if (this.state.views[id]?.attachment === 'attaching') return this.readTracePreviewDetail(id, record);
    const observation = this.lifecycles.observe(id);
    if (!observation) return;
    const { target } = observation;
    const cache = this.state.views[id]?.trace;
    if (!cache) return;
    const epoch = cache.epoch;
    const authority = this.traceAuthorities.get(id);
    const existing = cache.details[record];
    if (existing && (existing.loading || existing.detail)) return;
    const pending = beginTraceDetail(cache, record);
    const current = () => observation.current()
      && this.traceAuthorities.get(id) === authority
      && this.state.views[id]?.trace?.epoch === epoch
      && this.state.views[id]?.trace?.details[record] === pending.details[record];
    this.setSession(id, { trace: pending });
    try {
      const result = await this.request({ method: 'session/traceDetail', params: { target, record_id: record } }, 'trace_detail', undefined, current);
      if (!current()) return;
      this.setSession(id, { trace: completeTraceDetail(this.state.views[id].trace!, record, epoch, result.detail ?? undefined) });
    } catch (error) {
      if (current()) this.setSession(id, { trace: completeTraceDetail(this.state.views[id].trace!, record, epoch, undefined, String(error)) });
    }
  }
  /** A new navigation or an explicit reload retires the previous jump's landing. */
  invalidateReading(id: string) {
    const intent = (this.readingIntents.get(id) ?? 0) + 1;
    this.readingIntents.set(id, intent);
    const history = this.state.views[id]?.history;
    if (this.state.views[id]) this.setSession(id, { turnNavigation: { intent }, ...(history?.loading ? { history: { ...history, loading: false } } : {}) });
    return intent;
  }
  private readingAuthority(id: string) {
    const observation = this.lifecycles.observe(id);
    const authority = this.readingAuthorities.get(id), epoch = this.lifecycles.epoch(id);
    return () => !!observation?.current() && this.readingAuthorities.get(id) === authority && this.lifecycles.epoch(id) === epoch;
  }
  private retireOutline(id: string) {
    const read = this.outlineReads.get(id);
    this.outlineReads.delete(id);
    read?.active.resolve(); read?.pending?.resolve();
  }
  /** Automatic refresh preserves explicit paging; one later read discharges growth. */
  refreshTurns(id: string) {
    const read = this.outlineReads.get(id);
    if (read?.authority()) { read.refresh = true; return Promise.resolve(undefined); }
    const paging = this.state.views[id]?.turnOutline?.paging;
    return this.demandOutline(id, paging?.type === 'page' ? paging.offset : undefined, paging ?? { type: 'latest' }, () => true, true);
  }
  /** One in-flight RPC plus one replaceable latest demand. A skipped demand never
   * masquerades as a completed page. Equivalent demands share the native read. */
  readTurns(id: string, offset?: number, userCurrent: () => boolean = () => true): Promise<ConversationTurnPage | undefined> {
    return this.demandOutline(id, offset, offset === undefined ? { type: 'latest' } : { type: 'page', offset }, userCurrent);
  }
  private demandOutline(id: string, offset: number | undefined, paging: OutlinePagingIntent, userCurrent: () => boolean, automatic = false): Promise<ConversationTurnPage | undefined> {
    const view = this.state.views[id];
    if (!view || !userCurrent() || !this.lifecycles.observe(id)) return Promise.resolve(undefined);
    let read = this.outlineReads.get(id);
    if (read && !read.authority()) { this.retireOutline(id); read = undefined; }
    if (read) {
      const equivalent = !read.active.automatic && read.active.offset === offset ? read.active : read.pending?.offset === offset ? read.pending : undefined;
      if (equivalent) {
        if (equivalent === read.active) { read.pending?.resolve(); read.pending = undefined; }
        equivalent.current = userCurrent; equivalent.paging = paging;
        this.setSession(id, { turnOutline: { paging: { type: 'latest' }, ...view.turnOutline, loading: true, error: undefined } });
        return equivalent.work;
      }
    }
    let resolve!: OutlineDemand['resolve'];
    const demand: OutlineDemand = { offset, paging, automatic, current: userCurrent, work: new Promise(done => { resolve = done; }), resolve: page => resolve(page) };
    const start = !read;
    if (read) { read.pending?.resolve(); read.pending = demand; }
    else {
      read = { authority: this.readingAuthority(id), active: demand, refresh: false };
      this.outlineReads.set(id, read);
    }
    this.setSession(id, { turnOutline: { paging: { type: 'latest' }, ...view.turnOutline, loading: true, error: undefined } });
    if (start) void this.runOutline(id, read);
    return demand.work;
  }
  private async runOutline(id: string, read: OutlineRead) {
    const demand = read.active;
    const owned = () => this.outlineReads.get(id) === read && read.authority();
    const current = () => owned() && !read.pending?.current() && demand.current() && owned();
    let page: ConversationTurnPage | undefined;
    try {
      if (!current()) return;
      const target = this.lifecycles.observe(id)!.target;
      const result = await this.request({ method: 'session/turns', params: { target, offset: demand.offset ?? null, limit: HISTORY_PAGE_SIZE } }, 'conversation_turns', undefined, owned);
      if (current()) {
        if (result.page.cut.conversation_id !== target.conversation_id || result.page.turns.length > HISTORY_PAGE_SIZE) throw new Error('Invalid native turn outline.');
        page = result.page;
        this.setSession(id, { turnOutline: { paging: demand.paging, page } });
      }
    } catch (error) {
      if (current()) this.setSession(id, { turnOutline: { paging: { type: 'latest' }, ...this.state.views[id]?.turnOutline, loading: false, error: String(error) } });
    } finally {
      // A superseded automatic read still owes a refresh of the surviving presentation.
      if (owned() && demand.automatic && !current()) read.refresh = true;
      demand.resolve(page);
      if (this.outlineReads.get(id) === read) {
        const pending = read.pending;
        if (pending?.current() && owned()) {
          read.active = pending; read.pending = undefined;
          void this.runOutline(id, read);
        } else {
          this.outlineReads.delete(id); pending?.resolve();
          if (read.authority()) {
            const outline = this.state.views[id]?.turnOutline;
            if (outline?.loading) this.setSession(id, { turnOutline: { ...outline, loading: false } });
            if (read.refresh) void this.refreshTurns(id);
          }
        }
      }
    }
  }
  /** Native resolves the exact Turn at its outline cut in one bounded read. */
  async navigateTurn(id: string, selection: ConversationTurn | number, userCurrent: () => boolean = () => true) {
    const view = this.state.views[id];
    if (!view?.history || !userCurrent() || !this.lifecycles.observe(id)) return false;
    const authority = this.readingAuthority(id), intent = this.invalidateReading(id);
    const current = () => userCurrent() && authority() && this.readingIntents.get(id) === intent;
    let turn: ConversationTurn, cut = view.turnOutline?.page?.cut;
    if (typeof selection === 'number') {
      this.setSession(id, { turnNavigation: { intent, pending: `ordinal:${selection}` } });
      const total = view.turnOutline?.page?.total ?? 0;
      const offset = Math.floor((selection - 1) / HISTORY_PAGE_SIZE) * HISTORY_PAGE_SIZE;
      // Identity lookup is fixed even when presentation should continue following latest.
      const paging: OutlinePagingIntent = offset >= Math.floor((total - 1) / HISTORY_PAGE_SIZE) * HISTORY_PAGE_SIZE ? { type: 'latest' } : { type: 'page', offset };
      const page = await this.demandOutline(id, offset, paging, current);
      if (!current()) return false;
      if (!page) { this.setSession(id, { turnNavigation: { intent } }); return false; }
      const found = page.turns.find(row => row.ordinal === selection);
      if (!found) { this.setSession(id, { turnNavigation: { intent, error: 'This Turn is no longer in the native outline.' } }); return false; }
      turn = found; cut = page.cut;
    } else turn = selection;
    const cursor = turn.cursor, key = turnKey(turn.id);
    if (!current()) return false;
    if (cursor == null) { this.setSession(id, { turnNavigation: { intent } }); return false; }
    const anchored = () => !!this.state.views[id]?.history?.page.entries?.some(entry => entry.cursor === cursor
      && (entry.turn_process && turnKey(entry.turn_process) === key || entry.item.type === 'attempt_terminal' && turnKey(entry.item.turn) === key));
    if (anchored()) { this.setSession(id, { turnNavigation: { intent } }); return turn; }
    if (!cut) { this.setSession(id, { turnNavigation: { intent, error: 'Reload the native Turn outline before navigating.' } }); return false; }
    this.setSession(id, { turnNavigation: { intent, pending: key } });
    try {
      const result = await this.request({ method: 'session/transcript', params: { target: view.target!, at: { type: 'turn', id: turn.id, cut }, limit: HISTORY_PAGE_SIZE } }, 'transcript_window', undefined, current);
      if (!current()) return false;
      if (!sameReadCut(result.window.cut, cut) || result.window.cut.conversation_id !== view.target!.conversation_id || result.window.target_cursor !== cursor || !result.window.target || turnKey(result.window.target) !== key) throw new Error('Invalid native Turn window.');
      const history = installTranscriptWindow(result.window, this.state.views[id]?.history);
      if (!(history.page.entries ?? []).some(entry => entry.cursor === cursor)) throw new Error('This turn is outside the readable native history.');
      this.setSession(id, { history, turnNavigation: { intent } });
      return anchored() && current() ? turn : false;
    } catch (error) {
      if (current()) this.setSession(id, { turnNavigation: { intent, error: String(error) } });
      return false;
    }
  }
  returnToLatest(id: string) {
    if (!this.lifecycles.observe(id)) return;
    this.invalidateReading(id);
    const view = this.state.views[id];
    if (view?.snapshot) this.setSession(id, { history: replaceTranscript(view.snapshot.transcript, view.history) });
  }
  async loadLater(id: string) {
    const history = this.state.views[id]?.history, window = history?.window;
    if (!window?.newer_cursor || history?.loading) return;
    await this.readHistoryPage(id, { type: 'newer', after: window.newer_cursor, cut: window.cut });
  }
  private pageOlder(id: string): Promise<void> {
    const history = this.state.views[id]?.history;
    if (!history || history.page.next_cursor == null || history.loading) return Promise.resolve();
    return this.readHistoryPage(id, { type: 'older', before: history.page.next_cursor, cut: history.window?.cut ?? null });
  }
  /** One gesture, one read. Navigation intent fences both page and anchor reads. */
  private async readHistoryPage(id: string, at: import('../../../protocol/app-server/v38').ConversationWindowAt) {
    const history = this.state.views[id]?.history;
    const observation = this.lifecycles.observe(id);
    if (!history || !observation) return;
    const target = observation.target, authority = this.readingAuthority(id), intent = this.invalidateReading(id);
    const current = () => authority() && this.readingIntents.get(id) === intent && this.state.views[id]?.history?.epoch === history.epoch;
    this.setSession(id, { history: { ...history, loading: true, error: undefined } });
    try {
      const result = await this.request({ method: 'session/transcript', params: { target, at, limit: HISTORY_PAGE_SIZE } }, 'transcript_window', undefined, current);
      if (!current()) return;
      if (result.window.cut.conversation_id !== target.conversation_id || ('cut' in at && at.cut && !sameReadCut(result.window.cut, at.cut))) throw new Error('Invalid native history cut.');
      // Extend only the finite presentation window, trimming its opposite end.
      if (at.type === 'older' && result.window.page.next_cursor != null && BigInt(result.window.page.next_cursor) >= BigInt(at.before)) throw new Error('Invalid native history page.');
      const next = extendTranscriptWindow(result.window, this.state.views[id].history!, at.type === 'older');
      this.setSession(id, { history: next });
    } catch (error) {
      if (current()) this.setSession(id, { history: { ...this.state.views[id].history!, loading: false, error: String(error) } });
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
    if (cancellation && (snapshot.attempt?.attempt_id !== cancellation.attemptId || snapshot.attempt.phase.type === 'settled')) this.setSession(id, { cancellation: undefined });
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

  async send(id: string, text: string, receipts: readonly UploadReceipt[] = [], delivery: 'send' | 'steer' = 'send', acknowledged?: () => void, dispatchCurrent?: () => boolean) {
    if (receipts.some(receipt => receipt.session_id !== id)) throw new Error('Invalid Session upload receipts.');
    const content: UserInputBlock[] = [
      ...receipts.map(receipt => ({ type: 'upload' as const, ...receipt })),
      ...(text ? [{ type: 'text' as const, text }] : []),
    ];
    return this.sendContent(id, content, delivery, acknowledged, dispatchCurrent);
  }
  async sendContent(id: string, content: UserInputBlock[], delivery: 'send' | 'steer' = 'send', acknowledged?: () => void, dispatchCurrent?: () => boolean) {
    const generation = this.state.generation;
    const preparation = this.modelPreparations.get(id);
    if (preparation) await preparation.work;
    if (!this.current(generation)) throw new Error('Connection changed before sending.');
    if (this.state.views[id]?.modelIntent) throw new Error(this.state.views[id].modelIntent?.error ?? 'Model selection is not ready.');
    const target = this.target(id);
    if (this.state.views[id].modelMutation) throw new Error('Reread native model state before sending.');
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
  async controlGoal(id: string, expected: GoalRef, mutation: GoalMutation): Promise<GoalControlOutcome> {
    const generation = this.state.generation;
    let target: AttachmentTarget;
    try { target = this.target(id); } catch (error) { return { status: 'rejected', reason: error instanceof Error ? error.message : String(error), observed: false }; }
    const current = () => this.current(generation) && sameTarget(this.state.views[id]?.target, target);
    try {
      await this.request({ method: 'goal/control', params: { target, control: { action: 'mutate', expected, mutation } } }, 'goal');
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
  private modelPreparations = new Map<string, { work: Promise<import('../../../protocol/app-server/v38').SessionModelConfig>; retire: () => void }>();
  /** Client-owned selection while native resources initialize. Last unsent choice
   * wins; the existing native mutation and authoritative reread still own apply. */
  prepareAgentModel(id: string, config: import('../../../protocol/app-server/v38').SessionModelConfig) {
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
        if (observed?.model !== selection.model || (observed.reasoningProfile ?? undefined) !== (selection.reasoningProfile ?? undefined))
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
  /** Transport continuation guard, not Session model authority. A successful
   * mutation response alone cannot enable a dependent Send. */
  async setAgentModel(id: string, config: import('../../../protocol/app-server/v38').SessionModelConfig) {
    const target = this.target(id), generation = this.state.generation;
    if (this.state.views[id].modelMutation) throw new Error('Reread native model state before another mutation.');
    const current = () => this.current(generation) && sameTarget(this.state.views[id]?.target, target);
    const operation = { generation, status: 'in-flight' as const };
    this.setSession(id, { modelMutation: operation });
    try {
      await this.request({ method: 'session/setModel', params: { target, config } }, 'model');
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
    const target = this.target(id), generation = this.state.generation;
    const pending = this.compactionRequests.get(id);
    if (pending?.generation === generation && sameTarget(pending.target, target)) return false;
    const operation: CompactionRequestEvidence = { baselineCount: this.state.views[id]?.snapshot?.context?.compaction_count ?? 0, generation, requestId: crypto.randomUUID(), authorityId: this.state.authorityId, target, status: 'submitting' };
    this.compactionRequests.set(id, operation);
    this.setSession(id, { compactionRequest: operation });
    consume();
    const current = () => this.current(generation) && sameTarget(this.state.views[id]?.target, target)
      && this.state.views[id]?.compactionRequest?.requestId === operation.requestId;
    const release = () => { if (this.compactionRequests.get(id) === operation) this.compactionRequests.delete(id); };
    void this.request({ method: 'context/compact', params: { target, request_id: operation.requestId } }, 'context', undefined, current)
      .then(async () => {
        release();
        if (!current()) return;
        // The response follows native release. Read failure cannot erase this fact.
        this.setSession(id, { compactionRequest: { ...operation, status: 'succeeded' } });
        await this.refresh(id).catch(() => {});
      }, error => {
        release();
        if (this.state.views[id]?.compactionRequest !== operation) return;
        if (!current() && (this.state.views[id]?.target || this.state.authorityId !== operation.authorityId)) return;
        this.setSession(id, { compactionRequest: { ...operation, status: isOutcomeUncertain(error) ? 'uncertain' : 'failed', diagnostic: String(error) } });
      });
    return true;
  }

  async repairAgentModel(id: string) {
    const observation = this.lifecycles.observe(id);
    if (!observation) return;
    await this.refresh(id);
    if (observation.current() && this.state.views[id].modelMutation?.status !== 'in-flight') {
      const view = this.state.views[id], intent = view.modelIntent, observed = view.snapshot?.model?.configured;
      const confirmed = intent?.phase === 'failed' && observed?.model === intent.config.model
        && (observed.reasoningProfile ?? undefined) === (intent.config.reasoningProfile ?? undefined);
      this.setSession(id, { modelMutation: undefined, ...(confirmed ? { modelIntent: undefined } : {}) });
    }
  }
  cancellationTarget(id: string): CancellationTarget | undefined {
    const view = this.state.views[id], attempt = view?.snapshot?.attempt;
    if (!this.isAttachmentControlCurrent(id, view?.attachmentObservation) || !this.initialized || this.state.connection !== 'connected' || view?.attachmentIntent !== 'wanted' || view.attachment !== 'attached'
      || !view.target || view.deleting || view.cancellation || !attempt || attempt.phase.type === 'settled'
      || view.snapshot?.shutting_down || view.snapshot?.durability_failure || view.snapshot?.pending_interactions?.length) return;
    return { generation: this.state.generation, target: view.target, attemptId: attempt.attempt_id };
  }
  async cancelTurn(expected: CancellationTarget) {
    const { target, generation, attemptId } = expected, id = target.session_id;
    const live = this.cancellationTarget(id);
    if (!live || live.generation !== generation || !sameTarget(live.target, target) || live.attemptId !== attemptId) return;
    const current = () => this.current(generation) && sameTarget(this.state.views[id]?.target, target);
    const operation = { attemptId, status: 'in-flight' as const };
    this.setSession(id, { cancellation: operation });
    try {
      await this.request({ method: 'turn/cancel', params: { target } }, 'cancellation_accepted', undefined, () => {
        // Exact lifecycle control: revalidate beside the actual socket dispatch.
        const view = this.state.views[id];
        return current() && this.initialized && this.state.connection === 'connected' && view?.attachmentIntent === 'wanted'
          && view.attachment === 'attached' && !view.deleting && !view.snapshot?.shutting_down && !view.snapshot?.durability_failure
          && !view.snapshot?.pending_interactions?.length && view.snapshot?.attempt?.attempt_id === attemptId
          && view.snapshot.attempt.phase.type !== 'settled' && view.cancellation === operation;
      });
      if (current() && this.state.views[id].cancellation?.attemptId === attemptId) {
        this.setSession(id, { cancellation: { attemptId, status: 'acknowledged' } });
        await this.refresh(id);
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
  async answer(id: string, interaction: InteractionRef, response?: InteractionResponse) {
    const key = interactionKey(interaction);
    if (this.state.interactionOperations[key]) throw new Error('Response already in flight or uncertain. Refresh authoritative state.');
    const target = this.target(id);
    if (!this.state.views[id].snapshot?.pending_interactions?.some(item => interactionKey(item.interaction) === key)) throw new Error('Interaction is no longer pending.');
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
  switchNode(id: string, nodeId: string): Promise<void> {
    return this.lifecycles.command(id, { kind: 'switch', node: nodeId, current: () => true }, true).then(() => {});
  }
  private retireObservationWork(id: string) {
    this.acquiring.delete(id);
    this.readingIntents.delete(id); this.retireOutline(id); this.readingAuthorities.delete(id);
    this.traceReads.delete(id);
    this.traceAuthorities.delete(id);
    this.refreshes.delete(id); this.dirty.delete(id); this.resubscribe.delete(id);
  }
  private retireAttachmentWork(id: string) {
    this.retireObservationWork(id);
    this.summarySettled.delete(id);
    this.summaryObservedEpoch.delete(id);
    this.setSession(id, { history: undefined, submissions: undefined, turnOutline: undefined, turnNavigation: undefined });
  }
  clearError() { this.publish({ error: undefined }); }
  acknowledgeDiagnostic(id: string) {
    // This acknowledges only the notice. Uncertain interaction controls remain
    // disabled until native state resolves them; no RPC is emitted.
    this.publish({ uncertain: this.state.uncertain.filter(item => item.id !== id) });
  }
}

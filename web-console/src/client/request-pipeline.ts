import { sameTarget } from '../../../protocol/app-server/attachment-identity';
import { hasResponseDeadline, requestEffect, requestLane } from '../../../protocol/app-server/method-policy';
import { OutcomeUncertain, RequestAdmissionDeferred, RequestNotDispatched, RpcFailure } from './errors';
import { interactionKey, type ClientView, type OperationAdmission, type SessionView, type Socket, type Submission, type UncertainOperation } from './contracts';
import type { MethodResult, Request, Request1, Response } from '../../../protocol/app-server/v57';

interface Pending {
  /** Pure actor authority, evaluated after caller-owned admission callbacks. */
  authority?: () => boolean;
  dispatchCurrent?: (() => boolean) | OperationAdmission;
  /** Final validation holding an RPC slot; `timer` is its deadline until send. */
  validation?: AbortController;
  acknowledged?: (result: MethodResult) => void;
  request: Request;
  sessionId?: string;
  mutation: boolean;
  sent: boolean;
  expected: MethodResult['type'];
  resolve: (result: MethodResult) => void;
  reject: (error: Error) => void;
  timer?: ReturnType<typeof setTimeout>;
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

/** What the request pipeline reads from its connection. */
export interface RequestPipelinePort {
  timeoutMs(): number;
  socket(): Socket | undefined;
  initialized(): boolean;
  state(): Readonly<ClientView>;
  current(generation: number): boolean;
  /** Ends the connection of `generation` as stale, if it is still current. */
  lose(generation: number): void;
  /** Session lifecycle operations carry a one-use actor admission proof. */
  claimLifecycleAdmission(admission: OperationAdmission): boolean;
  observes(id: string, admission: SessionView['attachmentObservation']): boolean;
  controls(id: string, admission: SessionView['attachmentObservation']): boolean;
  /** The turn requests this pipeline owns for Session `id` changed. */
  inboundChanged(id: string): void;
  /** A turn request of Session `id` was answered; `accepted` names the acknowledged message. */
  inboundAnswered(id: string, accepted?: Submission): void;
  /** A native lineage commit whose durability is uncertain. */
  uncertainCommit(operation: UncertainOperation): void;
  /** Capacity became available again at this readiness cut. */
  admissionAvailable(revision: number): void;
}

/** The bounded request pipeline of one WebSocket connection: id allocation,
 * lane capacity derived from the shared method policy, operation admission
 * re-validated adjacent to send, response correlation and deadlines. No call
 * is ever retried; a sent mutation whose response is lost becomes uncertain
 * exactly once, when the connection ends. */
export class RequestPipeline {
  private nextId = 0;
  private pending = new Map<string, Pending>();
  private admissionRevision = 0;
  private admissionNotification = false;
  constructor(private readonly port: RequestPipelinePort) {}

  get size() { return this.pending.size; }
  /** Sessions with requests still owned by this pipeline, sent or not. */
  sessions(): string[] { return [...this.pending.values()].flatMap(item => item.sessionId ? [item.sessionId] : []); }
  /** Current-generation turn/start and turn/steer requests of one Session. */
  inboundRequests(id: string): number {
    return [...this.pending.values()].filter(item =>
      (item.request.method === 'turn/start' || item.request.method === 'turn/steer') && item.sessionId === id).length;
  }
  private capacityReleased() {
    ++this.admissionRevision;
    if (this.admissionNotification) return;
    this.admissionNotification = true;
    // Notify after pending-map mutation/pumping has committed. A readiness cut
    // also prevents a completion racing the refusal continuation from being lost.
    queueMicrotask(() => {
      this.admissionNotification = false;
      if (this.port.socket() && this.port.initialized() && this.pending.size < 64) this.port.admissionAvailable(this.admissionRevision);
    });
  }
  /** Correlation only. No call is ever retried. Every payload is a generated union. */
  async request<T extends MethodResult['type']>(operation: Request1, expected: T, acknowledged?: (result: Extract<MethodResult, { type: T }>) => void, dispatchCurrent?: (() => boolean) | OperationAdmission, observationSignal?: AbortSignal): Promise<Extract<MethodResult, { type: T }>> {
    if (!this.port.socket() || (!this.port.initialized() && operation.method !== 'initialize')) throw new RequestNotDispatched('Connect and initialize first.');
    if (['session/attach', 'session/detach', 'session/switchNode', 'session/delete', 'session/recoverDeletion'].includes(operation.method)
      && (typeof dispatchCurrent !== 'object' || !this.port.claimLifecycleAdmission(dispatchCurrent))) throw new RequestNotDispatched('Session lifecycle operations require actor admission.');
    // The Session actor owns control admission. Retained targets authorize only
    // lifecycle settlement; live reads and new effects require the exact committed
    // attachment proof, captured once and checked again adjacent to socket send.
    let authority: (() => boolean) | undefined;
    if ('target' in operation.params && 'session_id' in operation.params.target
      && mayCommit(operation)
      && operation.method !== 'session/detach' && operation.method !== 'session/switchNode') {
      const target = operation.params.target, id = target.session_id;
      const admission = this.port.state().views[id]?.attachmentObservation;
      const current = () => (operation.method === 'turn/cancel' || operation.method === 'context/compact' || operation.method === 'auxiliary/cancel'
        ? this.port.observes(id, admission)
        : this.port.controls(id, admission)) && sameTarget(admission?.target, target);
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
    if (mayCommit(operation) && this.port.state().uncertain.length + this.pending.size >= 64) throw new RequestNotDispatched('Uncertain-operation capacity reached. Inspect and acknowledge diagnostics first.');
    const generation = this.port.state().generation;
    const id = `${generation}:${++this.nextId}`;
    const request: Request = { jsonrpc: '2.0', id, ...operation };
    try {
      if (new TextEncoder().encode(JSON.stringify(request)).length > 1_048_576) throw new Error('Request exceeds the App Server 1 MiB limit.');
    } catch (cause) { throw new RequestNotDispatched(cause); }
    let abortObservation = () => {};
    const work = new Promise<MethodResult>((resolve, reject) => {
      const params = operation.params;
      const sessionId = 'target' in params && 'session_id' in params.target ? params.target.session_id : 'session_id' in params ? params.session_id : undefined;
      this.pending.set(id, { request, sessionId, mutation: mayCommit(operation), sent: false, expected, resolve, reject, dispatchCurrent, authority, acknowledged: result => acknowledged?.(result as Extract<MethodResult, { type: T }>) });
      abortObservation = () => {
        const auxiliaryRead = ['auxiliary/status', 'auxiliary/result', 'auxiliary/submissionLookup'].includes(operation.method);
        if (operation.method !== 'agent/observe' && !auxiliaryRead) return;
        const pending = this.pending.get(id);
        if (!pending) return;
        if (!pending.sent) { this.refuse(pending, new Error('Observation retired before dispatch')); this.pump(); }
        else if (auxiliaryRead) void this.request({method:'delivery/cancel',params:{request_id:id}},'delivery_cancel').catch(() => {});
        else void this.request({method:'agent/observeCancel',params:{request_id:id}},'agent_observe_cancelled').catch(() => {});
      };
      observationSignal?.addEventListener('abort', abortObservation, {once:true});
      if (observationSignal?.aborted) abortObservation();

      if (operation.method === 'turn/start' || operation.method === 'turn/steer') this.port.inboundChanged(operation.params.target.session_id);
      this.pump();
    });
    const result = observationSignal ? await work.finally(() => observationSignal.removeEventListener('abort', abortObservation)) : await work;
    if (!this.port.current(generation)) throw new Error('Obsolete connection response; inspect the current authoritative state.');
    if (result.type !== expected) {
      this.port.lose(this.port.state().generation);
      throw new Error(`Incompatible response: expected ${expected}, received ${result.type}.`);
    }
    return result as Extract<MethodResult, { type: T }>;
  }
  private dispatchAllowed(pending: Pending): boolean {
    try {
      return dispatchCurrent(pending) && (!pending.authority || pending.authority())
        && !!this.port.socket() && this.pending.get(String(pending.request.id)) === pending && !pending.sent;
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
  pump() {
    // An obsolete proof releases its reservation now, never after its Host read.
    for (const pending of this.pending.values()) if (pending.validation && !this.dispatchAllowed(pending)) this.refuse(pending);
    let occupied = [...this.pending.values()].filter(p => (p.sent || p.validation) && requestLane(p.request.method) === 'rpc').length;
    let observing = [...this.pending.values()].filter(p => p.sent && requestLane(p.request.method) === 'observation').length;
    let validating = [...this.pending.values()].filter(p => p.validation).length;
    for (const pending of this.pending.values()) {
      if (!this.port.socket()) break;
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
        }, this.port.timeoutMs());
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
    if (pending.request.method === 'turn/start' || pending.request.method === 'turn/steer') this.port.inboundChanged(pending.request.params.target.session_id);
  }
  private sendPending(pending: Pending) {
    const socket = this.port.socket(), generation = this.port.state().generation;
    let raw: string;
    try { raw = JSON.stringify(pending.request); }
    catch (cause) { this.refuse(pending, cause); return; }
    if (!this.dispatchAllowed(pending)) { this.refuse(pending); return; }
    if (!socket || this.port.socket() !== socket || !this.port.current(generation)
      || this.pending.get(String(pending.request.id)) !== pending || pending.sent) return;
    if (this.observeOccupied(pending)) return;
    if (socket.readyState !== 1) { this.port.lose(generation); return; }
    // No external observer runs between this final proof and send invocation.
    // sent means transmission attempted, never native acceptance.
    pending.sent = true;
    if (requestLane(pending.request.method) === 'rpc' && hasResponseDeadline(pending.request.method)) pending.timer = setTimeout(() => this.port.lose(generation), this.port.timeoutMs());
    try { socket.send(raw); } catch { this.port.lose(generation); }
    try { if (typeof pending.dispatchCurrent === 'object') pending.dispatchCurrent.sent?.(); }
    catch (error) { console.error('App Server dispatch observer failed', error); }
  }
  /** Correlates one decoded response of `generation`. A response naming no
   * transmitted request is ignored; a malformed one loses the connection. */
  response(value: Response, generation: number): void {
    const pending = value.id !== null ? this.pending.get(String(value.id)) : undefined;
    if (!pending || !pending.sent) return;
    if (('result' in value) === ('error' in value) ||
        ('result' in value && (!value.result || value.result.type !== pending.expected))) {
      // Leave the transmitted request registered while closing, so a malformed
      // acknowledgement cannot erase its uncertain mutation diagnostic.
      this.port.lose(generation); return;
    }
    this.pending.delete(String(value.id)); clearTimeout(pending.timer);
    this.capacityReleased();
    if ('result' in value && value.result?.type === 'session_transition' && value.result.durability_diagnostic) {
      this.port.uncertainCommit({ id: String(value.id), method: pending.request.method, sessionId: pending.sessionId, generation,
        committedNode: { sessionId: value.result.session.id, nodeId: value.result.session.active_node }, diagnostic: value.result.durability_diagnostic });
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
        const accepted = 'result' in value && value.result.type === 'inbound_accepted' && sameTarget(this.port.state().views[target.session_id]?.target, target)
          ? { messageId: value.result.message_id, content: operation.params.content } : undefined;
        // One publication hands request ownership to exact acknowledged identity.
        // Never publish a zero count before publishing the accepted MessageId.
        this.port.inboundAnswered(target.session_id, accepted);
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
  }
  /** The connection ended: every request settles now. A sent mutation becomes
   * uncertain exactly once; unsent work was provably never dispatched. */
  disconnect(generation: number): UncertainOperation[] {
    const uncertain: UncertainOperation[] = [];
    for (const [id, pending] of this.pending) {
      clearTimeout(pending.timer); pending.validation?.abort(); pending.validation = undefined;
      if (pending.sent && pending.mutation) {
        const params = pending.request.params;
        uncertain.push({ id, method: pending.request.method, sessionId: pending.sessionId, generation,
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
    return uncertain;
  }
}

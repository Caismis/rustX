import { TRACE_LIMIT, TRACE_PAGE_SIZE, prependTrace, refreshTrace, replaceTrace, selectTrace, traceInterests, type TraceCache } from './trace';
import type {
  PendingInboundRef, PendingMutationOutcome, AttachmentTarget, GoalMutation, GoalRef, InteractionRef, InteractionResponse, MethodResult, Notification,
  Request, Request1, Response, RuntimeClientCursor, RuntimeClientSnapshot,
  SessionPersistentState, SessionSummary, ServerCapabilities, UserInputBlock, UploadReceipt, UploadedFile,
} from '../../../protocol/app-server/v5';
import { UPLOAD_MAX_BYTES, UPLOAD_BATCH_MAX_BYTES, DRAFT_MAX_FILES } from './uploads';
import { HISTORY_LIMIT, HISTORY_PAGE_SIZE, prependTranscript, refreshTranscript, replaceTranscript, type TranscriptCache } from './transcript';
import { ProtocolLog, type WireContext } from './protocol-log';

export type InboundControlOutcome =
  | { status: 'known'; outcome: PendingMutationOutcome; observed: boolean }
  | { status: 'uncertain' }
  | { status: 'obsolete' }
  | { status: 'rejected'; reason: string; observed: boolean };

export type ConnectionState = 'disconnected' | 'connecting' | 'connected' | 'reconnecting' | 'resynchronizing' | 'stale' | 'incompatible' | 'error';
export interface SessionView {
  id: string;
  // Local future-control intent; never inferred from an RPC acknowledgement.
  attachmentIntent: 'wanted' | 'released';
  // Last server observation, independent of tab visibility and local intent.
  attachment: 'detached' | 'attaching' | 'attached' | 'resynchronizing' | 'stale' | 'unloaded' | 'error';
  target?: AttachmentTarget;
  /** Exact native node explicitly opened by this view; retained across reconnect. */
  nodeId?: string;
  snapshot?: RuntimeClientSnapshot;
  trace?: TraceCache;
  cursor?: RuntimeClientCursor;
  history?: TranscriptCache;
  settings?: SessionPersistentState;
  /** Current native source trust, not loaded resource activation or Host authorization. */
  projectTrusted?: boolean | null;
  /** Exact acknowledged MessageIds awaiting projection reconciliation, not queue authority. */
  submissions?: readonly Submission[];
  /** Current-generation turn/start or turn/steer requests awaiting an outcome.
   * Transport ownership only, including unsent requests in the bounded pipeline. */
  inboundRequests?: number;
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
  id: string;
  method: Request1['method'];
  sessionId?: string;
  interactionKey?: string;
  generation: number;
}
export interface ClientView {
  endpoint?: string;
  connection: ConnectionState;
  generation: number;
  capabilities?: ServerCapabilities;
  error?: string;
  sessions: readonly SessionSummary[];
  sessionResidencies?: Record<string, import('../../../protocol/app-server/v5').ResidencyState>;
  nextOffset?: number | null;
  views: Readonly<Record<string, SessionView>>;
  uncertain: readonly UncertainOperation[];
  interactionOperations: Readonly<Record<string, { sessionId: string; status: 'in-flight' | 'uncertain' | 'acknowledged' }>>;
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
interface Pending {
  request: Request;
  context: WireContext;
  mutation: boolean;
  sent: boolean;
  expected: MethodResult['type'];
  resolve: (result: MethodResult) => void;
  reject: (error: Error) => void;
  timer?: ReturnType<typeof setTimeout>;
}
export class OutcomeUncertain extends Error {
  constructor() { super('Response lost after transmission. Outcome uncertain; the request was not replayed. Reconnect and inspect authoritative state.'); }
}
export function isOutcomeUncertain(error: unknown): boolean {
  return error instanceof OutcomeUncertain || (error instanceof RpcFailure && error.error.data?.kind === "committed_durability_uncertain");
}
export class RpcFailure extends Error {
  constructor(readonly error: Extract<Response, { error: unknown }>['error']) { super(`${error.message} (${error.code})${error.data ? `: ${JSON.stringify(error.data)}` : ''}`); }
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
  'artifact/read', 'initialize', 'server/info', 'session/list', 'session/read', 'session/tree', 'session/deletePreview',
  'session/snapshot', 'session/transcript', 'session/trace', 'settings/read', 'settings/model', 'settings/models',
  'resources/read', 'background/status', 'subagent/status', 'settings/defaults', 'session/boundaries',
]);
export const interactionKey = (ref: InteractionRef) => JSON.stringify([ref.conversation_id, ref.interaction_id]);
export const sameTarget = (a?: AttachmentTarget, b?: AttachmentTarget) => !!a && !!b &&
  a.session_id === b.session_id && a.conversation_id === b.conversation_id &&
  a.runtime_incarnation === b.runtime_incarnation && a.attachment_id === b.attachment_id;

/** One native rustX connection. All retained snapshots are replaceable read caches.
 * No event fold, retry transaction ID, runtime lifetime, or browser persistence. */
export class AppServerClient {
  readonly log = new ProtocolLog();
  private socket?: Socket;
  private initialized = false;
  private nextId = 0;
  private listeners = new Set<() => void>();
  private pending = new Map<string, Pending>();
  private refreshes = new Map<string, Promise<void>>();
  private dirty = new Set<string>();
  private resubscribe = new Set<string>();
  private attachmentChanges = new Map<string, { kind: 'attach' | 'detach' | 'unload'; work: Promise<void> }>();
  private attachmentChangeCount = 0;
  private attachmentEpochs = new Map<string, number>();
  private state: ClientView = {
    connection: 'disconnected', generation: 0, sessions: [], views: {}, uncertain: [], interactionOperations: {},
  };
  constructor(private readonly socketFactory: SocketFactory = (url, protocols) => new WebSocket(url, protocols), private readonly timeoutMs = 30_000) {}
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  getSnapshot = () => this.state;
  private publish(patch: Partial<ClientView>) {
    this.state = { ...this.state, ...patch };
    for (const listener of this.listeners) listener();
  }
  private setSession(id: string, patch: Partial<SessionView>) {
    const view = this.state.views[id] ?? { id, attachmentIntent: 'released' as const, attachment: 'detached' as const };
    this.publish({ views: { ...this.state.views, [id]: { ...view, ...patch } } });
  }
  // Product policy is injected by the Web owner, not interpreted by this transport.
  // Fail closed when there is no admission owner (including after its disposal).
  private attachmentAdmission?: (id: string, current: () => boolean) => Promise<boolean>;
  setAttachmentAdmission(admit: (id: string, current: () => boolean) => Promise<boolean>) {
    this.attachmentAdmission = admit;
    return () => { if (this.attachmentAdmission === admit) this.attachmentAdmission = undefined; };
  }
  async admitAttachment(id: string, current: () => boolean = () => true): Promise<boolean> {
    const generation = this.state.generation;
    const valid = () => current() && this.current(generation);
    if (!valid()) return false;
    if (!this.attachmentAdmission) throw new Error('No Web attachment admission owner.');
    const allowed = await this.attachmentAdmission(id, valid);
    return allowed && valid();
  }
  restoreViews(ids: readonly string[]) {
    for (const id of ids.slice(0, 32)) if (!this.state.views[id]) this.setSession(id, { attachmentIntent: 'wanted' });
  }
  async connect(endpoint: string, token: string, reconnect = false) {
    const url = new URL(endpoint);
    if (!['ws:', 'wss:'].includes(url.protocol) || url.username || url.password || url.search || url.hash || url.pathname !== '/') {
      throw new Error('Use a ws:// or wss:// endpoint at / with no credentials, query, or fragment.');
    }
    if (!/^[A-Za-z0-9_-]{43,128}$/.test(token)) throw new Error('Enter the dedicated 43–128 character App Server transport token.');
    this.disconnect();
    const generation = this.state.generation;
    this.publish({ endpoint: url.href, connection: reconnect ? 'reconnecting' : 'connecting', capabilities: undefined, error: undefined });
    try {
      const socket = this.socketFactory(url.href, ['rustx.app-server.v5', `rustx-token.${token}`]);
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
        socket.onclose = () => { clearTimeout(timer); fail('WebSocket closed. Check endpoint and transport token.'); };
        socket.onerror = () => { clearTimeout(timer); fail('WebSocket failed. Check endpoint and transport token.'); };
      });
      const hello = await this.request({ method: 'initialize', params: {
        protocol_version: 5, client: { name: 'rustx-web-console', version: '0.1.0' },
        presentation: { images: true, questionnaires: true, reviews: true },
      } }, 'initialized');
      if (!this.current(generation)) return;
      if (hello.protocol_version !== 5 || !hello.capabilities.multi_session || !hello.capabilities.headless_interactions || !hello.capabilities.single_writable_controller) {
        throw new Error('Incompatible App Server protocol or capabilities. Protocol v5 with native multi-Session, headless interactions, and single-controller admission is required.');
      }
      this.initialized = true;
      this.publish({ capabilities: hello.capabilities, connection: 'resynchronizing' });
      await this.listSessions();
      // Sequential repair leaves room for controls below the transport's 16-work bound.
      for (const id of Object.keys(this.state.views)) {
        if (!this.current(generation)) return;
        // Re-read current intent after every await; reconnect never creates intent.
        if (this.state.views[id]?.attachmentIntent === 'wanted') await this.acquireAttachment(id).catch(() => {});
      }
      if (this.current(generation)) this.publish({ connection: 'connected' });
    } catch (error) {
      if (!this.current(generation)) return;
      const incompatible = (error instanceof RpcFailure && error.error.data?.kind === 'unsupported_version') || String(error).includes('Incompatible');
      this.lose(generation);
      this.publish({ connection: incompatible ? 'incompatible' : 'error', error: String(error) });
      throw error;
    }
  }
  private current(generation: number) { return generation === this.state.generation && !!this.socket; }
  /** Explicit transport loss only; never dispatches semantic cancellation or unload. */
  disconnect() { this.endConnection('disconnected'); }
  private lose(generation: number) {
    if (this.current(generation)) this.endConnection('stale');
  }
  private endConnection(connection: ConnectionState) {
    const oldSocket = this.socket;
    this.socket = undefined;
    this.initialized = false;
    const uncertain = [...this.state.uncertain];
    for (const [id, pending] of this.pending) {
      clearTimeout(pending.timer);
      if (pending.sent && pending.mutation) {
        const params = pending.request.params;
        uncertain.push({ id, method: pending.request.method, sessionId: pending.context.sessionId,
          generation: this.state.generation,
          ...('interaction' in params ? { interactionKey: interactionKey(params.interaction) } : {}),
        });
        pending.reject(new OutcomeUncertain());
      } else pending.reject(new Error('Disconnected before a response. Unsent operations were discarded.'));
    }
    this.pending.clear();
    this.refreshes.clear(); this.dirty.clear(); this.resubscribe.clear(); this.attachmentChanges.clear();
    const operations = { ...this.state.interactionOperations };
    for (const [key, operation] of Object.entries(operations)) if (operation.status === 'in-flight') delete operations[key];
    for (const item of uncertain) if (item.interactionKey) operations[item.interactionKey] = { sessionId: item.sessionId!, status: 'uncertain' };
    this.publish({ connection, generation: this.state.generation + 1, uncertain, interactionOperations: operations,
      views: Object.fromEntries(Object.entries(this.state.views).map(([id, view]) => [id, {
        ...view, history: undefined, target: undefined, submissions: undefined, inboundRequests: undefined, attachment: view.target || ['attaching', 'attached', 'resynchronizing'].includes(view.attachment) ? 'stale' : view.attachment,
      }])),
    });
    oldSocket?.close();
  }
  /** Correlation only. No call is ever retried. Every payload is a generated union. */
  async request<T extends MethodResult['type']>(operation: Request1, expected: T): Promise<Extract<MethodResult, { type: T }>> {
    if (!this.socket || (!this.initialized && operation.method !== 'initialize')) throw new Error('Connect and initialize first.');
    if (operation.method.startsWith('artifact/') && [...this.pending.values()].filter(item => item.request.method.startsWith('artifact/')).length >= 2) throw new Error('Artifact transfer capacity reached. Retry after current transfers finish.');
    if (this.pending.size >= 64) throw new Error('Client request capacity reached.');
    // Keep uncertain diagnostics finite without silently forgetting unresolved mutations.
    if (!READS.has(operation.method) && this.state.uncertain.length + this.pending.size >= 64) throw new Error('Uncertain-operation capacity reached. Inspect and acknowledge diagnostics first.');
    const generation = this.state.generation;
    const id = `${generation}:${++this.nextId}`;
    const request: Request = { jsonrpc: '2.0', id, ...operation };
    if (new TextEncoder().encode(JSON.stringify(request)).length > 1_048_576) throw new Error('Request exceeds the App Server 1 MiB limit.');
    const result = await new Promise<MethodResult>((resolve, reject) => {
      const params = operation.params;
      const context = { method: operation.method,
        sessionId: 'target' in params ? params.target.session_id : 'session_id' in params ? params.session_id : undefined };
      this.pending.set(id, { request, context, mutation: !READS.has(operation.method), sent: false, expected, resolve, reject });
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
  private pump() {
    let sent = [...this.pending.values()].filter(p => p.sent).length;
    for (const pending of this.pending.values()) {
      if (sent >= 8 || !this.socket) break;
      if (pending.sent) continue;
      const raw = JSON.stringify(pending.request);
      const generation = this.state.generation;
      pending.sent = true; sent++;
      this.log.observe('out', generation, raw, pending.context);
      pending.timer = setTimeout(() => this.lose(generation), this.timeoutMs);
      try { this.socket.send(raw); } catch { this.lose(generation); break; }
    }
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
      if ('error' in value) pending.reject(new RpcFailure(value.error));
      else if ('result' in value) pending.resolve(value.result);
      this.pump();
      return;
    }
    if (!['session/event', 'session/resyncRequired', 'session/closed'].includes(value.method) || !value.params?.target) {
      this.lose(generation); return;
    }
    const target = value.params.target;
    const view = this.state.views[target.session_id];
    // An attach response may be interleaved after its first notification.
    if (view?.attachment === 'attaching' && !view?.target) { this.dirty.add(target.session_id); return; }
    if (!sameTarget(view?.target, target)) return;
    if (value.method === 'session/closed') {
      this.retireAttachmentWork(target.session_id);
      this.setSession(target.session_id, { attachment: 'stale', target: undefined, error: 'Attachment closed by server; reattach to inspect residency.' });
    } else {
      if (value.method === 'session/event' && value.params.event.type === 'pending_inbound_changed') {
        void this.rereadPending(target.session_id, () => this.current(generation) && sameTarget(this.state.views[target.session_id]?.target, target));
        return;
      }
      if (value.method === 'session/resyncRequired') this.resubscribe.add(target.session_id);
      void this.refresh(target.session_id).catch(() => {});
    }
  }
  private listEpoch = 0;
  private listOffset = 0;
  private listQuery = '';
  async listSessions(offset = this.listOffset, query = this.listQuery, current: () => boolean = () => true) {
    const epoch = ++this.listEpoch;
    this.listOffset = offset; this.listQuery = query;
    const generation = this.state.generation;
    const result = await this.request({ method: 'session/list', params: { offset, limit: 32, query } }, 'sessions');
    if (this.current(generation) && epoch === this.listEpoch && current()) this.publish({ sessions: result.sessions, sessionResidencies: result.residencies, nextOffset: result.next_offset });
  }
  async deleteSession(id: string, expectedRevision: string) {
    const generation = this.state.generation;
    const result = await this.request({ method: 'session/delete', params: { session_id: id, expected_target_revision: expectedRevision } }, 'deletion');
    if (!this.current(generation)) return;
    if (result.result.status === 'deleted' || result.result.status === 'not_found') {
      this.retireAttachmentWork(id);
      this.attachmentEpochs.set(id, (this.attachmentEpochs.get(id) ?? 0) + 1);
      const views = { ...this.state.views }; delete views[id];
      this.publish({ views });
    }
    await this.listSessions();
    if (this.current(generation)) return result.result;
  }
  /** Explicit Open / Attach gesture. Visibility itself does not acquire a claim. */
  attach(id: string, nodeId?: string, navigationCurrent: () => boolean = () => true): Promise<void> {
    if (nodeId && this.state.views[id]?.target && this.state.views[id]?.nodeId !== nodeId) return Promise.reject(new Error('Unload the resident Session before opening another node.'));
    this.setSession(id, { attachmentIntent: 'wanted', ...(nodeId ? { nodeId } : {}) });
    return this.acquireAttachment(id, navigationCurrent);
  }
  private acquireAttachment(id: string, navigationCurrent: () => boolean = () => true): Promise<void> {
    return this.changeAttachment(id, 'attach', async generation => {
      if (!navigationCurrent() || this.state.views[id]?.attachmentIntent !== 'wanted') return;
      if (this.state.views[id]?.target) return this.refresh(id);
      this.setSession(id, { attachment: 'attaching', error: undefined, projectTrusted: undefined });
      const epoch = (this.attachmentEpochs.get(id) ?? 0) + 1;
      this.attachmentEpochs.set(id, epoch);
      await this.performAttach(id, generation, epoch, navigationCurrent);
    });
  }
  /** Serialize explicit attachment gestures, including close during attach and
   * reopen during release. This queue never retries and cannot cross generations. */
  private changeAttachment(id: string, kind: 'attach' | 'detach' | 'unload', operation: (generation: number) => Promise<void>): Promise<void> {
    const previous = this.attachmentChanges.get(id);
    // A newer Open has its own navigation/admission fence. Queue it behind the
    // preceding Open; if that one attached, the new gesture only refreshes it.
    if (previous?.kind === kind && kind !== 'attach') return previous.work;
    if (this.attachmentChangeCount >= 64) return Promise.reject(new Error('Attachment operation capacity reached. Disconnect to release external claims.'));
    const generation = this.state.generation;
    this.attachmentChangeCount++;
    const work = (async () => {
      if (previous) await previous.work.catch(() => {});
      if (!this.current(generation)) return;
      await operation(generation);
    })();
    const change = { kind, work };
    this.attachmentChanges.set(id, change);
    void work.finally(() => {
      this.attachmentChangeCount--;
      if (this.attachmentChanges.get(id) === change) this.attachmentChanges.delete(id);
    }).catch(() => {});
    return work;
  }
  private async performAttach(id: string, generation: number, epoch: number, navigationCurrent: () => boolean) {
    const current = () => this.current(generation) && this.attachmentEpochs.get(id) === epoch;
    const admissionCurrent = () => current() && navigationCurrent();
    let target: AttachmentTarget | undefined;
    try {
      if (!await this.admitAttachment(id, admissionCurrent) || !admissionCurrent()) return;
      const result = await this.request({ method: 'session/attach', params: { session_id: id, node_id: this.state.views[id]?.nodeId } }, 'attached');
      if (!current()) return;
      target = result.target;
      if (result.target.session_id !== id || result.target.conversation_id !== result.snapshot.conversation_id) throw new Error('Mismatched attachment identity.');
      this.setSession(id, { target: result.target, snapshot: result.snapshot, cursor: result.cursor, history: replaceTranscript(result.snapshot.transcript, this.state.views[id]?.history), trace: replaceTrace(result.snapshot.trace, this.state.views[id]?.trace), attachment: 'attached' });
      this.reconcileInteractions(id); this.settleSubmissions(id);
      const settings = await this.request({ method: 'settings/read', params: { session_id: id } }, 'settings');
      if (!current() || !sameTarget(this.state.views[id]?.target, result.target)) return;
      this.setSession(id, { settings: settings.settings, projectTrusted: settings.project_trusted });
      if (this.dirty.has(id)) { this.resubscribe.add(id); await this.refresh(id); }
    } catch (error) {
      if (current() && (!target || sameTarget(this.state.views[id]?.target, target))) this.setSession(id, { attachment: 'error', error: String(error) });
      throw error;
    }
  }
  /** Event invalidation coalesces to one dirty bit, not an event queue. */
  refresh(id: string): Promise<void> {
    this.dirty.add(id);
    const existing = this.refreshes.get(id);
    if (existing) return existing;
    const target = this.state.views[id]?.target;
    if (!target) return Promise.resolve();
    const generation = this.state.generation;
    const work = this.performRefresh(id, target, generation);
    this.refreshes.set(id, work);
    void work.finally(() => { if (this.refreshes.get(id) === work) this.refreshes.delete(id); }).catch(() => {});
    return work;
  }
  private async performRefresh(id: string, target: AttachmentTarget, generation: number) {
    const current = () => this.current(generation) && sameTarget(this.state.views[id]?.target, target);
    try {
      while (this.dirty.has(id) && current()) {
        this.dirty.delete(id);
        const resync = this.resubscribe.delete(id);
        if (resync) this.setSession(id, { attachment: 'resynchronizing', trace: replaceTrace({ entries: [], next_cursor: null }, this.state.views[id]?.trace), history: replaceTranscript({ entries: [] }, this.state.views[id]?.history) });
        const result = await this.request({ method: 'session/snapshot', params: { target, trace_records: traceInterests(this.state.views[id]?.trace) } }, 'snapshot');
        if (!current()) return;
        if (result.snapshot.conversation_id !== target.conversation_id) throw new Error('Mismatched snapshot conversation.');
        if (BigInt(result.cursor) >= BigInt(this.state.views[id].cursor ?? '0')) {
          this.setSession(id, { snapshot: result.snapshot, cursor: result.cursor, history: refreshTranscript(this.state.views[id]?.history, result.snapshot.transcript), trace: refreshTrace(this.state.views[id]?.trace, result.snapshot.trace, result.snapshot.trace_updates), error: undefined });
          this.reconcileInteractions(id); this.settleSubmissions(id);
        }
        if (resync) await this.request({ method: 'session/subscribe', params: { target, after_cursor: result.cursor } }, 'subscribed');
        if (current()) this.setSession(id, { attachment: 'attached' });
      }
    } catch (error) {
      if (current()) { this.resubscribe.add(id); this.setSession(id, { attachment: 'stale', error: String(error) }); }
      throw error;
    }
  }
  /** Older reads are fenced by attachment, connection and read-window epoch.
   * Ordinary live refreshes preserve the epoch only with a durable overlap. */
  async loadEarlier(id: string) {
    const target = this.target(id);
    const generation = this.state.generation;
    const history = this.state.views[id].history;
    if (!history || history.loading || history.page.next_cursor == null) return;
    const limit = Math.min(HISTORY_PAGE_SIZE, HISTORY_LIMIT - (history.page.entries?.length ?? 0));
    if (limit < 1) throw new Error('History window is full. Return to latest first.');
    const current = () => this.current(generation) && sameTarget(this.state.views[id]?.target, target)
      && this.state.views[id]?.history?.epoch === history.epoch;
    this.setSession(id, { history: { ...history, loading: true, error: undefined } });
    try {
      const result = await this.request({ method: 'session/transcript', params: { target, before: history.page.next_cursor, limit } }, 'transcript');
      if (!current()) return;
      this.setSession(id, { history: prependTranscript(this.state.views[id].history!, result.page) });
    } catch (error) {
      if (current()) this.setSession(id, { history: { ...this.state.views[id].history!, loading: false, error: String(error) } });
      throw error;
    }
  }
  async loadEarlierTrace(id: string) {
    const target = this.target(id);
    const generation = this.state.generation;
    const cache = this.state.views[id].trace;
    if (!cache || cache.loading || cache.page.next_cursor == null) return;
    const limit = Math.min(TRACE_PAGE_SIZE, TRACE_LIMIT - cache.page.entries.length);
    if (limit < 1) throw new Error('Trace window is full. Return to latest first.');
    const current = () => this.current(generation) && sameTarget(this.state.views[id]?.target, target)
      && this.state.views[id]?.trace?.epoch === cache.epoch;
    this.setSession(id, { trace: { ...cache, loading: true, error: undefined } });
    try {
      const result = await this.request({ method: 'session/trace', params: { target, before: cache.page.next_cursor, limit } }, 'trace');
      if (current()) {
        this.setSession(id, { trace: prependTrace(this.state.views[id].trace!, result.page) });
        // Include newly loaded identities in a repair even if their terminal
        // notification raced this pending historical read.
        await this.refresh(id);
      }
    } catch (error) {
      if (current()) this.setSession(id, { trace: { ...this.state.views[id].trace!, loading: false, error: String(error) } });
      throw error;
    }
  }
  selectTrace(id: string, record?: string) {
    const trace = this.state.views[id]?.trace;
    if (trace) this.setSession(id, { trace: selectTrace(trace, record) });
  }
  latestTrace(id: string) {
    const view = this.state.views[id];
    if (view?.snapshot) this.setSession(id, { trace: replaceTrace(view.snapshot.trace, view.trace) });
  }
  latestTranscript(id: string) {
    const view = this.state.views[id];
    if (view?.snapshot) this.setSession(id, { history: replaceTranscript(view.snapshot.transcript, view.history) });
  }
  private reconcileInteractions(id: string) {
    const snapshot = this.state.views[id].snapshot!;
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
    const view = this.state.views[id];
    if (!this.initialized || view?.attachment !== 'attached' || !view.target) throw new Error('Session is not authoritatively attached. Refresh or reconnect.');
    return view.target;
  }
  /** Retain an observed tree identity as navigation intent, never infer it from
   * the Session's mutable default. Reconnect then opens the same lineage. */
  rememberNode(target: AttachmentTarget, nodeId: string) {
    if (sameTarget(this.state.views[target.session_id]?.target, target)) this.setSession(target.session_id, { nodeId });
  }
  async upload(id: string, files: readonly File[]): Promise<UploadedFile[]> {
    const target = this.target(id);
    const generation = this.state.generation;
    if (!files.length || files.length > DRAFT_MAX_FILES || files.some(file => file.size > UPLOAD_MAX_BYTES)
      || files.reduce((sum, file) => sum + file.size, 0) > UPLOAD_BATCH_MAX_BYTES) throw new Error('Choose at most 8 files, 256 KiB each and 512 KiB per batch.');
    const encoded = [];
    for (const file of files) {
      const bytes = new Uint8Array(await file.arrayBuffer());
      let binary = '';
      for (const byte of bytes) binary += String.fromCharCode(byte);
      encoded.push({ name: file.name, data: btoa(binary) });
    }
    if (!this.current(generation) || !sameTarget(this.state.views[id]?.target, target)) throw new Error('Upload target changed before transfer.');
    const uploaded = await this.request({ method: 'session/upload', params: { target, files: encoded } }, 'session_uploaded');
    if (!this.current(generation) || !sameTarget(this.state.views[id]?.target, target)) throw new Error('Upload outcome belongs to an obsolete view. Remove this draft selection; do not replay it.');
    return uploaded.files;
  }
  async send(id: string, text: string, receipts: readonly UploadReceipt[] = [], delivery: 'send' | 'steer' = 'send') {
    if (receipts.length > DRAFT_MAX_FILES || receipts.some(receipt => receipt.session_id !== id)) throw new Error('Invalid Session upload receipts.');
    const content: UserInputBlock[] = [
      ...receipts.map(receipt => ({ type: 'upload' as const, ...receipt })),
      ...(text ? [{ type: 'text' as const, text }] : []),
    ];
    return this.sendContent(id, content, delivery);
  }
  async sendContent(id: string, content: UserInputBlock[], delivery: 'send' | 'steer' = 'send') {
    const target = this.target(id);
    // `turn/start` and `turn/steer` share one native inbound owner: an idle runtime
    // admits a fresh attempt, a running one drains the mailbox at a safe boundary.
    // The request pipeline owns unresolved transport and its acknowledgement
    // handoff. No MessageId or queue identity is invented before acceptance.
    return this.request({ method: delivery === 'steer' ? 'turn/steer' : 'turn/start', params: { target, content } }, 'inbound_accepted');
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
    try { await this.refresh(id); } catch { return false; }
    return current() && this.state.views[id]?.attachment === 'attached';
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
    const observed = await this.reread(id, current);
    const view = this.state.views[id];
    if (observed && view?.snapshot) this.setSession(id, { history: replaceTranscript(view.snapshot.transcript, view.history) });
    return observed;
  }
  async cancelTurn(id: string) { return this.request({ method: 'turn/cancel', params: { target: this.target(id) } }, 'cancellation_accepted'); }
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
  /** Explicit detach/unload or closing a view relinquishes future-control intent
   * immediately. Neither failure nor acknowledgement is allowed to reverse it. */
  release(id: string, unload: boolean): Promise<void> {
    this.setSession(id, { attachmentIntent: 'released' });
    return this.changeAttachment(id, unload ? 'unload' : 'detach', generation => this.performRelease(id, unload, generation));
  }
  private async performRelease(id: string, unload: boolean, generation: number) {
    // A close may arrive before attach completes, while stale, or disconnected.
    // Use an observed target if one exists; never attach just to release it.
    const target = this.state.views[id]?.target;
    if (!target) return;
    const epoch = this.attachmentEpochs.get(id);
    try {
      await this.request({ method: unload ? 'session/unload' : 'session/detach', params: { target } }, unload ? 'unloaded' : 'detached');
    } catch (error) {
      // Every terminal native unload result retires its route, including errors.
      // This says nothing about successful shutdown or final residency.
      if (unload && error instanceof RpcFailure && this.current(generation) && this.attachmentEpochs.get(id) === epoch) {
        this.retireAttachmentWork(id);
        this.setSession(id, { target: undefined, attachment: 'stale', error: String(error) });
      }
      throw error;
    }
    // Native unload may close the attachment before returning its acknowledgement.
    // A new attach increments the epoch, so this acknowledgement cannot retire it.
    if (this.current(generation) && this.attachmentEpochs.get(id) === epoch) {
      this.retireAttachmentWork(id);
      this.setSession(id, { target: undefined, attachment: unload ? 'unloaded' : 'detached', error: undefined });
    }
  }
  private retireAttachmentWork(id: string) {
    this.refreshes.delete(id); this.dirty.delete(id); this.resubscribe.delete(id);
    this.setSession(id, { history: undefined, submissions: undefined });
  }
  clearError() { this.publish({ error: undefined }); }
  acknowledgeDiagnostic(id: string) {
    // This acknowledges only the notice. Uncertain interaction controls remain
    // disabled until native state resolves them; no RPC is emitted.
    this.publish({ uncertain: this.state.uncertain.filter(item => item.id !== id) });
  }
}

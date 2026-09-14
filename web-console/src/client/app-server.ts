import type {
  AttachmentTarget, InteractionRef, InteractionResponse, MethodResult, Notification,
  Request, Request1, Response, RuntimeClientCursor, RuntimeClientSnapshot,
  SessionPersistentState, SessionSummary, ServerCapabilities,
} from '../../../protocol/app-server/v1';
import { ProtocolLog, type WireContext } from './protocol-log';

export type ConnectionState = 'disconnected' | 'connecting' | 'connected' | 'reconnecting' | 'resynchronizing' | 'stale' | 'incompatible' | 'error';
export interface SessionView {
  id: string;
  // Local future-control intent; never inferred from an RPC acknowledgement.
  attachmentIntent: 'wanted' | 'released';
  // Last server observation, independent of tab visibility and local intent.
  attachment: 'detached' | 'attaching' | 'attached' | 'resynchronizing' | 'stale' | 'unloaded' | 'error';
  target?: AttachmentTarget;
  snapshot?: RuntimeClientSnapshot;
  cursor?: RuntimeClientCursor;
  settings?: SessionPersistentState;
  error?: string;
}
export interface UncertainOperation {
  id: string;
  method: Request1['method'];
  sessionId?: string;
  interactionKey?: string;
  generation: number;
}
export interface ClientView {
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
export class RpcFailure extends Error {
  constructor(readonly error: Extract<Response, { error: unknown }>['error']) { super(`${error.message} (${error.code})${error.data ? `: ${JSON.stringify(error.data)}` : ''}`); }
}
const READS = new Set<Request1['method']>([
  'initialize', 'server/info', 'session/list', 'session/read', 'session/tree', 'session/deletePreview',
  'session/snapshot', 'session/transcript', 'settings/read', 'settings/model', 'settings/models',
  'resources/read', 'background/status', 'subagent/status', 'settings/defaults',
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
    this.publish({ connection: reconnect ? 'reconnecting' : 'connecting', capabilities: undefined, error: undefined });
    try {
      const socket = this.socketFactory(url.href, ['rustx.app-server.v1', `rustx-token.${token}`]);
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
        protocol_version: 1, client: { name: 'rustx-web-console', version: '0.1.0' },
        presentation: { images: false, questionnaires: true, reviews: true },
      } }, 'initialized');
      if (!this.current(generation)) return;
      if (hello.protocol_version !== 1 || !hello.capabilities.multi_session || !hello.capabilities.headless_interactions || !hello.capabilities.single_writable_controller) {
        throw new Error('Incompatible App Server protocol or capabilities. Protocol v1 with native multi-Session, headless interactions, and single-controller admission is required.');
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
        ...view, target: undefined, attachment: view.target || ['attaching', 'attached', 'resynchronizing'].includes(view.attachment) ? 'stale' : view.attachment,
      }])),
    });
    oldSocket?.close();
  }
  /** Correlation only. No call is ever retried. Every payload is a generated union. */
  async request<T extends MethodResult['type']>(operation: Request1, expected: T): Promise<Extract<MethodResult, { type: T }>> {
    if (!this.socket || (!this.initialized && operation.method !== 'initialize')) throw new Error('Connect and initialize first.');
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
      if (value.method === 'session/resyncRequired') this.resubscribe.add(target.session_id);
      void this.refresh(target.session_id).catch(() => {});
    }
  }
  async listSessions(offset = 0) {
    const generation = this.state.generation;
    const result = await this.request({ method: 'session/list', params: { offset, limit: 32 } }, 'sessions');
    if (this.current(generation)) this.publish({ sessions: result.sessions, nextOffset: result.next_offset });
  }
  async createSession(cwd: string) {
    const generation = this.state.generation;
    const result = await this.request({ method: 'session/create', params: { settings: { cwd } } }, 'session_transition');
    if (!this.current(generation)) return;
    if (result.durability_diagnostic) this.publish({ error: `Session create durability diagnostic: ${result.durability_diagnostic}` });
    await this.listSessions();
    if (this.current(generation)) await this.attach(result.session.id);
    return result.session.id;
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
  attach(id: string): Promise<void> {
    this.setSession(id, { attachmentIntent: 'wanted' });
    return this.acquireAttachment(id);
  }
  private acquireAttachment(id: string): Promise<void> {
    return this.changeAttachment(id, 'attach', async generation => {
      if (this.state.views[id]?.attachmentIntent !== 'wanted') return;
      if (this.state.views[id]?.target) return this.refresh(id);
      this.setSession(id, { attachment: 'attaching', error: undefined });
      const epoch = (this.attachmentEpochs.get(id) ?? 0) + 1;
      this.attachmentEpochs.set(id, epoch);
      await this.performAttach(id, generation, epoch);
    });
  }
  /** Serialize explicit attachment gestures, including close during attach and
   * reopen during release. This queue never retries and cannot cross generations. */
  private changeAttachment(id: string, kind: 'attach' | 'detach' | 'unload', operation: (generation: number) => Promise<void>): Promise<void> {
    const previous = this.attachmentChanges.get(id);
    if (previous?.kind === kind) return previous.work;
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
  private async performAttach(id: string, generation: number, epoch: number) {
    const current = () => this.current(generation) && this.attachmentEpochs.get(id) === epoch;
    let target: AttachmentTarget | undefined;
    try {
      const result = await this.request({ method: 'session/attach', params: { session_id: id } }, 'attached');
      if (!current()) return;
      target = result.target;
      if (result.target.session_id !== id || result.target.conversation_id !== result.snapshot.conversation_id) throw new Error('Mismatched attachment identity.');
      this.setSession(id, { target: result.target, snapshot: result.snapshot, cursor: result.cursor, attachment: 'attached' });
      this.reconcileInteractions(id);
      const settings = await this.request({ method: 'settings/read', params: { session_id: id } }, 'settings');
      if (!current() || !sameTarget(this.state.views[id]?.target, result.target)) return;
      this.setSession(id, { settings: settings.settings });
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
        if (resync) this.setSession(id, { attachment: 'resynchronizing' });
        const result = await this.request({ method: 'session/snapshot', params: { target } }, 'snapshot');
        if (!current()) return;
        if (result.snapshot.conversation_id !== target.conversation_id) throw new Error('Mismatched snapshot conversation.');
        if (BigInt(result.cursor) >= BigInt(this.state.views[id].cursor ?? '0')) {
          this.setSession(id, { snapshot: result.snapshot, cursor: result.cursor, error: undefined });
          this.reconcileInteractions(id);
        }
        if (resync) await this.request({ method: 'session/subscribe', params: { target, after_cursor: result.cursor } }, 'subscribed');
        if (current()) this.setSession(id, { attachment: 'attached' });
      }
    } catch (error) {
      if (current()) { this.resubscribe.add(id); this.setSession(id, { attachment: 'stale', error: String(error) }); }
      throw error;
    }
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
  async send(id: string, text: string, steer = false) {
    const target = this.target(id);
    return this.request({ method: steer ? 'turn/steer' : 'turn/start', params: { target, content: [{ type: 'text', text }] } }, 'inbound_accepted');
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
  }
  clearError() { this.publish({ error: undefined }); }
  acknowledgeDiagnostic(id: string) {
    // This acknowledges only the notice. Uncertain interaction controls remain
    // disabled until native state resolves them; no RPC is emitted.
    this.publish({ uncertain: this.state.uncertain.filter(item => item.id !== id) });
  }
}

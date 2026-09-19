import type { ProductHostWorkspaces } from '../src/workspaces/host';
import type { TraceDetail } from '../../protocol/app-server/v12';
import type { AttachmentTarget, MethodResult, Notification, Request, Response, RoutedInteraction, RuntimeClientSnapshot, SessionSummary, ServerCapabilities } from '../../protocol/app-server/v12';
import { fixtures } from '../../protocol/app-server/fixtures';
import { AppServerClient, RpcFailure, sameTarget, type Socket } from '../src/client/app-server';

export const TOKEN = 'fixture-transport-token-'.padEnd(43, 'x');
export const endpoint = 'ws://127.0.0.1:8080/';
const rustHello = fixtures.find(message => 'result' in message && message.result?.type === 'initialized');
if (!rustHello || !('result' in rustHello) || rustHello.result?.type !== 'initialized') throw new Error('Missing Rust-serialized initialize fixture');
export const capabilities: ServerCapabilities = rustHello.result.capabilities;
export function snapshot(id = 'A'): RuntimeClientSnapshot {
  return {
    settings_evidence: 'live_session',
    conversation_id: `conversation-${id}`, shutting_down: false, effective_approval_mode: 'policy',
    workflows: { revision: '0', runs: [], omitted_runs: 0 }, messages: [], transcript: { entries: [] }, trace_updates: [], trace: { records: [] },
    inbound: {}, capabilities: { revision: '0' }, pending_interactions: [],
  };
}
export function interaction(type: 'approval' | 'questionnaire', id = 'A', interactionId = `interaction-${type}`): RoutedInteraction {
  return { interaction: { conversation_id: `conversation-${id}`, interaction_id: interactionId }, source: { type: 'primary' },
    request: { id: interactionId, conversation_id: `conversation-${id}`, attempt_id: 'attempt-A', turn: 1,
      kind: type === 'approval' ? {
        type, invocation_id: { caller: 'agent', call_id: 'call-bash' }, tool_id: 'bash', tool_name: 'bash',
        origin: 'builtin', mode: 'foreground', arguments: { command: 'printf rustX' }, reason: 'Developer approval required',
      } : { type, invocation_id: { caller: 'agent', call_id: 'call-ask' }, requester: { tool_id: 'ask_user', tool_name: 'ask_user', origin: 'builtin' },
        questionnaire: { questions: [{ header: 'Direction', question: 'Which direction?', answer: { type: 'single_choice', allow_custom: true, options: [
          { label: 'Keep native (Recommended)', description: 'Use rustX owners.' }, { label: 'Simplify', description: 'Reduce presentation.' },
        ] } }] },
      },
    },
  };
}
export class FakeSocket implements Socket {
  onopen: Socket['onopen'] = null;
  onmessage: Socket['onmessage'] = null;
  onclose: Socket['onclose'] = null;
  onerror: Socket['onerror'] = null;
  requests: Request[] = [];
  closed = false;
  constructor(private handle: (request: Request, socket: FakeSocket) => void, private releaseClaims: () => void) {}
  send(raw: string) { const request = JSON.parse(raw) as Request; this.requests.push(request); this.handle(request, this); }
  open() { this.onopen?.(new Event('open')); }
  deliver(value: Response | Notification) { this.onmessage?.(new MessageEvent('message', { data: JSON.stringify(value) })); }
  success(request: Request, result: MethodResult) { this.deliver({ jsonrpc: '2.0', id: request.id, result }); }
  close() { if (!this.closed) { this.closed = true; this.releaseClaims(); this.onclose?.(new CloseEvent('close')); } }
}
export class Server {
  readonly workspaceHost: ProductHostWorkspaces = {
    listWorkspaces: async () => ({ endpoint, workspaces: [], picker: { kind: 'unavailable', reason: 'Test Host has no picker' } }),
    classifyLocations: async cwds => cwds.map(cwd => ({ authorized: ['/workspace/A', '/workspace/B', '/workspace/child', '/workspace/created', '/workspace/fork-child'].includes(cwd) })),
    resolveWorkspace: async () => { throw new Error('No test registration'); },
    adoptWorkspace: async () => {}, renameWorkspace: async () => {}, reorderWorkspace: async () => {}, removeWorkspace: async () => {},
  };

  handlers = new Map<Request['method'], (request: Request) => MethodResult>();
  sockets: FakeSocket[] = [];
  snapshots = new Map<string, RuntimeClientSnapshot>([['A', snapshot('A')], ['B', snapshot('B')]]);
  /** Explicit native catalog metadata; tests never derive preview in the browser. */
  summaries = new Map<string, Partial<SessionSummary>>();
  nodeSnapshots = new Map<string, RuntimeClientSnapshot>();
  cursor = 0n;
  private attachmentSequence = 0;
  private completed = new Map<Request, Response>();
  private reservations = new WeakMap<FakeSocket, Set<string>>();
  loaded = new Set<string>();
  coldLoads = new Map<string, number>();
  maxClaims = 0;
  private targets = new WeakMap<FakeSocket, Map<string, AttachmentTarget>>();
  held = new Set<Request['method']>();
  requests: { request: Request; socket: FakeSocket }[] = [];
  private waiters: { method: Request['method']; count: number; resolve: (request: Request) => void }[] = [];
  version = 12;
  /** Record details this scenario staged, keyed by Trace record identity. */
  readonly traceDetails = new Map<string, TraceDetail>();
  capabilities = capabilities;
  socketFactory = (_url: string, protocols: string[]) => {
    if (protocols[0] !== 'rustx.app-server.v12' || protocols[1] !== `rustx-token.${TOKEN}`) throw new Error('Wrong browser admission protocol');
    const socket = new FakeSocket((request, source) => this.receive(request, source), () => { this.targets.get(socket)?.clear(); this.reservations.get(socket)?.clear(); }); this.sockets.push(socket);
    queueMicrotask(() => socket.open()); return socket;
  };
  client = new AppServerClient(this.socketFactory);
  constructor() { this.client.setAttachmentAdmission(async () => true); } // Protocol-only fixture; App installs real Host admission.
  get socket() { return this.sockets[this.sockets.length - 1]; }
  target(id: string, socket = this.socket): AttachmentTarget {
    return this.targets.get(socket)!.get(id)!;
  }
  claims(socket = this.socket) { return [...(this.targets.get(socket)?.values() ?? [])]; }
  async connect() { await this.client.connect(endpoint, TOKEN); }
  async attached(...ids: string[]) { await this.connect(); for (const id of ids) await this.client.attach(id); }
  waitFor(method: Request['method'], count = this.requests.filter(item => item.request.method === method).length + 1): Promise<Request> {
    const existing = this.requests.filter(item => item.request.method === method)[count - 1];
    if (existing) return Promise.resolve(existing.request);
    return new Promise(resolve => this.waiters.push({ method, count, resolve }));
  }
  private receive(request: Request, socket: FakeSocket) {
    this.requests.push({ request, socket });
    if (request.method === 'session/attach') {
      const reserved = this.reservations.get(socket) ?? new Set<string>();
      this.reservations.set(socket, reserved);
      const id = request.params.session_id;
      const kind = this.targets.get(socket)?.has(id) || reserved.has(id) ? 'controller_in_use'
        : this.claims(socket).length + reserved.size >= 32 ? 'invalid_state' : undefined;
      if (kind) this.completed.set(request, { jsonrpc: '2.0', id: request.id, error: { code: -32000, message: kind, data: { kind } } });
      else reserved.add(id);
    }
    this.waiters = this.waiters.filter(waiter => {
      const matching = this.requests.filter(item => item.request.method === waiter.method)[waiter.count - 1];
      if (!matching) return true;
      waiter.resolve(matching.request); return false;
    });
    if (!this.held.has(request.method)) queueMicrotask(() => this.reply(request, socket));
  }
  reply(request: Request, socket = this.socket) { socket.deliver(this.commit(request, socket)); }
  /** Commit native state separately from acknowledgement delivery. */
  commit(request: Request, socket = this.socket): Response {
    const completed = this.completed.get(request);
    if (completed) return completed;
    let response: Response;
    try { response = { jsonrpc: '2.0', id: request.id, result: this.execute(request, socket) }; }
    catch (error) {
      if (!(error instanceof RpcFailure)) throw error;
      response = { jsonrpc: '2.0', id: request.id, error: error.error };
    }
    response = structuredClone(response);
    this.completed.set(request, response);
    return response;
  }
  summary(id: string): SessionSummary {
    if (!this.snapshots.has(id)) throw new RpcFailure({ code: -32000, message: 'Unknown Session', data: { kind: 'unknown_session', session_id: id } });
    return { id, cwd: `/workspace/${id}`, name: `Session ${id}`, updated_at: '2026-09-14T00:00:00Z', active_node: `node-${id}`, ...this.summaries.get(id) };
  }
  private execute(request: Request, socket: FakeSocket): MethodResult {
    const params = request.params;
    const id = 'target' in params ? params.target.session_id : 'session_id' in params ? params.session_id : 'A';
    if (socket.closed || ('target' in params && !sameTarget(this.targets.get(socket)?.get(id), params.target))) {
      throw new RpcFailure({ code: -32000, message: 'Stale attachment', data: { kind: 'stale_attachment' } });
    }
    const handler = this.handlers.get(request.method);
    if (handler) return handler(request);
    let result: MethodResult;
    switch (request.method) {
      case 'initialize': result = { type: 'initialized', protocol_version: this.version, capabilities: this.capabilities }; break;
      case 'server/info': result = { type: 'server_info', capabilities: this.capabilities }; break;
      case 'session/summary': result = { type: 'session_summary', summary: this.summary(request.params.session_id) }; break;
      case 'session/list': if (request.params.limit > 32) throw new Error('Native Session page limit is 32'); result = { type: 'sessions', sessions: [...this.snapshots.keys()].map(id => this.summary(id)).filter(row => !request.params.query || [row.id, row.name, row.preview].some(text => text?.toLowerCase().includes(request.params.query!.toLowerCase()))).slice(request.params.offset, request.params.offset + request.params.limit) }; break;
      case 'session/attach': {
        this.reservations.get(socket)?.delete(id);
        if (!this.loaded.has(id)) { this.loaded.add(id); this.coldLoads.set(id, (this.coldLoads.get(id) ?? 0) + 1); }
        if (this.sockets.some(source => this.claims(source).some(target => target.session_id === id))) throw new RpcFailure({ code: -32000, message: 'Controller in use', data: { kind: 'controller_in_use' } });
        const targets = this.targets.get(socket) ?? new Map<string, AttachmentTarget>();
        const attachedSnapshot = (request.params.node_id ? this.nodeSnapshots.get(request.params.node_id) : undefined) ?? this.snapshots.get(id)!;
        targets.set(id, { session_id: id, conversation_id: attachedSnapshot.conversation_id, runtime_incarnation: String(9007199254740992n + BigInt(this.coldLoads.get(id)!)), attachment_id: `${id}-${++this.attachmentSequence}` });
        this.targets.set(socket, targets);
        this.maxClaims = Math.max(this.maxClaims, targets.size);
        result = { type: 'attached', target: this.target(id, socket), snapshot: attachedSnapshot, cursor: String(this.cursor) }; break;
      }
      case 'settings/read': result = { type: 'settings', revision: '0', settings: { cwd: `/workspace/${id}` } }; break;
      case 'session/snapshot': result = { type: 'snapshot', snapshot: this.snapshots.get(id)!, cursor: String(this.cursor) }; break;
      case 'session/subscribe': result = { type: 'subscribed', after_cursor: request.params.after_cursor }; break;
      // Inspection detail is served per record, so the fixture answers from
      // the details its scenario staged and reports absence otherwise.
      case 'session/traceDetail': result = { type: 'trace_detail', detail: this.traceDetails.get(request.params.record_id) ?? null }; break;
      case 'session/detach': this.targets.get(socket)!.delete(id); result = { type: 'detached' }; break;
      case 'session/switchNode': this.targets.get(socket)!.delete(id); this.loaded.delete(id); result = { type: 'session', session: { id, node_count: 1, active_node: request.params.node_id, active_conversation_id: `conv-${id}`, created_at: '0', updated_at: '0' } }; break;
      case 'turn/start': case 'turn/steer': result = { type: 'inbound_accepted', message_id: 'accepted-user', inbound_sequence: '1' }; break;
      case 'turn/cancel': result = { type: 'cancellation_accepted', attempt_id: 'attempt-A' }; break;
      case 'goal/control': {
        // Deterministic stand-in for GoalDomain CAS: stale refs refuse with the
        // native serialized rejection; only a successful mutation bumps revision.
        const next = structuredClone(this.snapshots.get(id)!);
        const control = request.params.control;
        const current = next.goal?.current;
        if (control.action !== 'mutate' || !next.goal || !current) throw new RpcFailure({ code: -32602, message: JSON.stringify({ reason: 'No current Goal', current: null }), data: { kind: 'invalid_params' } });
        if (current.reference.id !== control.expected.id || current.reference.revision !== control.expected.revision) {
          throw new RpcFailure({ code: -32602, message: JSON.stringify({ reason: 'Stale GoalRef; observe current state before trying again', current }), data: { kind: 'invalid_params' } });
        }
        const mutation = control.mutation;
        if (mutation.action === 'pause') { current.phase = 'paused'; }
        else if (mutation.action === 'resume') { current.phase = 'active'; current.blocked_reason = null; }
        else if (mutation.action === 'edit') current.objective = mutation.objective;
        else if (mutation.action === 'budget') {
          // GoalDomain, not the browser, owns the 1..=100 range and consumption floor.
          if (mutation.rounds < 1 || mutation.rounds > 100 || mutation.rounds < current.autonomous_rounds_consumed) {
            throw new RpcFailure({ code: -32602, message: JSON.stringify({ reason: 'Invalid Goal transition or value', current }), data: { kind: 'invalid_params' } });
          }
          current.autonomous_round_budget = mutation.rounds;
        }
        else throw new Error(`Fixture Goal mutation ${mutation.action} is a model declaration, not a Web control`);
        current.reference = { ...current.reference, revision: String(BigInt(current.reference.revision) + 1n) };
        this.snapshots.set(id, next); this.cursor++;
        result = { type: 'goal', view: next.goal }; break;
      }
      case 'interaction/respond': case 'interaction/cancel': {
        const next = structuredClone(this.snapshots.get(id)!);
        next.pending_interactions = next.pending_interactions?.filter(item => item.interaction.interaction_id !== request.params.interaction.interaction_id);
        this.snapshots.set(id, next); this.cursor++;
        result = { type: 'interaction_settled', interaction: request.params.interaction }; break;
      }
      default: throw new Error(`Fixture needs an explicit native result for ${request.method}`);
    }
    return result;
  }
  async update(id: string, next: RuntimeClientSnapshot) {
    this.snapshots.set(id, next); this.cursor++;
    this.socket.deliver({ jsonrpc: '2.0', method: 'session/event', params: { target: this.target(id), cursor: String(this.cursor), event: { type: 'attempt_started', attempt_id: 'attempt-A' } } });
    await this.client.refresh(id);
  }
}

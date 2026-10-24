import { selectedRevision, revisionSelector } from '../src/app/settings/projection';
import type { ProductHostWorkspaces } from '../src/workspaces/host';
import type { TraceDetail } from '../../protocol/app-server/v57';
import type { AttachmentTarget, MethodResult, Notification, Request, Response, RoutedInteraction, RuntimeClientSnapshot, SessionSummary, ServerCapabilities } from '../../protocol/app-server/v57';
import { fixtures } from '../../protocol/app-server/fixtures';
import { cfg3Source } from './cfg3-data';
import { AppServerClient } from '../src/client/app-server';
import type { Socket } from '../src/client/contracts';
import { RpcFailure } from '../src/client/errors';
import { sameTarget } from '../../protocol/app-server/attachment-identity';

export const TOKEN = 'fixture-transport-token-'.padEnd(43, 'x');
export const endpoint = 'ws://127.0.0.1:8080/';
const rustHello = fixtures.find(message => 'result' in message && message.result?.type === 'initialized');
if (!rustHello || !('result' in rustHello) || rustHello.result?.type !== 'initialized') throw new Error('Missing Rust-serialized initialize fixture');
const protocolVersion = rustHello.result.protocol_version;
export const capabilities: ServerCapabilities = rustHello.result.capabilities;
export function snapshot(id = 'A'): RuntimeClientSnapshot {
  return {
    settings_evidence: 'live_session', configuration_adoption_eligibility: { status: 'eligible' },
    conversation_id: `conversation-${id}`, shutting_down: false, effective_approval_mode: 'policy',
    workflows: { revision: '0', runs: [], omitted_runs: 0 }, messages: [], transcript: { entries: [] }, trace_updates: [], trace: { records: [] },
    message_descriptors:[], publication_settlements:[], inbound: {}, capabilities: { configured_tools: [], revision: '0' }, pending_interactions: [],
  };
}
export function checkpoint(value: RuntimeClientSnapshot, cursor = '0'): import('../../protocol/app-server/v57').RuntimeClientCheckpoint {
  const attempt=value.attempt;
  const content:import('../../protocol/app-server/v57').ContentBinding[]=[];
  if(attempt?.in_flight&&attempt.attempt_id!=null)for(const [index,block] of (attempt.in_flight.blocks??[]).entries()){
    const text='text' in block?block.text:'arguments' in block?block.arguments:undefined;
    if(typeof text!=='string'||!text.length)continue;
    const bytes=new TextEncoder().encode(text).length;
    const event=contentAdvance({attempt_id:attempt.attempt_id,message_id:attempt.in_flight.message_id,block_index:block.block_index,kind:block.type==='tool_call'?'tool_call_arguments':block.type,delta:text},0,Math.max(1,bytes),value.conversation_id);
    content.push({path:['attempt','in_flight','blocks',String(index),block.type==='tool_call'?'arguments':'text'],content_ref:event.content_ref,deferred:false});
  }
  return {snapshot:value,cursor,read_cut:{conversation_id:value.conversation_id,revision:cursor,through:cursor,domain_revisions:{},collection_cuts:{}},content,collections:[]};
}
export function boundedSettings(value:import('../../protocol/app-server/v57').SourceSettings):Extract<MethodResult,{type:'source_settings'}>['projection'] {
 return {target:value.target,revision:'fixture-native-source',inline:value,fields:[],collections:[]};
}
export function sourceReceipt(value: import('../../protocol/app-server/v57').SourceSettings, mutation?: import('../../protocol/app-server/v57').SourceMutation) {
  return {target:value.target,revision:selectedRevision(value, mutation ? revisionSelector(mutation) : {kind:'config'})};
}
export function interaction(type: 'approval' | 'questionnaire', id = 'A', interactionId = `interaction-${type}`): import('../../protocol/app-server/v57').RuntimeClientInteractionHeader {
  const routed:RoutedInteraction = { interaction: { conversation_id: `conversation-${id}`, interaction_id: interactionId }, source: { type: 'primary' },
    request: { id: interactionId, conversation_id: `conversation-${id}`, execution:{type:'attempt' as const,attempt_id:'attempt-A',turn:1},
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
  return {workflow_node:null,workflow_node_deferred:false,workflow_node_path:null,source_offset:0,interaction:routed.interaction,source:routed.source,request_id:routed.request.id,conversation_id:routed.request.conversation_id,execution:routed.request.execution,kind:routed.request.kind.type,approval:null,body:routed.request,body_complete:true};
}
export function childConversation(page: RuntimeClientSnapshot['transcript'], agentId='agent-worker', activationId='activation-a', conversationId='conversation-worker'): Extract<MethodResult,{type:'agent_observe'}> {
 return {type:'agent_observe',observation:{type:'checkpoint',agent_id:agentId,conversation_id:conversationId,activation_id:activationId,source:'durable_only',cursor:null,checkpoint:{...checkpoint({...snapshot(),conversation_id:conversationId,transcript:page}),read_cut:{...checkpoint({...snapshot(),conversation_id:conversationId,transcript:page}).read_cut,activation_id:activationId}}}};
}
type SubscribedSeed=Omit<Extract<MethodResult,{type:'subscribed'}>,'target'|'registration_id'> & Partial<Pick<Extract<MethodResult,{type:'subscribed'}>,'target'|'registration_id'>>;
type FixtureNotification=Exclude<Notification,{method:'session/event'|'session/resyncRequired'}> | {jsonrpc:'2.0',method:'session/event',params:Omit<Extract<Notification,{method:'session/event'}>['params'],'registration_id'> & {registration_id?:string}} | {jsonrpc:'2.0',method:'session/resyncRequired',params:Omit<Extract<Notification,{method:'session/resyncRequired'}>['params'],'registration_id'|'reason'|'checkpoint_required'> & {registration_id?:string,reason?:Extract<Notification,{method:'session/resyncRequired'}>['params']['reason'],checkpoint_required?:boolean}};
export function editorPage(locator:import('../../protocol/app-server/v57').EditorLocator,content:import('../../protocol/app-server/v57').UserInputBlock[],block_index:number,offset:number,max_bytes:number):Extract<MethodResult,{type:'editor_page'}> {
 const block=content[block_index],next_block=block_index+1<content.length?block_index+1:null;
 if(!block)return {type:'editor_page',page:{locator,block_index,next_block:null,chunk:null,upload:null}};
 if(block.type==='upload')return {type:'editor_page',page:{locator,block_index,next_block,chunk:null,upload:{session_id:block.session_id,batch_id:block.batch_id,token:block.token}}};
 const bytes=new TextEncoder().encode(block.text),end=Math.min(offset+max_bytes,bytes.length);
 if(offset>bytes.length)throw new Error('Invalid fixture editor range');
 const ref={conversation_id:'conv_01900000-0000-7000-8000-000000000002',source:{type:'canonical_message_block' as const,component:'payload' as const,locator:{type:'by_message_id' as const,message_id:`fixture-editor-${locator.node_id}`},block_index},representation:'utf8_text' as const,version:1,content_revision:1,readable_end:bytes.length};
 return {type:'editor_page',page:{locator,block_index,next_block,upload:null,chunk:{content_ref:ref,offset,data_base64:btoa(String.fromCharCode(...bytes.slice(offset,end))),end_offset:end,readable_end:bytes.length,next_offset:end<bytes.length?end:null,complete:end===bytes.length}}};
}
export class FakeSocket implements Socket {
  onopen: Socket['onopen'] = null;
  onmessage: Socket['onmessage'] = null;
  onclose: Socket['onclose'] = null;
  onerror: Socket['onerror'] = null;
  requests: Request[] = [];
  closed = false;
  readyState = 0;
  constructor(private handle: (request: Request, socket: FakeSocket) => void, private releaseClaims: () => void,private registration:(target:AttachmentTarget)=>string|undefined,private installRegistration:(target:AttachmentTarget)=>string) {}
  send(raw: string) { const request = JSON.parse(raw) as Request; this.requests.push(request); this.handle(request, this); }
  open() { this.readyState = 1; this.onopen?.(new Event('open')); }
  deliver(value: Response | FixtureNotification) {
    if ('method' in value && (value.method==='session/event'||value.method==='session/resyncRequired')) {
      const params={...value.params,registration_id:value.params.registration_id??this.registration(value.params.target)??'unregistered'};
      if(value.method==='session/resyncRequired') Object.assign(params,{reason:'replay_unavailable',checkpoint_required:true,...value.params});
      value={...value,params} as Notification;
    }
    this.onmessage?.(new MessageEvent('message', { data: JSON.stringify(value) }));
  }
  success(request: Request, result: MethodResult|SubscribedSeed) {
    if(result.type==='subscribed' && request.method==='session/subscribe') result={...result,target:request.params.target,registration_id:result.registration_id??this.installRegistration(request.params.target)};
    this.deliver({ jsonrpc: '2.0', id: request.id, result:result as MethodResult });
  }
  close() { if (!this.closed) { this.readyState = 3; this.closed = true; this.releaseClaims(); this.onclose?.(new CloseEvent('close')); } }
}
export class Server {
  readonly workspaceHost: ProductHostWorkspaces = {
    listWorkspaces: async () => ({ authorityId: 'fixture-host', endpoint, workspaces: [
      { id: 'workspace-a', displayName: 'Workspace A', location: '/workspace', displayPath: '/workspace' },
      { id: 'workspace-b', displayName: 'Workspace B', location: '/workspace/B', displayPath: '/workspace/B' },
    ], picker: { kind: 'configured', locations: [{ id: 'workspace-a', displayName: 'Workspace A' }, { id: 'workspace-b', displayName: 'Workspace B' }] } }),
    classifyLocations: async cwds => cwds.map(cwd => ['/workspace/A', '/workspace/B', '/workspace/child', '/workspace/created', '/workspace/fork-child'].includes(cwd) ? { authorized: true } : { authorized: false, reason: 'denied' }),
    resolveWorkspace: async () => { throw new Error('No test registration'); },
    adoptWorkspace: async () => {}, renameWorkspace: async () => {}, reorderWorkspace: async () => {}, removeWorkspace: async () => {},
  };

  authorityId = 'fixture-app-server-authority';
  handlers = new Map<Request['method'], (request: Request) => MethodResult>();
  sockets: FakeSocket[] = [];
  snapshots = new Map<string, RuntimeClientSnapshot>([['A', snapshot('A')], ['B', snapshot('B')]]);
  /** Explicit native catalog metadata; tests never derive preview in the browser. */
  summaries = new Map<string, Partial<SessionSummary>>();
  nodeSnapshots = new Map<string, RuntimeClientSnapshot>();
  cursor = 0n;
  private checkpointSources=new Map<string,{cursor:string;value:string}>();
  private attachmentSequence = 0;
  private registrationSequence = 0;
  readonly registrations = new Map<string,string>();
  private readonly editors=new Map<string,import("../../protocol/app-server/v57").UserInputBlock[]>();
  editor(session:{id:string,active_node:string,[key:string]:unknown},content:import("../../protocol/app-server/v57").UserInputBlock[]|null|undefined):import("../../protocol/app-server/v57").EditorLocator|null {
    if(content==null)return null;
    const locator={session_id:session.id,node_id:session.active_node,source_session:"A",source_node:"node-A",source_surface_revision:"1"};
    this.editors.set(JSON.stringify(locator),structuredClone(content));return locator;
  }
  private completed = new Map<Request, Response>();
  private reservations = new WeakMap<FakeSocket, Set<string>>();
  loaded = new Set<string>();
  residentConversations = new Map<string, string>();
  coldLoads = new Map<string, number>();
  maxClaims = 0;
  private targets = new WeakMap<FakeSocket, Map<string, AttachmentTarget>>();
  held = new Set<Request['method']>();
  requests: { request: Request; socket: FakeSocket }[] = [];
  private waiters: { method: Request['method']; count: number; resolve: (request: Request) => void }[] = [];
  version = protocolVersion;
  /** Record details this scenario staged, keyed by Trace record identity. */
  readonly traceDetails = new Map<string, TraceDetail>();
  capabilities = capabilities;
  socketFactory = (_url: string, protocols: string[]) => {
    if (protocols[0] !== 'rustx.app-server.v57' || protocols[1] !== `rustx-token.${TOKEN}`) throw new Error('Wrong browser admission protocol');
    const socket = new FakeSocket((request, source) => this.receive(request, source), () => { this.targets.get(socket)?.clear(); this.reservations.get(socket)?.clear(); },target=>this.registrations.get(JSON.stringify(target)),target=>{const id=`registration-${++this.registrationSequence}`;this.registrations.set(JSON.stringify(target),id);return id;}); this.sockets.push(socket);
    queueMicrotask(() => socket.open()); return socket;
  };
  carrierTransfers = 0;
  client = new AppServerClient(this.socketFactory, 30_000, async () => { this.carrierTransfers++; });
  constructor() { this.client.setSessionAdmission(async () => ({ current: () => true, validate: async () => true })); } // Protocol-only fixture; App installs real Host admission.
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
    return { ownership_generation: '1', id, cwd: `/workspace/${id}`, name: `Session ${id}`, updated_at: '2026-09-14T00:00:00Z', active_node: `node-${id}`, ...this.summaries.get(id) };
  }
  readonly uploads = new Map<string, Extract<MethodResult, { type: 'upload_status' }>['outcome']>();
  private execute(request: Request, socket: FakeSocket): MethodResult {
    const params = request.params;
    const id = 'target' in params && 'session_id' in params.target ? params.target.session_id : 'session_id' in params ? params.session_id : 'A';
    if (socket.closed || ('target' in params && 'session_id' in params.target && !sameTarget(this.targets.get(socket)?.get(id), params.target))) {
      throw new RpcFailure({ code: -32000, message: 'Stale attachment', data: { kind: 'stale_attachment' } });
    }
    if (request.method === 'session/uploadPrepare') {
      this.uploads.set(request.params.operation_id, { state: 'ready', files: request.params.files.map((file, index) => ({ receipt: { session_id: id, batch_id: request.params.operation_id, token: `file-${index}` }, file: { name: file.name, batch_id: request.params.operation_id }, path: `/workspace/${id}/${file.name}` })) });
      return { type: 'upload_prepared', transfer: { path: '/session-upload/' + 'a'.repeat(43), loopback_port: null, expires_in_seconds: 60 } };
    }
    if (request.method === 'session/uploadStatus' && !this.handlers.has(request.method)) return { type: 'upload_status', outcome: this.uploads.get(request.params.operation_id) ?? { state: 'absent' } };
    const handler = this.handlers.get(request.method);
    if (handler) return handler(request);
    let result: MethodResult;
    switch (request.method) {
      case 'initialize': result = { type: 'initialized', authority_id: this.authorityId, protocol_version: this.version, capabilities: this.capabilities }; break;
      case 'server/info': result = { type: 'server_info', capabilities: this.capabilities }; break;
      case 'session/summary': result = { type: 'session_summary', summary: this.summary(request.params.session_id) }; break;
      case 'session/list': if (request.params.limit > 32) throw new Error('Native Session page limit is 32'); result = { type: 'sessions', sessions: [...this.snapshots.keys()].map(id => this.summary(id)).sort((a, b) => Date.parse(b.updated_at) - Date.parse(a.updated_at) || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0)).filter(row => !request.params.query || [row.id, row.name, row.preview].some(text => text?.toLowerCase().includes(request.params.query!.toLowerCase()))).slice(request.params.offset, request.params.offset + request.params.limit) }; break;
      case 'session/read': result = { type: 'session', session: { id, active_node: this.summary(id).active_node, active_conversation_id: (this.nodeSnapshots.get(this.summary(id).active_node) ?? this.snapshots.get(id)!).conversation_id, node_count: 1, created_at: '0', updated_at: '0' } }; break;
      case 'session/node': {
        const nodeId = request.params.node_id;
        const history = this.nodeSnapshots.get(nodeId) ?? (nodeId === this.summary(id).active_node ? this.snapshots.get(id) : undefined);
        if (!history) throw new RpcFailure({ code: -32000, message: 'Unknown Node', data: { kind: 'unknown_node', session_id: id, node_id: nodeId } });
        result = { type: 'node', node: { id: nodeId, conversation_id: history.conversation_id, ordinal: '1', origin: { type: 'new' } } }; break;
      }
      case 'session/tree': {
        const nodes = [{ id: this.summary(id).active_node, conversation_id: (this.nodeSnapshots.get(this.summary(id).active_node) ?? this.snapshots.get(id)!).conversation_id }, ...[...this.nodeSnapshots].map(([id, snapshot]) => ({ id, conversation_id: snapshot.conversation_id }))];
        result = { type: 'tree', nodes: nodes.slice(request.params.offset, request.params.offset + request.params.limit).map((node, i) => ({ ...node, ordinal: String(i), origin: { type: 'new' }, created_at: '0' })), next_offset: null }; break;
      }
      case 'session/statistics': {
        const selected = this.nodeSnapshots.get(request.params.node_id ?? '') ?? this.snapshots.get(id)!;
        result = { type: 'session_statistics', conversation_id: selected.conversation_id,
          read_cut: checkpoint(selected).read_cut, occupancy: selected.context?.occupancy,
          statistics: selected.transcript.statistics ?? { turns: '0', steps: '0', completed_responses: '0', model_requests: '0', requests_with_usage: '0' } }; break;
      }
      case 'session/traceHistory': {const selected=this.nodeSnapshots.get(request.params.node_id??'')??this.snapshots.get(id)!;result={type:'session_trace_history',conversation_id:selected.conversation_id,page:{...selected.trace,read_cut:selected.trace.read_cut??checkpoint(selected).read_cut}};break;}
      case 'session/traceHistoryDetail': result = { type: 'session_trace_history_detail', conversation_id: (this.nodeSnapshots.get(request.params.node_id ?? '') ?? this.snapshots.get(id)!).conversation_id, page:traceReceipt(null).page }; break;
      case 'session/history': {
        const history = (request.params.node_id ? this.nodeSnapshots.get(request.params.node_id) : undefined) ?? this.snapshots.get(request.params.session_id)!;
        if (!history) throw new RpcFailure({ code: -32000, message: 'Unknown Session' });
        const at = request.params.at, entries = history.transcript.entries ?? [];
        const target = at.type === 'message' ? entries.find(entry => entry.item.type === 'message' && entry.item.message.id === at.id) : undefined;
        if (at.type === 'message' && !target) throw new RpcFailure({ code: -32000, message: 'Unknown Message', data: { kind: 'operation_failed' } });
        const selected = at.type === 'older' ? entries.filter(entry => BigInt(entry.cursor) < BigInt(at.before)).slice(-request.params.limit) : at.type === 'newer' ? entries.filter(entry => BigInt(entry.cursor) > BigInt(at.after)).slice(0, request.params.limit) : target ? entries.slice(Math.max(0, entries.indexOf(target) - Math.floor(request.params.limit / 2)), Math.max(0, entries.indexOf(target) - Math.floor(request.params.limit / 2)) + request.params.limit) : entries.slice(-request.params.limit);
        result = { type: 'session_history', conversation_id: history.conversation_id, window: { cut: 'cut' in at && at.cut ? at.cut : { conversation_id: history.conversation_id, journal: '1000', transcript: '1000', mutation_revision: '0' ,ledger_through:'0',surface_revision:'0',transcript_total:'0',pending_through:'0',pending_total:'0'}, target_cursor: target?.cursor, page: { ...history.transcript, entries: selected } } }; break;
      }
      case 'session/attach': {
        this.reservations.get(socket)?.delete(id);
        const attachedSnapshot = (request.params.node_id ? this.nodeSnapshots.get(request.params.node_id) : this.nodeSnapshots.get(this.summary(id).active_node)) ?? this.snapshots.get(id)!;
        if (this.loaded.has(id) && this.residentConversations.get(id) !== attachedSnapshot.conversation_id) throw new RpcFailure({ code: -32000, message: 'Session already resident in another Conversation', data: { kind: 'operation_failed' } });
        if (!this.loaded.has(id)) { this.loaded.add(id); this.residentConversations.set(id, attachedSnapshot.conversation_id); this.coldLoads.set(id, (this.coldLoads.get(id) ?? 0) + 1); }
        if (this.sockets.some(source => this.claims(source).some(target => target.session_id === id))) throw new RpcFailure({ code: -32000, message: 'Controller in use', data: { kind: 'controller_in_use' } });
        const targets = this.targets.get(socket) ?? new Map<string, AttachmentTarget>();
        targets.set(id, { session_id: id, conversation_id: attachedSnapshot.conversation_id, runtime_incarnation: String(9007199254740992n + BigInt(this.coldLoads.get(id)!)), attachment_id: `${id}-${++this.attachmentSequence}` });
        this.targets.set(socket, targets);
        this.maxClaims = Math.max(this.maxClaims, targets.size);
        result = {type:'attached', target: this.target(id, socket), configuration_revision:'0', attachment_kind:'controller', observation_source:'live'}; break;
      }
      case 'session/attachmentStatus': result={type:'attachment_status',target:this.targets.get(socket)?.get(request.params.session_id)??null,configuration_revision:'0'};break;
      case 'session/editorRead': return editorPage(request.params.locator,this.editors.get(JSON.stringify(request.params.locator))??[],request.params.block_index,Number(request.params.offset),request.params.max_bytes);
      case 'session/projections': result = { type: 'projections', page: { records: request.params.after === '0' && this.snapshots.get(request.params.target.session_id)?.context?.projection_head ? [this.snapshots.get(request.params.target.session_id)!.context!.projection_head!] : [], next_activation: null } }; break;
      case 'session/configuration': result = { type: 'session_configuration', projection:(null) }; break;
      case 'configuration/sourcesRead': result = { type: 'source_settings', projection:boundedSettings(cfg3Source()) }; break;
      case 'session/turns': result = { type: 'conversation_turns', page: { cut: { conversation_id: this.target(id, socket).conversation_id, journal: '0', transcript: '0', mutation_revision: '0' ,ledger_through:'0',surface_revision:'0',transcript_total:'0',pending_through:'0',pending_total:'0'}, offset: 0, total: "0", turns: [] } }; break;
      case 'session/settings': result = settingsReceipt({ cwd: `/workspace/${id}` },'0'); break;
      case 'session/checkpoint': {
        const source=this.snapshots.get(id)?.conversation_id===request.params.target.conversation_id?this.snapshots.get(id)!:[...this.nodeSnapshots.values()].find(value=>value.conversation_id===request.params.target.conversation_id)!;
        this.checkpointSources.set(source.conversation_id,{cursor:String(this.cursor),value:JSON.stringify(source)});
        result={type:'checkpoint',checkpoint:checkpoint(source,String(this.cursor))};break;
      }
      case 'session/subscribe': {
        const captured=this.checkpointSources.get(request.params.target.conversation_id);
        const source=this.snapshots.get(id)?.conversation_id===request.params.target.conversation_id?this.snapshots.get(id):[...this.nodeSnapshots.values()].find(value=>value.conversation_id===request.params.target.conversation_id);
        // This fixture has no native replay journal. A direct source rewrite
        // requires explicit recovery rather than pretending events were applied.
        if(captured&&captured.cursor===request.params.after_cursor&&captured.value!==JSON.stringify(source))throw new RpcFailure({code:-32000,message:'Native replay unavailable in this fixture',data:{kind:'resync_required'}});
        result = { type: 'subscribed', target:request.params.target, registration_id:`registration-${++this.registrationSequence}`, after_cursor: request.params.after_cursor }; this.registrations.set(JSON.stringify(request.params.target),result.registration_id); break; }
      // Inspection detail is served per record, so the fixture answers from
      // the details its scenario staged and reports absence otherwise.
      case 'session/traceDetail': result = traceReceipt(this.traceDetails.get(request.params.record_id) ?? null); break;
      case 'session/detach': this.targets.get(socket)!.delete(id); result = { type: 'detached' }; break;
      case 'session/switchNode': {
        this.targets.get(socket)!.delete(id);
        const conversation = (this.nodeSnapshots.get(request.params.node_id) ?? this.snapshots.get(id)!).conversation_id;
        this.loaded.add(id); this.residentConversations.set(id, conversation);
        this.coldLoads.set(id, (this.coldLoads.get(id) ?? 0) + 1);
        this.summaries.set(id, { ...this.summaries.get(id), active_node: request.params.node_id });
        result = { type: 'session', session: { id, node_count: 1, active_node: request.params.node_id, active_conversation_id: conversation, created_at: '0', updated_at: '0' } }; break;
      }
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
        result = request.params.control.action==='show'?domainReceipt('goal',[next.goal]):goalReceipt(next.goal); break;
      }
      case 'interaction/respond': case 'interaction/cancel': {
        const next = structuredClone(this.snapshots.get(id)!);
        next.pending_interactions = next.pending_interactions?.filter(item => item.interaction.interaction_id !== request.params.interaction.interaction_id);
        this.snapshots.set(id, next); this.cursor++;
        result = { type: 'interaction_settled', interaction: request.params.interaction }; break;
      }
      case 'agent/statistics': return { type: 'agent_statistics', metrics: { statistics: { turns: '0', steps: '0', completed_responses: '0', model_requests: '0', requests_with_usage: '0' }, occupancy: null, duration: { settled_ms: '0', active: null } } };
      case 'agent/observeCancel': return {type:'agent_observe_cancelled',accepted:true};
      case 'agent/observe': result = childConversation({entries:[]},request.params.agent_id); break;
      case 'agent/turns': return { type: 'conversation_turns', page: { cut: { conversation_id: this.snapshots.get(id)?.agents?.find(agent => agent.agent_id === request.params.agent_id)?.child_conversation_id ?? 'conversation-worker', journal: '0', transcript: '0', mutation_revision: '0' ,ledger_through:'0',surface_revision:'0',transcript_total:'0',pending_through:'0',pending_total:'0'}, offset: 0, total: "0", turns: [] } };
      case 'agent/transcript': return { type: 'transcript_window', window: { cut: { conversation_id: this.snapshots.get(id)?.agents?.find(agent => agent.agent_id === request.params.agent_id)?.child_conversation_id ?? 'conversation-worker', journal: '0', transcript: '0', mutation_revision: '0' ,ledger_through:'0',surface_revision:'0',transcript_total:'0',pending_through:'0',pending_total:'0'}, page: { entries: [] } } };
      case 'session/models': return domainReceipt('model_catalog.models',({ models: [] }).models??[]);
      default: throw new Error(`Fixture needs an explicit native result for ${request.method}`);
    }
    return result;
  }
  /** The native post-commit Session metadata invalidation (Issue #386).
   * Carries no metadata and no attachment target: the client must reread. */
  invalidateSummary(id: string, socket = this.socket, catalog_changed = false) {
    socket.deliver({ jsonrpc: '2.0', method: 'session/summaryInvalidated', params: { session_id: id, catalog_changed } });
  }
  /** A real read-domain append preserves the currently admitted Attempt identity/phase. */
  durableUpdate(id: string, next: RuntimeClientSnapshot) {
    this.snapshots.set(id,next);this.cursor++;
    this.socket.deliver({jsonrpc:'2.0',method:'session/event',params:{target:this.target(id),cursor:String(this.cursor),event:{type:'read_domains_updated',transcript:next.transcript,trace:next.trace,todos:next.todos}}});
  }
  async update(id: string, next: RuntimeClientSnapshot) {
    this.snapshots.set(id, next); this.cursor++;
    // This synthetic native owner publishes the requested finite state as one
    // complete transition. Do not fabricate an AttemptStarted fact and force
    // client recovery for every ordinary observation update.
    this.socket.deliver({ jsonrpc: '2.0', method: 'session/event', params: { target: this.target(id), cursor: String(this.cursor), event: { type: 'checkpoint_advanced', transition: 'fixture-native-cut', checkpoint: checkpoint(next, String(this.cursor)) } } });
  }
}

export function readingWindow(page: import('../../protocol/app-server/v57').RuntimeClientTranscriptPage) { return { cut: { conversation_id: 'conversation-A', journal: '1000', transcript: '1000', mutation_revision: '0' ,ledger_through:'0',surface_revision:'0',transcript_total:'0',pending_through:'0',pending_total:'0'}, page }; }

export const utf8Length = (text: string) => new TextEncoder().encode(text).length;
export function contentAdvance(input: {attempt_id:string;message_id:string;block_index:number;position?:number;kind:'text'|'reasoning'|'refusal'|'tool_call_arguments';delta:string}, base=0, revision=1, conversation=snapshot().conversation_id): Extract<import('../../protocol/app-server/v57').RuntimeClientEvent,{type:'block_content_advanced'}> {
 const end=base+utf8Length(input.delta);
 return {type:'block_content_advanced',position:input.position??input.block_index,attempt_id:input.attempt_id,message_id:input.message_id,block_index:input.block_index,kind:input.kind,base_length:String(base),end_length:String(end),content_revision:String(revision),inline_delta:input.delta,
   content_ref:{conversation_id:conversation,source:{type:'live_publication_prefix',component:'payload',stream_id:`stream-${input.attempt_id}`,message_id:input.message_id,attempt_id:input.attempt_id,turn_id:`turn-${input.attempt_id}`,publication_request_id:`request-${input.attempt_id}`,block_index:input.block_index},representation:input.kind==='tool_call_arguments'?'raw_json_utf8':'utf8_text',version:1,content_revision:revision,readable_end:end}};
}

export function settingsReceipt(settings: import('../../protocol/app-server/v57').SessionPersistentState,revision='0'): Extract<import('../../protocol/app-server/v57').MethodResult,{type:'settings'}> {return {type:'settings',projection:{revision,cwd:settings.cwd,inline:settings,reference:null}};}

export function domainReceipt(domain:string,items:unknown[],options:{total?:number;next?:number|null}={}):Extract<import('../../protocol/app-server/v57').MethodResult,{type:'domain_page'}>{const cut=checkpoint(snapshot()).read_cut;return {type:'domain_page',page:{domain,cursor:'0',read_cut:cut,total:String(options.total??items.length),items:items.map((value,index)=>({offset:String(index),identity:String(index),value,content:[],deferred:false})),next_offset:options.next==null?null:String(options.next)}};}
export function jobReceipt(job:Pick<import('../../protocol/app-server/v57').RuntimeClientJob,'job_id'|'state'> & Partial<Omit<import('../../protocol/app-server/v57').RuntimeClientJob,'job_id'|'state'>>):Extract<import('../../protocol/app-server/v57').MethodResult,{type:'job_outcome'}>{return {type:'job_outcome',job_id:job.job_id,state:job.state};}
export function goalReceipt(view:import('../../protocol/app-server/v57').RuntimeClientGoalView):Extract<import('../../protocol/app-server/v57').MethodResult,{type:'goal_mutation'}>{return {type:'goal_mutation',reference:view.current?.reference??null};}
export function compactReceipt(context:Partial<import('../../protocol/app-server/v57').RuntimeClientContextView>):Extract<import('../../protocol/app-server/v57').MethodResult,{type:'context_compacted'}>{return {type:'context_compacted',compaction:context.latest_compaction??null};}

export function readCut(conversation_id=snapshot().conversation_id):import('../../protocol/app-server/v57').ConversationReadCut {return {conversation_id,journal:'0',transcript:'0',mutation_revision:'0',ledger_through:'0',surface_revision:'0',transcript_total:'0',pending_through:'0',pending_total:'0'};}

export function applicationReceipt(value:import('../../protocol/app-server/v57').ConfigurationApplication):import('../../protocol/app-server/v57').BoundedSessionApplication;
export function applicationReceipt(value:null|undefined):null;
export function applicationReceipt(value:import('../../protocol/app-server/v57').ConfigurationApplication|null|undefined):import('../../protocol/app-server/v57').BoundedSessionApplication|null;
export function applicationReceipt(value:import('../../protocol/app-server/v57').ConfigurationApplication|null|undefined):import('../../protocol/app-server/v57').BoundedSessionApplication|null{return value==null?null:{scope:value.scope,sources:value.sources,version:value.version,inline:value,reference:null};}

export function traceReceipt(detail:TraceDetail|null|undefined){return domainReceipt('trace.records.1.detail',detail==null?[]:[detail]);}
/** Authoritative projection read-domain fixture, separate from all canonical rows. */
export function projectionHead(text = '## Preserved context\n\nNative summary facts.', inherited = false): import('../../protocol/app-server/v57').RuntimeClientProjectionHeadView {
  return {checkpoint_id:'checkpoint-1',activation_id:'1',transcript_anchor:'2',base_frontier:'100',activation_ledger:'100',
    origin:{conversation_id:'conv_36524fd8-f674-7fc2-8125-06d01fee0e18',activation:'1',completed_journal:'200'},
    destination_installation:inherited?'installation:fixture':null,original_items:1,derived_items:1,
    summary_preview:{text,truncated:false}};
}

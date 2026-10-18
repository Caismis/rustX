/**
 * Scripted App Server peers for the deterministic client suites.
 *
 * Two doubles, at two different boundaries:
 *
 * ```text
 * FakeTransport        an AppServerTransport, with no bytes at all
 *                      -> proves client semantics: correlation, routing,
 *                         fencing, settlement, no-replay
 *
 * ScriptedAppServer    real JSONL over real pipes
 *                      -> proves the stdio transport: framing, EOF, stderr
 * ```
 *
 * A test drives either one by writing exact protocol messages, so ordering is
 * decided by the script rather than by timing. Nothing here sleeps, and no
 * test in this package proves an ordering with a timer.
 *
 * Neither double is a runtime. Neither holds conversation state, and neither
 * decides any semantics: they answer exactly what a test tells them to answer.
 */

import { PassThrough } from "node:stream";

import { decodeProtocolMessage } from "../../src/protocol/decoder.ts";
import { encodeRecord } from "../../src/protocol/jsonl.ts";
import {
  BaseTransport,
  TransportClosedError,
  type TransportCloseReason,
} from "../../src/app-server/transport.ts";
import type {
  AttachmentTarget,
  MethodName,
  MethodParams,
  MethodResult,
  Notification,
  Request,
  AppServerRequestId,
  RpcError,
  RuntimeClientCursor,
  RuntimeClientEvent,
} from "../../src/protocol/app-server.ts";

type SubscribedSeed=Omit<Extract<MethodResult,{type:'subscribed'}>,'target'|'registration_id'> & Partial<Pick<Extract<MethodResult,{type:'subscribed'}>,'target'|'registration_id'>>;
const registrations=new Map<string,string>();
let nextRegistration=0;
export function registrationId(target:AttachmentTarget):string{return registrations.get(JSON.stringify(target))??"unregistered";}
function receipt(id:AppServerRequestId,result:MethodResult|SubscribedSeed,log:RequestLog):MethodResult {
 if(result.type!=='subscribed') return result;
 const request=log.matching('session/subscribe').find(row=>row.id===id);
 if(!request || request.method!=='session/subscribe') throw new Error('Subscription requires its exact request owner');
 const registration_id=result.registration_id??`fixture-registration-${++nextRegistration}`;
 registrations.set(JSON.stringify(request.params.target),registration_id);
 return {...result,target:result.target??request.params.target,registration_id};
}
/** One request the client wrote, already decoded. */
export type ObservedRequest = Request;

/** The shared "what did the client ask for" bookkeeping of both doubles. */
class RequestLog {
  readonly #requests: ObservedRequest[] = [];
  readonly #waiters: Array<() => void> = [];

  get requests(): readonly ObservedRequest[] {
    return this.#requests;
  }

  /** Every request for one method, in write order. */
  matching(method: string): readonly ObservedRequest[] {
    return this.#requests.filter((request) => request.method === method);
  }

  /** How many times the client asked for one method. */
  count(method: string): number {
    return this.matching(method).length;
  }

  record(request: ObservedRequest): void {
    this.#requests.push(request);
    for (const waiter of this.#waiters.splice(0)) {
      waiter();
    }
  }

  /**
   * Resolves once at least `count` requests have been written.
   *
   * This is a data barrier, not a delay: the promise settles on the write
   * itself, so a test never guesses how long a write takes.
   */
  async awaitRequests(count: number): Promise<readonly ObservedRequest[]> {
    while (this.#requests.length < count) {
      await new Promise<void>((resolve) => this.#waiters.push(resolve));
    }
    return this.#requests;
  }

  /** Resolves once the client has issued `count` requests for one method. */
  async awaitMethod(
    method: string,
    count = 1,
  ): Promise<readonly ObservedRequest[]> {
    while (this.count(method) < count) {
      await new Promise<void>((resolve) => this.#waiters.push(resolve));
    }
    return this.matching(method);
  }
}

/**
 * An `AppServerTransport` with no bytes.
 *
 * Everything below the typed client is removed, so a test that exercises
 * correlation, routing or settlement cannot accidentally be testing framing.
 */
export class FakeTransport extends BaseTransport {
  readonly log = new RequestLog();
  readonly answered = new Set<AppServerRequestId>();
  async nextPending(method:MethodName):Promise<ObservedRequest>{
    for(;;){const pending=this.log.matching(method).find(row=>!this.answered.has(row.id));if(pending)return pending;await this.log.awaitMethod(method,this.log.count(method)+1);}
  }
  #disposed = false;
  #editors=new Map<string,import('../../src/protocol/app-server.ts').UserInputBlock[]>();
  editor(session:{id:string,active_node:string},content:import('../../src/protocol/app-server.ts').UserInputBlock[]|null|undefined) {
    if(content==null)return null;
    const locator={session_id:session.id,node_id:session.active_node,source_session:session.id,source_node:session.active_node,source_surface_revision:'1'};
    this.#editors.set(JSON.stringify(locator),content);return locator;
  }

  override describe(): string {
    return "fake";
  }

  /** How many requests for one method the client has written. */
  transportCount(method: string): number {
    return this.log.count(method);
  }

  /** Whether the transport released its resources. */
  get disposed(): boolean {
    return this.#disposed;
  }

  protected override writeMessage(message: unknown): Promise<void> {
    const request=message as ObservedRequest;
    this.log.record(request);
    if(request.method==='session/editorRead')queueMicrotask(()=>{
      const {locator,block_index,max_bytes}=request.params;const offset=Number(request.params.offset);
      const content=this.#editors.get(JSON.stringify(locator))??[],block=content[block_index];
      const next_block=block_index+1<content.length?block_index+1:null;
      if(!block){this.respond(request.id,{type:'editor_page',page:{locator,block_index,next_block:null,upload:null,chunk:null}});return;}
      if(block.type==='upload'){this.respond(request.id,{type:'editor_page',page:{locator,block_index,next_block,upload:{session_id:block.session_id,batch_id:block.batch_id,token:block.token},chunk:null}});return;}
      const data=Buffer.from(block.text,'utf8'),end=Math.min(offset+max_bytes,data.length);
      this.respond(request.id,{type:'editor_page',page:{locator,block_index,next_block,upload:null,chunk:{content_ref:{conversation_id:'conv_01900000-0000-7000-8000-000000000002',source:{type:'canonical_message_block',component:'payload',locator:{type:"by_message_id",message_id:'editor-fixture'},block_index},representation:'utf8_text',version:1,content_revision:1,readable_end:data.length},offset,data_base64:data.subarray(offset,end).toString('base64'),end_offset:end,readable_end:data.length,next_offset:end<data.length?end:null,complete:end===data.length}}});
    });
    return Promise.resolve();
  }

  protected override disposeTransport(): void {
    this.#disposed = true;
  }

  /** Delivers one protocol message to the client. */
  override deliver(record: unknown): void {
    super.deliver(record);
  }

  /** Answers one request with a typed result. */
  respond(id: AppServerRequestId, result: MethodResult|SubscribedSeed): void {
    this.answered.add(id);
    const record={jsonrpc:"2.0",id,result:receipt(id,result,this.log)};
    if(!decodeProtocolMessage(record))throw new Error(`Invalid scripted result ${JSON.stringify(record).slice(0,4096)}`);
    this.deliver(record);
  }

  /** Answers one request with a typed protocol failure. */
  respondError(id: AppServerRequestId, error: RpcError): void {
    this.answered.add(id);
    this.deliver({ jsonrpc: "2.0", id, error });
  }

  /** Publishes one routed observation. */
  emit(
    target: AttachmentTarget,
    cursor: RuntimeClientCursor,
    event: RuntimeClientEvent,
  ): void {
    this.deliver(notification("session/event", { target, cursor, event }));
  }

  /** Publishes one routed notification of any method. */
  notify(notificationMessage: Notification): void {
    this.deliver(notificationMessage);
  }

  /** Ends the transport as a terminal failure of the given kind. */
  fail(reason: TransportCloseReason, message = "scripted transport failure"): void {
    this.terminate(new TransportClosedError(reason, message));
  }
}

/**
 * The typed `params` of one observed request.
 *
 * `Request` is a union over the whole method vocabulary, so reading a
 * method-specific field needs the discriminator. Asserting it here keeps every
 * call site honest about which method it is inspecting.
 */
export function paramsOf<M extends MethodName>(
  request: ObservedRequest,
  method: M,
): MethodParams<M> {
  if (request.method !== method) {
    throw new Error(`expected a ${method} request, observed ${request.method}`);
  }
  return request.params as MethodParams<M>;
}

/** Builds one well-formed notification envelope. */
export function notification<M extends Notification["method"]>(
  method: M,
  params: M extends 'session/event' ? Omit<Extract<Notification,{method:'session/event'}>['params'],'registration_id'> & {registration_id?:string} : M extends 'session/resyncRequired' ? Omit<Extract<Notification,{method:'session/resyncRequired'}>['params'],'registration_id'|'reason'|'checkpoint_required'> & {registration_id?:string,reason?:Extract<Notification,{method:'session/resyncRequired'}>['params']['reason'],checkpoint_required?:boolean} : Extract<Notification,{method:M}>['params'],
): Notification {
  if(method==='session/event'||method==='session/resyncRequired') {
    const value=params as Extract<Notification,{method:'session/resyncRequired'}>['params'];
    return {jsonrpc:'2.0',method,params:{...(method==='session/resyncRequired'?{reason:'replay_unavailable',checkpoint_required:true}:{}),...value,registration_id:value.registration_id??registrations.get(JSON.stringify(value.target))??'unregistered'}} as Notification;
  }
  return { jsonrpc: "2.0", method, params } as Notification;
}

/**
 * A scripted App Server speaking real JSONL over real pipes.
 *
 * This is the double for the *transport* boundary: framing, record splitting,
 * EOF and stderr separation are all genuinely exercised.
 */
export class ScriptedAppServer {
  /** Bytes the App Server would write; the client reads this. */
  readonly serverOutput = new PassThrough();
  /** Bytes the client writes; the peer reads this. */
  readonly clientOutput = new PassThrough();
  readonly log = new RequestLog();
  #pendingBytes = "";

  constructor() {
    this.clientOutput.on("data", (chunk: Buffer) => {
      this.#pendingBytes += chunk.toString("utf8");
      for (;;) {
        const lf = this.#pendingBytes.indexOf("\n");
        if (lf === -1) {
          break;
        }
        const record = this.#pendingBytes.slice(0, lf);
        this.#pendingBytes = this.#pendingBytes.slice(lf + 1);
        this.log.record(JSON.parse(record) as ObservedRequest);
      }
    });
  }

  get requests(): readonly ObservedRequest[] {
    return this.log.requests;
  }

  awaitRequests(count: number): Promise<readonly ObservedRequest[]> {
    return this.log.awaitRequests(count);
  }

  awaitMethod(method: string, count = 1): Promise<readonly ObservedRequest[]> {
    return this.log.awaitMethod(method, count);
  }

  /** Writes one correlated success response. */
  respond(id: AppServerRequestId, result: MethodResult|SubscribedSeed): void {
    this.writeRecord({ jsonrpc: "2.0", id, result:receipt(id,result,this.log) });
  }

  /** Writes one correlated typed protocol error. */
  respondError(id: AppServerRequestId, error: RpcError): void {
    this.writeRecord({ jsonrpc: "2.0", id, error });
  }

  /** Writes one routed observation. */
  emit(
    target: AttachmentTarget,
    cursor: RuntimeClientCursor,
    event: RuntimeClientEvent,
  ): void {
    this.writeRecord(notification("session/event", { target, cursor, event }));
  }

  /** Writes one already-shaped protocol record. */
  writeRecord(record: unknown): void {
    this.serverOutput.write(encodeRecord(record));
  }

  /** Writes exact bytes, for framing-level tests. */
  writeRaw(bytes: string | Buffer): void {
    this.serverOutput.write(
      typeof bytes === "string" ? Buffer.from(bytes, "utf8") : bytes,
    );
  }

  /** Ends the server output stream: transport EOF. */
  endOutput(): void {
    this.serverOutput.end();
  }
}

/**
 * Waits until a condition holds, bounded by an outer liveness deadline.
 *
 * The condition is re-checked as soon as the event loop can run it, so this
 * returns the moment the awaited fact is observable — it is never a fixed
 * sleep. The deadline exists only so a genuine hang fails the test instead of
 * stalling it.
 */
export async function until(
  condition: () => boolean,
  what = "condition",
  timeoutMs = 30_000,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!condition()) {
    if (Date.now() >= deadline) {
      throw new Error(`${what} never became true within ${timeoutMs}ms`);
    }
    await new Promise<void>((resolve) => {
      setTimeout(resolve, 1);
    });
  }
}

/** Yields once, so a microtask-scheduled continuation can run. */
export function tick(): Promise<void> {
  return new Promise<void>((resolve) => setImmediate(resolve));
}

/** Answer one admitted attachment's separate read and registration boundaries. */
export async function followAttachment(transport:FakeTransport, target:AttachmentTarget, initial:import('../../src/protocol/app-server.ts').RuntimeClientSnapshot, cursor:RuntimeClientCursor) {
 const readCount=transport.log.count('session/checkpoint')+1;
 const read=(await transport.log.awaitMethod('session/checkpoint',readCount)).at(-1)!;
 const subscriptions=transport.log.count('session/subscribe')+1;
 transport.respond(read.id,{type:'checkpoint',checkpoint:{snapshot:{...initial,conversation_id:target.conversation_id},cursor,read_cut:{conversation_id:target.conversation_id,revision:cursor,through:cursor,domain_revisions:{},collection_cuts:{}},content:[],collections:[]}});
 const subscription=(await transport.log.awaitMethod('session/subscribe',subscriptions)).at(-1)!;
 const configurations=transport.log.count('session/configuration')+1;
 transport.respond(subscription.id,{type:'subscribed',target,after_cursor:cursor});
 const configuration=(await transport.log.awaitMethod('session/configuration',configurations)).at(-1)!;
 transport.respond(configuration.id,{type:'session_configuration',projection:(null)});
}

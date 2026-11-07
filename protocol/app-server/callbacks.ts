/** Live server-request correlation shared by the existing product clients.
 * A handler is explicitly installed by full native identity. Ordinary clients
 * ignore Host Tools they do not implement; they never reply tool-not-found.
 */
import type { AppServerRequestId, HostToolCall, HostToolResponse, InteractionRef, InteractionResponse, ServerRequest } from './v74.ts';

type Call = ServerRequest;
export type HostToolHandler = (call: HostToolCall, signal: AbortSignal) => Promise<HostToolResponse>;
const key = (ref: InteractionRef) => JSON.stringify([ref.conversation_id, ref.interaction_id]);
const toolKey = (namespace: string | null | undefined, name: string) => JSON.stringify([namespace ?? null, name]);

const object = (value: unknown): value is Record<string, unknown> => value !== null && typeof value === 'object' && !Array.isArray(value);
const validId = (value: unknown): value is AppServerRequestId => typeof value === 'number' ? Number.isSafeInteger(value) : typeof value === 'string' && value.length > 0 && new TextEncoder().encode(JSON.stringify(value)).length <= 256;
const conversation = (value: unknown): value is string => typeof value === 'string' && value.length > 0 && value.length <= 256;

export class ServerCallbacks {
  private pending = new Map<AppServerRequestId, { call: Call; controller: AbortController }>();
  private interactions = new Map<string, AppServerRequestId>();
  private handlers = new Map<string, HostToolHandler>();
  private listeners = new Set<() => void>();
  private readonly send: (message: unknown) => Promise<void>;
  constructor(send: (message: unknown) => Promise<void>) { this.send = send; }

  installHandler(namespace: string | undefined, name: string, handler: HostToolHandler) {
    const identity = toolKey(namespace, name);
    if (this.handlers.has(identity)) throw Error('Host Tool handler already installed.');
    this.handlers.set(identity, handler);
    return () => { if (this.handlers.get(identity) === handler) this.handlers.delete(identity); };
  }
  onChange(listener: () => void) { this.listeners.add(listener); return () => this.listeners.delete(listener); }
  private changed() { for (const listener of this.listeners) listener(); }

  receive(record: unknown): boolean {
    if (!object(record) || !('method' in record)) return false;
    if (record.method === 'serverRequest/resolved') {
      const params = record.params;
      if (record.jsonrpc !== '2.0' || !object(params) || !validId(params.requestId) || !conversation(params.threadId)) throw Error('Invalid server-request resolution.');
      const pending = this.pending.get(params.requestId);
      if (pending && pending.call.params.threadId !== params.threadId) throw Error('Mismatched server-request resolution.');
      pending?.controller.abort(); this.pending.delete(params.requestId);
      if (pending?.call.method === 'item/interaction/request') this.interactions.delete(key(pending.call.params.interaction.interaction));
      this.changed(); return true;
    }
    if (record.method !== 'item/tool/call' && record.method !== 'item/interaction/request') return false;
    if (record.jsonrpc !== '2.0' || !validId(record.id) || !object(record.params) || !conversation(record.params.threadId)) throw Error('Invalid server request.');
    const params = record.params;
    if (record.method === 'item/tool/call') {
      if (typeof params.turnId !== 'string' || typeof params.callId !== 'string' || typeof params.tool !== 'string' || !(params.namespace == null || typeof params.namespace === 'string') || !('arguments' in params)) throw Error('Invalid Host Tool call.');
    } else {
      if (!object(params.interaction) || !object(params.interaction.interaction) || !conversation(params.interaction.interaction.conversation_id) || typeof params.interaction.interaction.interaction_id !== 'string') throw Error('Invalid interaction callback.');
    }
    const call = record as unknown as Call;
    if (this.pending.has(call.id)) return true; // live replay retains invocation identity
    if (this.pending.size >= 128) throw Error('Server-request capacity exceeded.');
    const controller = new AbortController();
    this.pending.set(call.id, { call, controller });
    if (call.method === 'item/interaction/request') {
      if (!call.params.interaction?.interaction) throw Error('Invalid interaction callback.');
      this.interactions.set(key(call.params.interaction.interaction), call.id); this.changed();
    } else {
      const handler = this.handlers.get(toolKey(call.params.namespace, call.params.tool));
      if (handler) {
        const entry = this.pending.get(call.id);
        const handlerCall = structuredClone(call.params);
        void Promise.resolve().then(() => {
          if (this.pending.get(call.id) !== entry || controller.signal.aborted) return;
          return handler(handlerCall, controller.signal);
        }).then(
          result => { if (this.pending.get(call.id) === entry) return this.send({ jsonrpc:'2.0', id:call.id, result: result ?? null }); },
          () => { if (this.pending.get(call.id) === entry) return this.send({ jsonrpc:'2.0', id:call.id, error:{code:-32000,message:'Host Tool handler failed'} }); },
        ).catch(() => {}); // connection owner reports transport failure
      }
    }
    return true;
  }

  canAnswer(interaction: InteractionRef) { return this.interactions.has(key(interaction)); }
  async answer(interaction: InteractionRef, response?: InteractionResponse) {
    const id = this.interactions.get(key(interaction));
    if (id === undefined || !this.pending.has(id)) throw Error('Server request is no longer accepting responses.');
    await this.send(response === undefined
      ? { jsonrpc:'2.0', id, error:{code:-32000,message:'Interaction declined'} }
      : { jsonrpc:'2.0', id, result:response });
    // Sending is not native settlement. Only resolution or a fresh read can
    // establish absence; never replay an uncertain response on reconnect.
  }
  clear() {
    for (const pending of this.pending.values()) pending.controller.abort();
    this.pending.clear(); this.interactions.clear(); this.changed();
  }
}

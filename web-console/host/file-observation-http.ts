import type { ServerResponse } from 'node:http';
import { WorkspaceHostError, type ProductHostWorkspaces, type WorkspaceAuthorityScope } from '../src/workspaces/host.ts';
import { NativeFileReadError, validateDeliveryObservation } from './file-read.ts';
import type { WorkbenchCall } from '../src/workspaces/workbench.ts';
import type { DeliveryRead } from '../src/workspaces/host.ts';
import type { SessionFileReference } from '../../protocol/app-server/v55.ts';
type Operation = { id: string; channel: string; scope: WorkspaceAuthorityScope; abort: AbortController };
type Event = { kind: 'changed' | 'ended'; operationId: string };
type Channel = { scope: WorkspaceAuthorityScope; send(event: Event): void };
interface Input { channelId?: unknown; operationId?: unknown; call?: WorkbenchCall; read?: DeliveryRead; file?: SessionFileReference }
const sameScope = (a: WorkspaceAuthorityScope, b: WorkspaceAuthorityScope) => a.authorityId === b.authorityId && a.endpoint === b.endpoint;
const id = (value: unknown): value is string => typeof value === 'string' && /^[a-f0-9-]{36}$/.test(value);
/** One browser stream multiplexes bounded target hints. A target slot stays
 * owned until its Host observation actually ends, including initial admission. */
export class FileObservationHttp {
  private readonly operations = new Map<string, Operation>();
  private readonly channels = new Map<string, Channel>();
  private readonly host: ProductHostWorkspaces | undefined;
  constructor(host: ProductHostWorkspaces | undefined) { this.host = host; }
  async handle(method: string, body: Input, scope: WorkspaceAuthorityScope, response: ServerResponse): Promise<boolean> {
    if (method === 'file-observe-events') {
      if (Object.keys(body).some(key => !['scope', 'channelId'].includes(key)) || !id(body.channelId) || this.channels.has(body.channelId)) throw new Error('Invalid observation channel');
      const catalog = await this.host?.listWorkspaces();
      if (!catalog || !sameScope(catalog, scope)) throw new WorkspaceHostError('Workspace Host authority replaced', 'authority_replaced');
      if (this.channels.has(body.channelId)) throw new Error('Observation channel already admitted');
      if (this.channels.size >= 32) throw new WorkspaceHostError('File observation capacity reached', 'capacity');
      const channelId = body.channelId;
      let blocked = false;
      const pending = new Map<string, Event>();
      const send = (event: Event) => {
        if (response.destroyed) return;
        if (blocked) {
          if (!pending.has(event.operationId) && pending.size >= 32) { response.destroy(); return; }
          pending.set(event.operationId, event); return;
        }
        blocked = !response.write(JSON.stringify(event) + '\n');
      };
      const channel: Channel = { scope, send }; this.channels.set(channelId, channel);
      response.writeHead(200, { 'Content-Type': 'application/x-ndjson', 'Cache-Control': 'no-store', 'X-Rustx-Observation-Channel': channelId }); response.flushHeaders();
      response.write('{"kind":"connected"}\n');
      const drain = () => { blocked = false; for (const [key, event] of pending) { pending.delete(key); send(event); if (blocked) break; } };
      response.on('drain', drain);
      response.once('close', () => {
        response.off('drain', drain); this.channels.delete(channelId); pending.clear();
        for (const operation of this.operations.values()) if (operation.channel === channelId) operation.abort.abort();
      });
      return true;
    }
    if (method === 'file-observe-cancel') {
      if (Object.keys(body).some(key => !['scope', 'operationId'].includes(key)) || !id(body.operationId)) throw new Error('Invalid observation cancellation');
      const operation = this.operations.get(body.operationId);
      if (operation && sameScope(scope, operation.scope)) operation.abort.abort();
      response.writeHead(200, { 'Content-Type': 'application/json' }).end('null'); return true;
    }
    if (method !== 'file-observe') return false;
    if (Object.keys(body).some(key => !['scope', 'read', 'file', 'call', 'operationId', 'channelId'].includes(key)) || !id(body.operationId) || !id(body.channelId)
      || (body.call ? !this.host?.observeWorkspaceFile || body.read !== undefined || body.file !== undefined : !this.host?.observeDelivery)) throw new Error('Invalid file observation');
    if (body.call === undefined) validateDeliveryObservation(body.read, body.file);
    else if (!body.call || body.call.request?.kind !== 'preview') throw new Error('Invalid file observation');
    const channel = this.channels.get(body.channelId);
    if (!channel || !sameScope(channel.scope, scope)) throw new Error('Observation channel unavailable');
    if (this.operations.size >= 32 || this.operations.has(body.operationId)) throw new WorkspaceHostError('File observation capacity reached', 'capacity');
    const operation: Operation = { id: body.operationId, channel: body.channelId, scope, abort: new AbortController() };
    this.operations.set(operation.id, operation);
    const changed = () => { if (!operation.abort.signal.aborted) channel.send({ kind: 'changed', operationId: operation.id }); };
    const retire = () => { this.operations.delete(operation.id); channel.send({ kind: 'ended', operationId: operation.id }); };
    const closed = () => { if (!response.writableFinished) operation.abort.abort(); }; response.on('close', closed);
    response.writeHead(200, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store', 'X-Rustx-Observation-Operation': operation.id }); response.flushHeaders();
    try {
      const observation = body.call ? await this.host!.observeWorkspaceFile!(scope, body.call, changed, operation.abort.signal)
        : await this.host!.observeDelivery!(scope, body.read!, body.file!, changed, operation.abort.signal);
      response.end(JSON.stringify({ ok: true, version: observation.version }));
      void observation.done.then(retire, retire);
    } catch (error) {
      response.end(JSON.stringify({ ok: false, message: String(error), kind: error instanceof WorkspaceHostError ? error.kind : undefined, nativeError: error instanceof NativeFileReadError ? error.error : undefined }));
      retire();
    } finally { response.off('close', closed); }
    return true;
  }
}

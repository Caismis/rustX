import type { Page } from '@playwright/test';
import type { AttachmentTarget, CheckpointPage, EffectiveFieldRef, MethodResult, TraceDetail } from '../../../protocol/app-server/v57';
import { domainOriginalValue } from '../../../protocol/app-server/domain-observation';
import { sameTarget } from '../../../protocol/app-server/attachment-identity';

type NativeRead = 'session/checkpoint' | 'session/effectiveConfiguration';
type NativeReadResult<M extends NativeRead> = Extract<MethodResult, { type: M extends 'session/checkpoint' ? 'checkpoint' : 'effective_configuration' }>;

/** Observe the real socket; hold replies or lose a committed acknowledgement.
 * No snapshots, notifications, or semantic results are manufactured here. */
export async function wireProbe(page: Page) {
  const requests: { id: string | number; method: string; params: any }[] = [];
  const responses: { method: string; result: any }[] = [];
  const notifications: { method: string; params: any }[] = [];
  let lose: string | undefined;
  let lost = 0;
  let nextRead = 0;
  let hold = new Set<string>();
  const held: { method: string; send: () => void }[] = [];
  let currentAttachedTarget: ((sessionId: string) => Promise<AttachmentTarget>) | undefined;
  let nativeRead: ((method: NativeRead | 'session/effectiveConfigurationValueRead' | 'conversation/checkpointValueRead', params: object) => Promise<MethodResult>) | undefined;
  await page.routeWebSocket(/^wss?:\/\/[^/]+\/?$/, socket => {
    const server = socket.connectToServer();
    const methods = new Map<string | number, string>();
    type AttachmentObservation = {
      requestId?: string | number;
      target?: AttachmentTarget;
      error?: Error;
      waiters: { resolve: (target: AttachmentTarget) => void; reject: (error: Error) => void }[];
    };
    const attachments = new Map<string, AttachmentObservation>();
    const attaching = new Map<string | number, string>();
    const reads = new Map<string, { resolve: (result: MethodResult) => void; reject: (error: Error) => void }>();
    let retired = false;
    const retire = () => {
      retired = true;
      for (const read of reads.values()) read.reject(new Error('Native read transport retired.'));
      reads.clear();
      for (const observation of attachments.values()) for (const waiter of observation.waiters) waiter.reject(new Error('Attached-target transport retired.'));
      attachments.clear();
    };
    currentAttachedTarget = sessionId => {
      if (retired) return Promise.reject(new Error('Attached target requires the current browser transport.'));
      const observation = attachments.get(sessionId) ?? { waiters: [] };
      attachments.set(sessionId, observation);
      if (observation.error) return Promise.reject(observation.error);
      if (observation.target) return Promise.resolve(observation.target);
      return new Promise((resolve, reject) => observation.waiters.push({ resolve, reject }));
    };
    nativeRead = (method, params) => {
      if (retired) return Promise.reject(new Error('Native read requires the current browser transport.'));
      const id = `acceptance-read-${++nextRead}`;
      return new Promise((resolve, reject) => {
        reads.set(id, { resolve, reject });
        server.send(JSON.stringify({ jsonrpc: '2.0', id, method, params }));
      });
    };
    socket.onClose((code, reason) => { retire(); server.close({ code, reason }); });
    server.onClose((code, reason) => { retire(); socket.close({ code, reason }); });
    socket.onMessage(message => {
      if (retired) return;
      const request = JSON.parse(String(message));
      if (request.method && request.id !== undefined) {
        requests.push(request); methods.set(request.id, request.method);
        // A replacement claim must publish its own real ACK before direct reads.
        if (request.method === 'session/attach') {
          const sessionId: string = request.params.session_id;
          const previous = attachments.get(sessionId);
          attachments.set(sessionId, { requestId: request.id, waiters: previous?.waiters ?? [] });
          attaching.set(request.id, sessionId);
        }
      }
      server.send(message);
    });
    server.onMessage(message => {
      if (retired) return;
      const response = JSON.parse(String(message));
      const read = reads.get(response.id);
      if (read) {
        reads.delete(response.id);
        if (response.error) read.reject(new Error(`Native read rejected: ${JSON.stringify(response.error)}`));
        else read.resolve(response.result);
        return;
      }
      if (response.method) notifications.push(response);
      const method = methods.get(response.id);
      const attachedSession = attaching.get(response.id); attaching.delete(response.id);
      const deliver = () => {
        if (retired) return;
        socket.send(message);
        const observation = attachedSession ? attachments.get(attachedSession) : undefined;
        // A late held ACK still reaches the real browser, but only the newest
        // request of this Session may satisfy its current-target barrier.
        if (observation && observation.requestId === response.id) {
          if (response.result?.type === 'attached') {
            observation.target = response.result.target;
            for (const waiter of observation.waiters) waiter.resolve(observation.target!);
          } else if (response.error) {
            observation.error = new Error(`Native attach rejected: ${JSON.stringify(response.error)}`);
            for (const waiter of observation.waiters) waiter.reject(observation.error);
          }
          observation.waiters = [];
        }
        if (response.method === 'session/closed') {
          const target: AttachmentTarget = response.params.target;
          const closed = attachments.get(target.session_id);
          if (sameTarget(closed?.target, target)) {
            closed!.target = undefined;
            closed!.error = new Error('Attached target closed on the current browser transport.');
            for (const waiter of closed!.waiters) waiter.reject(closed!.error);
            closed!.waiters = [];
          }
        }
      };
      if (method) {
        methods.delete(response.id);
        responses.push({ method, result: response.result });
        if (hold.delete(method)) {
          held.push({ method, send: deliver });
          return;
        }
        if (method === lose && response.result) {
          lose = undefined; lost++;
          retire();
          // Closing forces immediate uncertainty; no wall-clock request timeout.
          socket.close({ code: 1011, reason: 'Acceptance: committed acknowledgement lost' });
          server.close();
          return;
        }
      }
      deliver();
    });
  });
  return { requests, responses, notifications,
    /** A real Attach ACK delivered on the current physical browser transport.
     * Historical responses remain available for assertions and request counts.
     * Held/lost replies and replies from retired sockets cannot satisfy this barrier. */
    attachedTarget(sessionId: string) {
      if (!currentAttachedTarget) return Promise.reject(new Error('Attached target requires a browser transport.'));
      return currentAttachedTarget(sessionId);
    },
    loseNext: (method: string) => { lose = method; }, lost: () => lost,
    holdNext: (...methods: string[]) => { hold = new Set(methods); }, held: () => held.length,
    releaseHeld: (method?: string) => {
      const index = method === undefined ? 0 : held.findIndex(reply => reply.method === method);
      if (index >= 0) held.splice(index, 1)[0]?.send();
    },
    /** Existing native read RPCs use the browser's one controller connection.
     * Probe IDs never enter its client; no authority or result is manufactured. */
    async read<M extends NativeRead>(method: M, target: AttachmentTarget): Promise<NativeReadResult<M>> {
      if (!nativeRead) throw new Error('Native read requires a browser transport.');
      const result = await nativeRead(method, { target });
      const expected = method === 'session/checkpoint' ? 'checkpoint' : 'effective_configuration';
      if (result?.type !== expected) throw new Error(`Unexpected native read result: ${JSON.stringify(result)}`);
      return result as NativeReadResult<M>;
    },
    async effectiveValueRead(target: AttachmentTarget, reference: EffectiveFieldRef, offset: number) {
      if (!nativeRead) throw new Error('Native read requires a browser transport.');
      const result = await nativeRead('session/effectiveConfigurationValueRead', { target, reference, offset, max_bytes: 65_536 });
      if (result.type !== 'configuration_value_chunk') throw new Error(`Unexpected native field result: ${JSON.stringify(result)}`);
      return result.chunk;
    },
    async traceDetail(target: AttachmentTarget, page: CheckpointPage) {
      return domainOriginalValue<TraceDetail>(page, {
        current: () => true,
        page: async () => { throw new Error('An exact Trace detail must contain one native item.'); },
        value: async (cut, item, offset) => {
          if (!nativeRead) throw new Error('Native read requires a browser transport.');
          const result = await nativeRead('conversation/checkpointValueRead', {
            target, agent_id: null, cut: cut.read_cut, domain: cut.domain,
            item_offset: Number(item.offset), path: [], offset, max_bytes: 65_536,
          });
          if (result.type !== 'checkpoint_value_chunk') throw new Error(`Unexpected native Trace range result: ${JSON.stringify(result)}`);
          return result.chunk;
        },
      });
    },
  };
}

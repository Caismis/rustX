import { carrierFetch } from '../carrier/http';
import { WorkspaceHostError, type WorkspaceAuthorityScope } from './host';
interface Record { changed(): void; finish(): void; done: Promise<void>; ended: boolean }
/** A single hint stream avoids the browser's HTTP/1.1 per-origin connection
 * ceiling. Initial probes still have individual settled HTTP acknowledgements. */
export class FileObservationFeed {
  readonly id = crypto.randomUUID();
  readonly records = new Map<string, Record>();
  private readonly abort = new AbortController();
  private lost = false;
  readonly ready: Promise<void>;
  constructor(base: string, readonly scope: WorkspaceAuthorityScope, private readonly empty: () => void) {
    let accept!: () => void, fail!: (cause: unknown) => void;
    this.ready = new Promise<void>((yes, no) => { accept = yes; fail = no; });
    void (async () => {
      try {
        const response = await carrierFetch(`${base}/file-observe-events`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ scope, channelId: this.id }), signal: this.abort.signal });
        if (!response.ok || response.headers.get('X-Rustx-Observation-Channel') !== this.id || !response.body) throw Error('Observation channel unavailable');
        const reader = response.body.getReader(), decoder = new TextDecoder('utf-8', { fatal: true }); let buffer = '', connected = false;
        try {
          while (true) {
            const chunk = await reader.read(); if (chunk.done) break;
            buffer += decoder.decode(chunk.value, { stream: true });
            if (buffer.length > 65536) throw Error('Invalid observation frame');
            let end: number;
            while ((end = buffer.indexOf('\n')) >= 0) {
              const event = JSON.parse(buffer.slice(0, end)); buffer = buffer.slice(end + 1);
              if (!connected && event.kind === 'connected') { connected = true; accept(); continue; }
              if (!connected || !['changed', 'ended'].includes(event.kind) || typeof event.operationId !== 'string') throw Error('Invalid observation frame');
              const record = this.records.get(event.operationId);
              if (!record) continue;
              if (event.kind === 'changed') record.changed();
              else { record.ended = true; record.finish(); this.release(event.operationId); }
            }
          }
        } finally { reader.releaseLock(); }
      } catch { /* Lost transport is not a physical target-retirement witness. */ }
      finally {
        this.lost = true; fail(new Error('File observation unavailable'));
        for (const record of this.records.values()) record.finish();
        this.records.clear(); this.empty();
      }
    })();
  }
  add(operationId: string, changed: () => void) {
    let finish!: () => void; const done = new Promise<void>(resolve => { finish = resolve; });
    const record = { changed, finish, done, ended: false }; this.records.set(operationId, record); return record;
  }
  release(operationId: string) { this.records.delete(operationId); if (!this.records.size) { this.abort.abort(); this.empty(); } }
  assertRetired(record: Record) { if (this.lost && !record.ended) throw new WorkspaceHostError('File observation settlement unavailable', 'file_settlement_unknown'); }
}

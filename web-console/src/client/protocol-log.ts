import type { Request1 } from '../../../protocol/app-server/v18';

export interface WireContext { method?: Request1['method']; sessionId?: string }
export interface WireEntry {
  sequence: number;
  generation: number;
  direction: 'in' | 'out';
  kind: 'request' | 'response' | 'notification' | 'invalid';
  method?: string;
  sessionId?: string;
  json: string;
  truncated: boolean;
}
export interface LogView { entries: readonly WireEntry[]; dropped: number; truncated: number; paused: boolean }

/** A finite wire observer. Pause freezes a bounded view, never protocol processing. */
export class ProtocolLog {
  private entries: WireEntry[] = [];
  private bytes = 0;
  private sequence = 0;
  private dropped = 0;
  private truncated = 0;
  private paused = false;
  private listeners = new Set<() => void>();
  private view: LogView = { entries: [], dropped: 0, truncated: 0, paused: false };
  constructor(readonly maxEntries = 300, readonly maxBytes = 1_048_576, readonly maxEntryBytes = 32_768) {}
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  getSnapshot = () => this.view;
  observe(direction: WireEntry['direction'], generation: number, raw: string, context: WireContext = {}) {
    let kind: WireEntry['kind'] = 'invalid';
    let method: string | undefined = context.method;
    let sessionId = context.sessionId;
    try {
      const envelope = JSON.parse(raw);
      if (typeof envelope === 'object' && envelope !== null) {
        kind = 'id' in envelope ? ('method' in envelope ? 'request' : 'response') : 'notification';
        method = envelope.method ?? method;
        if (method === 'session/upload' && Array.isArray(envelope.params?.files)) for (const file of envelope.params.files) file.data = '[upload bytes omitted]';
        if (method === 'artifact/read' && envelope.result?.data) envelope.result.data = '[artifact bytes omitted]';
        if (method === 'session/upload' || method?.startsWith('artifact/')) raw = JSON.stringify(envelope);
        sessionId = envelope.params?.target?.session_id ?? envelope.params?.session_id ?? sessionId
          ?? envelope.result?.target?.session_id ?? envelope.result?.session?.id ?? envelope.result?.result?.session_id;
      }
    } catch { /* Keep malformed wire text observable before rejecting it. */ }
    // UTF-16 storage has a conservative 2-byte/code-unit bound. No handshake
    // headers, endpoint credentials, or browser token ever enter this observer.
    const limit = Math.min(this.maxEntryBytes, this.maxBytes);
    const truncated = raw.length * 2 > limit;
    const json = truncated ? raw.slice(0, Math.floor(limit / 2)) : raw;
    if (truncated) this.truncated++;
    this.entries.push({ sequence: ++this.sequence, generation, direction, kind, method, sessionId, json, truncated });
    this.bytes += json.length * 2;
    while (this.entries.length > this.maxEntries || this.bytes > this.maxBytes) {
      this.bytes -= this.entries.shift()!.json.length * 2;
      this.dropped++;
    }
    if (!this.paused) this.publish();
  }
  pause(value: boolean) { this.paused = value; this.publish(value); }
  clear() { this.entries = []; this.bytes = 0; this.dropped = 0; this.truncated = 0; this.publish(false); }
  private publish(freeze = this.paused) {
    this.view = { entries: freeze ? this.view.entries : [...this.entries], dropped: this.dropped, truncated: this.truncated, paused: this.paused };
    for (const listener of this.listeners) listener();
  }
}
export function filterLog(view: LogView, method: string, session: string, kind = '') {
  return view.entries.filter(entry => (!method || entry.method?.includes(method)) &&
    (!session || entry.sessionId?.includes(session)) && (!kind || entry.kind === kind));
}

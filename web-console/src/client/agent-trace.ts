import type { TraceToolLocator } from '../../../protocol/app-server/v44';
import type { AppServerClient } from './app-server';
import type { Observation } from './session-lifecycle/port';
import { TRACE_LIMIT, TRACE_PAGE_SIZE, beginTraceDetail, completeTraceDetail, prependTrace, refreshTrace, replaceTrace, selectTrace, traceInterests, type TraceCache } from './trace';

/** One child Conversation's finite, read-only Trace domain. Parent attachment
 * is the access proof, not the identity of the records being inspected. */
export class AgentTraceReader {
  private cache: TraceCache = { ...replaceTrace({ records: [], next_cursor: null }), loading: true };
  private listeners = new Set<() => void>();
  private live = true;
  private busy = false;
  private dirty = false;
  private paging?: object;
  private navigation = {};
  constructor(private client: AppServerClient, private sessionId: string, private proof: Observation, private agentId: string) {}
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  snapshot = () => this.cache;
  private current = () => this.live && this.client.isAttachmentObservationCurrent(this.sessionId, this.proof);
  private publish(cache: TraceCache) { this.cache = cache; this.listeners.forEach(listener => listener()); }
  retire() { this.live = false; this.listeners.clear(); }
  select = (id?: string) => this.publish(selectTrace(this.cache, id));
  refresh = async () => {
    this.dirty = true;
    if (this.busy || !this.current()) return;
    this.busy = true;
    try {
      while (this.dirty && this.current()) {
        this.dirty = false;
        const navigation = this.navigation;
        try {
          const result = await this.client.request({ method: 'agent/trace', params: { target: this.proof.target, agent_id: this.agentId, limit: TRACE_PAGE_SIZE, records: traceInterests(this.cache) } }, 'trace', undefined, this.current);
          if (this.current() && this.navigation === navigation) {
            const next = refreshTrace(this.cache, result.page, result.page.updates);
            if (next.epoch !== this.cache.epoch) this.paging = undefined;
            this.publish({ ...next, error: undefined, loading: !!this.paging });
          }
        } catch (cause) {
          if (this.current() && this.navigation === navigation) this.publish({ ...this.cache, error: String(cause), loading: !!this.paging });
        }
        // A newer navigation/activity request still owns its queued refresh,
        // even when the superseded read failed.
      }
    } finally { this.busy = false; }
  };
  locate = async (locator: TraceToolLocator): Promise<boolean> => {
    if (!this.current()) return false;
    const navigation = this.navigation = {};
    this.paging = undefined;
    // Retire pending paging/detail replies and their loading markers.
    this.publish({ ...this.cache, epoch: this.cache.epoch + 1, details: {}, loading: false });
    const current = () => this.current() && this.navigation === navigation;
    const result = await this.client.request({ method: 'agent/traceLocateTool', params: { target: this.proof.target, agent_id: this.agentId, locator } }, 'trace_tool_location', undefined, current);
    if (!current()) return false;
    if (!result.location) throw new Error('This tool occurrence has no native execution record.');
    const cache = replaceTrace(result.location.page, this.cache);
    const repaired = refreshTrace({ ...cache, located: true }, result.location.page, result.location.page.updates);
    this.publish(selectTrace(repaired, result.location.record_id));
    return true;
  };
  latest = async () => {
    if (!this.current()) return;
    this.navigation = {}; this.paging = undefined;
    this.publish(replaceTrace({ records: [], next_cursor: null }, this.cache));
    await this.refresh();
  };
  earlier = async () => {
    const cache = this.cache, limit = Math.min(TRACE_PAGE_SIZE, TRACE_LIMIT - cache.page.records.length);
    if (!this.current() || cache.loading || !cache.page.next_cursor || limit < 1) return;
    const operation = {};
    const current = () => this.current() && this.cache.epoch === cache.epoch && this.paging === operation;
    this.paging = operation;
    this.publish({ ...cache, loading: true, error: undefined });
    try {
      const result = await this.client.request({ method: 'agent/trace', params: { target: this.proof.target, agent_id: this.agentId, before: cache.page.next_cursor, limit } }, 'trace', undefined, current);
      if (current()) { this.paging = undefined; this.publish(prependTrace(this.cache, result.page)); await this.refresh(); }
    } catch (cause) { if (current()) this.publish({ ...this.cache, loading: false, error: String(cause) }); }
    finally { if (this.paging === operation) this.paging = undefined; }
  };
  detail = async (id: string) => {
    if (!this.current() || this.cache.details[id]?.loading || this.cache.details[id]?.detail) return;
    const pending = beginTraceDetail(this.cache, id), epoch = pending.epoch;
    const current = () => this.current() && this.cache.epoch === epoch && this.cache.details[id] === pending.details[id];
    this.publish(pending);
    try {
      const result = await this.client.request({ method: 'agent/traceDetail', params: { target: this.proof.target, agent_id: this.agentId, record_id: id } }, 'trace_detail', undefined, current);
      if (current()) this.publish(completeTraceDetail(this.cache, id, epoch, result.detail ?? undefined));
    } catch (cause) { if (current()) this.publish(completeTraceDetail(this.cache, id, epoch, undefined, String(cause))); }
  };
}

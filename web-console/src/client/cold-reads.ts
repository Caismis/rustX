import { transcriptWindowReady } from './history-policy';
import { HISTORY_PAGE_SIZE, prependTranscript, replaceTranscript, type TranscriptCache } from './transcript';
import type { ClientView, ConnectionRequest, OperationAdmission, SessionView } from './contracts';
import type { SessionReads } from './session-reads';

/** What cold durable reads read from and write to their client. */
export interface ColdReadPort {
  request: ConnectionRequest;
  state(): Readonly<ClientView>;
  current(generation: number): boolean;
  epoch(id: string): number | undefined;
  admit(id: string, current: () => boolean): Promise<false | OperationAdmission>;
  readSettings: SessionReads['readSettings'];
  setSession(id: string, patch: Partial<Pick<SessionView, 'preview' | 'statisticsPreview' | 'settings'>>): void;
}

/** Read-only durable history of a Session that is still opening, or whose
 * activation was refused for capacity. One observation scope is fixed to the
 * exact native Node and Conversation, so a late read can never replace a live
 * view or an execution snapshot. DSH history is readable without activation. */
export class ColdReads {
  constructor(private readonly port: ColdReadPort) {}
  /** One observation scope for all cold durable reads. The Conversation is
   * exact native Node evidence, never inferred from whichever response arrives first. */
  scope(id: string, admitted: () => boolean = () => true) {
    const view = this.port.state().views[id];
    if (!view?.nodeId || !view.nodeConversationId) return;
    const node = view.nodeId, conversation = view.nodeConversationId;
    const generation = this.port.state().generation, epoch = this.port.epoch(id), revision = view.attachmentIntentRevision ?? 0;
    const current = () => {
      const latest = this.port.state().views[id];
      const passive = !!latest?.passiveHistory && !latest.target && latest.attachment !== 'attached' && latest.attachment !== 'attaching';
      return admitted() && this.port.current(generation) && this.port.epoch(id) === epoch
        && latest?.attachmentIntent === 'wanted' && (latest.attachment === 'attaching' || passive)
        && (latest.attachmentIntentRevision ?? 0) === revision
        && latest.nodeId === node && latest.nodeConversationId === conversation;
    };
    return { node, conversation, current };
  }
  /** Durable history after activation was refused. Uses the existing Session
   * admission and the same stale fence; it never retries a mutation. */
  async readPassiveHistory(id: string) {
    const scope = this.scope(id); if (!scope?.current()) return;
    const { node, conversation, current } = scope;
    let admission: false | OperationAdmission = false;
    const allowed = () => !!admission && admission.current() && current();
    const publish = (history: NonNullable<SessionView['preview']>['history']) => {
      if (allowed()) this.port.setSession(id, { preview: { conversationId: conversation, history } });
    };
    const settle = () => {
      const latest = this.port.state().views[id]?.preview;
      if (current() && latest?.conversationId === conversation && latest.history.loading) this.port.setSession(id, { preview: { conversationId: conversation, history: { ...latest.history, loading: false, initialLoading: false } } });
    };
    try {
      admission = await this.port.admit(id, current);
      if (!admission || !allowed()) return;
      let cache: TranscriptCache | undefined = this.port.state().views[id]?.preview?.conversationId === conversation ? this.port.state().views[id]!.preview!.history : undefined;
      if (!cache) {
        const result = await this.port.request({ method: 'session/history', params: { session_id: id, node_id: node, at: { type: 'latest' }, limit: HISTORY_PAGE_SIZE } }, 'session_history', undefined, admission);
        if (!allowed() || result.conversation_id !== conversation || result.window.cut.conversation_id !== conversation) return;
        cache = { ...replaceTranscript(result.window.page), loading: true, initialLoading: true };
        publish(cache);
      } else {
        cache = { ...cache, loading: true, initialLoading: true, error: undefined };
        publish(cache);
      }
      while (allowed() && cache.page.next_cursor != null && !transcriptWindowReady(cache.page.entries ?? [])) {
        const before: string = cache.page.next_cursor;
        const result = await this.port.request({ method: 'session/history', params: { session_id: id, node_id: node, at: { type: 'older', before }, limit: HISTORY_PAGE_SIZE } }, 'session_history', undefined, admission);
        if (!allowed()) return;
        if (result.conversation_id !== conversation || result.window.cut.conversation_id !== conversation || result.window.page.next_cursor != null && BigInt(result.window.page.next_cursor) >= BigInt(before)) throw new Error('Invalid history page.');
        cache = { ...prependTranscript(cache, result.window.page), loading: true, initialLoading: true };
        publish(cache);
      }
      if (allowed()) publish({ ...cache, loading: false, initialLoading: false });
    } catch (error) {
      if (!current()) return;
      const latest = this.port.state().views[id]?.preview;
      const history = latest?.conversationId === conversation ? { ...latest.history, loading: false, initialLoading: false, error: String(error) }
        : { page: { entries: [], next_cursor: null }, epoch: 1, loading: false, initialLoading: false, error: String(error) };
      this.port.setSession(id, { preview: { conversationId: conversation, history } });
    } finally { settle(); }
  }
  async readHistoryPreview(id: string, admitted: () => boolean) {
    const scope = this.scope(id, admitted); if (!scope) return;
    const { node, conversation, current } = scope;
    try {
      const result = await this.port.request({ method: 'session/history', params: { session_id: id, node_id: node, at: { type: 'latest' }, limit: HISTORY_PAGE_SIZE } }, 'session_history', undefined, current);
      if (current() && result.conversation_id === conversation && result.window.cut.conversation_id === conversation) this.port.setSession(id, { preview: { conversationId: conversation, history: replaceTranscript(result.window.page) } });
    } catch { /* Attachment recovery owns errors; a late read cannot replace the live view. */ }
  }
  async readMetadata(id: string, admitted: () => boolean) {
    const scope = this.scope(id, admitted); if (!scope) return;
    const { node, conversation, current } = scope;
    await Promise.allSettled([
      this.port.request({ method: 'session/statistics', params: { session_id: id, node_id: node } }, 'session_statistics', undefined, current)
        .then(result => { if (current() && result.conversation_id === conversation && result.read_cut.conversation_id === conversation) this.port.setSession(id, { statisticsPreview: { conversationId: result.conversation_id, nodeId: node, cut: result.read_cut, statistics: result.statistics, occupancy: result.occupancy } }); }),
      this.port.readSettings(id, current)
        .then(result => { if (current()) this.port.setSession(id, { settings: result.settings }); }),
    ]);
  }
  async loadEarlierPreview(id: string): Promise<void> {
    const view = this.port.state().views[id], preview = view?.preview, before = preview?.history.page.next_cursor;
    if (!preview || preview.history.loading || before == null || view.attachment !== 'attaching' && !view.passiveHistory) return;
    const scope = this.scope(id); if (!scope || preview.conversationId !== scope.conversation) return;
    const epoch = preview.history.epoch, node = scope.node;
    const current = () => scope.current() && this.port.state().views[id]?.preview?.conversationId === scope.conversation
      && this.port.state().views[id]?.preview?.history.epoch === epoch;
    this.port.setSession(id, { preview: { ...preview, history: { ...preview.history, loading: true, error: undefined } } });
    try {
      const admission = await this.port.admit(id, current);
      if (!admission) return;
      const result = await this.port.request({ method: 'session/history', params: { session_id: id, node_id: node, at: { type: 'older', before }, limit: HISTORY_PAGE_SIZE } }, 'session_history', undefined, admission);
      if (!current()) return;
      if (result.conversation_id !== preview.conversationId || result.window.cut.conversation_id !== preview.conversationId || result.window.page.next_cursor != null && BigInt(result.window.page.next_cursor) >= BigInt(before)) throw new Error('Invalid history page.');
      this.port.setSession(id, { preview: { ...preview, history: prependTranscript(preview.history, result.window.page) } });
    } catch (error) {
      if (current()) this.port.setSession(id, { preview: { ...preview, history: { ...preview.history, error: String(error) } } });
    } finally {
      const latest = this.port.state().views[id]?.preview;
      if (current() && latest) this.port.setSession(id, { preview: { ...latest, history: { ...latest.history, loading: false } } });
    }
  }
}

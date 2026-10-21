import { HISTORY_PAGE_SIZE, prependTranscript, replaceTranscript } from './transcript';
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

/** Read-only durable history of a Session whose attachment is still opening:
 * one observation scope fixed to the exact native Node and Conversation, so a
 * late read can never replace a live view or an execution snapshot. */
export class ColdReads {
  constructor(private readonly port: ColdReadPort) {}
  /** One observation scope for all cold durable reads. The Conversation is
   * exact native Node evidence, never inferred from whichever response arrives first. */
  scope(id: string, admitted: () => boolean = () => true) {
    const view = this.port.state().views[id];
    if (!view?.nodeId || !view.nodeConversationId) return;
    const node = view.nodeId, conversation = view.nodeConversationId;
    const generation = this.port.state().generation, epoch = this.port.epoch(id), revision = view.attachmentIntentRevision ?? 0;
    const current = () => admitted() && this.port.current(generation) && this.port.epoch(id) === epoch
      && this.port.state().views[id]?.attachment === 'attaching' && this.port.state().views[id]?.attachmentIntent === 'wanted'
      && (this.port.state().views[id]?.attachmentIntentRevision ?? 0) === revision
      && this.port.state().views[id]?.nodeId === node && this.port.state().views[id]?.nodeConversationId === conversation;
    return { node, conversation, current };
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
    if (!preview || preview.history.loading || before == null || view.attachment !== 'attaching') return;
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

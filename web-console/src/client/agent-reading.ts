import type { AttachmentTarget, ConversationTurn, ConversationTurnPage, ConversationWindowAt, RuntimeClientSnapshot } from '../../../protocol/app-server/v44';
import type { AppServerClient } from './app-server';
import { extendTranscriptWindow, HISTORY_PAGE_SIZE, installTranscriptWindow, refreshTranscript, replaceTranscript, sameReadCut, turnKey, type TranscriptCache } from './transcript';

export interface AgentReadingState {
  history?: TranscriptCache;
  outline?: ConversationTurnPage;
  outlineLoading?: boolean;
  outlineError?: string;
  pending?: string;
  navigationError?: string;
}

/** One disposable child reading surface. The parent attachment grants reads;
 * native child cuts/identities select history across all its activations. */
export class AgentReading {
  private state: AgentReadingState = {};
  private listeners = new Set<() => void>();
  private closed = false;
  private intent = 0;
  private outlineIntent = 0;
  private live?: RuntimeClientSnapshot;
  private liveRevision?: string;
  private outlineOffset?: number;
  private outlineWork?: Promise<ConversationTurnPage | undefined>;
  private refreshPending = false;
  constructor(private client: AppServerClient, private target: AttachmentTarget, private agentId: string, private conversationId: string, private owned: () => boolean) {}
  getSnapshot = () => this.state;
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  private current = () => !this.closed && this.owned();
  private publish(patch: Partial<AgentReadingState>) {
    if (!this.current()) return;
    this.state = { ...this.state, ...patch }; this.listeners.forEach(listener => listener());
  }
  dispose() { this.closed = true; this.intent++; this.outlineIntent++; this.listeners.clear(); }
  invalidate = () => { this.intent++; this.publish({ pending: undefined, history: this.state.history && { ...this.state.history, loading: false } }); };
  observe(snapshot: RuntimeClientSnapshot) {
    if (!this.current() || snapshot.conversation_id !== this.conversationId) return;
    this.live = snapshot;
    this.publish({ history: refreshTranscript(this.state.history, snapshot.transcript) });
    // Streaming text does not demand a new outline per delta. Native attempt
    // transitions and the committed transcript frontier refresh its previews.
    const revision = JSON.stringify([snapshot.attempt?.attempt_id, snapshot.attempt?.phase.type,
      snapshot.transcript.entries?.at(-1)?.cursor]);
    if (revision !== this.liveRevision) {
      this.liveRevision = revision;
      if (this.outlineWork) this.refreshPending = true; else void this.refreshTurns();
    }
  }
  refreshTurns = (): Promise<ConversationTurnPage | undefined> => {
    if (this.outlineWork) return this.outlineWork;
    return this.readTurns(this.outlineOffset);
  };
  private readTurns(offset?: number): Promise<ConversationTurnPage | undefined> {
    const intent = ++this.outlineIntent;
    const current = () => this.current() && intent === this.outlineIntent;
    this.publish({ outlineLoading: true, outlineError: undefined });
    const work = (async () => {
      try {
        const result = await this.client.request({ method: 'agent/turns', params: { target: this.target, agent_id: this.agentId, offset: offset ?? null, limit: HISTORY_PAGE_SIZE } }, 'conversation_turns', undefined, current);
        if (!current()) return;
        if (result.page.cut.conversation_id !== this.conversationId || result.page.turns.length > HISTORY_PAGE_SIZE) throw new Error('Invalid native child outline.');
        this.outlineOffset = offset;
        this.publish({ outline: result.page }); return result.page;
      } catch (error) { if (current()) this.publish({ outlineError: String(error) }); }
      finally { if (current()) this.publish({ outlineLoading: false }); }
    })();
    this.outlineWork = work;
    void work.finally(() => {
      if (this.outlineWork !== work) return;
      this.outlineWork = undefined;
      if (this.refreshPending && this.current()) { this.refreshPending = false; void this.refreshTurns(); }
    });
    return work;
  }
  async navigate(selection: ConversationTurn | number, gestureCurrent: () => boolean = () => true) {
    this.invalidate();
    const intent = this.intent;
    const current = () => this.current() && intent === this.intent && gestureCurrent();
    if (!current()) return false;
    this.publish({ pending: typeof selection === 'number' ? `ordinal:${selection}` : turnKey(selection.id), navigationError: undefined });
    try {
      let turn: ConversationTurn | undefined, cut = this.state.outline?.cut;
      if (typeof selection === 'number') {
        const offset = Math.floor((selection - 1) / HISTORY_PAGE_SIZE) * HISTORY_PAGE_SIZE;
        const page = await this.readTurns(offset);
        if (!current()) return false;
        turn = page?.turns.find(turn => turn.ordinal === selection); cut = page?.cut;
        // Follow newest again only if the selected native page was the last.
        if (page && page.offset + page.turns.length >= page.total) this.outlineOffset = undefined;
      } else turn = selection;
      if (!turn || !cut || turn.id.conversation_id !== this.conversationId || turn.cursor == null) throw new Error('Reload the native child outline before navigating.');
      const cursor = turn.cursor, key = turnKey(turn.id);
      const anchored = this.state.history?.page.entries?.some(entry => entry.cursor === cursor &&
        (entry.turn_process && turnKey(entry.turn_process) === key || entry.item.type === 'attempt_terminal' && turnKey(entry.item.turn) === key));
      if (!anchored) {
        const result = await this.client.request({ method: 'agent/transcript', params: { target: this.target, agent_id: this.agentId, at: { type: 'turn', id: turn.id, cut }, limit: HISTORY_PAGE_SIZE } }, 'transcript_window', undefined, current);
        if (!current()) return false;
        const window = result.window;
        if (!sameReadCut(window.cut, cut) || window.target_cursor !== cursor || !window.target || turnKey(window.target) !== key || !window.page.entries?.some(entry => entry.cursor === cursor)) throw new Error('Invalid native child Turn window.');
        this.publish({ history: installTranscriptWindow(window, this.state.history) });
      }
      if (!current()) return false;
      this.publish({ pending: undefined }); return turn;
    } catch (error) { if (current()) this.publish({ pending: undefined, navigationError: String(error) }); return false; }
  }
  returnToLatest = () => {
    this.invalidate();
    if (this.live) this.publish({ history: replaceTranscript(this.live.transcript, this.state.history), navigationError: undefined });
  };
  loadEarlier = () => {
    const history = this.state.history;
    if (!history?.page.next_cursor || history.loading) return Promise.resolve();
    return this.readWindow({ type: 'older', before: history.page.next_cursor, cut: history.window?.cut ?? null });
  };
  loadLater = () => {
    const window = this.state.history?.window;
    if (!window?.newer_cursor || this.state.history?.loading) return Promise.resolve();
    return this.readWindow({ type: 'newer', after: window.newer_cursor, cut: window.cut });
  };
  private async readWindow(at: ConversationWindowAt) {
    const history = this.state.history;
    if (!history || !this.current()) return;
    this.invalidate();
    const intent = this.intent, current = () => this.current() && this.intent === intent;
    this.publish({ history: { ...history, loading: true, error: undefined } });
    try {
      const result = await this.client.request({ method: 'agent/transcript', params: { target: this.target, agent_id: this.agentId, at, limit: HISTORY_PAGE_SIZE } }, 'transcript_window', undefined, current);
      if (!current()) return;
      const window = result.window;
      if (window.cut.conversation_id !== this.conversationId || 'cut' in at && at.cut && !sameReadCut(window.cut, at.cut)) throw new Error('Invalid native child history cut.');
      if (at.type === 'older' && window.page.next_cursor != null && BigInt(window.page.next_cursor) >= BigInt(at.before)) throw new Error('Invalid native child history page.');
      this.publish({ history: extendTranscriptWindow(window, this.state.history!, at.type === 'older') });
    } catch (error) { if (current()) this.publish({ history: { ...this.state.history!, loading: false, error: String(error) } }); }
  }
}

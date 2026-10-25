import type { AttachmentTarget, ObservedTurn, ObservedTurnPage, ConversationWindowAt, RuntimeClientSnapshot } from '../../../protocol/app-server/v57';
import type { AppServerClient } from './app-server';
import { extendTranscriptWindow, HISTORY_PAGE_SIZE, TURN_PAGE_SIZE, installTranscriptWindow, refreshTranscript, replaceTranscript, transcriptRepairs, repairTranscript, sameReadCut, turnKey, turnPageCount, type TranscriptCache, type TurnSelection } from './transcript';
import { transcriptWindowReady } from './history-policy';

export interface AgentReadingState {
  history?: TranscriptCache;
  outline?: ObservedTurnPage;
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
  private outlineWork?: Promise<ObservedTurnPage | undefined>;
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
  /** Hiding retires outstanding read generations while retaining the loaded cut. */
  suspend() {
    this.intent++; this.outlineIntent++; this.refreshPending = !!this.outlineWork;
    this.state = {...this.state,pending:undefined,outlineLoading:false,history:this.state.history && {...this.state.history,loading:false}};
    this.listeners.forEach(listener=>listener());
  }
  invalidate = () => { this.intent++; this.publish({ pending: undefined, history: this.state.history && { ...this.state.history, loading: false } }); };
  observe(snapshot: RuntimeClientSnapshot) {
    if (!this.current() || snapshot.conversation_id !== this.conversationId) return;
    this.live = snapshot;
    this.publish({ history: refreshTranscript(this.state.history, snapshot.transcript) });
    void this.refreshEntries();
    // Streaming text does not demand a new outline per delta. Native attempt
    // transitions and the committed transcript frontier refresh its previews.
    const revision = JSON.stringify([snapshot.attempt?.attempt_id, snapshot.attempt?.attempt_id_source, snapshot.attempt?.phase.type,
      snapshot.transcript.entries?.at(-1)?.cursor]);
    if (revision !== this.liveRevision || this.refreshPending && !this.outlineWork || !this.state.outline && !this.state.outlineError) {
      this.liveRevision = revision;
      if (this.outlineWork) this.refreshPending = true; else void this.refreshTurns();
    }
  }
  refreshTurns = (): Promise<ObservedTurnPage | undefined> => {
    if (this.outlineWork) return this.outlineWork;
    return this.readTurns(this.outlineOffset);
  };
  private readTurns(offset?: number): Promise<ObservedTurnPage | undefined> {
    this.refreshPending = false;
    const intent = ++this.outlineIntent;
    const current = () => this.current() && intent === this.outlineIntent;
    this.publish({ outlineLoading: true, outlineError: undefined });
    const work = (async () => {
      let page: ObservedTurnPage | undefined;
      try {
        const result = await this.client.request({ method: 'agent/turns', params: { target: this.target, agent_id: this.agentId, offset: offset ?? null, limit: TURN_PAGE_SIZE } }, 'conversation_turns', undefined, current);
        if (!current()) return;
        if (result.page.cut.conversation_id !== this.conversationId || result.page.turns.length > TURN_PAGE_SIZE) throw new Error('Invalid native child outline.');
        page=result.page;
        turnPageCount(result.page);
        this.outlineOffset = offset;
        this.publish({ outline: result.page }); return result.page;
      } catch (error) { if (current()) this.publish({ ...(page?{outline:page}:{}), outlineError: String(error) }); }
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
  async navigate(selection: TurnSelection, gestureCurrent: () => boolean = () => true) {
    this.invalidate();
    const intent = this.intent;
    const current = () => this.current() && intent === this.intent && gestureCurrent();
    if (!current()) return false;
    this.publish({ pending: typeof selection === 'number' ? `ordinal:${selection}` : turnKey(selection.turn.id), navigationError: undefined });
    try {
      let turn: ObservedTurn | undefined, cut: ObservedTurnPage['cut'] | undefined;
      if (typeof selection === 'number') {
        if (!Number.isSafeInteger(selection) || selection < 1) throw new Error('Invalid native child Turn ordinal.');
        const offset = Math.floor((selection - 1) / TURN_PAGE_SIZE) * TURN_PAGE_SIZE;
        const page = await this.readTurns(offset);
        if (!current()) return false;
        turn = page?.turns.find(turn => turn.ordinal === selection); cut = page?.cut;
        // Follow newest again only if the selected native page was the last.
        if (page && BigInt(page.offset + page.turns.length) >= BigInt(page.total)) this.outlineOffset = undefined;
      } else { turn = selection.turn; cut = selection.cut; }
      if (!turn || turn.cursor == null || turn.ordinal > 0 && (!cut || cut.conversation_id !== this.conversationId)) throw new Error('Reload the native child outline before navigating.');
      const cursor = turn.cursor, key = turnKey(turn.id);
      const anchored = this.state.history?.page.entries?.some(entry => entry.cursor === cursor &&
        (entry.turn_process && turnKey(entry.turn_process) === key || entry.item.type === 'attempt_terminal' && turnKey(entry.item.turn) === key));
      if (!anchored) {
        if (!cut || !Number.isSafeInteger(turn.ordinal) || turn.ordinal < 1) throw new Error('The live child Turn has no loaded native anchor.');
        const result = await this.client.request({ method: 'agent/transcript', params: { target: this.target, agent_id: this.agentId, at: { type: 'turn_ordinal', ordinal: turn.ordinal, cut }, limit: HISTORY_PAGE_SIZE } }, 'transcript_window', undefined, current);
        if (!current()) return false;
        const window = result.window;
        if (window.cut.conversation_id !== this.conversationId || !sameReadCut(window.cut, cut) || window.target_cursor !== cursor || !window.target || turnKey(window.target) !== key || !window.page.entries?.some(entry => entry.cursor === cursor)) throw new Error('Invalid native child Turn window.');
        this.publish({ history: installTranscriptWindow(window, this.state.history) });
      }
      if (!current()) return false;
      this.publish({ pending: undefined }); return turn;
    } catch (error) { if (current()) this.publish({ pending: undefined, navigationError: String(error) }); return false; }
  }
  returnToLatest = () => {
    this.invalidate();
    if (this.live && this.state.history?.window) this.publish({ history: replaceTranscript(this.live.transcript, this.state.history), navigationError: undefined });
    void this.refreshEntries();
  };
  loadEarlier = () => {
    const history = this.state.history;
    if (!history?.page.next_cursor || history.loading) return Promise.resolve();
    return this.readWindow({ type: 'older', before: history.page.next_cursor, cut: history.window?.cut ?? null });
  };
  loadInitial = () => {
    const history = this.state.history;
    if (!history || history.window || history.loading || history.error || !history.page.next_cursor || transcriptWindowReady(history.page.entries ?? [])) return Promise.resolve();
    return this.readWindow({ type: 'older', before: history.page.next_cursor, cut: null }, true);
  };
  loadLater = () => {
    const window = this.state.history?.window;
    if (!window?.newer_cursor || this.state.history?.loading) return Promise.resolve();
    return this.readWindow({ type: 'newer', after: window.newer_cursor, cut: window.cut });
  };
  private repairs?: { current: () => boolean; dirty: boolean; work: Promise<void> };
  private refreshEntries(): Promise<void> {
    if (this.repairs?.current()) { this.repairs.dirty = true; return this.repairs.work; }
    const history = this.state.history, intent = this.intent;
    if (!history || history.window || !this.live || !this.current()) return Promise.resolve();
    const current = () => this.current() && this.intent === intent && this.state.history?.epoch === history.epoch && !this.state.history?.window;
    const read = { current, dirty: true, work: Promise.resolve() };
    this.repairs = read;
    read.work = (async () => {
      try {
        while (read.dirty && current()) {
          read.dirty = false;
          for (const entry of transcriptRepairs(this.state.history, this.live!.transcript)) {
            if (!current() || entry.item.type !== 'message') return;
            const result = await this.client.request({ method: 'agent/transcript', params: { target: this.target, agent_id: this.agentId, at: { type: 'message', id: entry.item.message.id, cut: null }, limit: 1 } }, 'transcript_window', undefined, current);
            if (!current()) return;
            if (result.window.cut.conversation_id !== this.conversationId) throw new Error('Native child transcript refresh changed Conversation.');
            this.publish({ history: refreshTranscript(repairTranscript(this.state.history!, result.window, entry), this.live!.transcript) });
          }
        }
      } catch (error) { if (current()) this.publish({ history: { ...this.state.history!, error: String(error) } }); }
      finally { if (this.repairs === read) this.repairs = undefined; }
    })();
    return read.work;
  }
  private async readWindow(at: ConversationWindowAt, initial = false) {
    const history = this.state.history;
    if (!history || !this.current()) return;
    this.invalidate();
    const intent = this.intent, current = () => this.current() && this.intent === intent;
    this.publish({ history: { ...history, loading: true, initialLoading: initial, error: undefined } });
    try {
      let pageAt = at, next = history;
      const added: NonNullable<typeof history.page.entries> = [];
      do {
        const result = await this.client.request({ method: 'agent/transcript', params: { target: this.target, agent_id: this.agentId, at: pageAt, limit: HISTORY_PAGE_SIZE } }, 'transcript_window', undefined, current);
        if (!current()) return;
        const window = result.window;
        if (window.cut.conversation_id !== this.conversationId || 'cut' in pageAt && pageAt.cut && !sameReadCut(window.cut, pageAt.cut)) throw new Error('Invalid native child history cut.');
        if (pageAt.type === 'older' && window.page.next_cursor != null && BigInt(window.page.next_cursor) >= BigInt(pageAt.before)) throw new Error('Invalid native child history page.');
        added.push(...window.page.entries ?? []);
        next = extendTranscriptWindow(window, next, at.type === 'older');
        if (at.type !== 'older' || next.page.next_cursor == null || transcriptWindowReady(initial ? next.page.entries! : added)) break;
        pageAt = { type: 'older', before: next.page.next_cursor, cut: window.cut };
      } while (current());
      if (!current()) return;
      if (!history.window && at.type === 'older' && next.window?.newer_cursor == null) {
        const { window: _, ...following } = next;
        next = this.live ? refreshTranscript(following, this.live.transcript) : following;
      }
      this.publish({ history: { ...next, loading: false, initialLoading: false } });
      void this.refreshEntries();
    } catch (error) { if (current()) this.publish({ history: { ...this.state.history!, loading: false, initialLoading: false, error: String(error) } }); }
  }
}

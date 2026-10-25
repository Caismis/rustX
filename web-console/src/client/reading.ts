import { HISTORY_PAGE_SIZE, TURN_PAGE_SIZE, sameReadCut, extendTranscriptWindow, installTranscriptWindow, refreshTranscript, replaceTranscript, transcriptRepairs, repairTranscript, turnKey, turnPageCount, type TurnSelection } from './transcript';
import { transcriptWindowReady } from './history-policy';
import type { ConversationWindowAt, ObservedTurn, ObservedTurnPage, RuntimeClientTranscriptEntry } from '../../../protocol/app-server/v57';
import type { SessionLifecycles } from './session-lifecycle/system';
import type { ClientView, ConnectionRequest, OutlinePagingIntent, SessionView } from './contracts';

interface OutlineDemand {
  offset?: number;
  /** Absent for a disposable preview; only navigation owns committed paging. */
  paging?: OutlinePagingIntent;
  automatic: boolean;
  current: () => boolean;
  work: Promise<ObservedTurnPage | undefined>;
  resolve: (page?: ObservedTurnPage) => void;
}
interface OutlineRead {
  authority: () => boolean;
  active: OutlineDemand;
  pending?: OutlineDemand;
  refresh: boolean;
}

/** What reading navigation reads from and writes to its client. */
export interface ReadingPort {
  request: ConnectionRequest;
  state(): Readonly<ClientView>;
  setSession(id: string, patch: Partial<Pick<SessionView, 'history' | 'turnOutline' | 'turnNavigation'>>): void;
  /** The Session actor's live observation proof, if attached. */
  observe: SessionLifecycles['observe'];
  epoch(id: string): number | undefined;
}

/** Reading navigation of attached Sessions: the bounded native Turn outline
 * pipeline, exact Turn/message jumps, finite transcript window paging, and
 * exact repair of loaded rows that left the live head.
 * Every read is fenced by the Session's observation, its reading authority
 * and the latest navigation intent; a superseded gesture never lands. */
export class ReadingNavigation {
  constructor(private readonly port: ReadingPort) {}
  private readingIntents = new Map<string, number>();
  private readingAuthorities = new Map<string, number>();
  private outlineReads = new Map<string, OutlineRead>();
  /** A new navigation or an explicit reload retires the previous jump's landing. */
  invalidateReading(id: string) {
    const intent = (this.readingIntents.get(id) ?? 0) + 1;
    this.readingIntents.set(id, intent);
    const history = this.port.state().views[id]?.history;
    if (this.port.state().views[id]) this.port.setSession(id, { turnNavigation: { intent }, ...(history?.loading ? { history: { ...history, loading: false } } : {}) });
    return intent;
  }
  private readingAuthority(id: string) {
    const observation = this.port.observe(id);
    const authority = this.readingAuthorities.get(id), epoch = this.port.epoch(id);
    return () => !!observation?.current() && this.readingAuthorities.get(id) === authority && this.port.epoch(id) === epoch;
  }
  retireOutline(id: string) {
    const read = this.outlineReads.get(id);
    this.outlineReads.delete(id);
    read?.active.resolve(); read?.pending?.resolve();
  }
  /** Automatic refresh preserves explicit paging; one later read discharges growth. */
  refreshTurns(id: string) {
    const read = this.outlineReads.get(id);
    if (read?.authority()) { read.refresh = true; return Promise.resolve(undefined); }
    const paging = this.port.state().views[id]?.turnOutline?.paging;
    return this.demandOutline(id, paging?.type === 'page' ? paging.offset : undefined, paging ?? { type: 'latest' }, () => true, true);
  }
  /** One in-flight RPC plus one replaceable latest demand. A skipped demand never
   * masquerades as a completed page. Equivalent demands share the native read. */
  readTurns(id: string, offset?: number, userCurrent: () => boolean = () => true): Promise<ObservedTurnPage | undefined> {
    return this.demandOutline(id, offset, offset === undefined ? { type: 'latest' } : { type: 'page', offset }, userCurrent);
  }
  /** Shares the bounded outline pipeline without publishing a reading destination. */
  previewTurns(id: string, offset: number, current: () => boolean): Promise<ObservedTurnPage | undefined> {
    const intent = this.readingIntents.get(id);
    const valid = () => current() && this.readingIntents.get(id) === intent;
    return this.demandOutline(id, offset, undefined, valid).then(page => valid() ? page : undefined);
  }
  private demandOutline(id: string, offset: number | undefined, paging: OutlinePagingIntent | undefined, userCurrent: () => boolean, automatic = false): Promise<ObservedTurnPage | undefined> {
    const view = this.port.state().views[id];
    if (!view || !userCurrent() || !this.port.observe(id)) return Promise.resolve(undefined);
    let read = this.outlineReads.get(id);
    if (read && !read.authority()) { this.retireOutline(id); read = undefined; }
    if (read) {
      // Preview may observe a matching navigation read, but never take its authority.
      if (!paging) {
        const committed = [read.active, read.pending].find(demand => demand?.paging && demand.offset === offset);
        if (committed) return committed.work.then(page => userCurrent() ? page : undefined);
        if (read.pending?.paging) return Promise.resolve(undefined);
      }
      const equivalent = !read.active.automatic && read.active.offset === offset ? read.active : read.pending?.offset === offset ? read.pending : undefined;
      if (equivalent) {
        if (equivalent === read.active) { read.pending?.resolve(); read.pending = undefined; }
        equivalent.current = userCurrent; equivalent.paging = paging;
        if (paging) this.port.setSession(id, { turnOutline: { paging: { type: 'latest' }, ...view.turnOutline, loading: true, error: undefined } });
        return equivalent.work;
      }
    }
    let resolve!: OutlineDemand['resolve'];
    const demand: OutlineDemand = { offset, paging, automatic, current: userCurrent, work: new Promise(done => { resolve = done; }), resolve: page => resolve(page) };
    const start = !read;
    if (read) { read.pending?.resolve(); read.pending = demand; }
    else {
      read = { authority: this.readingAuthority(id), active: demand, refresh: false };
      this.outlineReads.set(id, read);
    }
    if (paging) this.port.setSession(id, { turnOutline: { paging: { type: 'latest' }, ...view.turnOutline, loading: true, error: undefined } });
    if (start) void this.runOutline(id, read);
    return demand.work;
  }
  private async runOutline(id: string, read: OutlineRead) {
    const demand = read.active;
    const owned = () => this.outlineReads.get(id) === read && read.authority();
    const current = () => owned() && !(read.pending?.paging && read.pending.current()) && demand.current() && owned();
    let page: ObservedTurnPage | undefined;
    try {
      if (!current()) return;
      const target = this.port.observe(id)!.target;
      const result = await this.port.request({ method: 'session/turns', params: { target, offset: demand.offset ?? null, limit: TURN_PAGE_SIZE } }, 'conversation_turns', undefined, owned);
      if (current()) {
        if (result.page.cut.conversation_id !== target.conversation_id || result.page.turns.length > TURN_PAGE_SIZE) throw new Error('Invalid native turn outline.');
        page = result.page;
        turnPageCount(result.page);
        if (demand.paging) this.port.setSession(id, { turnOutline: { paging: demand.paging, page } });
      }
    } catch (error) {
      if (demand.paging && current()) this.port.setSession(id, { turnOutline: { paging: { type: 'latest' }, ...this.port.state().views[id]?.turnOutline, ...(page?{page}:{}), loading: false, error: String(error) } });
      page=undefined;
    } finally {
      // A superseded automatic read still owes a refresh of the surviving presentation.
      if (owned() && demand.automatic && !current()) read.refresh = true;
      demand.resolve(page);
      if (this.outlineReads.get(id) === read) {
        const pending = read.pending;
        if (pending?.current() && owned()) {
          read.active = pending; read.pending = undefined;
          void this.runOutline(id, read);
        } else {
          this.outlineReads.delete(id); pending?.resolve();
          if (read.authority()) {
            const outline = this.port.state().views[id]?.turnOutline;
            if (outline?.loading) this.port.setSession(id, { turnOutline: { ...outline, loading: false } });
            if (read.refresh) void this.refreshTurns(id);
          }
        }
      }
    }
  }
  /** Native resolves the exact Turn at its outline cut in one bounded read. */
  async navigateTurn(id: string, selection: TurnSelection, userCurrent: () => boolean = () => true) {
    const view = this.port.state().views[id];
    if (!view?.history || !userCurrent() || !this.port.observe(id)) return false;
    const authority = this.readingAuthority(id), intent = this.invalidateReading(id);
    const current = () => userCurrent() && authority() && this.readingIntents.get(id) === intent;
    let turn: ObservedTurn, cut: ObservedTurnPage['cut'] | undefined;
    if (typeof selection === 'number') {
      this.port.setSession(id, { turnNavigation: { intent, pending: `ordinal:${selection}` } });
      if (!Number.isSafeInteger(selection) || selection < 1) { this.port.setSession(id,{turnNavigation:{intent,error:'Invalid native Turn ordinal.'}}); return false; }
      const total = BigInt(view.turnOutline?.page?.total ?? '0');
      const offset = Math.floor((selection - 1) / TURN_PAGE_SIZE) * TURN_PAGE_SIZE;
      // Identity lookup is fixed even when presentation should continue following latest.
      const paging: OutlinePagingIntent = BigInt(offset) >= (total - 1n) / BigInt(TURN_PAGE_SIZE) * BigInt(TURN_PAGE_SIZE) ? { type: 'latest' } : { type: 'page', offset };
      const page = await this.demandOutline(id, offset, paging, current);
      if (!current()) return false;
      if (!page) { this.port.setSession(id, { turnNavigation: { intent } }); return false; }
      const found = page.turns.find(row => row.ordinal === selection);
      if (!found) { this.port.setSession(id, { turnNavigation: { intent, error: 'This Turn is no longer in the native outline.' } }); return false; }
      turn = found; cut = page.cut;
    } else {
      turn = selection.turn; cut = selection.cut;
      const outline = this.port.state().views[id]?.turnOutline;
      if (outline?.page) {
        const offset = Math.floor((turn.ordinal - 1) / TURN_PAGE_SIZE) * TURN_PAGE_SIZE;
        const paging: OutlinePagingIntent = BigInt(offset) >= (BigInt(outline.page.total) - 1n) / BigInt(TURN_PAGE_SIZE) * BigInt(TURN_PAGE_SIZE) ? { type: 'latest' } : { type: 'page', offset };
        this.port.setSession(id, { turnOutline: { ...outline, paging } });
      }
    }
    const cursor = turn.cursor, key = turnKey(turn.id);
    if (!current()) return false;
    if (cursor == null) { this.port.setSession(id, { turnNavigation: { intent } }); return false; }
    const anchored = () => !!this.port.state().views[id]?.history?.page.entries?.some(entry => entry.cursor === cursor
      && (entry.turn_process && turnKey(entry.turn_process) === key || entry.item.type === 'attempt_terminal' && turnKey(entry.item.turn) === key));
    if (turn.ordinal > 0 && (!cut || cut.conversation_id !== view.target!.conversation_id)) { this.port.setSession(id, { turnNavigation: { intent, error: 'Reload the native Turn outline before navigating.' } }); return false; }
    if (anchored()) { this.port.setSession(id, { turnNavigation: { intent } }); return turn; }
    if (!cut || !Number.isSafeInteger(turn.ordinal) || turn.ordinal < 1) { this.port.setSession(id, { turnNavigation: { intent, error: 'The live Turn has no loaded native anchor.' } }); return false; }
    this.port.setSession(id, { turnNavigation: { intent, pending: key } });
    try {
      const result = await this.port.request({ method: 'session/transcript', params: { target: view.target!, at: { type: 'turn_ordinal', ordinal: turn.ordinal, cut }, limit: HISTORY_PAGE_SIZE } }, 'transcript_window', undefined, current);
      if (!current()) return false;
      if (!sameReadCut(result.window.cut, cut) || result.window.cut.conversation_id !== view.target!.conversation_id || result.window.target_cursor !== cursor || !result.window.target || turnKey(result.window.target) !== key) throw new Error('Invalid native Turn window.');
      const history = installTranscriptWindow(result.window, this.port.state().views[id]?.history);
      if (!(history.page.entries ?? []).some(entry => entry.cursor === cursor)) throw new Error('This turn is outside the readable native history.');
      this.port.setSession(id, { history, turnNavigation: { intent } });
      return anchored() && current() ? turn : false;
    } catch (error) {
      if (current()) this.port.setSession(id, { turnNavigation: { intent, error: String(error) } });
      return false;
    }
  }
  /** Native owns the exact message position; one read installs a bounded window. */
  async navigateMessage(id: string, messageId: string, userCurrent: () => boolean = () => true) {
    const view = this.port.state().views[id];
    const observation = this.port.observe(id);
    if (!view?.history || !observation || !userCurrent()) return false;
    const authority = this.readingAuthority(id), intent = this.invalidateReading(id), target = observation.target;
    const current = () => observation.current() && authority() && this.readingIntents.get(id) === intent && userCurrent();
    const anchored = () => this.port.state().views[id]?.history?.page.entries?.some(entry => entry.item.type === 'message' && entry.item.message.id === messageId);
    if (anchored()) return current();
    this.port.setSession(id, { turnNavigation: { intent, pending: `message:${messageId}` } });
    try {
      const result = await this.port.request({ method: 'session/transcript', params: { target, at: { type: 'message', id: messageId, cut: null }, limit: HISTORY_PAGE_SIZE } }, 'transcript_window', undefined, current);
      if (!current()) return false;
      const cursor = result.window.target_cursor, entries = result.window.page.entries ?? [];
      if (result.window.cut.conversation_id !== target.conversation_id || cursor == null || entries.length > HISTORY_PAGE_SIZE
        || !entries.some(entry => entry.cursor === cursor && entry.item.type === 'message' && entry.item.message.id === messageId)) throw new Error('Invalid native message window.');
      const history = installTranscriptWindow(result.window, this.port.state().views[id]?.history);
      if (!history.page.entries?.some(entry => entry.item.type === 'message' && entry.item.message.id === messageId)) throw new Error('The fork source message is outside the readable native history.');
      this.port.setSession(id, { history, turnNavigation: { intent } });
      return current() && !!anchored();
    } catch (error) {
      if (current()) this.port.setSession(id, { turnNavigation: { intent, error: String(error) } });
      return false;
    }
  }
  returnToLatest(id: string) {
    if (!this.port.observe(id)) return;
    this.invalidateReading(id);
    const view = this.port.state().views[id];
    if (view?.snapshot && view.history?.window) this.port.setSession(id, { history: replaceTranscript(view.snapshot.transcript, view.history) });
    void this.refreshTranscriptDomain(id);
  }
  private transcriptReads = new Map<string, { current: () => boolean; dirty: boolean; work: Promise<void> }>();
  /** Native completion can update a row after it leaves the live head. Reread
   * that exact message without clearing history, detaching scrolling or asking
   * the reader to load it again. One later observation coalesces one fresh pass. */
  refreshTranscriptDomain(id: string): Promise<void> {
    const existing = this.transcriptReads.get(id);
    if (existing?.current()) { existing.dirty = true; return existing.work; }
    const observation = this.port.observe(id), history = this.port.state().views[id]?.history;
    if (!observation || !history || history.window) return Promise.resolve();
    const authority = this.readingAuthority(id), intent = this.readingIntents.get(id);
    const current = () => authority() && this.readingIntents.get(id) === intent
      && this.port.state().views[id]?.history?.epoch === history.epoch && !this.port.state().views[id]?.history?.window;
    const read = { current, dirty: true, work: Promise.resolve() };
    this.transcriptReads.set(id, read);
    read.work = (async () => {
      try {
        while (read.dirty && current()) {
          read.dirty = false;
          const view = this.port.state().views[id];
          for (const entry of transcriptRepairs(view.history, view.snapshot!.transcript)) {
            if (!current() || entry.item.type !== 'message') return;
            const result = await this.port.request({ method: 'session/transcript', params: { target: observation.target, at: { type: 'message', id: entry.item.message.id, cut: null }, limit: 1 } }, 'transcript_window', undefined, current);
            if (!current()) return;
            if (result.window.cut.conversation_id !== observation.target.conversation_id) throw new Error('Native transcript refresh changed Conversation.');
            const latest = this.port.state().views[id];
            this.port.setSession(id, { history: refreshTranscript(repairTranscript(latest.history!, result.window, entry), latest.snapshot!.transcript) });
          }
        }
      } catch (error) {
        if (current()) this.port.setSession(id, { history: { ...this.port.state().views[id].history!, error: String(error) } });
      } finally { if (this.transcriptReads.get(id) === read) this.transcriptReads.delete(id); }
    })();
    return read.work;
  }
  loadInitialHistory(id: string) {
    const history = this.port.state().views[id]?.history;
    if (!history || history.window || history.loading || history.error || history.page.next_cursor == null || transcriptWindowReady(history.page.entries ?? [])) return Promise.resolve();
    return this.readHistoryPage(id, { type: 'older', before: history.page.next_cursor, cut: null }, true);
  }
  async loadLater(id: string) {
    const history = this.port.state().views[id]?.history, window = history?.window;
    if (!window?.newer_cursor || history?.loading) return;
    await this.readHistoryPage(id, { type: 'newer', after: window.newer_cursor, cut: window.cut });
  }
  /** One Turn-aligned reading window, assembled from bounded native pages. */
  loadEarlier(id: string): Promise<void> {
    const history = this.port.state().views[id]?.history;
    if (!history || history.page.next_cursor == null || history.loading) return Promise.resolve();
    return this.readHistoryPage(id, { type: 'older', before: history.page.next_cursor, cut: history.window?.cut ?? null });
  }
  /** Navigation intent fences the whole reading window, including every wire page. */
  private async readHistoryPage(id: string, at: ConversationWindowAt, initial = false) {
    const history = this.port.state().views[id]?.history;
    const observation = this.port.observe(id);
    if (!history || !observation) return;
    const target = observation.target, authority = this.readingAuthority(id), intent = this.invalidateReading(id);
    const current = () => authority() && this.readingIntents.get(id) === intent && this.port.state().views[id]?.history?.epoch === history.epoch;
    this.port.setSession(id, { history: { ...history, loading: true, initialLoading: initial, error: undefined } });
    try {
      let pageAt = at, next = history;
      const added: RuntimeClientTranscriptEntry[] = [];
      do {
        const result = await this.port.request({ method: 'session/transcript', params: { target, at: pageAt, limit: HISTORY_PAGE_SIZE } }, 'transcript_window', undefined, current);
        if (!current()) return;
        if (result.window.cut.conversation_id !== target.conversation_id || ('cut' in pageAt && pageAt.cut && !sameReadCut(result.window.cut, pageAt.cut))) throw new Error('Invalid native history cut.');
        if (pageAt.type === 'older' && result.window.page.next_cursor != null && BigInt(result.window.page.next_cursor) >= BigInt(pageAt.before)) throw new Error('Invalid native history page.');
        added.push(...result.window.page.entries ?? []);
        next = extendTranscriptWindow(result.window, next, at.type === 'older');
        if (at.type !== 'older' || next.page.next_cursor == null || transcriptWindowReady(initial ? next.page.entries! : added)) break;
        pageAt = { type: 'older', before: next.page.next_cursor, cut: result.window.cut };
      } while (current());
      if (!current()) return;
      // Ordinary pagination remains a live conversation. Only an explicit Turn
      // or message jump owns a frozen historical window and its continuation.
      if (!history.window && at.type === 'older' && next.window?.newer_cursor == null) {
        const { window: _, ...following } = next;
        next = refreshTranscript(following, this.port.state().views[id].snapshot!.transcript);
      }
      this.port.setSession(id, { history: { ...next, loading: false, initialLoading: false } });
      void this.refreshTranscriptDomain(id);
    } catch (error) {
      if (current()) this.port.setSession(id, { history: { ...this.port.state().views[id].history!, loading: false, initialLoading: false, error: String(error) } });
    }
  }
  /** Retired observation work keeps no reading intent or outline. */
  retire(id: string) { this.readingIntents.delete(id); this.retireOutline(id); this.readingAuthorities.delete(id); this.transcriptReads.delete(id); }
  /** A replaced App Server authority keeps no reading state. */
  retireAuthority() { this.readingAuthorities.clear(); for (const id of this.outlineReads.keys()) this.retireOutline(id); this.readingIntents.clear(); }
  /** A lost connection ends every transcript repair. */
  disconnect() { this.transcriptReads.clear(); }
}

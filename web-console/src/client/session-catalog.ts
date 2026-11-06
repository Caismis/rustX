import type { ClientView, ConnectionRequest } from './contracts';

/** What the Session catalog reads from and writes to its client. It writes
 * only catalog rows and the cached summaries of Session views. */
export interface SessionCatalogPort {
  request: ConnectionRequest;
  state(): Readonly<ClientView>;
  current(generation: number): boolean;
  /** The Session actor's attachment epoch; summaries are fenced by it. */
  epoch(id: string): number | undefined;
  publish(patch: Pick<ClientView, 'sessions' | 'views'> & Partial<Pick<ClientView, 'nextOffset'>>): void;
}

/** Native Session catalog membership and exact Session summary metadata of
 * one App Server authority: paged catalog reads, causally ordered summary
 * reads, and the post-commit invalidations that reopen them (Issue #386). */
export class SessionCatalog {
  constructor(private readonly port: SessionCatalogPort) {}
  // Native metadata invalidation (Issue #386). One monotonic clock orders every
  // metadata observation this client makes: `readSequence` is ticked at
  // the *start* of each catalog list and each exact summary read — its causal
  // start cut — and again for each `session/summaryInvalidated` notification.
  // `invalidated` holds the ticket of the latest invalidation observed
  // for a Session, so an observation discharges it only when it started later.
  // `settled` records that a causally later read completed, so it is
  // evidence about a read, never a permanent conclusion drawn from canonical
  // history. A newer invalidation always outranks an older settlement.
  private invalidated = new Map<string, number>();
  private settled = new Set<string>();
  private observedEpoch = new Map<string, number>();
  private inFlight = new Map<string, { generation: number; invalidations: number; work: Promise<void> }>();
  private readSequence = 0;
  // Start cuts of the metadata observations still outstanding on this connection.
  private observations = new Set<number>();
  // Minimum acceptable metadata sequence: last observation or a committed rename read floor.
  private readFloors = new Map<string, number>();
  private listEpoch = 0;
  private listOffset = 0;
  private listQuery = '';
  async list(offset = this.listOffset, query = this.listQuery, current: () => boolean = () => true) {
    const epoch = ++this.listEpoch;
    // The causal start cut of this page: the server captured its rows no earlier.
    const summaryRead = ++this.readSequence;
    this.observations.add(summaryRead);
    this.listOffset = offset; this.listQuery = query;
    const generation = this.port.state().generation;
    try {
      const result = await this.port.request({ method: 'session/list', params: { offset, limit: 32, query } }, 'sessions');
      if (this.port.current(generation) && epoch === this.listEpoch && current()) {
        const views = { ...this.port.state().views };
        const stale = new Set<string>();
        const sessions = result.sessions.map(row => {
          const fence = this.readFloors.get(row.id) ?? 0;
          const cached = views[row.id]?.summary ?? this.port.state().sessions.find(item => item.id === row.id);
          const summary = fence > summaryRead && cached ? cached : row;
          // Accepting a row never discharges an invalidation observed after this
          // request's start cut — including for a Session this page introduces,
          // which no exact read could have been issued for while it was uncached.
          if (summary === row) {
            if (fence > summaryRead) stale.add(row.id); else this.readFloors.set(row.id, summaryRead);
          }
          if ((this.invalidated.get(row.id) ?? 0) > (this.readFloors.get(row.id) ?? 0)) stale.add(row.id);
          if (views[row.id]) views[row.id] = { ...views[row.id], summary };
          return summary;
        });
        // Retain ordering fences only for metadata actually cached by this client.
        for (const id of this.readFloors.keys()) if (!views[id] && !sessions.some(row => row.id === id)) this.readFloors.delete(id);
        this.port.publish({ sessions, nextOffset: result.next_offset, views });
        // Exact metadata repair only: never page membership, ordering, or query.
        for (const id of stale) void this.readSummary(id).catch(() => {});
      }
    } finally {
      this.observations.delete(summaryRead);
      this.retireInvalidations();
    }
  }
  /** Invalidation evidence stays bounded by cached state and by the observations
   * that could still reintroduce stale metadata. It is retired for a Session
   * this client does not cache once no older observation remains outstanding. */
  private retireInvalidations() {
    let oldest = Number.POSITIVE_INFINITY;
    for (const start of this.observations) oldest = Math.min(oldest, start);
    for (const [id, at] of this.invalidated) {
      if (at >= oldest || this.inFlight.has(id)) continue;
      if (!this.port.state().views[id] && !this.port.state().sessions.some(row => row.id === id)) this.invalidated.delete(id);
    }
  }
  /** Cached values remain renderable across transport loss. Each attachment
   * establishes fresh exact metadata independently of its first-message check.
   * A legitimate null projection settles here and is never polled; the native
   * `session/summaryInvalidated` notification is what reopens it. */
  async refreshDisplaySummary(id: string) {
    const generation = this.port.state().generation, epoch = this.port.epoch(id);
    const current = () => this.port.current(generation) && this.port.epoch(id) === epoch
      && this.port.state().views[id]?.attachmentIntent === 'wanted';
    if (!current()) return;
    const existing = this.inFlight.get(id);
    if (existing?.generation === generation) {
      await existing.work.catch(() => {});
      if (!current()) return;
    }
    const summary = this.port.state().sessions.find(row => row.id === id) ?? this.port.state().views[id]?.summary;
    if (this.observedEpoch.get(id) === epoch && summary
      && (summary.name || summary.generated_title || summary.preview || this.settled.has(id) || !this.hasCanonicalUser(id))) return;
    await this.readSummary(id).catch(() => {});
  }
  private hasCanonicalUser(id: string) {
    return !!this.port.state().views[id]?.snapshot?.transcript.entries?.some(entry => entry.item.type === 'message' && entry.item.message.role === 'user');
  }
  /** Exact native metadata observation, never a catalog search or membership change.
   * Coalesce within a connection and within one invalidation epoch. Canonical
   * user history is not by itself proof that the native display projection has
   * been published: publication is a separate, later catalog commit, so a read
   * settles the first-message check only when it was also causally after every
   * invalidation observed for this Session. */
  readSummary(id: string): Promise<void> {
    const invalidations = this.invalidated.get(id) ?? 0;
    const existing = this.inFlight.get(id);
    // Coalesce only reads that are causally equivalent. A read begun before an
    // invalidation can neither satisfy nor clear that newer invalidation, so it
    // never stands in for the causally later read it requires.
    if (existing?.generation === this.port.state().generation && existing.invalidations === invalidations) return existing.work;
    const generation = this.port.state().generation, summaryRead = ++this.readSequence;
    this.observations.add(summaryRead);
    const epoch = this.port.epoch(id), canonicalUser = this.hasCanonicalUser(id);
    const work = (async () => {
      const { summary } = await this.port.request({ method: 'session/summary', params: { session_id: id } }, 'session_summary');
      if (!this.port.current(generation) || this.port.epoch(id) !== epoch) throw new Error('Obsolete Session summary read.');
      if (summary.id !== id) throw new Error('Mismatched Session summary identity.');
      if ((this.readFloors.get(id) ?? 0) <= summaryRead) {
        this.readFloors.set(id, summaryRead);
        this.port.publish({ sessions: this.port.state().sessions.map(row => row.id === id ? summary : row),
          views: this.port.state().views[id] ? { ...this.port.state().views, [id]: { ...this.port.state().views[id], summary } } : this.port.state().views });
        if (epoch !== undefined && this.port.state().views[id]?.attachmentIntent === 'wanted') {
          this.observedEpoch.set(id, epoch);
          // Settle only when this read was causally after every invalidation
          // observed so far. One that arrived mid-flight keeps the Session
          // dirty and is answered by the read `invalidateSummary` started.
          if (canonicalUser && (this.invalidated.get(id) ?? 0) === invalidations) this.settled.add(id);
        }
      }
    })();
    const read = { generation, invalidations, work };
    this.inFlight.set(id, read);
    void work.finally(() => {
      if (this.inFlight.get(id) === read) this.inFlight.delete(id);
      this.observations.delete(summaryRead);
      this.retireInvalidations();
    }).catch(() => {});
    return work;
  }
  private catalogRefresh?: { generation: number; dirty: boolean; work: Promise<void> };
  /** Coalesced native membership invalidation. Never awaited by attach or send.
   * A failure settles this read; another invalidation/reconnect may reread. */
  invalidateCatalog(generation: number) {
    const existing = this.catalogRefresh;
    if (existing?.generation === generation) { existing.dirty = true; return; }
    const refresh = { generation, dirty: true, work: Promise.resolve() };
    this.catalogRefresh = refresh;
    refresh.work = (async () => {
      while (refresh.dirty && this.port.current(generation)) {
        refresh.dirty = false;
        await this.list();
      }
    })();
    void refresh.work.catch(() => {}).finally(() => { if (this.catalogRefresh === refresh) this.catalogRefresh = undefined; });
  }
  /** Native post-commit Session metadata invalidation (Issue #386).
   * Authoritative rereading, not a value: it overrides any earlier cached-null
   * check, and a read already in flight cannot answer it. A cached Session is
   * reread now; an uncached one is reread only if a list request older than this
   * invalidation later introduces its row, so an invalidation is never a poll. */
  invalidateSummary(id: string, generation: number) {
    if (!this.port.current(generation)) return;
    this.invalidated.set(id, ++this.readSequence);
    this.settled.delete(id);
    // Evidence is retained even for a Session this client does not cache: an
    // older list request still in flight can introduce that row, and accepting
    // it must not lose this invalidation.
    if (!this.port.state().views[id] && !this.port.state().sessions.some(row => row.id === id)) { this.retireInvalidations(); return; }
    void this.readSummary(id).catch(() => {});
  }
  /** A committed rename requires a read started after its acknowledgement.
   * Ordinary observers still coalesce; the old read cannot publish past this floor. */
  async rename(id: string, name: string): Promise<void> {
    const generation = this.port.state().generation;
    await this.port.request({ method: 'session/name', params: { session_id: id, name } }, 'session');
    if (!this.port.current(generation)) return;
    await this.readFreshSummary(id);
  }
  private readFreshSummary(id: string): Promise<void> {
    this.readFloors.set(id, ++this.readSequence);
    this.observedEpoch.delete(id);
    this.inFlight.delete(id);
    return this.readSummary(id);
  }
  /** A new attachment epoch must establish fresh first-message evidence. */
  epochChanged(id: string) { this.settled.delete(id); }
  /** A retired attachment keeps no first-message evidence. */
  forgetAttachment(id: string) { this.settled.delete(id); this.observedEpoch.delete(id); }
  /** A replaced App Server authority shares no catalog evidence or paging. */
  retireAuthority() {
    this.settled.clear(); this.invalidated.clear(); this.observedEpoch.clear(); this.inFlight.clear(); this.readFloors.clear(); this.observations.clear();
    this.listEpoch++; this.listOffset = 0; this.listQuery = '';
  }
}

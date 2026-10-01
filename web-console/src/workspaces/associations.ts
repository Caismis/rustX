import { WorkspaceAuthority, type WorkspaceAuthorityObservation } from './authority';
import type { AppServerClient, ClientView } from '../client/app-server';
import { endpointIdentity, sameEndpoint } from './endpoint';
import { validateLocations, type SessionLocation, type WorkspaceCatalog } from './host';

export type AssociationStatus = 'pending' | 'refreshing' | 'ready' | 'unavailable' | 'disconnected' | 'revoked';
/** Display evidence deliberately has no authorized field. */
export interface DisplayAssociation {
  readonly cwd: string;
  readonly confirmed?: { readonly workspaceId?: string };
  readonly status: AssociationStatus;
}
interface Entry extends DisplayAssociation { readonly revision: number }
export interface AssociationSnapshot {
  readonly catalog?: WorkspaceCatalog;
  readonly entries: ReadonlyMap<string, DisplayAssociation>;
  readonly status: AssociationStatus;
}
const EMPTY: AssociationSnapshot = { entries: new Map(), status: 'pending' };
const LIMIT = 128;
interface ClassificationRead {
  readonly epoch: number;
  readonly scope: string;
  readonly generation: number;
  readonly revision: number;
  readonly catalog: WorkspaceCatalog;
  readonly authority: WorkspaceAuthorityObservation;
  readonly rows: readonly { readonly id: string; readonly cwd: string }[];
  readonly controller: AbortController;
}

/** One App-lifetime display projection. Native owns summaries; Host owns registration
 * and admission. At most 128 identities, one 32-row page + selected demand, two
 * active reads per lane and one coalesced pending demand. No persisted data. */
export class WorkspaceAssociations {
  private snapshot: AssociationSnapshot = EMPTY;
  private entries = new Map<string, Entry>();
  private listeners = new Set<() => void>();
  private selected?: string;
  private scope = '';
  private generation = -1;
  private connected = false;
  private revision = 0;
  private classificationEpoch = 0;
  private nextRead = 0;
  private catalogRequest = 0;
  private catalog?: WorkspaceCatalog;
  private catalogStatus: AssociationStatus = 'pending';
  private demand: readonly { id: string; cwd: string }[] = [];
  private demandKey = '';
  private reading = new Map<number, ClassificationRead>();
  private catalogs = new Map<number, AbortController>();
  private catalogQueued = false;
  private releaseAuthority?: () => void;
  private releaseDeletion?: () => void;
  private release?: () => void;
  private disposed = false;
  constructor(private readonly client: AppServerClient, private readonly authority: WorkspaceAuthority) {}
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  getSnapshot = () => this.scope === this.identity(this.client.getSnapshot()) ? this.snapshot : EMPTY;
  start() {
    this.disposed = false;
    this.connected = false;
    this.release = this.client.subscribe(this.observe);
    this.releaseAuthority = this.authority.subscribe(this.retireHost);
    this.releaseDeletion = this.client.subscribeSessionDeletion(id => { this.invalidate(); this.entries.delete(id); this.publish(); this.schedule(); });
    this.observe();
    return () => this.dispose();
  }
  dispose() {
    this.disposed = true; this.releaseAuthority?.(); this.releaseAuthority = undefined; this.release?.(); this.release = undefined; this.releaseDeletion?.(); this.releaseDeletion = undefined;
    this.invalidate(); ++this.catalogRequest;
    for (const controller of this.catalogs.values()) controller.abort();
    this.catalogQueued = false;
  }
  select(id?: string) {
    if (id === this.selected) return;
    this.selected = id;
    this.observe();
  }
  private retireHost = () => {
    this.invalidate(); ++this.revision; ++this.catalogRequest;
    for (const pending of this.catalogs.values()) pending.abort();
    this.catalogQueued = false;
    this.entries.clear();
    const catalog = this.authority.getCatalog();
    this.catalog = catalog && sameEndpoint(catalog.endpoint, this.client.getSnapshot().endpoint) ? catalog : undefined;
    this.catalogStatus = this.catalog ? 'ready' : 'unavailable';
    this.publish(); this.schedule();
  };
  private identity(state: ClientView) {
    return JSON.stringify([state.authorityRevision, state.authorityId, state.endpoint && endpointIdentity(state.endpoint)]);
  }
  private invalidate() {
    ++this.classificationEpoch;
    for (const read of this.reading.values()) read.controller.abort();
  }
  private observe = () => {
    if (this.disposed) return;
    const state = this.client.getSnapshot();
    const scope = this.identity(state);
    const replaced = scope !== this.scope;
    const generationChanged = state.generation !== this.generation;
    const connected = state.connection === 'connected';
    const resumed = connected && !this.connected;
    const connectionChanged = connected !== this.connected;
    if (replaced) {
      this.scope = scope; this.entries.clear(); this.catalog = undefined;
      this.catalogStatus = 'pending'; ++this.catalogRequest;
      for (const controller of this.catalogs.values()) controller.abort();
    }
    if (replaced || generationChanged || connected !== this.connected) this.invalidate();
    this.generation = state.generation; this.connected = connected;
    const rows = state.sessions.slice(0, 32).map(({ id, cwd }) => ({ id, cwd }));
    if (this.selected && !rows.some(row => row.id === this.selected)) {
      const view = state.views[this.selected];
      const cwd = view?.summary?.cwd ?? view?.settings?.cwd;
      if (cwd && !view?.deleting) rows.push({ id: this.selected, cwd });
    }
    const key = JSON.stringify(rows.map(({ id, cwd }) => [id, cwd]).sort());
    const demandChanged = key !== this.demandKey;
    if (demandChanged || replaced) {
      this.demandKey = key; this.demand = rows;
      if (rows.some(row => this.entries.has(row.id) && this.entries.get(row.id)!.cwd !== row.cwd)) this.invalidate();
      for (const row of rows) {
        const old = this.entries.get(row.id);
        this.entries.delete(row.id);
        this.entries.set(row.id, old?.cwd === row.cwd ? old : { cwd: row.cwd, status: 'pending', revision: -1 });
      }
      while (this.entries.size > LIMIT) this.entries.delete(this.entries.keys().next().value!);
    }
    if (replaced || resumed) this.refresh();
    else if (demandChanged || generationChanged || connectionChanged) { this.publish(); this.schedule(); }
  };
  /** Only a completion from this captured Product Host scope may commit display evidence.
   * Navigation, native reconnect and catalog rereads do not retire a Host scope. */
  captureMutation(): (removed?: string) => boolean {
    const observation = this.catalog && this.authority.capture(this.catalog);
    return removed => {
      if (this.disposed || !observation?.current() || !this.catalog || !this.authority.capture(this.catalog)
        || !sameEndpoint(observation.catalog.endpoint, this.client.getSnapshot().endpoint)) return false;
      this.refreshCatalog(removed);
      return true;
    };
  }
  refresh() { this.refreshCatalog(); }
  /** A same-scope committed unregister is stronger than a later failed catalog reread. */
  private refreshCatalog(removed?: string) {
    this.invalidate(); ++this.revision; ++this.catalogRequest;
    if (removed) {
      if (this.catalog) this.catalog = { ...this.catalog, workspaces: this.catalog.workspaces.filter(row => row.id !== removed) };
      for (const [id, entry] of this.entries) if (entry.confirmed?.workspaceId === removed) {
        this.entries.set(id, { ...entry, confirmed: {}, status: 'refreshing', revision: -1 });
      }
    }
    this.catalogStatus = this.catalog ? 'refreshing' : 'pending';
    this.catalogQueued = true;
    for (const controller of this.catalogs.values()) controller.abort();
    this.publish(); this.readCatalog();
  }
  private valid(scope: string, generation: number) {
    const state = this.client.getSnapshot();
    return !this.disposed && scope === this.scope && scope === this.identity(state) && generation === state.generation;
  }
  private readCatalog() {
    if (!this.catalogQueued || this.catalogs.size >= 2 || this.disposed || !this.connected) return;
    this.catalogQueued = false;
    const token = this.catalogRequest, scope = this.scope, generation = this.generation;
    const controller = new AbortController(); this.catalogs.set(token, controller);
    const current = () => this.valid(scope, generation) && token === this.catalogRequest;
    void this.authority.observe(controller.signal).then(({ catalog }) => {
      if (!current()) return;
      if (!sameEndpoint(catalog.endpoint, this.client.getSnapshot().endpoint)) {
        this.invalidate(); this.entries.clear(); this.catalog = undefined; this.catalogStatus = 'unavailable'; this.publish(); return;
      }
      this.catalog = catalog; this.catalogStatus = 'ready';
      const ids = new Set(catalog.workspaces.map(row => row.id));
      for (const [id, entry] of this.entries) if (entry.confirmed?.workspaceId && !ids.has(entry.confirmed.workspaceId)) {
        this.entries.set(id, { ...entry, confirmed: undefined, revision: -1, status: 'pending' });
      }
      this.publish(); this.schedule();
    }).catch(() => {
      if (current()) { this.catalogStatus = 'unavailable'; this.publish(); }
    }).finally(() => { this.catalogs.delete(token); this.readCatalog(); });
  }
  private schedule() {
    if (this.disposed || !this.connected || this.catalogStatus !== 'ready' || !this.catalog) return;
    const covering = [...this.reading.values()].filter(read => this.currentRead(read));
    const rows = this.demand.filter(row => {
      const entry = this.entries.get(row.id);
      const satisfied = entry?.cwd === row.cwd && entry.revision === this.revision;
      return !satisfied && !covering.some(read => read.rows.some(captured => captured.id === row.id && captured.cwd === row.cwd));
    });
    if (!rows.length || this.reading.size >= 2) return;
    const scope = this.scope, generation = this.generation, revision = this.revision, catalog = this.catalog;
    const authority = this.authority.capture(catalog);
    if (!authority) return;
    const endpoint = this.client.getSnapshot().endpoint!;
    const controller = new AbortController(), id = ++this.nextRead;
    const read: ClassificationRead = { epoch: this.classificationEpoch, scope, generation, revision, catalog, authority, rows, controller };
    this.reading.set(id, read);
    const current = () => this.currentRead(read);
    for (const row of rows) {
      const entry = this.entries.get(row.id);
      this.entries.set(row.id, { cwd: row.cwd, confirmed: entry?.cwd === row.cwd ? entry.confirmed : undefined, status: entry?.confirmed ? 'refreshing' : 'pending', revision: -1 });
    }
    this.publish();
    void (async () => {
      const results: SessionLocation[] = [];
      // A selected off-page Session is a separate batch, never a 33rd Host input.
      for (let start = 0; start < rows.length; start += 32) {
        const batch = rows.slice(start, start + 32);
        const locations = await this.authority.classify(batch.map(row => row.cwd), endpoint, authority, controller.signal);
        if (!current()) return;
        validateLocations(locations, batch.length);
        if (locations.some(location => location.authorized && location.workspaceId !== undefined && !catalog.workspaces.some(row => row.id === location.workspaceId))) throw new Error('Invalid Workspace classification');
        results.push(...locations);
      }
      if (!current()) return;
      // Correlation is through this immutable captured vector, never the live page.
      const next = new Map(this.entries);
      rows.forEach((row, index) => {
        const result = results[index], old = next.get(row.id);
        // Eviction or a newer observation cannot be undone by a captured row.
        if (old?.cwd !== row.cwd || old.revision === revision) return;
        next.set(row.id, { cwd: row.cwd, revision,
          confirmed: result.authorized ? { workspaceId: result.workspaceId } : result.reason === 'unavailable' ? old?.confirmed : undefined,
          status: result.authorized ? 'ready' : result.reason === 'unavailable' ? 'unavailable' : 'revoked' });
      });
      this.entries = next; this.publish();
    })().catch(() => {
      if (!current()) return;
      for (const row of rows) {
        const old = this.entries.get(row.id);
        if (old?.cwd !== row.cwd || old.revision === revision) continue;
        this.entries.set(row.id, { cwd: row.cwd, confirmed: old?.confirmed, status: 'unavailable', revision });
      }
      this.publish();
    }).finally(() => { this.reading.delete(id); this.schedule(); });
  }
  private currentRead(read: ClassificationRead) {
    return this.valid(read.scope, read.generation) && this.connected && !read.controller.signal.aborted
      && read.epoch === this.classificationEpoch && read.revision === this.revision && read.catalog === this.catalog && read.authority.current();
  }
  private publish() {
    const entries = new Map<string, DisplayAssociation>();
    for (const [id, entry] of this.entries) entries.set(id, { cwd: entry.cwd, confirmed: entry.confirmed,
      status: !this.connected ? 'disconnected' : this.catalogStatus === 'unavailable' ? 'unavailable'
        : this.catalogStatus !== 'ready' && entry.confirmed ? 'refreshing' : entry.status });
    // Historical entries retain evidence but cannot report current read demand.
    // Connection/catalog observation outranks demand. Within demand, unavailable
    // outranks unsatisfied work, then pending, refreshing, and settled (including
    // a definitive denial). Revision, not promise count, detects queued work.
    let status = !this.connected ? 'disconnected' as const : this.catalogStatus;
    if (status === 'ready') {
      const statuses = this.demand.map(row => {
        const entry = this.entries.get(row.id);
        if (!entry || entry.cwd !== row.cwd) return 'pending';
        if (entry.revision !== this.revision) return entry.confirmed ? 'refreshing' : 'pending';
        return entry.status;
      });
      status = statuses.includes('unavailable') ? 'unavailable' : statuses.includes('pending') ? 'pending'
        : statuses.includes('refreshing') ? 'refreshing' : 'ready';
    }
    this.snapshot = { catalog: this.catalog, entries, status };
    this.listeners.forEach(listener => listener());
  }
}

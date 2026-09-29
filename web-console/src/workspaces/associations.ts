import type { AppServerClient, ClientView } from '../client/app-server';
import { endpointIdentity, sameEndpoint } from './endpoint';
import { WorkspaceHostError, validateLocations, type ProductHostWorkspaces, type SessionLocation, type WorkspaceCatalog } from './host';

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
  private request = 0;
  private catalogRequest = 0;
  private catalog?: WorkspaceCatalog;
  private catalogStatus: AssociationStatus = 'pending';
  private demand: readonly { id: string; cwd: string }[] = [];
  private demandKey = '';
  private reading = new Map<number, AbortController>();
  private catalogs = new Map<number, AbortController>();
  private queued = false;
  private catalogQueued = false;
  private releaseDeletion?: () => void;
  private release?: () => void;
  private disposed = false;
  constructor(private readonly client: AppServerClient, private readonly host: ProductHostWorkspaces) {}
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  getSnapshot = () => this.scope === this.identity(this.client.getSnapshot()) ? this.snapshot : EMPTY;
  start() {
    this.disposed = false;
    this.connected = false;
    this.release = this.client.subscribe(this.observe);
    this.releaseDeletion = this.client.subscribeSessionDeletion(id => { this.invalidate(); this.entries.delete(id); this.publish(); this.schedule(); });
    this.observe();
    return () => this.dispose();
  }
  dispose() {
    this.disposed = true; this.release?.(); this.release = undefined; this.releaseDeletion?.(); this.releaseDeletion = undefined;
    this.invalidate(); ++this.catalogRequest;
    for (const controller of this.catalogs.values()) controller.abort();
    this.catalogQueued = false; this.queued = false;
  }
  select(id?: string) {
    if (id === this.selected) return;
    this.selected = id;
    // Overlapping pending page/selection demand shares its existing read.
    if (!this.reading.has(this.request) && !['pending', 'refreshing'].includes(this.catalogStatus)) this.refresh();
    this.observe();
  }
  private identity(state: ClientView) {
    return JSON.stringify([state.authorityRevision, state.authorityId, state.endpoint && endpointIdentity(state.endpoint)]);
  }
  private invalidate() {
    ++this.request;
    for (const controller of this.reading.values()) controller.abort();
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
  /** A committed unregister is stronger than a later failed catalog reread. */
  refresh(removed?: string) {
    this.invalidate(); ++this.revision; ++this.catalogRequest;
    if (removed) {
      if (this.catalog) this.catalog = { ...this.catalog, workspaces: this.catalog.workspaces.filter(row => row.id !== removed) };
      for (const [id, entry] of this.entries) if (entry.confirmed?.workspaceId === removed) {
        this.entries.set(id, { ...entry, confirmed: {}, status: 'ready', revision: this.revision });
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
    void this.host.listWorkspaces(controller.signal).then(catalog => {
      if (!current()) return;
      if (!catalog || typeof catalog.authorityId !== 'string' || !catalog.authorityId || !Array.isArray(catalog.workspaces)
        || new Set(catalog.workspaces.map(row => row.id)).size !== catalog.workspaces.length
        || catalog.workspaces.some(row => typeof row.id !== 'string' || typeof row.displayName !== 'string')
        || !catalog.picker || !['configured', 'unavailable'].includes(catalog.picker.kind)
        || (catalog.picker.kind === 'configured' && !Array.isArray(catalog.picker.locations))) throw new Error('Invalid Workspace catalog');
      if (this.catalog && this.catalog.authorityId !== catalog.authorityId) this.entries.clear();
      if (!sameEndpoint(catalog.endpoint, this.client.getSnapshot().endpoint)) {
        this.invalidate(); this.entries.clear(); this.catalog = undefined; this.catalogStatus = 'unavailable'; this.publish(); return;
      }
      this.catalog = catalog; this.catalogStatus = 'ready';
      const ids = new Set(catalog.workspaces.map(row => row.id));
      for (const [id, entry] of this.entries) if (entry.confirmed?.workspaceId && !ids.has(entry.confirmed.workspaceId)) {
        this.entries.set(id, { ...entry, confirmed: {}, revision: this.revision, status: 'ready' });
      }
      this.publish(); this.schedule();
    }).catch(() => {
      if (current()) { this.catalogStatus = 'unavailable'; this.publish(); }
    }).finally(() => { this.catalogs.delete(token); this.readCatalog(); });
  }
  private schedule() {
    if (this.disposed || !this.connected || this.catalogStatus !== 'ready' || !this.catalog) return;
    const rows = this.demand.filter(row => {
      const entry = this.entries.get(row.id);
      return !entry || entry.cwd !== row.cwd || entry.revision !== this.revision;
    });
    if (!rows.length) return;
    const token = this.request;
    if (this.reading.has(token)) { this.queued = true; return; }
    if (this.reading.size >= 2) { this.queued = true; return; }
    this.queued = false;
    const scope = this.scope, generation = this.generation, revision = this.revision, catalog = this.catalog;
    const endpoint = this.client.getSnapshot().endpoint!;
    const controller = new AbortController(); this.reading.set(token, controller);
    const current = () => this.valid(scope, generation) && this.connected && token === this.request && revision === this.revision && catalog === this.catalog && rows.every(row => this.entries.get(row.id)?.cwd === row.cwd);
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
        const locations = await this.host.classifyLocations(batch.map(row => row.cwd), endpoint, catalog.authorityId, controller.signal);
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
        next.set(row.id, { cwd: row.cwd, revision,
          confirmed: result.authorized ? { workspaceId: result.workspaceId } : result.reason === 'unavailable' ? old?.confirmed : undefined,
          status: result.authorized ? 'ready' : result.reason === 'unavailable' ? 'unavailable' : 'revoked' });
      });
      this.entries = next; this.publish();
    })().catch(cause => {
      if (!current()) return;
      if (cause instanceof WorkspaceHostError && cause.kind === 'authority_replaced') {
        this.entries.clear(); this.catalog = undefined; this.refresh(); return;
      }
      for (const row of rows) {
        const old = this.entries.get(row.id);
        this.entries.set(row.id, { cwd: row.cwd, confirmed: old?.confirmed, status: 'unavailable', revision });
      }
      this.publish();
    }).finally(() => { this.reading.delete(token); if (this.queued) this.schedule(); });
  }
  private publish() {
    const entries = new Map<string, DisplayAssociation>();
    for (const [id, entry] of this.entries) entries.set(id, { cwd: entry.cwd, confirmed: entry.confirmed,
      status: !this.connected ? 'disconnected' : this.catalogStatus === 'unavailable' ? 'unavailable'
        : this.catalogStatus !== 'ready' && entry.confirmed ? 'refreshing' : entry.status });
    this.snapshot = { catalog: this.catalog, entries, status: !this.connected ? 'disconnected' : this.catalogStatus };
    this.listeners.forEach(listener => listener());
  }
}

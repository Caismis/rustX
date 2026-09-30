import { WorkspaceHostError, validateLocations, type ProductHostWorkspaces, type WorkspaceCatalog } from './host';

export interface WorkspaceAuthorityObservation {
  readonly catalog: WorkspaceCatalog;
  readonly current: () => boolean;
}
/** App-lifetime Product Host authority, independent of all display consumers.
 * Every acceptance of a different identity, including the first, advances the
 * fence. A read started before it may confirm the accepted Host, never replace it. */
export class WorkspaceAuthority {
  private epoch = 0;
  private catalog?: WorkspaceCatalog;
  private listeners = new Set<() => void>();
  constructor(private readonly host: ProductHostWorkspaces) {}
  getCatalog = () => this.catalog;
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  private replaced() { ++this.epoch; this.listeners.forEach(listener => listener()); }
  async observe(signal?: AbortSignal): Promise<WorkspaceAuthorityObservation> {
    const epoch = this.epoch;
    const catalog = await this.host.listWorkspaces(signal);
    if (!catalog || typeof catalog.authorityId !== 'string' || !catalog.authorityId || !Array.isArray(catalog.workspaces)
      || new Set(catalog.workspaces.map(row => row.id)).size !== catalog.workspaces.length
      || catalog.workspaces.some(row => typeof row.id !== 'string' || typeof row.displayName !== 'string')
      || !catalog.picker || !['configured', 'unavailable'].includes(catalog.picker.kind)
      || (catalog.picker.kind === 'configured' && !Array.isArray(catalog.picker.locations))) throw new Error('Invalid Workspace catalog');
    const replaced = this.catalog?.authorityId !== catalog.authorityId;
    if (replaced && epoch !== this.epoch) throw new WorkspaceHostError('Obsolete Workspace authority observation', 'authority_replaced');
    this.catalog = catalog;
    if (replaced) this.replaced();
    const captured = this.epoch;
    return { catalog, current: () => captured === this.epoch };
  }
  async classify(cwds: readonly string[], endpoint: string, observation: WorkspaceAuthorityObservation, signal?: AbortSignal) {
    if (!observation.current()) throw new WorkspaceHostError('Workspace authority replaced', 'authority_replaced');
    try {
      const locations = await this.host.classifyLocations(cwds, endpoint, observation.catalog.authorityId, signal);
      if (!observation.current()) throw new WorkspaceHostError('Workspace authority replaced', 'authority_replaced');
      validateLocations(locations, cwds.length);
      return locations;
    } catch (cause) {
      if (observation.current() && cause instanceof WorkspaceHostError && cause.kind === 'authority_replaced') {
        this.catalog = undefined; this.replaced();
      }
      throw cause;
    }
  }
  capture(): WorkspaceAuthorityObservation | undefined {
    if (!this.catalog) return;
    const epoch = this.epoch;
    return { catalog: this.catalog, current: () => epoch === this.epoch };
  }
}

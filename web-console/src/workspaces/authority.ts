import { sameEndpoint } from './endpoint';
import { WorkspaceHostError, validateLocations, type ProductHostWorkspaces, type WorkspaceAuthorityScope, type WorkspaceCatalog } from './host';

export interface WorkspaceAuthorityObservation {
  readonly scope: WorkspaceAuthorityScope;
  readonly catalog: WorkspaceCatalog;
  readonly current: () => boolean;
}
/** Authority scope: one Product Host process bound to one normalized rustX endpoint. */
const sameAuthority = (left: WorkspaceCatalog | undefined, right: WorkspaceCatalog) =>
  !!left && left.authorityId === right.authorityId && sameEndpoint(left.endpoint, right.endpoint);
/** App-lifetime Product Host authority, independent of all display consumers.
 * Every acceptance of a different scope, including the first, advances the
 * fence. A read started before it may confirm the accepted scope, never replace it. */
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
    if (!catalog || typeof catalog.authorityId !== 'string' || !catalog.authorityId || typeof catalog.endpoint !== 'string' || !URL.canParse(catalog.endpoint) || !Array.isArray(catalog.workspaces)
      || new Set(catalog.workspaces.map(row => row.id)).size !== catalog.workspaces.length
      || catalog.workspaces.some(row => typeof row.id !== 'string' || typeof row.displayName !== 'string')
      || !catalog.picker || !['configured', 'unavailable'].includes(catalog.picker.kind)
      || (catalog.picker.kind === 'configured' && !Array.isArray(catalog.picker.locations))) throw new Error('Invalid Workspace catalog');
    const replaced = !sameAuthority(this.catalog, catalog);
    if (replaced && epoch !== this.epoch) throw new WorkspaceHostError('Obsolete Workspace authority observation', 'authority_replaced');
    this.catalog = catalog;
    if (replaced) this.replaced();
    const captured = this.epoch;
    return { scope: { authorityId: catalog.authorityId, endpoint: catalog.endpoint }, catalog, current: () => captured === this.epoch };
  }
  /** Classification never leaves the observation's endpoint scope. */
  async classify(cwds: readonly string[], endpoint: string, observation: WorkspaceAuthorityObservation, signal?: AbortSignal) {
    if (!observation.current()) throw new WorkspaceHostError('Workspace authority replaced', 'authority_replaced');
    if (!sameEndpoint(observation.catalog.endpoint, endpoint)) throw new Error('No matching rustX endpoint.');
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
  /** Capture the current scope, optionally requiring a display catalog in that scope. */
  capture(catalog?: WorkspaceCatalog): WorkspaceAuthorityObservation | undefined {
    if (!this.catalog || (catalog && !sameAuthority(this.catalog, catalog))) return;
    const epoch = this.epoch;
    return { scope: { authorityId: this.catalog.authorityId, endpoint: this.catalog.endpoint }, catalog: this.catalog, current: () => epoch === this.epoch };
  }
}

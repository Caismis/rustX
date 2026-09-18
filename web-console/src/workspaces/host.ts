import { carrierFetch } from '../carrier/http.ts';
/** Product Host contract. No rustX trust, configuration, or Session ownership. */
export interface ProductHostWorkspace { id: string; displayName: string; location: string; displayPath: string }
export interface WorkspaceCatalog {
  endpoint: string;
  workspaces: ProductHostWorkspace[];
  picker: { kind: 'configured'; locations: { id: string; displayName: string }[] } | { kind: 'unavailable'; reason: string };
}
export type SessionLocation = { authorized: false } | { authorized: true; workspaceId?: string };
export interface ProductHostWorkspaces {
  listWorkspaces(): Promise<WorkspaceCatalog>;
  adoptWorkspace(location: string): Promise<void>;
  renameWorkspace(id: string, displayName: string): Promise<void>;
  reorderWorkspace(id: string, before?: string): Promise<void>;
  removeWorkspace(id: string): Promise<void>;
  resolveWorkspace(id: string, endpoint: string): Promise<{ cwd: string }>;
  /** Authorization is independent of registration. Exact Host-owned classification, bounded to a page. */
  classifyLocations(cwds: readonly string[], endpoint: string): Promise<SessionLocation[]>;
}
export class HttpWorkspaceHost implements ProductHostWorkspaces {
  constructor(private readonly base = '/product-host') {}
  private async call<T>(method: string, body: unknown = {}): Promise<T> {
    const response = await carrierFetch(`${this.base}/${method}`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
    if (!response.ok) throw new Error(`Workspace Host: ${await response.text()}`);
    return response.json();
  }
  listWorkspaces = () => this.call<WorkspaceCatalog>('list');
  adoptWorkspace = (location: string) => this.call<void>('adopt', { location });
  renameWorkspace = (id: string, displayName: string) => this.call<void>('rename', { id, displayName });
  reorderWorkspace = (id: string, before?: string) => this.call<void>('reorder', { id, before });
  removeWorkspace = (id: string) => this.call<void>('remove', { id });
  resolveWorkspace = (id: string, endpoint: string) => this.call<{ cwd: string }>('resolve', { id, endpoint });
  classifyLocations = (cwds: readonly string[], endpoint: string) => this.call<SessionLocation[]>('classify', { cwds, endpoint });
}

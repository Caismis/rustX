/** Product Host contract. No rustX trust, configuration, or Session ownership. */
export interface ProductHostWorkspace { id: string; displayName: string; location: string; displayPath: string }
export interface WorkspaceCatalog {
  endpoint: string;
  workspaces: ProductHostWorkspace[];
  picker: { kind: 'configured'; locations: { id: string; displayName: string }[] } | { kind: 'unavailable'; reason: string };
}
export interface ProductHostWorkspaces {
  listWorkspaces(): Promise<WorkspaceCatalog>;
  adoptWorkspace(location: string): Promise<void>;
  renameWorkspace(id: string, displayName: string): Promise<void>;
  reorderWorkspace(id: string, before?: string): Promise<void>;
  removeWorkspace(id: string): Promise<void>;
  resolveWorkspace(id: string, endpoint: string): Promise<{ cwd: string }>;
  /** Host resolves membership; no browser path-prefix authorization. Bounded to a page. */
  groupSessions(cwds: readonly string[], endpoint: string): Promise<(string | null)[]>;
}
export class HttpWorkspaceHost implements ProductHostWorkspaces {
  constructor(private readonly base = '/product-host') {}
  private async call<T>(method: string, body: unknown = {}): Promise<T> {
    const response = await fetch(`${this.base}/${method}`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
    if (!response.ok) throw new Error(`Workspace Host: ${await response.text()}`);
    return response.json();
  }
  listWorkspaces = () => this.call<WorkspaceCatalog>('list');
  adoptWorkspace = (location: string) => this.call<void>('adopt', { location });
  renameWorkspace = (id: string, displayName: string) => this.call<void>('rename', { id, displayName });
  reorderWorkspace = (id: string, before?: string) => this.call<void>('reorder', { id, before });
  removeWorkspace = (id: string) => this.call<void>('remove', { id });
  resolveWorkspace = (id: string, endpoint: string) => this.call<{ cwd: string }>('resolve', { id, endpoint });
  groupSessions = (cwds: readonly string[], endpoint: string) => this.call<(string | null)[]>('group', { cwds, endpoint });
}

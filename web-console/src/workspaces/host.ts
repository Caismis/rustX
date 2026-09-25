import { carrierFetch } from '../carrier/http.ts';
import type { SourceMutation, SourceSettings } from '../../../protocol/app-server/v23.ts';
export type WorkspaceConfigurationOperation = { kind: 'read' | 'reconcile' } | { kind: 'write'; expected_revision: string; mutation: SourceMutation };
/** The separate authoritative read attempted after a confirmed write. It may
 * succeed or fail without changing the fact that the write committed. */
export type WorkspaceConfigurationReread =
  | { status: 'observed'; projection: SourceSettings }
  | { status: 'failed'; error: unknown };
/** One confirmed native configuration mutation. The acknowledgement is exactly
 * the fact that this mutation committed and the revision it committed at; it is
 * never a projection, an application observation, or a read outcome. */
export interface WorkspaceConfigurationCommit {
  acknowledgement: SourceSettings;
  reread: WorkspaceConfigurationReread;
}
/** A Workspace configuration operation outcome: acknowledgement and
 * authoritative reread stay distinct facts for a write, so a failed reread can
 * never be mistaken for an uncommitted write. */
export type WorkspaceConfigurationResult =
  | { kind: 'read' | 'reconcile'; projection: SourceSettings }
  | { kind: 'write'; commit: WorkspaceConfigurationCommit };
/** Product Host contract. No rustX trust, configuration, or Session ownership. */
export interface ProductHostWorkspace { id: string; displayName: string; location: string; displayPath: string }
export interface WorkspaceCatalog {
  endpoint: string;
  workspaces: ProductHostWorkspace[];
  picker: { kind: 'configured'; locations: { id: string; displayName: string }[] } | { kind: 'unavailable'; reason: string };
}
export type SessionLocation = { authorized: false } | { authorized: true; workspaceId?: string };
/** Typed Product Host failures stay independent of the browser client. */
export class WorkspaceHostError extends Error {
  constructor(message: string, readonly kind?: string, readonly uncertain = false) { super(message); this.name = 'WorkspaceHostError'; }
}
export interface ProductHostWorkspaces {
  configureWorkspace?(id: string, endpoint: string, operation: WorkspaceConfigurationOperation): Promise<WorkspaceConfigurationResult>;
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
    if (!response.ok) {
      const failure = await response.json();
      const nativeError = failure.nativeError ?? (() => {
        if (typeof failure.message !== 'string') return undefined;
        try { return JSON.parse(failure.message).nativeError; } catch { return undefined; }
      })();
      if (nativeError) throw new WorkspaceHostError(nativeError.message ?? String(nativeError), nativeError.data?.kind);
      if (failure.uncertain) throw new WorkspaceHostError(String(failure.message), undefined, true);
      throw new WorkspaceHostError(`Workspace Host: ${failure.message}`);
    }
    return response.json();
  }
  listWorkspaces = () => this.call<WorkspaceCatalog>('list');
  configureWorkspace = (id: string, endpoint: string, operation: WorkspaceConfigurationOperation) => this.call<WorkspaceConfigurationResult>('configuration', { id, endpoint, operation });
  adoptWorkspace = (location: string) => this.call<void>('adopt', { location });
  renameWorkspace = (id: string, displayName: string) => this.call<void>('rename', { id, displayName });
  reorderWorkspace = (id: string, before?: string) => this.call<void>('reorder', { id, before });
  removeWorkspace = (id: string) => this.call<void>('remove', { id });
  resolveWorkspace = (id: string, endpoint: string) => this.call<{ cwd: string }>('resolve', { id, endpoint });
  classifyLocations = (cwds: readonly string[], endpoint: string) => this.call<SessionLocation[]>('classify', { cwds, endpoint });
}

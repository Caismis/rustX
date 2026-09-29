/** Browser HTTP adapter. Never imported by the Node Product Host. */
import { carrierFetch } from '../carrier/http.ts';
import { WorkspaceHostError, validateLocations, type ProductHostWorkspaces, type WorkspaceCatalog, type WorkspaceConfigurationOperation, type WorkspaceConfigurationResult } from './host.ts';

export class HttpWorkspaceHost implements ProductHostWorkspaces {
  constructor(private readonly base = '/product-host') {}
  private async call<T>(method: string, body: unknown = {}, signal?: AbortSignal): Promise<T> {
    const response = await carrierFetch(`${this.base}/${method}`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body), signal });
    if (!response.ok) {
      const failure = await response.json();
      const nativeError = failure.nativeError ?? (() => {
        if (typeof failure.message !== 'string') return undefined;
        try { return JSON.parse(failure.message).nativeError; } catch { return undefined; }
      })();
      if (nativeError) throw new WorkspaceHostError(nativeError.message ?? String(nativeError), nativeError.data?.kind);
      if (failure.uncertain) throw new WorkspaceHostError(String(failure.message), undefined, true);
      throw new WorkspaceHostError(`Workspace Host: ${failure.message}`, failure.kind);
    }
    return response.json();
  }
  listWorkspaces = (signal?: AbortSignal) => this.call<WorkspaceCatalog>('list', {}, signal);
  configureWorkspace = (id: string, endpoint: string, operation: WorkspaceConfigurationOperation) => this.call<WorkspaceConfigurationResult>('configuration', { id, endpoint, operation });
  adoptWorkspace = (location: string) => this.call<void>('adopt', { location });
  renameWorkspace = (id: string, displayName: string) => this.call<void>('rename', { id, displayName });
  reorderWorkspace = (id: string, before?: string) => this.call<void>('reorder', { id, before });
  removeWorkspace = (id: string) => this.call<void>('remove', { id });
  resolveWorkspace = (id: string, endpoint: string) => this.call<{ cwd: string }>('resolve', { id, endpoint });
  classifyLocations = async (cwds: readonly string[], endpoint: string, authorityId?: string, signal?: AbortSignal) => {
    const result = await this.call<unknown>('classify', { cwds, endpoint, authorityId }, signal);
    validateLocations(result, cwds.length);
    return result;
  };
}

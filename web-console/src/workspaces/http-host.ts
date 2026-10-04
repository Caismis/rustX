/** Browser HTTP adapter. Never imported by the Node Product Host. */
import { carrierFetch } from '../carrier/http.ts';
import { WorkspaceHostError, validateLocations, type ProductHostWorkspaces, type WorkspaceAuthorityScope, type WorkspaceCatalog, type WorkspaceConfigurationOperation, type WorkspaceConfigurationResult, type DeliveryRead, type DeliveryBytes } from './host.ts';

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
  desktopCatalog = (scope: WorkspaceAuthorityScope, refresh = false) => this.call<import('./desktop.ts').DesktopCatalog>('desktop-catalog', { scope, refresh });
  openWorkspace = async (scope: WorkspaceAuthorityScope, target: import('./desktop.ts').DesktopTarget, application: import('./desktop.ts').DesktopAppId) => {
    try { return await this.call<import('./desktop.ts').DesktopLaunch>('desktop-open', { scope, target, application }); }
    catch (cause) {
      if (cause instanceof WorkspaceHostError) throw cause;
      throw new WorkspaceHostError('Launch acknowledgement lost; the application may have started. Check the Host desktop before trying again.', undefined, true);
    }
  };
  listWorkspaces = (signal?: AbortSignal) => this.call<WorkspaceCatalog>('list', {}, signal);
  previewDocument = (scope: WorkspaceAuthorityScope, request: import('../client/document-types').DocumentRequest, signal?: AbortSignal) => this.call<import('../client/document-types').DocumentResult>('document-preview', { scope, request }, signal);
  readDelivery = (scope: WorkspaceAuthorityScope, read: DeliveryRead, signal?: AbortSignal) => this.call<DeliveryBytes>('file-read', { scope, read }, signal);
  configureWorkspace = (id: string, endpoint: string, operation: WorkspaceConfigurationOperation) => this.call<WorkspaceConfigurationResult>('configuration', { id, endpoint, operation });
  adoptWorkspace = (scope: WorkspaceAuthorityScope, location: string) => this.call<void>('adopt', { scope, location });
  renameWorkspace = (scope: WorkspaceAuthorityScope, id: string, displayName: string) => this.call<void>('rename', { scope, id, displayName });
  reorderWorkspace = (scope: WorkspaceAuthorityScope, id: string, before?: string) => this.call<void>('reorder', { scope, id, before });
  removeWorkspace = (scope: WorkspaceAuthorityScope, id: string) => this.call<void>('remove', { scope, id });
  resolveWorkspace = (id: string, endpoint: string) => this.call<{ cwd: string }>('resolve', { id, endpoint });
  classifyLocations = async (cwds: readonly string[], endpoint: string, authorityId?: string, signal?: AbortSignal) => {
    const result = await this.call<unknown>('classify', { cwds, endpoint, authorityId }, signal);
    validateLocations(result, cwds.length);
    return result;
  };
}

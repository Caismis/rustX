/** Browser HTTP adapter. Never imported by the Node Product Host. */
import { carrierFetch } from '../carrier/http.ts';
import { WorkspaceHostError, settlementFailureKind, validateLocations, type ProductHostWorkspaces, type WorkspaceAuthorityScope, type WorkspaceCatalog, type WorkspaceConfigurationOperation, type WorkspaceConfigurationResult, type DeliveryRead, type DeliveryBytes } from './host.ts';

export class HttpWorkspaceHost implements ProductHostWorkspaces {
  constructor(private readonly base = '/product-host') {}
  private async call<T>(method: string, body: unknown = {}, signal?: AbortSignal): Promise<T> {
    const response = await carrierFetch(`${this.base}/${method}`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body), signal });
    if (!response.ok) throw hostFailure(await response.json());
    return response.json();
  }
  workbench = (scope: WorkspaceAuthorityScope, call: import('./workbench').WorkbenchCall, signal?: AbortSignal) => this.call<import('./workbench').WorkbenchResult>('workbench', { scope, call }, signal);
  desktopCatalog = (scope: WorkspaceAuthorityScope, refresh = false) => this.call<import('./desktop.ts').DesktopCatalog>('desktop-catalog', { scope, refresh });
  openWorkspace = async (scope: WorkspaceAuthorityScope, target: import('./desktop.ts').DesktopTarget, application: import('./desktop.ts').DesktopAppId) => {
    try { return await this.call<import('./desktop.ts').DesktopLaunch>('desktop-open', { scope, target, application }); }
    catch (cause) {
      if (cause instanceof WorkspaceHostError) throw cause;
      throw new WorkspaceHostError('Launch acknowledgement lost; the application may have started. Check the Host desktop before trying again.', undefined, true);
    }
  };
  listWorkspaces = (signal?: AbortSignal) => this.call<WorkspaceCatalog>('list', {}, signal);
  /** File reads and derivations must retain their HTTP response until physical
   * Host settlement; fetch cancellation is not a settlement witness. */
  private async settledFileCall<T>(kind: 'document' | 'file', scope: WorkspaceAuthorityScope, coordinates: unknown, signal?: AbortSignal): Promise<T> {
    signal?.throwIfAborted();
    const operationId = crypto.randomUUID(), method = kind === 'document' ? 'document-preview' : 'file-read';
    const unknown = `${kind}_settlement_unknown`;
    let response: Response;
    try {
      response = await carrierFetch(`${this.base}/${method}`, { method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ scope, operationId, [kind === 'document' ? 'request' : 'read']: coordinates }) });
    } catch { throw new WorkspaceHostError('File operation settlement unavailable', unknown); }
    if (!response.ok) throw hostFailure(await response.json());
    if (response.headers.get(`X-Rustx-${kind}-Operation`) !== operationId) throw new WorkspaceHostError('File operation settlement unavailable', unknown);
    let cancellation: Promise<void> | undefined;
    const cancel = () => {
      // Cancellation acknowledgement alone does not release admission. Keep
      // observing the original response even if the cancel carrier fails.
      cancellation ??= this.call<void>(`${kind}-cancel`, { scope, operationId });
      void cancellation.catch(() => {});
    };
    signal?.addEventListener('abort', cancel, { once: true });
    if (signal?.aborted) cancel();
    try {
      let result: { ok: boolean; value?: T } & HostFailure;
      try { result = await response.json(); }
      catch { throw new WorkspaceHostError('File operation settlement unavailable', unknown); }
      if (!result || typeof result.ok !== 'boolean' || (result.ok && result.value === undefined)) {
        throw new WorkspaceHostError('File operation settlement unavailable', unknown);
      }
      // Physical uncertainty outranks canceled publication, regardless of which
      // nested owner reported it. A valid terminal envelope preserves that fact.
      const failure = hostFailure(result);
      if (settlementFailureKind(failure)) throw failure;
      signal?.throwIfAborted();
      if (!result.ok || result.value === undefined) throw hostFailure(result);
      return result.value;
    } finally { signal?.removeEventListener('abort', cancel); }
  }
  previewDocument = (scope: WorkspaceAuthorityScope, request: import('../../shared/documents.ts').DocumentRequest, signal?: AbortSignal) =>
    this.settledFileCall<import('../../shared/documents.ts').DocumentResult>('document', scope, request, signal);
  readDelivery = (scope: WorkspaceAuthorityScope, read: DeliveryRead, signal?: AbortSignal) =>
    this.settledFileCall<DeliveryBytes>('file', scope, read, signal);
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

interface HostFailure { message?: unknown; kind?: string; uncertain?: boolean; nativeError?: { message?: string; data?: { kind?: string } } }
function hostFailure(failure: HostFailure) {
  const nativeError = failure.nativeError ?? (() => {
    if (typeof failure.message !== 'string') return undefined;
    try { return JSON.parse(failure.message).nativeError; } catch { return undefined; }
  })();
  if (nativeError) return new WorkspaceHostError(nativeError.message ?? String(nativeError), nativeError.data?.kind);
  if (failure.uncertain) return new WorkspaceHostError(String(failure.message), undefined, true);
  return new WorkspaceHostError(`Workspace Host: ${failure.message}`, failure.kind);
}

/** Browser HTTP adapter. Never imported by the Node Product Host. */
import { carrierFetch } from '../carrier/http.ts';
import { fileResponse } from './file-response';
import { FILE_PREVIEW_LIMITS } from '../../shared/file-preview.ts';
import { FileObservationFeed } from './file-observation';
import { WorkspaceHostError, settlementFailureKind, validateLocations, type ProductHostWorkspaces, type WorkspaceAuthorityScope, type WorkspaceCatalog, type WorkspaceConfigurationOperation, type WorkspaceConfigurationResult, type DeliveryRead, type DeliveryBytes, type WorkflowHostOperation, type WorkflowHostResult } from './host.ts';

export class HttpWorkspaceHost implements ProductHostWorkspaces {
  private observations?: FileObservationFeed;
  constructor(private readonly base = '/product-host') {}
  async observeDelivery(scope: WorkspaceAuthorityScope, read: DeliveryRead, file: import('../../../protocol/app-server/v59.ts').SessionFileReference, changed: () => void, signal: AbortSignal): Promise<import('../../shared/file-observation.ts').PreviewObservation> {
    return this.observeFile(scope, { read, file }, changed, signal);
  }
  observeWorkspaceFile(scope: WorkspaceAuthorityScope, call: import('./workbench.ts').WorkbenchCall, changed: () => void, signal: AbortSignal) {
    return this.observeFile(scope, { call }, changed, signal);
  }
  private async observeFile(scope: WorkspaceAuthorityScope, coordinates: unknown, changed: () => void, signal: AbortSignal): Promise<import('../../shared/file-observation.ts').PreviewObservation> {
    signal.throwIfAborted();
    const operationId = crypto.randomUUID();
    let feed = this.observations;
    if (!feed || feed.scope.authorityId !== scope.authorityId || feed.scope.endpoint !== scope.endpoint) {
      const created = new FileObservationFeed(this.base, scope, () => { if (this.observations === created) this.observations = undefined; });
      feed = this.observations = created;
    }
    const record = feed.add(operationId, () => { if (!signal.aborted) changed(); });
    let cancel: Promise<void> | undefined;
    const abort = () => { cancel ??= this.call<void>('file-observe-cancel', { scope, operationId }); void cancel.catch(() => {}); };
    let dispatched = false;
    try {
      await feed.ready; signal.throwIfAborted();
      let response: Response;
      dispatched = true;
      try { response = await carrierFetch(`${this.base}/file-observe`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ scope, ...coordinates as object, channelId: feed.id, operationId }) }); }
      catch { throw new WorkspaceHostError('File observation settlement unavailable', 'file_settlement_unknown'); }
      if (!response.ok) { dispatched = false; throw hostFailure(await response.json()); }
      if (response.headers.get('X-Rustx-Observation-Operation') !== operationId) throw new WorkspaceHostError('File observation settlement unavailable', 'file_settlement_unknown');
      signal.addEventListener('abort', abort, { once: true }); if (signal.aborted) abort();
      // The independent initial response joins the actual Host probe. Abort
      // never releases that read slot before this settled acknowledgement.
      const result = await response.json().catch(() => { throw new WorkspaceHostError('File observation settlement unavailable', 'file_settlement_unknown'); });
      if (!result.ok) throw hostFailure(result);
      if (typeof result.version !== 'string' || !result.version) throw new Error('Invalid observation version');
      if (signal.aborted || record.ended) { await record.done; feed.assertRetired(record); throw signal.reason ?? new Error('File observation unavailable'); }
      const captured = feed;
      void record.done.then(() => signal.removeEventListener('abort', abort));
      return { version: result.version, done: record.done, close: async () => { abort(); await record.done; captured.assertRetired(record); } };
    } catch (error) {
      signal.removeEventListener('abort', abort);
      if (dispatched) { abort(); if (!settlementFailureKind(error)) await record.done; }
      feed.release(operationId); throw error;
    }
  }
  private async call<T>(method: string, body: unknown = {}, signal?: AbortSignal): Promise<T> {
    const response = await carrierFetch(`${this.base}/${method}`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body), signal });
    if (!response.ok) throw hostFailure(await response.json());
    return response.json();
  }
  workbench = (scope: WorkspaceAuthorityScope, call: import('./workbench').WorkbenchCall, signal?: AbortSignal) => ['preview', 'office'].includes(call.request.kind)
    ? this.settledFileCall<import('./workbench').WorkbenchResult>('workbench', scope, call, signal)
    : this.call<import('./workbench').WorkbenchResult>('workbench', { scope, call }, signal);
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
  private async settledFileCall<T>(kind: 'document' | 'file' | 'workbench', scope: WorkspaceAuthorityScope, coordinates: unknown, signal?: AbortSignal): Promise<T> {
    signal?.throwIfAborted();
    const operationId = crypto.randomUUID(), method = kind === 'document' ? 'document-preview' : kind === 'workbench' ? 'workbench-preview' : 'file-read';
    const unknown = kind === 'workbench' ? 'document_settlement_unknown' : `${kind}_settlement_unknown`;
    let response: Response;
    try {
      response = await carrierFetch(`${this.base}/${method}`, { method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ scope, operationId, [kind === 'document' ? 'request' : kind === 'workbench' ? 'call' : 'read']: coordinates }) });
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
      try { result = await fileResponse(response, kind === 'file' ? FILE_PREVIEW_LIMITS.fileBytes : FILE_PREVIEW_LIMITS.convertedPdfBytes) as typeof result; }
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
  workflowWorkspace = async (scope: WorkspaceAuthorityScope, native: string, id: string, operation: WorkflowHostOperation) => {
    try { return await this.call<WorkflowHostResult>('workflow', { scope, native, id, operation }); }
    catch (cause) {
      // A lost write carrier conceals whether native committed it.
      if (cause instanceof WorkspaceHostError || operation.kind !== 'write') throw cause;
      throw new WorkspaceHostError('Workflow write acknowledgement lost; reread the file before editing.', undefined, true);
    }
  };
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

interface HostFailure { message?: unknown; kind?: string; uncertain?: boolean; nativeError?: { message?: string; data?: import('../../../protocol/app-server/v59.ts').ErrorData } }
function hostFailure(failure: HostFailure) {
  const nativeError = failure.nativeError ?? (() => {
    if (typeof failure.message !== 'string') return undefined;
    try { return JSON.parse(failure.message).nativeError; } catch { return undefined; }
  })();
  if (nativeError) return new WorkspaceHostError(nativeError.message ?? String(nativeError), nativeError.data?.kind, false, nativeError.data);
  if (failure.uncertain) return new WorkspaceHostError(String(failure.message), undefined, true);
  return new WorkspaceHostError(`Workspace Host: ${failure.message}`, failure.kind);
}

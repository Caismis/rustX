/** Local trusted Product Host. This module runs in Node, never in the browser. */
import { deriveWorkspaceOffice } from './documents/operation.ts';
import { observeTerminalOwnership } from './terminal-ownership.ts';
import { WorkspaceTerminals, workspaceFile, withWorkspacePath } from './workbench.ts';
import { OfficeSettlementError } from './documents/office-cgroup.ts';
import { readFileSync, writeFileSync, renameSync, realpathSync, statSync, fstatSync, existsSync } from 'node:fs';
import { isAbsolute, join, relative, resolve } from 'node:path';
import { randomUUID } from 'node:crypto';
import { sameEndpoint } from '../src/workspaces/endpoint.ts';
import { WorkspaceHostError } from '../src/workspaces/host.ts';
import type { ProductHostWorkspaces, WorkspaceAuthorityScope, WorkspaceCatalog, SessionLocation, WorkspaceConfigurationOperation, WorkspaceConfigurationResult, WorkspaceConfigurationReread } from '../src/workspaces/host.ts';
import { AppServerClient } from '../../tui/src/app-server/client.ts';
import { WebSocketTransport } from '../../tui/src/app-server/websocket-transport.ts';
import { DesktopAdapter } from './desktop.ts';
import { readDesktopSession } from './desktop-session.ts';
import type { DesktopAppId, DesktopTarget, DesktopCatalog } from '../src/workspaces/desktop.ts';
import { NativeFileReadError, readNativeDelivery, readNativeSource } from './file-read.ts';

import { deriveDocument } from './documents/operation.ts';
import type { DocumentRequest, DocumentResult } from '../shared/documents.ts';

export interface LocalHostConfig {
  /** Operator attests native runtime and Host share the same filesystem namespace. */
  nativeFilesystem?: 'shared';
  /** Explicit launcher-owned native process supervisor, never browser input. */
  terminalSupervisor?: string;
  transportToken?: string;
  /** Launcher-provisioned secret for native file reads. Never sent to browser. */
  productHostToken?: string;
  endpoint: string;
  roots: { id: string; cwd: string; displayName: string }[];
  picker: boolean;
  /** Operator-owned file outside the browser and rustX runtime root. */
  metadataFile: string;
}
type Registration = { id: string; location: string; displayName: string };
export class LocalWorkspaceHost implements ProductHostWorkspaces {
  private readonly terminals: WorkspaceTerminals;
  private terminalObserver?: Promise<() => Promise<void>>;
  private terminalFailure?: unknown;
  private terminalObserverEnded = false;
  private closing?: Promise<void>;
  private documentReads = new Set<AbortController>();
  private fileReads = new Set<AbortController>();
  private closed = false;
  private readonly authorityId = randomUUID();
  private registrations: Registration[];
  private readonly roots: LocalHostConfig['roots'];
  private readonly config: LocalHostConfig;
  private readonly desktop: DesktopAdapter;
  private readonly readSession: typeof readDesktopSession;
  private readonly observeTerminals: typeof observeTerminalOwnership;
  constructor(config: LocalHostConfig, desktop = new DesktopAdapter(), readSession = readDesktopSession, observeTerminals = observeTerminalOwnership) {
    this.observeTerminals = observeTerminals;
    this.terminals = new WorkspaceTerminals(config.terminalSupervisor);
    this.desktop = desktop; this.readSession = readSession;
    this.config = config;
    if (!isAbsolute(config.metadataFile)) throw new Error('Host metadataFile must be absolute');
    this.roots = config.roots.map(root => {
      if (!isAbsolute(root.cwd) || !statSync(root.cwd).isDirectory()) throw new Error('Host roots must be absolute existing directories');
      return { ...root, cwd: realpathSync(root.cwd) };
    });
    if (new Set(this.roots.map(root => root.id)).size !== this.roots.length || new Set(this.roots.map(root => root.cwd)).size !== this.roots.length) throw new Error('Duplicate Host location');
    this.registrations = existsSync(config.metadataFile) ? JSON.parse(readFileSync(config.metadataFile, 'utf8')) as Registration[]
      : this.roots.map(root => ({ id: randomUUID(), location: root.id, displayName: root.displayName }));
    if (!Array.isArray(this.registrations) || this.registrations.some(row => typeof row.id !== 'string' || typeof row.displayName !== 'string' || !this.roots.some(root => root.id === row.location))) throw new Error('Invalid Host registrations');
    this.commit(this.registrations);
  }
  private commit(rows: Registration[]) {
    const temporary = `${this.config.metadataFile}.${randomUUID()}.tmp`;
    writeFileSync(temporary, JSON.stringify(rows), { mode: 0o600 });
    renameSync(temporary, this.config.metadataFile);
    const previous = new Set(this.registrations.map(row => row.location));
    this.registrations = rows;
    if (previous.size !== new Set(rows.map(row => row.location)).size || rows.some(row => !previous.has(row.location))) {
      for (const read of [...this.fileReads, ...this.documentReads]) read.abort();
    }
  }
  private registered(id: string) {
    const row = this.registrations.find(row => row.id === id);
    if (!row) throw new Error('Unknown Workspace registration');
    return row;
  }
  private cwd(location: string) {
    const root = this.roots.find(root => root.id === location);
    if (!root || realpathSync(root.cwd) !== root.cwd || !statSync(root.cwd).isDirectory()) throw new Error('Authorized location is unavailable');
    return root.cwd;
  }
  private route(endpoint: string) {
    if (!sameEndpoint(endpoint, this.config.endpoint)) throw new Error('Workspace Host belongs to a different rustX process');
  }
  async listWorkspaces(): Promise<WorkspaceCatalog> {
    return { authorityId: this.authorityId, endpoint: this.config.endpoint,
      workspaces: this.registrations.map(row => ({ ...row, displayPath: this.roots.find(root => root.id === row.location)!.cwd })),
      picker: this.config.picker ? { kind: 'configured', locations: this.roots.map(root => ({ id: root.id, displayName: root.displayName })) }
        : { kind: 'unavailable', reason: 'This Host has no directory picker. Ask its operator to configure authorized roots.' } };
  }
  private mutationScope(scope: WorkspaceAuthorityScope) {
    if (this.closed || scope.authorityId !== this.authorityId || !sameEndpoint(scope.endpoint, this.config.endpoint)) {
      throw new WorkspaceHostError('Workspace Host authority replaced', 'authority_replaced');
    }
  }
  close(): Promise<void> {
    this.closed = true;
    for (const read of [...this.fileReads, ...this.documentReads]) read.abort();
    return this.closing ??= (async () => {
      const result = await Promise.allSettled([this.terminals.close(), this.terminalObserver?.then(close => close())]);
      const failures = result.flatMap(item => item.status === 'rejected' ? [item.reason] : []);
      if (this.terminalFailure) failures.push(this.terminalFailure);
      if (failures.length) throw new AggregateError(failures, 'Host terminal settlement failed');
    })();
  }
  private async watchTerminals() {
    if (this.terminalFailure) throw this.terminalFailure;
    await (this.terminalObserver ??= this.observeTerminals(this.config.endpoint, this.config.transportToken!, (session, retiredThrough) => {
      if (this.closed || this.terminalObserverEnded) return;
      void this.terminals.retireOwnership(session, retiredThrough).catch(error => { this.terminalFailure = error; });
    }, () => {
      if (this.closed || this.terminalObserverEnded) return;
      this.terminalObserverEnded = true;
      this.terminalFailure ??= new Error('Native terminal ownership connection ended');
      void this.terminals.close().catch(error => { this.terminalFailure = error; });
    }));
  }
  /** Only currently registered Workspaces authorize bytes. Configured picker
   * locations alone authorize neither an initial nor a historical file read. */
  async readDelivery(scope: WorkspaceAuthorityScope, read: import('../src/workspaces/host.ts').DeliveryRead, signal?: AbortSignal): Promise<import('../src/workspaces/host.ts').DeliveryBytes> {
    this.mutationScope(scope);
    if (!read || !Number.isInteger(read.delivery_index) || read.delivery_index < 0 || read.delivery_index > 7
      || typeof read.message_id !== 'string' || !read.message_id || read.message_id.length > 256
      || !read.target || typeof read.target.session_id !== 'string' || typeof read.target.conversation_id !== 'string'
      || typeof read.target.attachment_id !== 'string' || Object.keys(read).some(key => !['target', 'message_id', 'delivery_index'].includes(key))) throw new Error('Invalid delivery coordinates');
    if (!this.config.productHostToken) throw new Error('Product Host native file mapping unavailable');
    if (this.fileReads.size >= 2) throw new Error('Session file read capacity reached');
    const operation = new AbortController();
    this.fileReads.add(operation);
    let settled = true;
    const abort = () => operation.abort();
    signal?.addEventListener('abort', abort, { once: true });
    try {
      signal?.throwIfAborted();
      const roots = this.registrations.map(row => this.cwd(row.location));
      if (!roots.length || roots.length > 32) throw new Error('Product Host file roots unavailable');
      const bytes = await readNativeDelivery(this.config.endpoint, this.config.productHostToken, read, roots, operation.signal);
      this.mutationScope(scope);
      operation.signal.throwIfAborted();
      // Recheck root availability after asynchronous native work too.
      for (const row of this.registrations) this.cwd(row.location);
      return bytes;
    } catch (cause) {
      if (cause instanceof WorkspaceHostError && cause.kind === 'file_settlement_unknown') settled = false;
      throw cause;
    } finally { signal?.removeEventListener('abort', abort); if (settled) this.fileReads.delete(operation); }
  }
  async previewDocument(scope: WorkspaceAuthorityScope, request: DocumentRequest, signal?: AbortSignal): Promise<DocumentResult> {
    this.mutationScope(scope);
    if (!request || !['docx', 'pptx', 'xlsx'].includes(request.extension) || !/^[a-f0-9]{64}$/.test(request.digest)
      || Object.keys(request).some(key => !['target', 'source', 'extension', 'digest'].includes(key))
      || !request.target || typeof request.target.session_id !== 'string' || typeof request.target.attachment_id !== 'string'
      || !request.source || !['session_file', 'artifact'].includes(request.source.kind)) throw new Error('Invalid document coordinates');
    const source = request.source;
    if (source.kind === 'artifact') {
      if (typeof source.artifact_id !== 'string' || source.artifact_id.length > 256
        || Object.keys(source).some(key => !['kind', 'artifact_id'].includes(key))) throw new Error('Invalid document coordinates');
    } else if (typeof source.message_id !== 'string' || source.message_id.length > 256 || !Number.isInteger(source.delivery_index)
      || source.delivery_index < 0 || source.delivery_index > 7
      || Object.keys(source).some(key => !['kind', 'message_id', 'delivery_index'].includes(key))) throw new Error('Invalid document coordinates');
    if (!this.config.productHostToken) throw new Error('preview_unavailable');
    if (this.documentReads.size) throw new Error('capacity');
    const operation = new AbortController(); this.documentReads.add(operation);
    let settled = true;
    const abort = () => operation.abort(); signal?.addEventListener('abort', abort, { once: true });
    try {
      signal?.throwIfAborted();
      return await deriveDocument(request, async () => {
        this.mutationScope(scope); operation.signal.throwIfAborted();
        if (source.kind === 'session_file') return this.readDelivery(scope, { target: request.target, message_id: source.message_id, delivery_index: source.delivery_index }, operation.signal);
        const result = await readNativeSource(this.config.endpoint, this.config.productHostToken!, request.target, source, [], operation.signal);
        this.mutationScope(scope); operation.signal.throwIfAborted(); return result;
      }, operation.signal);
    } catch (cause) {
      if (cause instanceof OfficeSettlementError) {
        settled = false;
        throw new WorkspaceHostError('converter_unavailable', 'converter_settlement_unknown');
      }
      if (cause instanceof WorkspaceHostError && cause.kind === 'file_settlement_unknown') {
        settled = false;
        throw cause;
      }
      if (cause instanceof NativeFileReadError && cause.error.data?.kind === 'session_file_read') {
        const reason = cause.error.data.reason;
        throw new Error(({ missing: 'source_missing', unauthorized: 'authorization_revoked', unavailable: 'source_unavailable',
          not_regular: 'source_unavailable', replaced: 'source_changed', too_large: 'too_large', capacity: 'capacity', read_failed: 'failure' } as const)[reason]);
      }
      throw cause;
    } finally { signal?.removeEventListener('abort', abort); if (settled) this.documentReads.delete(operation); }
  }
  async workbench(scope: WorkspaceAuthorityScope, call: import('../src/workspaces/workbench.ts').WorkbenchCall, signal?: AbortSignal) {
    this.mutationScope(scope);
    const target = call?.target;
    if (!target || !['session_id', 'active_node'].every(key => typeof target[key as keyof DesktopTarget] === 'string' && /^[a-zA-Z0-9_-]{1,128}$/.test(target[key as keyof DesktopTarget])) || !call.request) throw new Error('Invalid workbench target');
    if (this.config.nativeFilesystem !== 'shared' || !this.config.transportToken) throw new Error('Native filesystem mapping unavailable on this Product Host');
    const read = async () => {
      const native = await this.readSession(this.config.endpoint, this.config.transportToken!, target);
      this.mutationScope(scope); signal?.throwIfAborted();
      if (!this.classifyLocation(native.cwd).authorized) throw new Error('Workspace is not authorized');
      return { ...native, cwd: realpathSync(native.cwd) };
    };
    const request = call.request;
    if (request.kind === 'create' || request.kind === 'input' || request.kind === 'resize' || request.kind === 'poll' || request.kind === 'terminals' || request.kind === 'close') {
      if (request.kind !== 'close') await this.watchTerminals();
      this.mutationScope(scope);
      if (this.terminalFailure) throw this.terminalFailure;
      const value = await this.terminals.request(target, read, request, signal);
      this.mutationScope(scope); signal?.throwIfAborted();
      return value;
    }
    const { cwd: root } = await read();
    if (request.kind === 'resolve') {
      const input = request.path;
      if (typeof input !== 'string' || !input || input.length > 4096 || /[\u0000-\u001f\u007f]/.test(input)) throw new Error('Invalid file reference');
      const path = relative(root, resolve(root, input));
      if (!path || path === '..' || path.startsWith('../') || isAbsolute(path)) throw new Error('File reference is outside this workspace');
      return withWorkspacePath(root, path, false, fd => {
        if (!fstatSync(fd).isFile()) throw new Error('Not a regular file');
        return { path };
      });
    }
    if (request.kind === 'office') {
      const path = request.path, extension = path.split('.').at(-1)?.toLowerCase();
      if (extension !== 'docx' && extension !== 'pptx') throw new Error('converter_unavailable');
      const result = await deriveWorkspaceOffice(extension, async () => {
        this.mutationScope(scope); signal?.throwIfAborted();
        const { cwd: current } = await this.readSession(this.config.endpoint, this.config.transportToken!, target);
        this.mutationScope(scope); signal?.throwIfAborted();
        if (!this.classifyLocation(current).authorized || realpathSync(current) !== root) throw new Error('source_changed');
        return { data: workspaceFile(root, path, true, true).base64! };
      }, signal ?? new AbortController().signal);
      if (result.preview.kind !== 'pdf') throw new Error('converter_failure');
      return { cwd: root, base64: result.preview.data };
    }
    if (request.kind === 'applications') return { applications: this.desktop.catalog(true) };
    if (request.kind === 'open') {
      if (!['files', 'code'].includes(request.application) || typeof request.directory !== 'boolean') throw new Error('Invalid workspace application');
      if (this.desktopPending) throw new Error('A desktop launch is already pending');
      this.desktopPending = true;
      try {
        const launch = this.desktop.prepare(request.application);
        // Authorize this Workspace location now. Desktop launch passes a pathname:
        // an external application may resolve a replaced object later. No descriptor
        // lifetime or pathname recheck can promise identity through that boundary.
        withWorkspacePath(root, request.path, request.directory, fd => {
          if (!request.directory && !fstatSync(fd).isFile()) throw new Error('Not a regular file');
        });
        await launch(root, join(root, request.path), !request.directory);
        return {};
      } finally { this.desktopPending = false; }
    }
    return workspaceFile(root, request.path, request.kind !== 'files', request.kind === 'bytes');
  }
  async desktopCatalog(scope: WorkspaceAuthorityScope, refresh = false): Promise<DesktopCatalog> {
    this.mutationScope(scope);
    if (this.config.nativeFilesystem !== 'shared' || !this.config.transportToken) return { available: false, reason: 'mapping' };
    return this.desktop.catalog(refresh);
  }
  private desktopPending = false;
  async openWorkspace(scope: WorkspaceAuthorityScope, target: DesktopTarget, application: DesktopAppId) {
    this.mutationScope(scope);
    if (!target || typeof target !== 'object' || Array.isArray(target) || Object.keys(target).length !== 2
      || !['session_id', 'active_node'].every(key => typeof target[key as keyof DesktopTarget] === 'string' && /^[a-zA-Z0-9_-]{1,128}$/.test(target[key as keyof DesktopTarget]))
      || !['files', 'terminal', 'code'].includes(application)) throw new Error('Invalid desktop target or application');
    if (this.config.nativeFilesystem !== 'shared' || !this.config.transportToken) throw new Error('Native filesystem mapping unavailable on this Product Host');
    if (this.desktopPending) throw new Error('A desktop launch is already pending');
    this.desktopPending = true;
    try {
      const launch = this.desktop.prepare(application);
      // Native read is the Session admission point. It rejects a retired Session
      // or changed active node. No display path enters this operation.
      const { cwd } = await this.readSession(this.config.endpoint, this.config.transportToken, target);
      this.mutationScope(scope);
      const location = this.classifyLocation(cwd);
      if (!location.authorized) throw new Error(`Workspace directory is ${location.reason}; it cannot be opened`);
      // Classification verifies canonical equality. No await between final Host
      // authority/path admission and the adapter's synchronous spawn call.
      return await launch(realpathSync(cwd));
    } finally { this.desktopPending = false; }
  }
  async adoptWorkspace(scope: WorkspaceAuthorityScope, location: string) {
    this.mutationScope(scope);
    if (!this.config.picker) throw new Error('Directory picker unavailable');
    this.cwd(location);
    if (this.registrations.some(row => row.location === location)) return;
    this.commit([...this.registrations, { id: randomUUID(), location, displayName: this.roots.find(root => root.id === location)!.displayName }]);
  }
  async renameWorkspace(scope: WorkspaceAuthorityScope, id: string, displayName: string) {
    this.mutationScope(scope);
    this.registered(id);
    if (!displayName.trim() || displayName.length > 120) throw new Error('Workspace name must contain 1–120 characters');
    this.commit(this.registrations.map(row => row.id === id ? { ...row, displayName: displayName.trim() } : row));
  }
  async reorderWorkspace(scope: WorkspaceAuthorityScope, id: string, before?: string) {
    this.mutationScope(scope);
    const row = this.registered(id);
    if (before === id) return;
    if (before) this.registered(before);
    const rows = this.registrations.filter(row => row.id !== id);
    rows.splice(before ? rows.findIndex(row => row.id === before) : rows.length, 0, row);
    this.commit(rows);
  }
  /** Per-registration authority lane. A native configuration mutation holds its
   * registration's lane from first resolution to final reread, so removal can
   * never commit while that registration still has a native write in flight.
   * rename/reorder/adopt commit synchronously between awaits and the reads are
   * pure, so they need no lane. */
  private lanes = new Map<string, Promise<unknown>>();
  private lane<T>(id: string, operation: () => Promise<T>): Promise<T> {
    const previous = this.lanes.get(id) ?? Promise.resolve();
    const work = previous.catch(() => {}).then(operation);
    this.lanes.set(id, work);
    return work.finally(() => { if (this.lanes.get(id) === work) this.lanes.delete(id); });
  }
  async removeWorkspace(scope: WorkspaceAuthorityScope, id: string) {
    // Validate inside the lane, when the queued write actually executes.
    return this.lane(id, async () => { this.mutationScope(scope); this.registered(id); this.commit(this.registrations.filter(row => row.id !== id)); });
  }
  async resolveWorkspace(id: string, endpoint: string) { this.route(endpoint); return { cwd: this.cwd(this.registered(id).location) }; }
  async configureWorkspace(id: string, endpoint: string, operation: WorkspaceConfigurationOperation): Promise<WorkspaceConfigurationResult> {
    const run = async (): Promise<WorkspaceConfigurationResult> => {
      const { cwd } = await this.resolveWorkspace(id, endpoint);
      if (!this.config.transportToken) throw new Error('Workspace Host has no native configuration connection');
      const transport = await WebSocketTransport.connect({ endpoint: this.config.endpoint, token: this.config.transportToken });
      const client = await AppServerClient.initialize({ transport, identity: { name: 'rustx-product-host', version: '0.1.0' } }).catch(error => { transport.close(); throw error; });
      try {
        // Resolve again after asynchronous admission, immediately before submission.
        if ((await this.resolveWorkspace(id, endpoint)).cwd !== cwd) throw new Error('Workspace authority changed');
        const target = { kind: 'workspace' as const, directory: cwd };
        if (operation.kind === 'write') {
          // The native write is the linearization point. Once it acknowledges,
          // the mutation is committed and nothing below may turn it into a
          // rejection; the authoritative reread is a separate, independent fact.
          const acknowledgement = await client.call('configuration/sourceWrite', { target, expected_revision: operation.expected_revision, mutation: operation.mutation }, 'source_settings');
          let reread: WorkspaceConfigurationReread;
          try {
            const result = await client.call('configuration/sourcesRead', { target }, 'source_settings');
            reread = (await this.resolveWorkspace(id, endpoint)).cwd === cwd
              ? { status: 'observed', projection: result.projection }
              : { status: 'failed', error: 'Workspace authority changed during the authoritative reread' };
          } catch (error) { reread = { status: 'failed', error: error instanceof Error ? error.message : String(error) }; }
          return { kind: 'write', commit: { acknowledgement: acknowledgement.projection, reread } };
        }
        if (operation.kind === 'reconcile') await client.call('configuration/reconcile', { target }, 'configuration_application');
        else if (operation.kind !== 'read') throw new Error('Unknown configuration operation');
        const result = await client.call('configuration/sourcesRead', { target }, 'source_settings');
        if ((await this.resolveWorkspace(id, endpoint)).cwd !== cwd) throw new Error('Workspace authority changed');
        return { kind: operation.kind, projection: result.projection };
      } finally { await client.close(); }
    };
    return this.lane(id, run);
  }
  async classifyLocations(cwds: readonly string[], endpoint: string, authorityId?: string): Promise<SessionLocation[]> {
    this.route(endpoint);
    if (authorityId !== undefined && authorityId !== this.authorityId) throw new WorkspaceHostError('Workspace Host authority replaced', 'authority_replaced');
    if (cwds.length > 32) throw new Error('Host classification is bounded to 32 summaries');
    return cwds.map(cwd => this.classifyLocation(cwd));
  }
  /** Shared location authority for display classification and desktop admission. */
  private classifyLocation(cwd: string): SessionLocation {
    try {
      const canonical = isAbsolute(cwd) ? realpathSync(cwd) : undefined;
      if (this.roots.some(root => root.cwd === cwd && canonical !== root.cwd)) return { authorized: false, reason: 'denied' };
      const root = this.roots.find(root => root.cwd === canonical);
      if (!root) return { authorized: false, reason: 'denied' };
      this.cwd(root.id);
      const workspaceId = this.registrations.find(row => row.location === root.id)?.id;
      return { authorized: true, ...(workspaceId ? { workspaceId } : {}) };
    } catch { return { authorized: false, reason: 'unavailable' }; }
  }
}

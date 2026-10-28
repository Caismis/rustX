import { materializeSource } from '../../protocol/app-server/source-observation.ts';
import { analysisMatches } from '../../protocol/app-server/workflow-analysis.ts';
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
import type { ProductHostWorkspaces, WorkspaceAuthorityScope, WorkspaceCatalog, SessionLocation, WorkspaceConfigurationOperation, WorkspaceConfigurationResult, WorkspaceConfigurationReread, WorkflowHostOperation, WorkflowHostResult, WorkflowReread } from '../src/workspaces/host.ts';
import { AppServerClient } from '../../tui/src/app-server/client.ts';
import { WebSocketTransport } from '../../tui/src/app-server/websocket-transport.ts';
import { DesktopAdapter } from './desktop.ts';
import { readDesktopSession } from './desktop-session.ts';
import type { DesktopAppId, DesktopTarget, DesktopCatalog } from '../src/workspaces/desktop.ts';
import type { WorkbenchCall, WorkbenchResult } from '../src/workspaces/workbench.ts';
import { NativeFileReadError, readNativeDelivery, readNativeSource, validDeliveryLocator, validateDeliveryRead, validateDeliveryObservation } from './file-read.ts';

import { deriveDocument } from './documents/operation.ts';
import type { DocumentRequest, DocumentResult } from '../shared/documents.ts';
import { FILE_PREVIEW_LIMITS } from '../shared/file-preview.ts';
import { observeFile } from './file-observation.ts';
import { sameSessionFile } from '../shared/session-file-identity.ts';
import { readWorkspacePreview } from './workspace-preview.ts';
import { observeNativeDelivery } from './native-file-observation.ts';

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
  private fileObservations = new Set<AbortController>();
  private fileObservationTasks = new Set<Promise<void>>();
  private fileHints = new Map<AbortController, { session: string; changed: () => void }>();
  private fileOwnership?: Promise<() => Promise<void>>;
  private fileOwnershipLost = false;
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
      for (const read of [...this.fileReads, ...this.documentReads, ...this.fileObservations]) read.abort();
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
    for (const read of [...this.fileReads, ...this.documentReads, ...this.fileObservations]) read.abort();
    return this.closing ??= (async () => {
      const result = await Promise.allSettled([this.terminals.close(), this.terminalObserver?.then(close => close()), this.fileOwnership?.then(close => close()), ...this.fileObservationTasks]);
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
      void this.terminals.retireNative().catch(error => { this.terminalFailure = error; });
    }));
  }
  /** Only currently registered Workspaces authorize bytes. Configured picker
   * locations alone authorize neither an initial nor a historical file read. */
  async readDelivery(scope: WorkspaceAuthorityScope, read: import('../src/workspaces/host.ts').DeliveryRead, signal?: AbortSignal): Promise<import('../src/workspaces/host.ts').DeliveryBytes> {
    return this.readFile(scope, read, FILE_PREVIEW_LIMITS.fileBytes, signal);
  }
  async observeDelivery(scope: WorkspaceAuthorityScope, read: import('../src/workspaces/host.ts').DeliveryRead, file: import('../../protocol/app-server/v60.ts').SessionFileReference, changed: () => void, signal: AbortSignal) {
    this.mutationScope(scope); validateDeliveryObservation(read, file);
    if (this.config.nativeFilesystem !== 'shared') throw new Error('Native filesystem observation unavailable on this Product Host');
    const probe = async (signal: AbortSignal) => {
      const value = await this.readFile(scope, { ...read, read: { kind: 'bytes', range: { offset: 0, length: 1 } } }, FILE_PREVIEW_LIMITS.fileBytes, signal);
      if (!sameSessionFile(value.file, file)) throw new Error('Delivery identity changed');
      return value.version;
    };
    return this.observeAuthorizedFile(scope, async signal => {
      if (!this.config.productHostToken) throw new Error('Product Host native file mapping unavailable');
      const roots = this.registrations.map(row => this.cwd(row.location));
      if (this.fileReads.size >= 2) throw new Error('Session file read capacity reached');
      const admission = new AbortController(); this.fileReads.add(admission);
      let authority: Awaited<ReturnType<typeof observeNativeDelivery>>, settled = true;
      try { authority = await observeNativeDelivery(this.config.endpoint, this.config.productHostToken, read, roots, changed, AbortSignal.any([signal, admission.signal])); }
      catch (error) { if (error instanceof WorkspaceHostError && error.kind === 'file_settlement_unknown') settled = false; throw error; }
      finally { if (settled) this.fileReads.delete(admission); }
      try {
        if (!sameSessionFile(authority.file, file)) throw new Error('Delivery identity changed');
        const root = this.registrations.map(row => this.cwd(row.location)).find(path => {
          const stat = statSync(path, { bigint: true }); return String(stat.dev) === file.scope.device && String(stat.ino) === file.scope.inode;
        });
        if (!root) throw new Error('Authorized observation root unavailable');
        return { path: join(root, file.path), authority };
      } catch (error) { await authority.close(); throw error; }
    }, probe, undefined, changed, signal);
  }
  observeWorkspaceFile(scope: WorkspaceAuthorityScope, call: WorkbenchCall, changed: () => void, signal: AbortSignal) {
    if (call?.request?.kind !== 'preview') throw new Error('Invalid workspace file observation');
    const path = call.request.path;
    const probe = (signal: AbortSignal) => this.workbench(scope, { target: call.target, request: { kind: 'preview', path, read: { kind: 'bytes', range: { offset: 0, length: 1 } } } }, signal);
    return this.observeAuthorizedFile(scope, async signal => {
      const value = await probe(signal);
      if (!value.cwd || !value.preview) throw new Error('Workspace file observation unavailable');
      return { path: join(value.cwd, path) };
    }, async signal => {
      const value = await probe(signal);
      if (!value.preview) throw new Error('Workspace file observation unavailable');
      return value.preview.version;
    }, 'session_id' in call.target ? call.target.session_id : undefined, changed, signal);
  }
  private async observeAuthorizedFile(scope: WorkspaceAuthorityScope, locate: (signal: AbortSignal) => Promise<{ path: string; authority?: import('../shared/file-observation.ts').PreviewObservation }>, probe: (signal: AbortSignal) => Promise<string>, nativeOwnership: string | undefined, changed: () => void, signal: AbortSignal) {
    this.mutationScope(scope); signal.throwIfAborted();
    if (nativeOwnership && this.fileOwnershipLost) throw new Error('Native file ownership observation unavailable');
    if (this.fileObservations.size >= 32) throw new WorkspaceHostError('File observation capacity reached', 'capacity');
    const operation = new AbortController(); this.fileObservations.add(operation);
    let finish!: () => void;
    const retired = new Promise<void>(resolve => { finish = resolve; }); this.fileObservationTasks.add(retired);
    const abort = () => operation.abort(); signal.addEventListener('abort', abort, { once: true });
    const release = () => { signal.removeEventListener('abort', abort); this.fileObservations.delete(operation); this.fileHints.delete(operation); this.fileObservationTasks.delete(retired); finish(); };
    let native: import('../shared/file-observation.ts').PreviewObservation | undefined;
    try {
      if (nativeOwnership) this.fileHints.set(operation, { session: nativeOwnership, changed });
      // Ownership notifications cover original Session deletion/root changes,
      // which are independent of filesystem events. Recheck authority, not
      // content, on a watermark; equal native versions do not refresh a file.
      if (nativeOwnership) await (this.fileOwnership ??= this.observeTerminals(this.config.endpoint, this.config.transportToken!, session => {
        for (const hint of this.fileHints.values()) if (hint.session === session) hint.changed();
      }, () => {
        this.fileOwnershipLost = true;
        for (const observer of this.fileHints.keys()) observer.abort();
      }));
      if (nativeOwnership && this.fileOwnershipLost) throw new Error('Native file ownership observation unavailable');
      // Only a native-authorized committed delivery may supply watch coordinates.
      const target = await locate(operation.signal); native = target.authority;
      void native?.done.then(() => { changed(); operation.abort(); }); operation.signal.throwIfAborted();
      const observation = await observeFile(target.path, () => probe(operation.signal), changed, operation.signal);
      void observation.done.then(() => native?.close());
      const done = Promise.all([observation.done, native?.done]).then(() => { release(); });
      return { version: observation.version, done, close: async () => { operation.abort(); await Promise.all([observation.close(), native?.close()]); await done; } };
    } catch (error) { await native?.close(); release(); throw error; }
  }
  private async readFile(scope: WorkspaceAuthorityScope, read: import('../src/workspaces/host.ts').DeliveryRead, maxBytes: number, signal?: AbortSignal): Promise<import('../src/workspaces/host.ts').DeliveryBytes> {
    this.mutationScope(scope);
    validateDeliveryRead(read);
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
      const bytes = await readNativeDelivery(this.config.endpoint, this.config.productHostToken, read, roots, operation.signal, maxBytes);
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
    if (!request || !['docx', 'pptx'].includes(request.extension)
      || Object.keys(request).some(key => !['target', 'source', 'extension'].includes(key))
      || !request.target || typeof request.target.session_id !== 'string' || typeof request.target.attachment_id !== 'string'
      || !request.source || !['session_file', 'artifact'].includes(request.source.kind)) throw new Error('Invalid document coordinates');
    const source = request.source;
    if (source.agent_id !== undefined && (typeof source.agent_id !== 'string' || !source.agent_id || source.agent_id.length > 256)) throw new Error('Invalid Agent coordinates');
    if (source.kind === 'artifact') {
      if (typeof source.artifact_id !== 'string' || source.artifact_id.length > 256
        || Object.keys(source).some(key => !['kind', 'artifact_id', 'agent_id'].includes(key))) throw new Error('Invalid document coordinates');
    } else if (!validDeliveryLocator(source.locator) || !Number.isSafeInteger(source.delivery_index)
      || source.delivery_index < 0
      || Object.keys(source).some(key => !['kind', 'locator', 'delivery_index', 'agent_id'].includes(key))) throw new Error('Invalid document coordinates');
    if (!this.config.productHostToken) throw new Error('preview_unavailable');
    if (this.documentReads.size) throw new Error('capacity');
    const operation = new AbortController(); this.documentReads.add(operation);
    let settled = true;
    const abort = () => operation.abort(); signal?.addEventListener('abort', abort, { once: true });
    try {
      signal?.throwIfAborted();
      return await deriveDocument(request, async () => {
        this.mutationScope(scope); operation.signal.throwIfAborted();
        if (source.kind === 'session_file') return this.readFile(scope, { target: request.target, locator: source.locator, delivery_index: source.delivery_index, agent_id: source.agent_id }, FILE_PREVIEW_LIMITS.officeBytes, operation.signal);
        const result = await readNativeSource(this.config.endpoint, this.config.productHostToken!, request.target, source, [], operation.signal, { kind: 'bytes' }, FILE_PREVIEW_LIMITS.officeBytes);
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
  async workbench(scope: WorkspaceAuthorityScope, call: WorkbenchCall, signal?: AbortSignal): Promise<WorkbenchResult> {
    if (call?.request?.kind !== 'office') return this.runWorkbench(scope, call, signal);
    this.mutationScope(scope); signal?.throwIfAborted();
    if (this.documentReads.size) throw new Error('capacity');
    const operation = new AbortController(); this.documentReads.add(operation);
    const abort = () => operation.abort(); signal?.addEventListener('abort', abort, { once: true });
    let settled = true;
    try { return await this.runWorkbench(scope, call, operation.signal); }
    catch (error) {
      if (error instanceof OfficeSettlementError) { settled = false; throw new WorkspaceHostError('converter_unavailable', 'converter_settlement_unknown'); }
      throw error;
    } finally { signal?.removeEventListener('abort', abort); if (settled) this.documentReads.delete(operation); }
  }
  private async workspacePreview(scope: WorkspaceAuthorityScope, root: string, path: string, options: import('../shared/file-preview.ts').FileReadOptions, signal: AbortSignal, max = FILE_PREVIEW_LIMITS.fileBytes) {
    this.mutationScope(scope); signal.throwIfAborted();
    if (this.fileReads.size >= 2) throw new WorkspaceHostError('File read capacity reached', 'capacity');
    const operation = new AbortController(); this.fileReads.add(operation);
    const abort = () => operation.abort(); signal.addEventListener('abort', abort, { once: true });
    try { return await readWorkspacePreview(root, path, options, operation.signal, max); }
    finally { signal.removeEventListener('abort', abort); this.fileReads.delete(operation); }
  }
  private async runWorkbench(scope: WorkspaceAuthorityScope, call: WorkbenchCall, signal?: AbortSignal): Promise<WorkbenchResult> {
    this.mutationScope(scope);
    const target = call?.target;
    if (!target || typeof target !== 'object' || Array.isArray(target) || !call.request) throw new Error('Invalid workbench target');
    const keys = 'workspace_id' in target ? ['workspace_id'] : ['session_id', 'active_node'];
    if (Object.keys(target).length !== keys.length || !Object.entries(target).every(([key, value]) => keys.includes(key) && typeof value === 'string' && /^[a-zA-Z0-9_-]{1,128}$/.test(value))) throw new Error('Invalid workbench target');
    if ('session_id' in target && (this.config.nativeFilesystem !== 'shared' || !this.config.transportToken)) throw new Error('Native filesystem mapping unavailable on this Product Host');
    const read = async () => {
      this.mutationScope(scope); signal?.throwIfAborted();
      if ('workspace_id' in target) return { cwd: this.cwd(this.registered(target.workspace_id).location) };
      const native = await this.readSession(this.config.endpoint, this.config.transportToken!, target);
      this.mutationScope(scope); signal?.throwIfAborted();
      if (!this.classifyLocation(native.cwd).authorized) throw new WorkspaceHostError('Workspace is not authorized', 'authorization_revoked');
      return { ...native, cwd: realpathSync(native.cwd) };
    };
    const check = (cwd: string) => {
      this.mutationScope(scope); signal?.throwIfAborted();
      if ('workspace_id' in target && this.cwd(this.registered(target.workspace_id).location) !== cwd) throw new Error('Workspace authority changed');
    };
    const request = call.request;
    if (request.kind === 'create' || request.kind === 'input' || request.kind === 'resize' || request.kind === 'poll' || request.kind === 'terminals' || request.kind === 'close') {
      if ('session_id' in target && request.kind !== 'close') await this.watchTerminals();
      this.mutationScope(scope);
      if ('session_id' in target && this.terminalFailure) throw this.terminalFailure;
      const value = await this.terminals.request(target, read, request, signal);
      this.mutationScope(scope); signal?.throwIfAborted();
      return value;
    }
    const { cwd: root } = await read();
    check(root);
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
        const { cwd: current } = await read();
        check(current);
        if (current !== root) throw new Error('source_changed');
        return this.workspacePreview(scope, root, path, { kind: 'bytes' }, signal ?? new AbortController().signal, FILE_PREVIEW_LIMITS.officeBytes);
      }, signal ?? new AbortController().signal);
      check(root);
      if (result.preview.kind !== 'pdf') throw new Error('converter_failure');
      return { cwd: root, preview: { data: result.preview.data, version: result.digest, offset: 0, bytes: result.preview.data.byteLength, eof: true } };
    }
    if (request.kind === 'preview') {
      const preview = await this.workspacePreview(scope, root, request.path, request.read, signal ?? new AbortController().signal);
      const { cwd: current } = await read(); check(current);
      if (current !== root) throw new Error('source_changed');
      return { cwd: root, preview };
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
    return this.lane(id, async () => {
      this.mutationScope(scope); this.registered(id);
      this.commit(this.registrations.filter(row => row.id !== id));
      await this.terminals.retireWorkspace(id);
    });
  }
  async resolveWorkspace(id: string, endpoint: string) { this.route(endpoint); return { cwd: this.cwd(this.registered(id).location) }; }
  /** One native App Server connection in one registration's authority lane.
   * The registration is resolved before connecting and again immediately
   * before submission; `current` re-resolves it after native work. */
  private nativeLane<T>(id: string, resolve: () => Promise<string>, run: (client: AppServerClient, cwd: string, current: () => Promise<boolean>) => Promise<T>, native?: string): Promise<T> {
    return this.lane(id, async () => {
      const cwd = await resolve();
      if (!this.config.transportToken) throw new Error('Workspace Host has no native configuration connection');
      const transport = await WebSocketTransport.connect({ endpoint: this.config.endpoint, token: this.config.transportToken });
      const client = await AppServerClient.initialize({ transport, identity: { name: 'rustx-product-host', version: '0.1.0' } }).catch(error => { transport.close(); throw error; });
      try {
        // The connection just opened may reach another App Server process at
        // the same endpoint; a transaction bound to one never runs on another.
        if (native !== undefined && client.authorityId !== native) throw new WorkspaceHostError('App Server authority replaced', 'native_replaced');
        // Resolve again after asynchronous admission, immediately before submission.
        if (await resolve() !== cwd) throw new Error('Workspace authority changed');
        return await run(client, cwd, async () => await resolve() === cwd);
      } finally { await client.close(); }
    });
  }
  async configureWorkspace(id: string, endpoint: string, operation: WorkspaceConfigurationOperation): Promise<WorkspaceConfigurationResult> {
    return this.nativeLane(id, async () => (await this.resolveWorkspace(id, endpoint)).cwd, async (client, cwd, current) => {
      const target = { kind: 'workspace' as const, directory: cwd };
      const readSource = async () => {
        const result = await client.call('configuration/sourcesRead', {target}, 'source_settings');
        return materializeSource(result.projection, async(reference,offset)=>(await client.call('configuration/sourceValueRead',{target,reference,offset,max_bytes:65536},'source_value_chunk')).chunk, ()=>true);
      };
      if (operation.kind === 'mcp_probe') {
        const { result } = await client.call('mcp/probe', { target, id: operation.id, expected_revision: operation.expected_revision }, 'mcp_probe');
        if (!await current()) throw new Error('Workspace authority changed');
        return { kind: 'mcp_probe', result };
      }
      if (operation.kind === 'write') {
        // The native write is the linearization point. Once it acknowledges,
        // the mutation is committed and nothing below may turn it into a
        // rejection; the authoritative reread is a separate, independent fact.
        const acknowledgement = await client.call('configuration/sourceWrite', { target, expected_revision: operation.expected_revision, mutation: operation.mutation }, 'sources_written');
        let reread: WorkspaceConfigurationReread;
        try {
          const projection = await readSource();
          reread = await current()
            ? { status: 'observed', projection }
            : { status: 'failed', error: 'Workspace authority changed during the authoritative reread' };
        } catch (error) { reread = { status: 'failed', error: error instanceof Error ? error.message : String(error) }; }
        return { kind: 'write', commit: { acknowledgement: {target, revision:acknowledgement.revision}, reread } };
      }
      if (operation.kind === 'reconcile') await client.call('configuration/reconcile', { target }, 'configuration_receipt');
      else if (operation.kind !== 'read') throw new Error('Unknown configuration operation');
      const projection = await readSource();
      if (!await current()) throw new Error('Workspace authority changed');
      return { kind: operation.kind, projection };
    });
  }
  /** Native Workflow file authoring in exactly this registered Workspace of
   * exactly the Product Host authority that admitted the transaction. A
   * replaced authority is `authority_replaced` even when it reuses the
   * registration ID; a registration that is unknown, unavailable or changed
   * is `workspace_unavailable`. Neither falls back to another directory. */
  async workflowWorkspace(scope: WorkspaceAuthorityScope, native: string, id: string, operation: WorkflowHostOperation): Promise<WorkflowHostResult> {
    validateWorkflowOperation(operation);
    if (typeof native !== 'string' || !native) throw new WorkspaceHostError('Expected native App Server authority', 'native_replaced');
    // Both lane resolutions, including the one immediately before native
    // dispatch, validate the authority before mapping the registration.
    const resolve = async () => {
      this.mutationScope(scope);
      try { return (await this.resolveWorkspace(id, scope.endpoint)).cwd; }
      catch (cause) { throw new WorkspaceHostError(cause instanceof Error ? cause.message : String(cause), 'workspace_unavailable'); }
    };
    return this.nativeLane(id, resolve, async (client, cwd, current) => {
      const target = { kind: 'workspace' as const, directory: cwd };
      const unchanged = async () => {
        if (!await current()) throw new WorkspaceHostError('Workspace authority changed', 'workspace_unavailable');
      };
      switch (operation.kind) {
        case 'list': {
          const { library } = await client.call('workflow/list', { target }, 'workflow_library');
          await unchanged();
          return { kind: 'list', library };
        }
        case 'read': {
          const { source } = await client.call('workflow/read', { target, id: operation.id }, 'workflow_source');
          await unchanged();
          return { kind: 'read', source };
        }
        case 'model_choices': {
          const { choices } = await client.call('workflow/modelChoices', { target, query: operation.query, expected_revision: operation.expected_revision }, 'workflow_model_choices');
          await unchanged();
          return { kind: 'model_choices', choices };
        }
        case 'analyze': {
          const { report } = await client.call('workflow/analyze', { target, id: operation.id, document: operation.document, generation: operation.generation, query: operation.query }, 'workflow_analysis');
          await unchanged();
          if (!analysisMatches(report, target, operation.id, operation.generation, operation.query)) throw new Error('Mismatched Workflow analysis target or query');
          return { kind: 'analyze', report };
        }
        case 'check': {
          const params = operation.scope === 'workspace'
            ? { target, id: operation.id, document: operation.document, explain: operation.explain }
            : { target: { kind: 'user' as const }, id: operation.id, document: operation.document, context: cwd, explain: operation.explain };
          const { report } = await client.call('workflow/check', params, 'workflow_check');
          await unchanged();
          return { kind: 'check', report };
        }
        case 'write': {
          // The native write is the linearization point; its reread is a
          // separate fact that can fail without undoing the commit.
          const acknowledgement = await client.call('configuration/sourceWrite', { target, expected_revision: operation.expected_revision, mutation: { kind: 'workflow', id: operation.id, edit: operation.edit } }, 'sources_written');
          let reread: WorkflowReread;
          try {
            const { source } = await client.call('workflow/read', { target, id: operation.id }, 'workflow_source');
            reread = await current() ? { status: 'observed', source } : { status: 'failed', error: 'Workspace authority changed during the authoritative reread' };
          } catch (error) { reread = { status: 'failed', error: error instanceof Error ? error.message : String(error) }; }
          return { kind: 'write', commit: { acknowledgement: { revision: acknowledgement.revision, configuration_revision: acknowledgement.configuration_revision ?? null }, reread } };
        }
      }
    }, native);
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

/** Browser-supplied Workflow coordinates; native validates the documents. */
function validateWorkflowOperation(operation: WorkflowHostOperation) {
  const value = operation as unknown as Record<string, unknown> | null;
  const id = () => typeof value?.id === 'string' && value.id.length > 0 && value.id.length <= 64;
  const object = (field: unknown) => !!field && typeof field === 'object' && !Array.isArray(field);
  const fields = (allowed: string[]) => Object.keys(value ?? {}).every(key => allowed.includes(key));
  const choicesQuery = (query: unknown) => {
    if (!object(query)) return false;
    const row = query as Record<string, unknown>;
    return Number.isSafeInteger(row.offset) && Number(row.offset) >= 0
      && (row.type === 'models' && Object.keys(row).every(key => ['type', 'offset'].includes(key))
        || row.type === 'profiles' && typeof row.model === 'string' && row.model.length > 0 && row.model.length <= 256 && Object.keys(row).every(key => ['type', 'model', 'offset'].includes(key)));
  };
  const valid = object(value) && (
    value!.kind === 'list' ? fields(['kind'])
      : value!.kind === 'read' ? fields(['kind', 'id']) && id()
        : value!.kind === 'model_choices' ? fields(['kind', 'query', 'expected_revision']) && choicesQuery(value!.query) && (value!.expected_revision == null || typeof value!.expected_revision === 'string')
        : value!.kind === 'analyze' ? fields(['kind', 'id', 'document', 'generation', 'query']) && id() && object(value!.document) && typeof value!.generation === 'string' && value!.generation.length > 0 && value!.generation.length <= 64 && (value!.query == null || object(value!.query))
        : value!.kind === 'check' ? fields(['kind', 'scope', 'id', 'document', 'explain']) && id() && ['workspace', 'user'].includes(value!.scope as string) && object(value!.document) && typeof value!.explain === 'boolean'
          : value!.kind === 'write' && fields(['kind', 'id', 'expected_revision', 'edit']) && id() && typeof value!.expected_revision === 'string' && object(value!.edit));
  if (!valid) throw new Error('Invalid Workflow operation');
}

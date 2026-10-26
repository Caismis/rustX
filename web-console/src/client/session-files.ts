import { canonicalLocatorKey } from '../../../protocol/app-server/content-identity';
import type { CanonicalContentLocator } from '../../../protocol/app-server/v60';
import { sameSessionFile } from '../../shared/session-file-identity.ts';
import { validateRaster } from './raster';
import type { AppServerClient } from './app-server';
import { sameTarget } from '../../../protocol/app-server/attachment-identity';
import { ArtifactResources, safeArtifactMime } from './artifacts';
import type { SessionFileReference } from '../../../protocol/app-server/v60';
import type { ProductHostWorkspaces, WorkspaceAuthorityScope } from '../workspaces/host';
import { settlementFailureKind } from '../workspaces/host';
import type { WorkspaceAuthority } from '../workspaces/authority';
import { PREVIEW_POLICY } from './preview-policy';
import { FILE_PREVIEW_LIMITS, isFileData, type FileReadOptions } from '../../shared/file-preview.ts';
import type { DeliveryBytes } from '../workspaces/host';
import { PreviewContentCache, PreviewContentOwner, type PreviewTextPage, type PreviewBytes } from './preview-content';

export const SESSION_FILE_MAX_BYTES = FILE_PREVIEW_LIMITS.fileBytes;
export const SESSION_FILE_MAX_TRANSFERS = 2;
export const SESSION_FILE_MAX_URLS = PREVIEW_POLICY.urls;
type PreviewOwnership = { owned?: () => boolean; subscribeOwned?: (listener: () => void) => () => void };
export type PreviewSource = PreviewOwnership & ({ kind: 'artifact'; id: string; agentId?: string } | { kind: 'session_file'; conversationId:string; locator: CanonicalContentLocator; index: number; file: SessionFileReference; agentId?: string });
export type LoadedFile = { url: string; bytes?: Uint8Array<ArrayBuffer>; text?: string; error?: string };

/** Equality within a compatible Session scope; never a permission to read. */
export function samePreviewSource(left: PreviewSource, right: PreviewSource): boolean {
  if (left.agentId !== right.agentId) return false;
  if(left.agentId!=null&&left.owned!==right.owned)return false;
  if (left.kind === 'artifact') return right.kind === 'artifact' && left.id === right.id;
  return right.kind === 'session_file' && left.conversationId===right.conversationId && canonicalLocatorKey(left.locator) === canonicalLocatorKey(right.locator) && left.index === right.index
    && sameSessionFile(left.file, right.file);
}

/** Finite visible demand, removed synchronously on abort. An active slot belongs
 * to physical transport settlement, even after its requesting view retires. */
export class PreviewDemand {
  private active = 0;
  private waiting: { start: () => void; cancel: () => void }[] = [];
  private unavailable = false;
  constructor(private readonly capacity: number, private readonly waitingLimit: number, private readonly unavailableCode: string,
    private readonly settlementUnknown: (cause: unknown) => boolean) {}
  run<T>(signal: AbortSignal, work: () => Promise<T>): Promise<T> {
    if (signal.aborted) return Promise.reject(new Error('Obsolete preview demand'));
    if (this.unavailable) return Promise.reject(new Error(this.unavailableCode));
    if (this.active >= this.capacity && this.waiting.length >= this.waitingLimit) return Promise.reject(new Error('capacity'));
    return new Promise<T>((resolve, reject) => {
      const entry = {
        start: () => {
          signal.removeEventListener('abort', entry.cancel);
          if (signal.aborted) { reject(new Error('Obsolete preview demand')); return; }
          this.active++;
          void (async () => {
            try { resolve(await work()); }
            catch (cause) {
              if (this.settlementUnknown(cause)) this.unavailable = true;
              reject(cause);
            } finally {
              this.active--;
              if (this.unavailable) { for (const demand of this.waiting.splice(0)) demand.cancel(); }
              else this.waiting.shift()?.start();
            }
          })();
        },
        cancel: () => {
          const index = this.waiting.indexOf(entry);
          if (index >= 0) this.waiting.splice(index, 1);
          signal.removeEventListener('abort', entry.cancel);
          reject(new Error(this.unavailable ? this.unavailableCode : 'Obsolete preview demand'));
        },
      };
      if (this.active < this.capacity) entry.start();
      else { this.waiting.push(entry); signal.addEventListener('abort', entry.cancel, { once: true }); }
    });
  }
}
// Same Host coordination survives Session changes until retiring work settles.
// A new Host authority gets a new admission domain; no source/result cache exists.
const hostDemand = new WeakMap<ProductHostWorkspaces, { identity: string | undefined; privateReads: PreviewDemand; documents: PreviewDemand }>();
/** Admission never grants authority. The unavailable Host domain still lets
 * managed Artifacts use their independent native ArtifactResources contract. */
export function previewDemands(host: ProductHostWorkspaces, scope: WorkspaceAuthorityScope | undefined) {
  const identity = scope ? JSON.stringify([scope.authorityId, scope.endpoint]) : undefined;
  let value = hostDemand.get(host);
  if (!value || value.identity !== identity) {
    value = { identity, privateReads: new PreviewDemand(SESSION_FILE_MAX_TRANSFERS, PREVIEW_POLICY.activeBodies + PREVIEW_POLICY.downloads, 'File settlement unavailable',
      cause => { const kind = settlementFailureKind(cause); return kind === 'file_settlement_unknown' || kind === 'document_settlement_unknown'; }),
      documents: new PreviewDemand(1, PREVIEW_POLICY.activeBodies, 'converter_unavailable', cause => settlementFailureKind(cause) !== undefined) };
    hostDemand.set(host, value);
  }
  return value;
}

/** One compatible selected Session's transport coordination, never logical tabs.
 * The workspace disposes this owner when Session/runtime/authority/target retires. */
export class FilePreviewCoordinator {
  private readonly artifacts: ArtifactResources;
  private readonly views = new Set<FilePreviewLease>();
  private readonly contents = new Map<number, { source: PreviewSource; owner: PreviewContentOwner }>();
  private downloadLease?: FilePreviewLease;
  private readonly target;
  private readonly generation: number;
  private readonly revision: number | undefined;
  private readonly hostRevision: number;
  private readonly demand;
  private readonly subscriptions: (() => void)[];
  private disposed = false;
  constructor(private readonly client: AppServerClient, private readonly sessionId: string,
    private readonly host: ProductHostWorkspaces, private readonly authority: WorkspaceAuthority, private readonly cache = new PreviewContentCache()) {
    this.target = client.target(sessionId);
    this.generation = client.getSnapshot().generation;
    this.revision = client.getSnapshot().authorityRevision;
    this.hostRevision = authority.getRevision();
    this.artifacts = new ArtifactResources(client, sessionId);
    this.demand = previewDemands(host, authority.capture()?.scope);
    const invalidate = () => { if (!this.current()) this.dispose(); };
    this.subscriptions = [client.subscribe(invalidate), authority.subscribe(invalidate)];
  }
  current = () => !this.disposed && this.generation === this.client.getSnapshot().generation
    && this.revision === this.client.getSnapshot().authorityRevision && this.hostRevision === this.authority.getRevision()
    && sameTarget(this.client.getSnapshot().views[this.sessionId]?.target, this.target);
  get downloading() { return this.downloadLease !== undefined; }
  acquire(occurrenceId: number, source: PreviewSource): FilePreviewLease {
    if (!this.current()) throw new Error('Obsolete preview workspace');
    if (this.views.size >= PREVIEW_POLICY.activeBodies || [...this.views].some(view => view.occurrenceId === occurrenceId)) throw new Error('Preview body capacity reached');
    let retained = this.contents.get(occurrenceId);
    if (retained && !samePreviewSource(retained.source, source)) { this.forget(occurrenceId); retained = undefined; }
    if (!retained) {
      retained = { source, owner: this.contentOwner(source) }; this.contents.set(occurrenceId, retained);
    }
    this.cache.release(retained.owner); retained.owner.activate();
    const lease = new FilePreviewLease(this, occurrenceId, source, retained.owner); this.views.add(lease); return lease;
  }
  private contentOwner(source: PreviewSource) {
    const probe = async (signal: AbortSignal) => {
      if (source.kind === 'artifact') return source.id;
      const result = await this.readSession(source, { kind: 'bytes', range: { offset: 0, length: 1 } }, signal);
      if (!result.version || result.data.byteLength > 1 || result.offset !== 0 || result.lines !== undefined
        || !Number.isSafeInteger(result.bytes) || result.bytes < result.data.byteLength || result.eof !== (result.data.byteLength >= result.bytes)) throw new Error('Invalid file metadata');
      return result.version;
    };
    return new PreviewContentOwner({ current: () => this.sourceCurrent(source), immutable: source.kind === 'artifact',
      page: (offset, signal) => this.readPage(source, offset, signal), bytes: signal => this.read(source, signal),
      document: (extension, signal) => this.derive(source, extension, signal), probe,
      observe: source.kind === 'session_file' && this.host.observeDelivery ? (signal, changed) => this.demand.privateReads.run(signal, async () => {
        const observation = this.authority.capture();
        if (!observation || !this.sourceCurrent(source)) throw new Error('Obsolete file view');
        const watch = await this.host.observeDelivery!(observation.scope, { target: this.target, locator: source.locator, delivery_index: source.index, agent_id: source.agentId }, source.file, changed, signal);
        if (!this.sourceCurrent(source) || !observation.current()) { await watch.close(); throw new Error('Obsolete file view'); }
        return watch;
      }) : undefined });
  }
  forget(occurrenceId: number) {
    const content = this.contents.get(occurrenceId);
    if (content) { this.cache.release(content.owner); content.owner.dispose(); this.contents.delete(occurrenceId); }
  }
  /** Independent original-byte intent: no logical workspace/pane mutation. */
  async download(source: PreviewSource, name: string, mime?: string): Promise<void> {
    if (!this.current()) throw new Error('Obsolete preview workspace');
    if (this.downloadLease) throw new Error('Download capacity reached');
    const lease = this.downloadLease = new FilePreviewLease(this, 0, source);
    try {
      const { url } = await lease.load(mime, false, undefined, true);
      if (!lease.current()) throw new Error('Obsolete download');
      const anchor = document.createElement('a'); anchor.href = url; anchor.download = name; anchor.click();
    } finally { lease.dispose(); }
  }
  retire(lease: FilePreviewLease) {
    this.views.delete(lease);
    const content = this.contents.get(lease.occurrenceId);
    if (content) { content.owner.deactivate(); this.cache.retain(content.owner); }
    if (this.downloadLease === lease) this.downloadLease = undefined;
  }
  sourceCurrent(source:PreviewSource):boolean {
    return this.current()&&(!source.owned||source.owned())&&(source.kind==='artifact'||(source.agentId!=null?source.owned!=null:source.conversationId===this.target.conversation_id));
  }
  async read(source: PreviewSource, signal: AbortSignal): Promise<PreviewBytes> {
    if (!this.sourceCurrent(source) || signal.aborted) throw new Error('Obsolete file view');
    // Public artifact/read owns its separate ArtifactResources transfer budget;
    // it never acquires the Product Host private native read capacity.
    if (source.kind === 'artifact') return { data: await this.artifacts.readBytes(source.id, signal, source.agentId), version: source.id };
    const result = await this.readSession(source, { kind: 'bytes' }, signal);
    if (result.data.byteLength > SESSION_FILE_MAX_BYTES || result.data.byteLength !== result.bytes || result.offset !== 0 || !result.eof || result.lines !== undefined) throw new Error('Invalid complete file response');
    return { data: result.data, version: result.version };
  }
  async readPage(source: PreviewSource, offset: number, signal: AbortSignal): Promise<PreviewTextPage> {
    if (source.kind === 'artifact') {
      const bytes = await this.read(source, signal);
      const text = new TextDecoder('utf-8', { fatal: true }).decode(bytes.data);
      return { text, version: source.id, offset: 1, lines: text ? text.split('\n').length : 0, eof: true };
    }
    const result = await this.readSession(source, { kind: 'text', offset, limit: FILE_PREVIEW_LIMITS.pageLines }, signal);
    if (result.data.byteLength > FILE_PREVIEW_LIMITS.pageBytes || result.offset !== offset
      || !Number.isSafeInteger(result.lines) || result.lines! < 0 || result.lines! > FILE_PREVIEW_LIMITS.pageLines
      || !Number.isSafeInteger(result.offset + result.lines!) || typeof result.eof !== 'boolean'
      || (!result.eof && result.lines === 0) || typeof result.version !== 'string' || !result.version) throw new Error('Invalid text page');
    const text = new TextDecoder('utf-8', { fatal: true }).decode(result.data);
    if (text.includes('\0')) throw new Error('File is not text');
    return { text, version: result.version, offset, lines: result.lines!, eof: result.eof };
  }
  private async readSession(source: Extract<PreviewSource, { kind: 'session_file' }>, read: FileReadOptions, signal: AbortSignal): Promise<DeliveryBytes> {
    return this.demand.privateReads.run(signal, async () => {
      if (!this.sourceCurrent(source) || signal.aborted) throw new Error('Obsolete file view');
      if (!this.host.readDelivery) throw new Error('Product Host file mapping unavailable');
      const observation = this.authority.capture();
      if (!observation) throw new Error('Product Host authority unavailable');
      const result = await this.host.readDelivery(observation.scope, { target: this.target, locator: source.locator, delivery_index: source.index, agent_id: source.agentId, read }, signal);
      if (!this.sourceCurrent(source) || signal.aborted || !observation.current()) throw new Error('Obsolete file response');
      if (!sameSessionFile(result.file, source.file)) throw new Error('Delivery identity changed');
      if (!isFileData(result.data)) throw new Error('Invalid file bytes');
      return result;
    });
  }
  async derive(source: PreviewSource, extension: 'docx' | 'pptx', signal: AbortSignal) {
    // Both source kinds reauthorize through the private native/Host read budget.
    // Keep one read permit for its whole lifetime so Session-file Download cannot race that
    // internal reread even while the converter itself is between read phases.
    return this.demand.documents.run(signal, () => this.demand.privateReads.run(signal, async () => {
      const current = () => { if (!this.sourceCurrent(source) || signal.aborted) throw new Error('obsolete'); };
      current();
      if (!this.host.previewDocument) throw new Error('preview_unavailable');
      const observation = this.authority.capture();
      if (!observation) throw new Error('authority_replaced');
      const result = await this.host.previewDocument(observation.scope, { target: this.target, extension,
        source: source.kind === 'artifact' ? { kind: 'artifact', artifact_id: source.id, agent_id: source.agentId }
          : { kind: 'session_file', locator: source.locator, delivery_index: source.index, agent_id: source.agentId },
      }, signal);
      current();
      if (!observation.current()) throw new Error('obsolete');
      if (!/^[a-f0-9]{64}$/.test(result.digest) || (source.kind === 'session_file' && !sameSessionFile(result.file, source.file))) throw new Error('source_changed');
      if (result.preview.kind !== 'pdf' || !isFileData(result.preview.data)) throw new Error('malformed');
      if (result.preview.data.byteLength > FILE_PREVIEW_LIMITS.convertedPdfBytes) throw new Error('too_large');
      return { ...result.preview, version: result.digest };
    }));
  }
  dispose() {
    if (this.disposed) return;
    this.disposed = true;
    this.subscriptions.forEach(stop => stop());
    for (const view of this.views) view.dispose();
    for (const id of this.contents.keys()) this.forget(id);
    this.downloadLease?.dispose(); this.artifacts.dispose();
  }
}

/** Exactly one active occurrence or transient Download. No resource outlives
 * disposal: its signal is retired before URLs are revoked and admission released. */
export class FilePreviewLease {
  private readonly abort = new AbortController();
  private url?: string;
  private reading = false;
  private deriving = false;
  readonly signal = this.abort.signal;
  constructor(private readonly owner: FilePreviewCoordinator, readonly occurrenceId: number, readonly source: PreviewSource, readonly content?: PreviewContentOwner) {}
  current = () => !this.signal.aborted && this.owner.sourceCurrent(this.source);
  async readData(signal: AbortSignal): Promise<PreviewBytes> {
    if (!this.current() || signal.aborted) throw new Error('Obsolete file view');
    if (this.reading) throw new Error('File lease capacity reached');
    this.reading = true;
    try { return await this.owner.read(this.source, AbortSignal.any([this.signal, signal])); }
    finally { this.reading = false; }
  }
  retain(bytes: Uint8Array<ArrayBuffer>, mime?: string): string {
    if (!this.current() || this.url) throw new Error('File lease capacity reached');
    return this.url = URL.createObjectURL(new Blob([bytes], { type: safeArtifactMime(mime) }));
  }
  async load(mime?: string, image = false, signal?: AbortSignal, retainBytes = false): Promise<LoadedFile> {
    if (!this.current()) throw new Error('Obsolete file view');
    if (this.reading || this.url) throw new Error('File lease capacity reached');
    this.reading = true;
    const active = signal ? AbortSignal.any([this.signal, signal]) : this.signal;
    try {
      const { data: bytes } = await this.owner.read(this.source, active);
      if (!this.current() || active.aborted) throw new Error('Obsolete file response');
      const url = this.url = URL.createObjectURL(new Blob([bytes], { type: safeArtifactMime(mime) }));
      if (retainBytes) return { url, bytes };
      if (image) { try { validateRaster(bytes); } catch (cause) { return { url, error: String(cause) }; } }
      return { url };
    } finally { this.reading = false; }
  }
  async loadPage(offset: number, signal: AbortSignal): Promise<PreviewTextPage> {
    if (!this.current() || signal.aborted) throw new Error('Obsolete file view');
    if (this.reading) throw new Error('File lease capacity reached');
    this.reading = true;
    try { return await this.owner.readPage(this.source, offset, AbortSignal.any([this.signal, signal])); }
    finally { this.reading = false; }
  }
  async derive(extension: 'docx' | 'pptx', signal: AbortSignal) {
    if (!this.current() || signal.aborted) throw new Error('obsolete');
    if (this.deriving) throw new Error('capacity');
    this.deriving = true;
    try { return await this.owner.derive(this.source, extension, AbortSignal.any([this.signal, signal])); }
    finally { this.deriving = false; }
  }
  release(url: string) { if (this.url === url) { this.url = undefined; URL.revokeObjectURL(url); } }
  dispose() {
    if (this.signal.aborted) return;
    this.abort.abort();
    if (this.url) this.release(this.url);
    this.owner.retire(this);
  }
}

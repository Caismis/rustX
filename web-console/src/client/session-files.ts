import { sameSessionFile } from '../../shared/session-file-identity.ts';
import { validateRaster } from './raster';
import type { AppServerClient } from './app-server';
import { sameTarget } from './app-server';
import { ArtifactResources, isTextMime, safeArtifactMime } from './artifacts';
import type { SessionFileReference } from '../../../protocol/app-server/v41';
import type { ProductHostWorkspaces } from '../workspaces/host';
import { settlementFailureKind } from '../workspaces/host';
import type { WorkspaceAuthority } from '../workspaces/authority';
import { PREVIEW_POLICY } from './preview-policy';

export const SESSION_FILE_MAX_BYTES = 512 * 1024;
export const SESSION_FILE_MAX_BASE64 = Math.ceil(SESSION_FILE_MAX_BYTES / 3) * 4;
export const SESSION_FILE_MAX_TRANSFERS = 2;
export const SESSION_FILE_MAX_URLS = PREVIEW_POLICY.urls;
export const FILE_RENDER_MAX_BYTES = 512 * 1024;
export type PreviewSource = { kind: 'artifact'; id: string; agentId?: string } | { kind: 'session_file'; messageId: string; index: number; file: SessionFileReference; agentId?: string };
export type LoadedFile = { url: string; bytes?: Uint8Array<ArrayBuffer>; text?: string; error?: string };

/** Equality within a compatible Session scope; never a permission to read. */
export function samePreviewSource(left: PreviewSource, right: PreviewSource): boolean {
  if (left.agentId !== right.agentId) return false;
  if (left.kind === 'artifact') return right.kind === 'artifact' && left.id === right.id;
  return right.kind === 'session_file' && left.messageId === right.messageId && left.index === right.index
    && sameSessionFile(left.file, right.file);
}

/** Finite visible demand, removed synchronously on abort. An active slot belongs
 * to physical transport settlement, even after its requesting view retires. */
class PreviewDemand {
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
const hostDemand = new WeakMap<ProductHostWorkspaces, { revision: number; privateReads: PreviewDemand; documents: PreviewDemand }>();
function demands(host: ProductHostWorkspaces, authority: WorkspaceAuthority) {
  let value = hostDemand.get(host);
  if (!value || value.revision !== authority.getRevision()) {
    value = { revision: authority.getRevision(), privateReads: new PreviewDemand(SESSION_FILE_MAX_TRANSFERS, PREVIEW_POLICY.activeBodies + PREVIEW_POLICY.downloads, 'File settlement unavailable',
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
  private downloadLease?: FilePreviewLease;
  private readonly target;
  private readonly generation: number;
  private readonly revision: number | undefined;
  private readonly hostRevision: number;
  private readonly demand;
  private disposed = false;
  constructor(private readonly client: AppServerClient, private readonly sessionId: string,
    private readonly host: ProductHostWorkspaces, private readonly authority: WorkspaceAuthority) {
    this.target = client.target(sessionId);
    this.generation = client.getSnapshot().generation;
    this.revision = client.getSnapshot().authorityRevision;
    this.hostRevision = authority.getRevision();
    this.artifacts = new ArtifactResources(client, sessionId);
    this.demand = demands(host, authority);
  }
  current = () => !this.disposed && this.generation === this.client.getSnapshot().generation
    && this.revision === this.client.getSnapshot().authorityRevision && this.hostRevision === this.authority.getRevision()
    && sameTarget(this.client.getSnapshot().views[this.sessionId]?.target, this.target);
  acquire(occurrenceId: number, source: PreviewSource): FilePreviewLease {
    if (!this.current()) throw new Error('Obsolete preview workspace');
    if (this.views.size >= PREVIEW_POLICY.activeBodies || [...this.views].some(view => view.occurrenceId === occurrenceId)) throw new Error('Preview body capacity reached');
    const lease = new FilePreviewLease(this, occurrenceId, source); this.views.add(lease); return lease;
  }
  /** Independent original-byte intent: no logical workspace/pane mutation. */
  async download(source: PreviewSource, name: string, mime?: string): Promise<void> {
    if (!this.current()) throw new Error('Obsolete preview workspace');
    if (this.downloadLease) throw new Error('Download capacity reached');
    const lease = this.downloadLease = new FilePreviewLease(this, 0, source);
    try {
      const { url } = await lease.load(mime);
      if (!lease.current()) throw new Error('Obsolete download');
      const anchor = document.createElement('a'); anchor.href = url; anchor.download = name; anchor.click();
    } finally { lease.dispose(); }
  }
  retire(lease: FilePreviewLease) {
    this.views.delete(lease);
    if (this.downloadLease === lease) this.downloadLease = undefined;
  }
  async read(source: PreviewSource, signal: AbortSignal): Promise<Uint8Array<ArrayBuffer>> {
    if (!this.current() || signal.aborted) throw new Error('Obsolete file view');
    // Public artifact/read owns its separate ArtifactResources transfer budget;
    // it never acquires the Product Host private native read capacity.
    if (source.kind === 'artifact') return this.artifacts.readBytes(source.id, signal, source.agentId);
    return this.demand.privateReads.run(signal, async () => {
      if (!this.current() || signal.aborted) throw new Error('Obsolete file view');
      if (!this.host.readDelivery) throw new Error('Product Host file mapping unavailable');
      const observation = this.authority.capture();
      if (!observation) throw new Error('Product Host authority unavailable');
      const result = await this.host.readDelivery(observation.scope, { target: this.target, message_id: source.messageId, delivery_index: source.index, agent_id: source.agentId }, signal);
      if (!this.current() || signal.aborted || !observation.current()) throw new Error('Obsolete file response');
      if (!sameSessionFile(result.file, source.file)) throw new Error('Delivery identity changed');
      if (result.data.length > SESSION_FILE_MAX_BASE64) throw new Error('Session file exceeds 512 KiB');
      const decoded = atob(result.data);
      if (decoded.length > SESSION_FILE_MAX_BYTES) throw new Error('Session file exceeds 512 KiB');
      return Uint8Array.from(decoded, c => c.charCodeAt(0));
    });
  }
  async derive(source: PreviewSource, extension: 'docx' | 'pptx' | 'xlsx', bytes: Uint8Array<ArrayBuffer>, signal: AbortSignal) {
    // Both source kinds reauthorize through the private native/Host read budget.
    // Keep one read permit for its whole lifetime so Session-file Download cannot race that
    // internal reread even while the converter itself is between read phases.
    return this.demand.documents.run(signal, () => this.demand.privateReads.run(signal, async () => {
      const current = () => { if (!this.current() || signal.aborted) throw new Error('obsolete'); };
      current();
      if (!this.host.previewDocument) throw new Error('preview_unavailable');
      const observation = this.authority.capture();
      if (!observation) throw new Error('authority_replaced');
      const hash = await crypto.subtle.digest('SHA-256', bytes); current();
      const digest = [...new Uint8Array(hash)].map(v => v.toString(16).padStart(2, '0')).join('');
      const result = await this.host.previewDocument(observation.scope, { target: this.target, extension, digest,
        source: source.kind === 'artifact' ? { kind: 'artifact', artifact_id: source.id, agent_id: source.agentId }
          : { kind: 'session_file', message_id: source.messageId, delivery_index: source.index, agent_id: source.agentId },
      }, signal);
      current();
      if (!observation.current()) throw new Error('obsolete');
      if (result.digest !== digest || (source.kind === 'session_file' && !sameSessionFile(result.file, source.file))) throw new Error('source_changed');
      return result.preview;
    }));
  }
  dispose() {
    if (this.disposed) return;
    this.disposed = true;
    for (const view of this.views) view.dispose();
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
  constructor(private readonly owner: FilePreviewCoordinator, readonly occurrenceId: number, readonly source: PreviewSource) {}
  current = () => !this.signal.aborted && this.owner.current();
  async load(mime?: string, image = false, signal?: AbortSignal, retainBytes = false): Promise<LoadedFile> {
    if (!this.current()) throw new Error('Obsolete file view');
    if (this.reading || this.url) throw new Error('File lease capacity reached');
    this.reading = true;
    const active = signal ? AbortSignal.any([this.signal, signal]) : this.signal;
    try {
      const bytes = await this.owner.read(this.source, active);
      if (!this.current() || active.aborted) throw new Error('Obsolete file response');
      const url = this.url = URL.createObjectURL(new Blob([bytes], { type: safeArtifactMime(mime) }));
      if (retainBytes) return { url, bytes };
      if (isTextMime(mime)) {
        if (bytes.length > FILE_RENDER_MAX_BYTES) return { url, error: 'File exceeds rendered text policy' };
        try { return { url, text: new TextDecoder('utf-8', { fatal: true }).decode(bytes) }; }
        catch { return { url, error: 'File is not valid UTF-8' }; }
      }
      if (image) { try { validateRaster(bytes); } catch (cause) { return { url, error: String(cause) }; } }
      return { url };
    } finally { this.reading = false; }
  }
  async derive(extension: 'docx' | 'pptx' | 'xlsx', bytes: Uint8Array<ArrayBuffer>, signal: AbortSignal) {
    if (!this.current() || signal.aborted) throw new Error('obsolete');
    if (this.deriving) throw new Error('capacity');
    this.deriving = true;
    try { return await this.owner.derive(this.source, extension, bytes, AbortSignal.any([this.signal, signal])); }
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

import { validateRaster } from './raster';
import type { AppServerClient } from './app-server';
import { sameTarget } from './app-server';
import { ArtifactResources, isTextMime, safeArtifactMime } from './artifacts';
import type { SessionFileReference } from '../../../protocol/app-server/v34';
import type { ProductHostWorkspaces } from '../workspaces/host';
import type { WorkspaceAuthority } from '../workspaces/authority';

export const SESSION_FILE_MAX_BYTES = 512 * 1024;
export const SESSION_FILE_MAX_BASE64 = Math.ceil(SESSION_FILE_MAX_BYTES / 3) * 4;
export const SESSION_FILE_MAX_TRANSFERS = 2;
export const SESSION_FILE_MAX_URLS = 2;
export const FILE_RENDER_MAX_BYTES = 512 * 1024;
export type PreviewSource = { kind: 'artifact'; id: string } | { kind: 'session_file'; messageId: string; index: number; file: SessionFileReference };
export type LoadedFile = { url: string; text?: string; error?: string };

/** One selected native view. Separate concrete owners, one presentation seat. */
export class FilePreviewResources {
  readonly artifacts: ArtifactResources;
  private urls = new Set<string>();
  private reads = new Set<AbortController>();
  private disposed = false;
  constructor(private client: AppServerClient, private sessionId: string, private host: ProductHostWorkspaces, private authority: WorkspaceAuthority) {
    this.artifacts = new ArtifactResources(client, sessionId);
  }
  async load(source: PreviewSource, mime?: string, image = false, signal?: AbortSignal): Promise<LoadedFile> {
    if (source.kind === 'artifact') return this.artifacts.load(source.id, mime, image, signal);
    if (this.disposed) throw new Error('Obsolete file view');
    if (this.reads.size >= SESSION_FILE_MAX_TRANSFERS || this.urls.size + this.reads.size >= SESSION_FILE_MAX_URLS) throw new Error('File capacity reached; close a preview and retry.');
    if (!this.host.readDelivery) throw new Error('Product Host file mapping unavailable');
    const observation = this.authority.capture();
    if (!observation) throw new Error('Product Host authority unavailable');
    const target = this.client.target(this.sessionId);
    const revision = this.client.getSnapshot().authorityRevision;
    const read = new AbortController();
    this.reads.add(read);
    try {
      const result = await this.host.readDelivery(observation.scope, { target, message_id: source.messageId, delivery_index: source.index }, signal ? AbortSignal.any([read.signal, signal]) : read.signal);
      if (this.disposed || read.signal.aborted || signal?.aborted || !observation.current() || revision !== this.client.getSnapshot().authorityRevision
        || !sameTarget(this.client.getSnapshot().views[this.sessionId]?.target, target)) throw new Error('Obsolete file response');
      const expected = source.file, actual = result.file;
      if (actual.scope.conversation_id !== expected.scope.conversation_id || actual.scope.device !== expected.scope.device || actual.scope.inode !== expected.scope.inode
        || actual.path !== expected.path || actual.name !== expected.name || actual.mime_type !== expected.mime_type || (actual.description ?? null) !== (expected.description ?? null)) throw new Error('Delivery identity changed');
      if (result.data.length > SESSION_FILE_MAX_BASE64) throw new Error('Session file exceeds 512 KiB');
      const decoded = atob(result.data);
      if (decoded.length > SESSION_FILE_MAX_BYTES) throw new Error('Session file exceeds 512 KiB');
      const bytes = Uint8Array.from(decoded, c => c.charCodeAt(0));
      const url = URL.createObjectURL(new Blob([bytes], { type: safeArtifactMime(mime) }));
      this.urls.add(url);
      if (isTextMime(mime)) {
        if (bytes.length > FILE_RENDER_MAX_BYTES) return { url, error: 'File exceeds rendered text policy' };
        try { return { url, text: new TextDecoder('utf-8', { fatal: true }).decode(bytes) }; }
        catch { return { url, error: 'File is not valid UTF-8' }; }
      }
      if (image) { try { validateRaster(bytes); } catch (cause) { return { url, error: String(cause) }; } }
      return { url };
    } finally { this.reads.delete(read); }
  }
  release(source: PreviewSource, url: string) {
    if (source.kind === 'artifact') this.artifacts.release(url);
    else if (this.urls.delete(url)) URL.revokeObjectURL(url);
  }
  dispose() {
    this.disposed = true;
    this.artifacts.dispose();
    for (const read of this.reads) read.abort();
    for (const url of this.urls) URL.revokeObjectURL(url);
    this.urls.clear();
  }
}

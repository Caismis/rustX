import type { AppServerClient } from './app-server';
import { sameTarget } from './app-server';
export const ARTIFACT_MAX_BYTES = 256 * 1024;
export const ARTIFACT_MAX_TRANSFERS = 2;
export const ARTIFACT_MAX_URLS = 16;
/** Tool managed artifacts only. Per-view, disposable URL owner. No binaries enter conversation JSON or storage. */
export class ArtifactResources {
  private urls = new Set<string>();
  private active = 0;
  private disposed = false;
  constructor(private client: AppServerClient, private sessionId: string) {}
  async read(id: string, mimeType?: string): Promise<string> {
    if (this.disposed) throw new Error('Obsolete artifact view');
    if (this.active >= ARTIFACT_MAX_TRANSFERS || this.urls.size + this.active >= ARTIFACT_MAX_URLS) throw new Error('Artifact capacity reached; close a preview and retry.');
    const target = this.client.target(this.sessionId);
    this.active++;
    try {
      const result = await this.client.request({ method: 'artifact/read', params: { target, artifact_id: id } }, 'artifact_bytes');
      if (this.disposed || !sameTarget(this.client.getSnapshot().views[this.sessionId]?.target, target)) throw new Error('Obsolete artifact response');
      if (result.data.length > Math.ceil(ARTIFACT_MAX_BYTES / 3) * 4) throw new Error('Artifact exceeds 256 KiB');
      const decoded = atob(result.data);
      if (decoded.length > ARTIFACT_MAX_BYTES) throw new Error('Artifact exceeds 256 KiB');
      const bytes = Uint8Array.from(decoded, c => c.charCodeAt(0));
      const url = URL.createObjectURL(new Blob([bytes], { type: safeArtifactMime(mimeType) }));
      this.urls.add(url);
      return url;
    } finally { this.active--; }
  }
  release(url: string) { if (this.urls.delete(url)) URL.revokeObjectURL(url); }
  dispose() { this.disposed = true; for (const url of this.urls) URL.revokeObjectURL(url); this.urls.clear(); }
}

/** Only inert media/text MIME values from typed metadata; never filename inference.
 * Empty MIME lets Chromium decode semantic images directly from bounded bytes. */
export function safeArtifactMime(mimeType?: string): string {
  return mimeType && /^(image\/(png|jpeg|gif|webp|avif|bmp)|text\/plain|application\/(pdf|octet-stream))$/i.test(mimeType) ? mimeType : '';
}

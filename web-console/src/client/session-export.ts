import { archiveDownloadUrl } from '../../../protocol/app-server/download';
import type { ArchiveDownloadDescriptor } from '../../../protocol/app-server/v13';

/** Browser-native download; no archive bytes enter JavaScript. */
export function downloadArchive(url: string, filename: string): void {
  const anchor = document.createElement('a');
  anchor.href = url;
  anchor.download = filename;
  anchor.referrerPolicy = 'no-referrer';
  anchor.click();
}

/** Concurrent gestures share one native preparation and one browser save. */
export class SessionExportController {
  private active = new Map<string, Promise<void>>();
  constructor(private prepare: (id: string) => Promise<{ download: ArchiveDownloadDescriptor; endpoint: string }>, private save = downloadArchive) {}
  download(id: string): Promise<void> {
    const current = this.active.get(id);
    if (current) return current;
    const operation = Promise.resolve().then(async () => {
      const { download, endpoint } = await this.prepare(id);
      this.save(archiveDownloadUrl(download, endpoint), download.filename);
    }).finally(() => { this.active.delete(id); });
    this.active.set(id, operation);
    return operation;
  }
}

import type { ArchiveDownloadDescriptor } from './v25.ts';

/** Resolve only against the selected native authority. A remote descriptor
 * cannot redirect the client or assign a local filesystem destination. */
export function archiveDownloadUrl(download: ArchiveDownloadDescriptor, endpoint?: string): string {
  if (!/^\/session-archive\/[A-Za-z0-9_-]{43}$/.test(download.path)) throw new Error('Invalid archive download capability');
  let origin: URL;
  if (endpoint !== undefined) {
    if (download.loopback_port != null) throw new Error('Remote archive cannot select a loopback port');
    origin = new URL(endpoint);
    if (!['ws:', 'wss:'].includes(origin.protocol) || origin.username || origin.password || origin.pathname !== '/' || origin.search || origin.hash) throw new Error('Invalid App Server origin');
    origin.protocol = origin.protocol === 'wss:' ? 'https:' : 'http:';
  } else {
    if (!Number.isInteger(download.loopback_port) || download.loopback_port! < 1 || download.loopback_port! > 65535) throw new Error('Missing owned-child archive port');
    origin = new URL(`http://127.0.0.1:${download.loopback_port}`);
  }
  return new URL(download.path, origin).href;
}

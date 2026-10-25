import { closeSync, fstatSync, read } from 'node:fs';
import { openWorkspacePath } from './workbench.ts';
import { WorkspaceHostError } from '../src/workspaces/host.ts';
import { FILE_PREVIEW_LIMITS, validateFileRead, type FileBytes, type FileReadOptions } from '../shared/file-preview.ts';
const version = (fd: number) => { const stat = fstatSync(fd, { bigint: true }); return `${stat.dev}:${stat.ino}:${stat.size}:${stat.mtimeNs}:${stat.ctimeNs}`; };
function openPreviewPath(root: string, path: string) {
  try { return openWorkspacePath(root, path, false); }
  catch (error) {
    const code = (error as NodeJS.ErrnoException).code;
    if (code === 'ENOENT') throw new WorkspaceHostError('source_missing', 'source_missing');
    if (code === 'EACCES' || code === 'EPERM') throw new WorkspaceHostError('authorization_revoked', 'authorization_revoked');
    if (code === 'ENOTDIR' || code === 'ELOOP') throw new WorkspaceHostError('source_unavailable', 'source_unavailable');
    throw error;
  }
}
/** Workspace authority remains Host-owned. This adapter implements the same
 * page/complete-byte contract without turning paths into native deliveries. */
export async function readWorkspacePreview(root: string, path: string, options: FileReadOptions, signal: AbortSignal, max = FILE_PREVIEW_LIMITS.fileBytes): Promise<FileBytes> {
  validateFileRead(options); signal.throwIfAborted();
  const fd = openPreviewPath(root, path);
  try {
    const stat = fstatSync(fd), initial = version(fd);
    if (!stat.isFile()) throw new WorkspaceHostError('source_unavailable', 'source_unavailable');
    const chunk = Buffer.alloc(64 * 1024), parts: Buffer[] = []; let size = 0, lines = 0, line = 1, position = options.kind === 'bytes' ? options.range?.offset ?? 0 : 0, eof = true, pending = false;
    const readChunk = (length: number) => new Promise<number>((resolve, reject) => read(fd, chunk, 0, length, position, (error, count) => error ? reject(error) : resolve(count)));
    if (options.kind === 'bytes' && !options.range && stat.size > max) throw new Error('too_large');
    const capacity = options.kind === 'bytes' ? options.range?.length ?? max : FILE_PREVIEW_LIMITS.pageBytes;
    reading: while (true) {
      signal.throwIfAborted();
      const length = options.kind === 'bytes' ? Math.min(chunk.length, capacity + Number(!options.range) - size) : chunk.length;
      if (length === 0) { eof = position >= stat.size; break; }
      const count = await readChunk(length); signal.throwIfAborted();
      if (!count) break;
      position += count;
      if (options.kind === 'bytes') { parts.push(Buffer.from(chunk.subarray(0, count))); size += count; if (size > capacity) throw new Error('too_large'); continue; }
      let at = 0;
      while (at < count) {
        if (line >= options.offset + options.limit) { eof = false; break reading; }
        const newline = chunk.indexOf(10, at), end = newline >= 0 && newline < count ? newline : count;
        if (line >= options.offset) {
          const separator = Number(!pending && lines > 0);
          if (size + separator + end - at > capacity) throw new Error('too_large');
          if (separator) parts.push(Buffer.from('\n'));
          parts.push(Buffer.from(chunk.subarray(at, end))); size += separator + end - at; pending = true;
          if (end < count) { lines++; pending = false; }
        }
        if (end < count) line++;
        at = end + Number(end < count);
      }
    }
    if (pending) lines++;
    const data = Uint8Array.from(Buffer.concat(parts, size));
    if (options.kind === 'text') { if (data.includes(0)) throw new Error('malformed'); new TextDecoder('utf-8', { fatal: true }).decode(data); }
    signal.throwIfAborted();
    // Reopen through the secure walk; a pathname replacement or a symlink at
    // any edge must not publish data from the old descriptor.
    if (initial !== version(fd)) throw new WorkspaceHostError('source_changed', 'source_changed');
    const current = openPreviewPath(root, path);
    try { if (initial !== version(current)) throw new WorkspaceHostError('source_changed', 'source_changed'); }
    finally { closeSync(current); }
    return { data, version: initial, bytes: stat.size, offset: options.kind === 'text' ? options.offset : options.range?.offset ?? 0, eof, ...(options.kind === 'text' ? { lines } : {}) };
  } finally { closeSync(fd); }
}

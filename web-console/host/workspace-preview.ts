import { closeSync, fstatSync, read, type BigIntStats } from 'node:fs';
import { openWorkspacePath } from './workbench.ts';
import { WorkspaceHostError } from '../src/workspaces/host.ts';
import { FILE_PREVIEW_LIMITS, PreviewCancelled, validateFileRead, type FileBytes, type FileReadOptions } from '../shared/file-preview.ts';
const version = (stat: BigIntStats) => `${stat.dev}:${stat.ino}:${stat.size}:${stat.mtimeNs}:${stat.ctimeNs}`;
export const descriptorVersion = (fd: number, stat = fstatSync(fd, { bigint: true })) => version(stat);
export function openPreviewPath(root: string, path: string) {
  try { return openWorkspacePath(root, path, false); }
  catch (error) {
    const code = (error as NodeJS.ErrnoException).code;
    if (code === 'ENOENT') throw new WorkspaceHostError('source_missing', 'source_missing');
    if (code === 'EACCES' || code === 'EPERM') throw new WorkspaceHostError('authorization_revoked', 'authorization_revoked');
    if (code === 'ENOTDIR' || code === 'ELOOP') throw new WorkspaceHostError('source_unavailable', 'source_unavailable');
    throw error;
  }
}
/** Read directly into the delivery buffer. Explicit positions keep concurrent
 * pages independent, and every started read settles before the owner closes fd. */
async function readBytes(fd: number, size: number, options: Extract<FileReadOptions, { kind: 'bytes' }>, signal: AbortSignal, max: number) {
  if (!options.range && size > max) throw new Error('too_large');
  const offset = options.range?.offset ?? 0;
  const capacity = options.range?.length ?? max;
  const data = new Uint8Array(Math.min(capacity, Math.max(0, size - offset)));
  const readInto = (target: Uint8Array, at: number, length: number, position: number) =>
    new Promise<number>((resolve, reject) => read(fd, target, at, length, position, (error, count) => error ? reject(error) : resolve(count)));
  let filled = 0;
  while (filled < data.length) {
    signal.throwIfAborted();
    const count = await readInto(data, filled, Math.min(FILE_PREVIEW_LIMITS.pageBytes, data.length - filled), offset + filled);
    signal.throwIfAborted();
    if (!count) throw new WorkspaceHostError('source_changed', 'source_changed');
    filled += count;
  }
  // A stat-sized allocation must not hide growth, including an empty file that
  // grows before its first read. Complete reads retain the max+1 size fence.
  if (filled === data.length && (!options.range || filled < capacity)) {
    signal.throwIfAborted();
    const count = await readInto(new Uint8Array(1), 0, 1, offset + filled);
    signal.throwIfAborted();
    if (count) {
      if (!options.range && filled === max) throw new Error('too_large');
      throw new WorkspaceHostError('source_changed', 'source_changed');
    }
  }
  return { data, eof: offset + filled >= size };
}
/** Workspace authority remains Host-owned. This adapter implements the same
 * page/complete-byte contract without turning paths into native deliveries. */
export async function readWorkspacePreview(root: string, path: string, options: FileReadOptions, signal: AbortSignal, max = FILE_PREVIEW_LIMITS.fileBytes): Promise<FileBytes> {
  validateFileRead(options); signal.throwIfAborted();
  const fd = openPreviewPath(root, path);
  try {
    const stat = fstatSync(fd, { bigint: true }), initial = version(stat);
    if (stat.size < 0n || stat.size > BigInt(Number.MAX_SAFE_INTEGER)) throw new WorkspaceHostError('source_unavailable', 'source_unavailable');
    const bytes = Number(stat.size);
    if (!stat.isFile()) throw new WorkspaceHostError('source_unavailable', 'source_unavailable');
    let data: Uint8Array<ArrayBuffer>, eof = true, lines = 0;
    if (options.kind === 'bytes') {
      ({ data, eof } = await readBytes(fd, bytes, options, signal, max));
    } else {
      const chunk = Buffer.alloc(64 * 1024), parts: Buffer[] = []; let size = 0, line = 1, position = 0, pending = false;
      const readChunk = (length: number) => new Promise<number>((resolve, reject) => read(fd, chunk, 0, length, position, (error, count) => error ? reject(error) : resolve(count)));
      const capacity = FILE_PREVIEW_LIMITS.pageBytes;
      reading: while (true) {
        signal.throwIfAborted();
        const count = await readChunk(chunk.length); signal.throwIfAborted();
        if (!count) break;
        position += count;
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
      data = Uint8Array.from(Buffer.concat(parts, size));
      if (data.includes(0)) throw new Error('malformed');
      new TextDecoder('utf-8', { fatal: true }).decode(data);
    }
    signal.throwIfAborted();
    // Reopen through the secure walk; a pathname replacement or a symlink at
    // any edge must not publish data from the old descriptor.
    if (initial !== descriptorVersion(fd)) throw new WorkspaceHostError('source_changed', 'source_changed');
    const current = openPreviewPath(root, path);
    try { if (initial !== descriptorVersion(current)) throw new WorkspaceHostError('source_changed', 'source_changed'); }
    finally { closeSync(current); }
    return { data, version: initial, bytes, offset: options.kind === 'text' ? options.offset : options.range?.offset ?? 0, eof, ...(options.kind === 'text' ? { lines } : {}) };
  } finally { closeSync(fd); }
}

/** One securely opened, read-only descriptor binds every positional page. */
export function openWorkspaceBytePreview(root: string, path: string, signal: AbortSignal): import('../shared/file-preview.ts').FilePreviewOperation {
  signal.throwIfAborted();
  const fd = openPreviewPath(root, path);
  let initial: string, bytes: number;
  try {
    const stat = fstatSync(fd, { bigint: true });
    if (!stat.isFile() || stat.size < 0n || stat.size > BigInt(Number.MAX_SAFE_INTEGER)) throw new WorkspaceHostError('source_unavailable', 'source_unavailable');
    initial = version(stat); bytes = Number(stat.size);
  } catch (error) { closeSync(fd); throw error; }
  let closing: Promise<void> | undefined;
  const pending = new Set<Promise<FileBytes>>();
  const verify = (reopen = false) => {
    signal.throwIfAborted();
    if (descriptorVersion(fd) !== initial) throw new WorkspaceHostError('source_changed', 'source_changed');
    if (reopen) {
      const current = openPreviewPath(root, path);
      try { if (descriptorVersion(current) !== initial) throw new WorkspaceHostError('source_changed', 'source_changed'); }
      finally { closeSync(current); }
    }
    signal.throwIfAborted();
  };
  return {
    version: initial, bytes,
    read(range, active = signal) {
      validateFileRead({ kind: 'bytes', range });
      if (closing || signal.aborted || pending.size >= 2) return Promise.reject(new Error('Obsolete or full preview operation'));
      const task = (async () => {
        const combined = AbortSignal.any([signal, active]); combined.throwIfAborted(); verify();
        const result = await readBytes(fd, bytes, { kind: 'bytes', range }, combined, FILE_PREVIEW_LIMITS.geometryBytes);
        combined.throwIfAborted(); verify();
        return { ...result, version: initial, bytes, offset: range.offset };
      })();
      pending.add(task); void task.finally(() => pending.delete(task)).catch(() => {}); return task;
    },
    close() {
      return closing ??= (async () => {
        await Promise.allSettled([...pending]);
        try {
          if (signal.aborted) throw new PreviewCancelled();
          verify(true);
        } finally { closeSync(fd); }
      })();
    },
  };
}

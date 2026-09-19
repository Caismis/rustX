/** Client-local destination ownership. Only the prepare closure contacts native
 * authority; it receives no destination and returns one byte source. */
import { open, unlink } from 'node:fs/promises';
import { homedir } from 'node:os';
import { resolve } from 'node:path';

export function archiveDestination(sessionId: string, argument: string, cwd = process.cwd(), home = homedir()): string {
  const input = argument.trim() || `rustx-session-${sessionId.replace(/[^A-Za-z0-9_-]/g, "_")}.zip`;
  if (input.includes('\0') || /[\r\n]/.test(input)) throw new Error('Invalid export destination');
  return resolve(cwd, input === '~' ? home : input.startsWith('~/') ? `${home}/${input.slice(2)}` : input);
}

export async function saveSessionArchive(destination: string, prepare: () => Promise<Response>, signal?: AbortSignal): Promise<string> {
  signal?.throwIfAborted();
  const response = await prepare();
  if (!response.ok || !response.body) { await response.body?.cancel(); throw new Error(`Archive download failed: HTTP ${response.status}`); }
  const reader = response.body.getReader();
  let file: Awaited<ReturnType<typeof open>> | undefined;
  const abort = () => { void reader.cancel(signal?.reason); };
  signal?.addEventListener('abort', abort, { once: true });
  try {
    signal?.throwIfAborted();
    // Never truncate existing user data. The successfully created file is ours
    // to remove if the server fails or cancellation leaves partial output.
    file = await open(destination, 'wx', 0o600);
    for (;;) {
      signal?.throwIfAborted();
      const { done, value } = await reader.read();
      signal?.throwIfAborted();
      if (done) break;
      let offset = 0;
      while (offset < value.length) {
        const { bytesWritten } = await file.write(value, offset, value.length - offset);
        if (bytesWritten === 0) throw new Error('Archive destination stopped accepting bytes');
        offset += bytesWritten;
      }
    }
    await file.sync();
    await file.close();
    file = undefined;
    return destination;
  } catch (error) {
    if (file) {
      await file.close().catch(() => {});
      try { await unlink(destination); }
      catch (cleanup) { throw new AggregateError([error, cleanup], `Export failed; partial output remains at ${destination}`); }
    }
    throw error;
  } finally {
    signal?.removeEventListener('abort', abort);
    await reader.cancel().catch(() => {});
    reader.releaseLock();
  }
}

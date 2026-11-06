import { deferred } from './deferred.ts';
import { spawn } from 'node:child_process';
import type { OfficeSandbox } from './office-sandbox.ts';
import { writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { OfficeSettlementError } from './office-cgroup.ts';
import { FILE_PREVIEW_LIMITS } from '../../shared/file-preview.ts';

const maximumOutput = FILE_PREVIEW_LIMITS.convertedPdfBytes;
/** Seatbelt paths are literals, never interpolated predicates. */
const literal = (path: string) => JSON.stringify(path);
export function macOfficeProfile(directory: string, runtimeRoots: string[]): string {
  return `(version 1)
(deny default)
(import "system.sb")
(deny network*)
(allow process-fork process-exec sysctl-read mach-lookup dynamic-code-generation)
(allow signal (target children))
(allow process-info* (target self))
(allow file-map-executable ${[...runtimeRoots, '/System', '/usr/lib'].map(path => `(subpath ${literal(path)})`).join(' ')})
(allow file-read-metadata)
(allow file-read* ${[directory, ...runtimeRoots, '/System', '/usr/lib', '/usr/share', '/Library/Fonts'].map(path => `(subpath ${literal(path)})`).join(' ')})
(allow file-read* (literal "/dev/null") (literal "/dev/random") (literal "/dev/urandom"))
(allow file-write* (subpath ${literal(directory)}) (literal "/dev/null"))
`;
}

/** The reusable kit worker and every descendant inherit the same Seatbelt
 * profile. The Host's per-document watchdog owns cancellation of the group. */
export async function startMacOfficeSandbox(directory: string, command: string[], runtimeRoots: string[], signal: AbortSignal): Promise<OfficeSandbox> {
  signal.throwIfAborted();
  const profile = join(directory, 'sandbox.sb');
  await writeFile(profile, macOfficeProfile(directory, runtimeRoots), { mode: 0o600, flag: 'wx', signal });
  signal.throwIfAborted();
  const child = spawn('/usr/bin/sandbox-exec', ['-f', profile, ...command], {
    detached: true, cwd: directory, stdio: ['pipe', 'pipe', 'pipe'],
    env: { PATH: '/usr/bin:/bin', HOME: directory, TMPDIR: directory, LANG: 'en_US.UTF-8' },
  });
  child.stdin.on('error', () => {}); child.stderr.resume();
  const closed = deferred<void>(); void closed.promise.catch(() => {});
  let terminated = false, failure: Error | undefined, stopping = false;
  const killGroup = () => {
    if (terminated || !child.pid) return;
    try { process.kill(-child.pid, 'SIGKILL'); terminated = true; }
    catch (cause) { if ((cause as NodeJS.ErrnoException).code === 'ESRCH') terminated = true; else failure = new OfficeSettlementError(); }
  };
  const abort = () => { failure ??= new Error('obsolete'); stopping = true; killGroup(); };
  signal.addEventListener('abort', abort, { once: true });
  child.once('error', () => { failure ??= new Error('converter_unavailable'); });
  child.once('close', code => {
    signal.removeEventListener('abort', abort); killGroup();
    if (failure) closed.reject(failure);
    else if (code !== 0 && !stopping) closed.reject(new Error('converter_failure'));
    else closed.resolve();
  });
  if (signal.aborted) abort();
  return { input: child.stdin, output: child.stdout, pid: child.pid, closed: closed.promise,
    deadline: async () => {},
    dispose: async () => {
      stopping = true; killGroup();
      try { await closed.promise; } catch (cause) { if (cause instanceof OfficeSettlementError) throw cause; }
    },
  };
}

/** One-shot operator probes use the identical Seatbelt process boundary. */
export async function runMacOfficeSandbox(directory: string, command: string[], runtimeRoots: string[], signal: AbortSignal): Promise<Buffer> {
  const sandbox = await startMacOfficeSandbox(directory, command, runtimeRoots, signal);
  const chunks: Buffer[] = []; let size = 0, failure: Error | undefined;
  const timer = setTimeout(() => { failure = new Error('converter_timeout'); void sandbox.dispose(); }, 60000);
  sandbox.output.on('data', (chunk: Buffer) => {
    size += chunk.length;
    if (size > maximumOutput) { failure ??= new Error('too_large'); void sandbox.dispose(); }
    else if (!failure) chunks.push(chunk);
  });
  try { await sandbox.closed; if (failure) throw failure; return Buffer.concat(chunks); }
  finally { clearTimeout(timer); }
}

import { spawn } from 'node:child_process';
import { access, copyFile, mkdtemp, realpath, rm, writeFile } from 'node:fs/promises';
import { constants } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { OfficeSettlementError } from './office-cgroup.ts';

const maximumOutput = 4 * 1024 * 1024;
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

/** All engine descendants inherit Seatbelt and the owned process group. No network,
 * user home, workspace or Host environment is exposed to the document engine. */
export async function runMacOfficeSandbox(directory: string, command: string[], runtimeRoots: string[], signal: AbortSignal): Promise<Buffer> {
  signal.throwIfAborted();
  const profile = join(directory, 'sandbox.sb');
  await writeFile(profile, macOfficeProfile(directory, runtimeRoots), { mode: 0o600, flag: 'wx', signal });
  signal.throwIfAborted();
  return await new Promise<Buffer>((resolveResult, reject) => {
    const child = spawn('/usr/bin/sandbox-exec', ['-f', profile, ...command], {
      detached: true, cwd: directory, stdio: ['ignore', 'pipe', 'pipe'],
      env: { PATH: '/usr/bin:/bin', HOME: directory, TMPDIR: directory, LANG: 'en_US.UTF-8' },
    });
    const chunks: Buffer[] = []; let size = 0, failure: Error | undefined;
    let diagnostic = '';
    child.stderr.on('data', (chunk: Buffer) => { diagnostic = (diagnostic + chunk.toString()).slice(0, 8192); });
    const killGroup = () => {
      if (!child.pid) return;
      try { process.kill(-child.pid, 'SIGKILL'); }
      catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ESRCH') failure = new OfficeSettlementError(); }
    };
    const stop = (error: Error) => { failure ??= error; killGroup(); };
    const abort = () => stop(new Error('obsolete'));
    const timer = setTimeout(() => stop(new Error('converter_timeout')), 15000);
    signal.addEventListener('abort', abort, { once: true });
    if (signal.aborted) abort();
    child.stdout.on('data', (chunk: Buffer) => {
      size += chunk.length;
      if (size > maximumOutput) stop(new Error('too_large'));
      else if (!failure) chunks.push(chunk);
    });
    child.once('error', () => { failure ??= new Error('converter_unavailable'); });
    child.once('close', (code, exitSignal) => {
      clearTimeout(timer); signal.removeEventListener('abort', abort);
      // Retire any residual descendants before releasing private files or the seat.
      killGroup();
      if (failure) reject(failure);
      else if (code !== 0) reject(new Error('converter_failure', { cause: new Error(`exit=${code}, signal=${exitSignal}: ${diagnostic}`) }));
      else resolveResult(Buffer.concat(chunks));
    });
  });
}

export async function convertMacOffice(bytes: Buffer, extension: 'docx' | 'pptx', signal: AbortSignal): Promise<Buffer> {
  try { await access('/usr/bin/sandbox-exec', constants.X_OK); }
  catch { throw new Error('converter_unavailable'); }
  const directory = await realpath(await mkdtemp(join(tmpdir(), 'rustx-document-')));
  try {
    const entry = fileURLToPath(import.meta.resolve('@deepseek-ai/libreoffice-kit'));
    // pnpm's sibling optional engine packages live under this installation tree.
    const modules = await realpath(new URL('../../node_modules', import.meta.url));
    const node = await realpath(process.execPath);
    const worker = join(directory, 'worker.mjs');
    await copyFile(new URL('./office-macos-worker.mjs', import.meta.url), worker);
    await writeFile(join(directory, `source.${extension}`), bytes, { mode: 0o600, flag: 'wx', signal });
    const result = await runMacOfficeSandbox(directory,
      ['/bin/sh', '-c', 'ulimit -f 8192; ulimit -n 128; ulimit -t 15; exec "$@"', 'rustx-office', node, worker, entry, directory, extension],
      [modules, resolve(dirname(node), '..'), '/bin/sh'], signal);
    if (!result.subarray(0, 5).equals(Buffer.from('%PDF-'))) throw new Error('converter_failure');
    return result;
  } finally {
    try { await rm(directory, { recursive: true, force: true }); }
    catch { throw new OfficeSettlementError(); }
  }
}

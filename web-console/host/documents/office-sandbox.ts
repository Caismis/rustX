import { deferred } from './deferred.ts';
import { OfficeSettlementError, observeOfficeCgroup } from './office-cgroup.ts';
import { spawn } from 'node:child_process';
import { PassThrough, type Writable, type Readable } from 'node:stream';
import { randomUUID } from 'node:crypto';
import { DERIVED_PDF_MAX_BYTES } from '../../shared/documents.ts';
import { OFFICE_LIMITS as limits } from './limits.ts';

export interface OfficeSandbox {
  readonly input: Writable;
  readonly output: Readable;
  readonly pid: number | undefined;
  readonly closed: Promise<void>;
  deadline(active: boolean): Promise<void>;
  dispose(): Promise<void>;
}
/** One admitted, reusable process tree. Each active document owns a renewed
 * manager deadline; idle engines retain their process and memory boundaries. */
export async function startOfficeSandbox(directory: string, command: string[], signal: AbortSignal): Promise<OfficeSandbox> {
  signal.throwIfAborted();
  const unit = `rustx-document-${randomUUID()}.service`;
  // Verify effective kernel controls inside the service BEFORE admitting payload.
  // The stdin handshake also fences cancellation while systemd is starting it.
  const gate = `group=/sys/fs/cgroup$(sed -n 's/^0:://p' /proc/self/cgroup)
    printf 'RUSTX-CGROUP %s\\n' "$group"
    read -r permit
    test "$permit" = run
    exec "$@"`;
  const args = ['--user', '--quiet', '--wait', '--pipe', '--collect', `--unit=${unit}`,
    '--property=Type=exec', '--property=ExitType=cgroup', '--property=Restart=no',
    `--property=MemoryMax=${limits.memory}`, '--property=MemorySwapMax=0', '--property=OOMPolicy=kill',
    `--property=TasksMax=${limits.tasks}`, '--property=CPUQuota=100%', '--property=CPUQuotaPeriodSec=100ms',
    '--property=RuntimeMaxSec=infinity', '--property=TimeoutStopSec=1',
    '--property=KillMode=control-group', '--property=KillSignal=SIGKILL',
    // The user manager can supply secrets independently of the client's env.
    // exec env -i gives bwrap an EMPTY initial environment, including the bytes
    // exposed through /proc/1/environ. bwrap --clearenv alone cannot promise this.
    '--', '/usr/bin/sh', '-ec', gate, 'rustx-document', '/usr/bin/env', '-i', '/usr/bin/bwrap',
    '--unshare-all', '--unshare-user', '--disable-userns', '--die-with-parent', '--new-session', '--cap-drop', 'ALL',
    '--clearenv', '--setenv', 'PATH', '/usr/bin', '--setenv', 'HOME', '/tmp/home', '--setenv', 'LANG', 'C.UTF-8',
    '--setenv', 'SAL_USE_VCLPLUGIN', 'svp', '--setenv', 'TMPDIR', '/tmp',
    '--ro-bind', '/usr', '/usr', '--symlink', 'usr/bin', '/bin', '--symlink', 'usr/lib', '/lib', '--symlink', 'usr/lib64', '/lib64',
    '--proc', '/proc', '--remount-ro', '/proc', '--dir', '/dev',
    '--dev-bind', '/dev/null', '/dev/null', '--dev-bind', '/dev/zero', '/dev/zero',
    '--dev-bind', '/dev/random', '/dev/random', '--dev-bind', '/dev/urandom', '/dev/urandom',
    '--symlink', '/proc/self/fd', '/dev/fd', '--symlink', '/tmp/shm', '/dev/shm', '--dir', '/etc',
    '--ro-bind-try', '/etc/libreoffice/registry', '/etc/libreoffice/registry',
    '--ro-bind', directory, '/input', '--size', String(limits.writableBytes), '--tmpfs', '/tmp',
    '--dir', '/tmp/shm', '--remount-ro', '/', '--chdir', '/tmp',
    '--', '/usr/bin/prlimit', `--fsize=${limits.fileBytes}`, `--nofile=${limits.fileDescriptors}`, '--core=0', ...command];
  const env = Object.fromEntries(['XDG_RUNTIME_DIR', 'DBUS_SESSION_BUS_ADDRESS'].flatMap(key => process.env[key] ? [[key, process.env[key]!]] : []));
  const child = spawn('/usr/bin/systemd-run', args, { env, stdio: ['pipe', 'pipe', 'ignore'] });
  const output = new PassThrough();
  const admitted = deferred<OfficeSandbox>();
  const closed = deferred<void>();
  void closed.promise.catch(() => {});
  let owner: Awaited<ReturnType<typeof observeOfficeCgroup>> | undefined;
  let admission: Promise<void> | undefined, prefix = '', stopping = false, failure: Error | undefined;
  const stop = (cause?: Error) => {
    failure ??= cause; stopping = true; child.stdin.end();
    if (owner) void owner.kill().catch(() => {});
  };
  const abort = () => stop(new Error('obsolete'));
  signal.addEventListener('abort', abort, { once: true });
  const timer = setTimeout(() => stop(new Error('converter_timeout')), limits.runtimeSeconds * 1000);
  const sandbox: OfficeSandbox = {
    input: child.stdin, output, pid: child.pid, closed: closed.promise,
    deadline: async active => { if (stopping || !owner) throw new Error('converter_unavailable'); await owner.deadline(active); },
    dispose: async () => {
      stop();
      try { await closed.promise; } catch (error) { if (error instanceof OfficeSettlementError) throw error; }
    },
  };
  const header = (chunk: Buffer) => {
    prefix += chunk.toString('ascii');
    if (prefix.length > 4096 || admission) { stop(new Error('converter_unavailable')); return; }
    if (!prefix.endsWith('\n')) return;
    if (!prefix.startsWith('RUSTX-CGROUP ')) { stop(new Error('converter_unavailable')); return; }
    admission = (async () => {
      try {
        owner = await observeOfficeCgroup(prefix.slice(13).trim(), unit);
        child.stdout.off('data', header);
        if (stopping || signal.aborted) { stop(failure ?? new Error('obsolete')); return; }
        await owner.deadline(true);
        if (stopping || signal.aborted) { stop(failure ?? new Error('obsolete')); return; }
        child.stdout.pipe(output);
        child.stdin.write('run\n');
        clearTimeout(timer); admitted.resolve(sandbox);
      } catch { stop(new Error('converter_unavailable')); }
    })();
  };
  child.stdin.on('error', () => {});
  child.stdout.on('data', header);
  child.on('error', () => stop(new Error('converter_unavailable')));
  // Descendants can retain the pipes after the launcher exits. Retire them
  // immediately so their inherited handles cannot delay physical settlement.
  child.once('exit', code => { if (code !== 0 && !stopping) stop(new Error(owner ? 'converter_failure' : 'converter_unavailable')); });
  child.once('close', () => { void (async () => {
    clearTimeout(timer); signal.removeEventListener('abort', abort);
    try {
      await admission; await owner?.retire();
      if (failure) throw failure;
      if (!owner) throw new Error('converter_unavailable');
      closed.resolve();
    } catch (error) { closed.reject(error); }
    finally { output.end(); }
    try { await closed.promise; admitted.reject(new Error('converter_unavailable')); }
    catch (error) { admitted.reject(error); }
  })(); });
  if (signal.aborted) abort();
  return admitted.promise;
}

/** Bounded one-shot execution for the CI/operator admission probe. Document
 * previews use startOfficeSandbox and retain the same process across jobs. */
export async function runOfficeSandbox(directory: string, command: string[], signal: AbortSignal): Promise<Buffer> {
  const sandbox = await startOfficeSandbox(directory, command, signal);
  const chunks: Buffer[] = []; let size = 0, failure: Error | undefined;
  const timer = setTimeout(() => { failure ??= new Error('converter_timeout'); void sandbox.dispose().catch(() => {}); }, limits.runtimeSeconds * 1000);
  sandbox.output.on('data', (chunk: Buffer) => {
    size += chunk.length;
    if (size > DERIVED_PDF_MAX_BYTES) { failure ??= new Error('too_large'); void sandbox.dispose().catch(() => {}); }
    else if (!failure) chunks.push(chunk);
  });
  try { await sandbox.closed; if (failure) throw failure; return Buffer.concat(chunks); }
  finally { clearTimeout(timer); }
}

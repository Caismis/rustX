import { observeOfficeCgroup } from './office-cgroup.ts';
import { spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { DERIVED_PDF_MAX_BYTES } from '../../shared/documents.ts';
import { OFFICE_LIMITS as limits } from './limits.ts';

/** A single transient service owns the complete converter cgroup. No queue/restart. */
export async function runOfficeSandbox(directory: string, command: string[], signal: AbortSignal): Promise<Buffer> {
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
    `--property=RuntimeMaxSec=${limits.runtimeSeconds}`, '--property=TimeoutStopSec=1',
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
  // Manager connection only; never pass Host credentials to the launcher.
  const env = Object.fromEntries(['XDG_RUNTIME_DIR', 'DBUS_SESSION_BUS_ADDRESS'].flatMap(key => process.env[key] ? [[key, process.env[key]!]] : []));
  let owner: Awaited<ReturnType<typeof observeOfficeCgroup>> | undefined;
  let admission: Promise<void> | undefined;
  try {
    return await new Promise<Buffer>((resolve, reject) => {
      const child = spawn('/usr/bin/systemd-run', args, { env, stdio: ['pipe', 'pipe', 'ignore'] });
      const chunks: Buffer[] = []; let size = 0, admitted = false, prefix = '', error: Error | undefined;
      const stop = (cause: Error) => {
        error ??= cause;
        child.stdin.end(); // No permit can reach a service whose startup is still pending.
        if (owner) void owner.kill().catch(() => {});
      };
      const abort = () => stop(new Error('obsolete'));
      child.stdin.on('error', () => {}); // Early admission failure may close the handshake pipe.
      signal.addEventListener('abort', abort, { once: true });
      const timer = setTimeout(() => stop(new Error('converter_timeout')), limits.runtimeSeconds * 1000);
      child.stdout.on('data', (chunk: Buffer) => {
        if (!admitted) {
          prefix += chunk.toString('ascii');
          if (prefix.length > 4096 || admission) return stop(new Error('converter_unavailable'));
          if (!prefix.endsWith('\n')) return;
          if (!prefix.startsWith('RUSTX-CGROUP ')) return stop(new Error('converter_unavailable'));
          admission = (async () => {
            try {
              owner = await observeOfficeCgroup(prefix.slice(13).trim(), unit);
              admitted = true;
              if (error || signal.aborted) stop(error ?? new Error('obsolete'));
              else child.stdin.end('run\n');
            } catch { stop(new Error('converter_unavailable')); }
          })();
          return;
        }
        size += chunk.length;
        if (size > DERIVED_PDF_MAX_BYTES) stop(new Error('too_large'));
        else if (!error) chunks.push(chunk);
      });
      child.on('error', () => stop(new Error('converter_unavailable')));
      child.once('exit', code => { if (code !== 0) stop(new Error(admitted ? 'converter_failure' : 'converter_unavailable')); });
      // --wait plus ExitType=cgroup waits for ALL descendants; close also waits
      // for forwarded output pipes. Never kill this wait client on cancellation.
      child.once('close', code => {
        clearTimeout(timer); signal.removeEventListener('abort', abort);
        if (error) reject(error);
        else if (!admitted) reject(new Error('converter_unavailable'));
        else if (code !== 0) reject(new Error('converter_failure'));
        else resolve(Buffer.concat(chunks));
      });
      if (signal.aborted) abort();
    });
  } finally {
    await admission;
    // Even a crashed wait client cannot leave an admitted service behind.
    // Kernel populated=0 is the physical settlement witness, independent of IPC.
    await owner?.retire();
  }
}

import { spawn } from 'node:child_process';
import { access, mkdtemp, writeFile, rm } from 'node:fs/promises';
import { constants } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { DOCUMENT_LIMITS } from '../../src/client/document-types.ts';

/** Only this Linux process boundary is supported. No unsandboxed fallback. */
export async function convertOffice(bytes: Buffer, extension: 'docx' | 'pptx', signal: AbortSignal): Promise<Buffer> {
  if (process.platform !== 'linux') throw new Error('converter_unavailable');
  signal.throwIfAborted();
  try { await Promise.all(['/usr/bin/bwrap', '/usr/bin/prlimit', '/usr/bin/libreoffice'].map(path => access(path, constants.X_OK))); }
  catch { throw new Error('converter_unavailable'); }
  signal.throwIfAborted();
  const directory = await mkdtemp(join(tmpdir(), 'rustx-document-'));
  try {
    await writeFile(join(directory, `source.${extension}`), bytes, { mode: 0o600, flag: 'wx', signal });
    // Source admission has already rejected VBA, ActiveX, embedded packages,
    // templates and every external relationship. Disable document macros too.
    await writeFile(join(directory, 'registrymodifications.xcu'), '<oor:items xmlns:oor="http://openoffice.org/2001/registry"><item oor:path="/org.openoffice.Office.Common/Security/Scripting"><prop oor:name="MacroSecurityLevel" oor:op="fuse"><value>3</value></prop></item></oor:items>', { mode: 0o600, flag: 'wx', signal });
    signal.throwIfAborted();
    const args = [
      '--unshare-all', '--unshare-user', '--die-with-parent', '--new-session', '--cap-drop', 'ALL',
      '--clearenv', '--setenv', 'PATH', '/usr/bin', '--setenv', 'HOME', '/tmp/home', '--setenv', 'LANG', 'C.UTF-8',
      '--setenv', 'SAL_USE_VCLPLUGIN', 'svp', '--setenv', 'TMPDIR', '/tmp',
      '--ro-bind', '/usr', '/usr', '--symlink', 'usr/bin', '/bin', '--symlink', 'usr/lib', '/lib', '--symlink', 'usr/lib64', '/lib64',
      '--proc', '/proc', '--dev', '/dev', '--dir', '/etc',
      // Debian/Ubuntu package the system registry outside /usr and symlink it
      // from LibreOffice's share directory. Expose only that runtime data.
      '--ro-bind-try', '/etc/libreoffice/registry', '/etc/libreoffice/registry',
      '--ro-bind', directory, '/input',
      '--size', '67108864', '--tmpfs', '/tmp', '--chdir', '/tmp',
      '--', '/usr/bin/prlimit', '--as=1073741824', '--fsize=8388608', '--cpu=15', '--nofile=128', '--core=0',
      '/usr/bin/sh', '-ec',
      `mkdir -p /tmp/home /tmp/profile/user /tmp/output; cp /input/registrymodifications.xcu /tmp/profile/user/registrymodifications.xcu; libreoffice -env:UserInstallation=file:///tmp/profile --headless --nologo --nodefault --norestore --nolockcheck --convert-to pdf --outdir /tmp/output /input/source.${extension} >/dev/null 2>/dev/null; exec cat /tmp/output/source.pdf`,
    ];
    return await new Promise<Buffer>((resolve, reject) => {
      const child = spawn('/usr/bin/bwrap', args, { env: {}, stdio: ['ignore', 'pipe', 'ignore'] });
      const chunks: Buffer[] = []; let size = 0, error: Error | undefined;
      const stop = (cause: Error) => { error ??= cause; child.kill('SIGKILL'); };
      const abort = () => stop(new Error('obsolete'));
      signal.addEventListener('abort', abort, { once: true });
      const timer = setTimeout(() => stop(new Error('converter_timeout')), DOCUMENT_LIMITS.timeout);
      child.stdout.on('data', (chunk: Buffer) => {
        size += chunk.length;
        if (size > DOCUMENT_LIMITS.pdfBytes) stop(new Error('too_large'));
        else if (!error) chunks.push(chunk);
      });
      child.on('error', () => { error ??= new Error('converter_unavailable'); });
      // close follows process exit AND all pipe closure. Killing bubblewrap
      // destroys its PID namespace, including descendants holding these pipes.
      child.on('close', code => {
        clearTimeout(timer); signal.removeEventListener('abort', abort);
        const result = Buffer.concat(chunks);
        if (error) reject(error);
        else if (code !== 0) reject(new Error('converter_failure'));
        else if (!result.subarray(0, 5).equals(Buffer.from('%PDF-'))) reject(new Error('converter_failure'));
        else resolve(result);
      });
      if (signal.aborted) abort();
    });
  } finally { await rm(directory, { recursive: true, force: true }); }
}

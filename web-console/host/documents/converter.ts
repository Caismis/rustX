import { OfficeSettlementError } from './office-cgroup.ts';
import { access, mkdtemp, writeFile, rm } from 'node:fs/promises';
import { constants } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { OOXML_LIMITS } from './archive.ts';
import { runOfficeSandbox } from './office-sandbox.ts';

/** Only this Linux process boundary is supported. No unsandboxed fallback. */
export async function convertOffice(bytes: Buffer, extension: 'docx' | 'pptx', signal: AbortSignal): Promise<Buffer> {
  if (process.platform !== 'linux') throw new Error('converter_unavailable');
  if (bytes.length > OOXML_LIMITS.source) throw new Error('too_large');
  signal.throwIfAborted();
  try { await Promise.all(['/usr/bin/bwrap', '/usr/bin/prlimit', '/usr/bin/libreoffice', '/usr/bin/systemd-run'].map(path => access(path, constants.X_OK))); }
  catch { throw new Error('converter_unavailable'); }
  signal.throwIfAborted();
  const directory = await mkdtemp(join(tmpdir(), 'rustx-document-'));
  try {
    await writeFile(join(directory, `source.${extension}`), bytes, { mode: 0o600, flag: 'wx', signal });
    // Source admission has already rejected VBA, ActiveX, embedded packages,
    // templates and every external relationship. Disable document macros too.
    await writeFile(join(directory, 'registrymodifications.xcu'), '<oor:items xmlns:oor="http://openoffice.org/2001/registry"><item oor:path="/org.openoffice.Office.Common/Security/Scripting"><prop oor:name="MacroSecurityLevel" oor:op="fuse"><value>3</value></prop></item></oor:items>', { mode: 0o600, flag: 'wx', signal });
    signal.throwIfAborted();
    const result = await runOfficeSandbox(directory, ['/usr/bin/sh', '-ec',
      `mkdir -p /tmp/home /tmp/profile/user /tmp/output; cp /input/registrymodifications.xcu /tmp/profile/user/registrymodifications.xcu; libreoffice -env:UserInstallation=file:///tmp/profile --headless --nologo --nodefault --norestore --nolockcheck --convert-to pdf --outdir /tmp/output /input/source.${extension} >/dev/null 2>/dev/null; exec cat /tmp/output/source.pdf`,
    ], signal);
    if (!result.subarray(0, 5).equals(Buffer.from('%PDF-'))) throw new Error('converter_failure');
    return result;
  } finally {
    try { await rm(directory, { recursive: true, force: true }); }
    catch { throw new OfficeSettlementError(); }
  }
}

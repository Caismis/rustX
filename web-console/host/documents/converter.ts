import { access, copyFile, mkdtemp, realpath, rm, writeFile } from 'node:fs/promises';
import { constants } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { randomUUID } from 'node:crypto';
import { OfficeSettlementError } from './office-cgroup.ts';
import { OOXML_LIMITS } from './archive.ts';
import { OFFICE_LIMITS } from './limits.ts';
import { startOfficeSandbox, type OfficeSandbox } from './office-sandbox.ts';
import { OfficeChannel } from './office-channel.ts';

interface Runtime { sandbox: OfficeSandbox; channel: OfficeChannel; engine: number }
interface Slot { controller: AbortController; start: Promise<Runtime>; directory?: string; retirement?: Promise<void> }
/** The Host owns one serial converter slot, as in Harness. Healthy documents
 * keep their engine; cancellation/failure retires the tree before replacement. */
export class OfficeConverter {
  private slot?: Slot;
  private active?: Promise<Buffer>;
  private closed = false;
  private poisoned = false;
  private disposal?: Promise<void>;
  get identity() { return this.slot?.start.then(runtime => ({ worker: runtime.sandbox.pid, engine: runtime.engine })); }
  async warm(): Promise<void> { const slot = this.ensure(); try { await slot.start; } catch (cause) { await this.retire(slot); throw cause; } }
  private ensure(): Slot {
    if (this.closed || this.poisoned) throw new Error('converter_unavailable');
    if (this.slot && !this.slot.retirement) return this.slot;
    const previous = this.slot?.retirement;
    const slot: Slot = { controller: new AbortController(), start: undefined! };
    this.slot = slot;
    slot.start = (async () => {
      await previous;
      if (this.poisoned) throw new OfficeSettlementError();
      slot.controller.signal.throwIfAborted();
      const runtime = await this.start(slot);
      // Idle crashes retire the private tree too. A later acquisition waits
      // for that retirement before starting a replacement engine.
      void runtime.sandbox.closed.then(() => this.retire(slot), () => this.retire(slot)).catch(() => {});
      return runtime;
    })();
    void slot.start.catch(() => {});
    return slot;
  }
  private async start(slot: Slot): Promise<Runtime> {
    const signal = slot.controller.signal;
    if (!['linux', 'darwin'].includes(process.platform)) throw new Error('converter_unavailable');
    try { await Promise.all((process.platform === 'darwin' ? ['/usr/bin/sandbox-exec'] : ['/usr/bin/env', '/usr/bin/bwrap', '/usr/bin/prlimit', '/usr/bin/libreoffice', '/usr/bin/systemd-run', '/usr/bin/python3']).map(path => access(path, constants.X_OK))); }
    catch { throw new Error('converter_unavailable'); }
    signal.throwIfAborted();
    const directory = slot.directory = await realpath(await mkdtemp(join(tmpdir(), 'rustx-office-slot-')));
    if (process.platform === 'darwin') {
      const { startMacOfficeSandbox } = await import('./office-macos.ts');
      const entry = fileURLToPath(import.meta.resolve('@deepseek-ai/libreoffice-kit'));
      const modules = await realpath(new URL('../../node_modules', import.meta.url)), node = await realpath(process.execPath);
      const worker = join(directory, 'worker.mjs');
      await copyFile(new URL('./office-macos-worker.mjs', import.meta.url), worker);
      const sandbox = await startMacOfficeSandbox(directory, ['/bin/sh', '-c', 'ulimit -f 524288; ulimit -n 128; exec "$@"', 'rustx-office', node, worker, entry, directory], [modules, resolve(dirname(node), '..'), '/bin/sh'], signal);
      return this.ready(sandbox);
    }
    await copyFile(new URL('./office-linux-worker.py', import.meta.url), join(directory, 'worker.py'));
    await writeFile(join(directory, 'registrymodifications.xcu'), '<oor:items xmlns:oor="http://openoffice.org/2001/registry"><item oor:path="/org.openoffice.Office.Common/Security/Scripting"><prop oor:name="MacroSecurityLevel" oor:op="fuse"><value>3</value></prop></item></oor:items>', { mode: 0o600, flag: 'wx', signal });
    const sandbox = await startOfficeSandbox(directory, ['/usr/bin/python3', '/input/worker.py'], signal);
    return this.ready(sandbox);
  }
  private async ready(sandbox: OfficeSandbox): Promise<Runtime> {
    const channel = new OfficeChannel(sandbox);
    let timedOut = false;
    const timer = setTimeout(() => { timedOut = true; void sandbox.dispose().catch(() => {}); }, OFFICE_LIMITS.runtimeSeconds * 1000);
    try {
      const engine = await channel.ready();
      await sandbox.deadline(false);
      return { sandbox, channel, engine };
    } catch (error) { await sandbox.dispose(); if (timedOut) throw new Error('converter_timeout'); throw error; }
    finally { clearTimeout(timer); }
  }
  private retire(slot: Slot): Promise<void> {
    return slot.retirement ??= (async () => {
      slot.controller.abort();
      try {
        let runtime: Runtime | undefined;
        try { runtime = await slot.start; } catch (error) { if (error instanceof OfficeSettlementError) throw error; }
        await runtime?.sandbox.dispose();
        if (slot.directory) await rm(slot.directory, { recursive: true, force: true });
        if (this.slot === slot) this.slot = undefined;
      } catch (cause) { this.poisoned = true; throw new OfficeSettlementError(); }
    })();
  }
  async render(bytes: Buffer, extension: 'docx' | 'pptx', signal: AbortSignal): Promise<Buffer> {
    signal.throwIfAborted();
    if (bytes.length > OOXML_LIMITS.source) throw new Error('too_large');
    if (this.active) throw new Error('capacity');
    const slot = this.ensure(), id = randomUUID();
    let timedOut = false;
    const abort = () => { void this.retire(slot).catch(() => {}); };
    signal.addEventListener('abort', abort, { once: true });
    const timer = setTimeout(() => { timedOut = true; abort(); }, OFFICE_LIMITS.runtimeSeconds * 1000);
    const work = (async () => {
      try {
        const runtime = await slot.start;
        signal.throwIfAborted(); slot.controller.signal.throwIfAborted();
        await runtime.sandbox.deadline(true);
        const input = join(slot.directory!, `${id}.${extension}`);
        await writeFile(input, bytes, { mode: 0o600, flag: 'wx', signal });
        signal.throwIfAborted();
        const result = runtime.channel.result(id, runtime.engine);
        runtime.sandbox.input.write(JSON.stringify({ id, extension }) + '\n');
        const pdf = await result;
        signal.throwIfAborted(); slot.controller.signal.throwIfAborted();
        try { await rm(input); } catch { throw new OfficeSettlementError(); }
        await runtime.sandbox.deadline(false);
        return pdf;
      } catch (cause) {
        await this.retire(slot);
        if (cause instanceof OfficeSettlementError) { this.poisoned = true; throw cause; }
        if (timedOut) throw new Error('converter_timeout');
        if (signal.aborted || this.closed) throw new Error('obsolete');
        throw cause;
      } finally { clearTimeout(timer); signal.removeEventListener('abort', abort); }
    })();
    this.active = work;
    try { return await work; } finally { if (this.active === work) this.active = undefined; }
  }
  dispose(): Promise<void> {
    this.closed = true;
    return this.disposal ??= (async () => {
      const slot = this.slot;
      if (slot) await this.retire(slot);
      try { await this.active; } catch (error) { if (error instanceof OfficeSettlementError) throw error; }
    })();
  }
}

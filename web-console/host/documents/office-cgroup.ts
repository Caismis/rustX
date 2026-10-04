import { watch } from 'node:fs';
import { open, readFile, statfs } from 'node:fs/promises';
import { OFFICE_LIMITS } from './limits.ts';

/** Do not release the Host seat if physical retirement cannot be established. */
export class OfficeSettlementError extends Error { constructor() { super('converter_unavailable'); } }

/** Kernel handles retained outside the sandbox before the payload receives a permit. */
export async function observeOfficeCgroup(path: string, unit: string) {
  if (!path.startsWith('/sys/fs/cgroup/') || path.includes('/../') || !path.endsWith(`/${unit}`)
    || (await statfs(path)).type !== 0x63677270) throw new Error('converter_unavailable');
  for (const [file, expected] of Object.entries({ 'memory.max': String(OFFICE_LIMITS.memory), 'memory.swap.max': '0',
    'pids.max': String(OFFICE_LIMITS.tasks), 'cpu.max': '100000 100000', 'memory.oom.group': '1' })) {
    if ((await readFile(`${path}/${file}`, 'utf8')).trim() !== expected) throw new Error('converter_unavailable');
  }
  const control = await open(`${path}/cgroup.kill`, 'w');
  let finish!: () => void, fail!: () => void;
  const settled = new Promise<void>((resolve, reject) => { finish = resolve; fail = () => reject(new OfficeSettlementError()); });
  // Attach rejection handling immediately while the conversion is still active.
  void settled.catch(() => {});
  let checking = Promise.resolve(), closing = false;
  const check = () => { if (closing) return; checking = checking.then(async () => {
    try { if ((await readFile(`${path}/cgroup.events`, 'utf8')).split('\n').includes('populated 0')) finish(); }
    catch (error) { if (['ENOENT', 'ENODEV'].includes((error as NodeJS.ErrnoException).code ?? '')) finish(); else fail(); }
  }); };
  let observer: ReturnType<typeof watch>;
  try { observer = watch(`${path}/cgroup.events`, () => { void check(); }); }
  catch (error) { await control.close(); throw error; }
  observer.on('error', fail);
  void check();
  let killing: Promise<void> | undefined;
  return {
    kill: () => killing ??= control.write('1', 0, 'utf8').then(() => {}, async error => {
      // An already removed cgroup has no remaining tasks. Other errors must not
      // turn an unknown retirement into a released capacity reservation.
      if ((error as NodeJS.ErrnoException).code !== 'ENODEV') throw new OfficeSettlementError();
    }),
    async retire() {
      try { await this.kill(); await settled; }
      finally { closing = true; observer.close(); await checking; await control.close(); }
    },
  };
}

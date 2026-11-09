// @vitest-environment node
import { EventEmitter } from 'node:events';
import { PassThrough } from 'node:stream';
import { expect, it, vi } from 'vitest';
import { deferred } from '../host/documents/deferred';

const boundary = vi.hoisted(() => ({ spawn: vi.fn(), observe: vi.fn() }));
vi.mock('node:child_process', async original => ({ ...await original<typeof import('node:child_process')>(), spawn: boundary.spawn }));
vi.mock('../host/documents/office-cgroup.ts', async original => ({ ...await original<typeof import('../host/documents/office-cgroup.ts')>(), observeOfficeCgroup: boundary.observe }));
import { startOfficeSandbox } from '../host/documents/office-sandbox';

it('retires descendants on launcher exit before inherited pipes close, and awaits physical settlement', async () => {
  const child = Object.assign(new EventEmitter(), { pid: 987654, stdin: new PassThrough(), stdout: new PassThrough() });
  const retiring = deferred<void>(), retired = deferred<void>();
  const owner = {
    deadline: vi.fn(async () => {}),
    kill: vi.fn(async () => {}),
    retire: vi.fn(async () => { retiring.resolve(); await retired.promise; }),
  };
  boundary.spawn.mockReturnValue(child);
  boundary.observe.mockResolvedValue(owner);
  const opening = startOfficeSandbox('/input', ['converter'], new AbortController().signal);
  child.stdout.write('RUSTX-CGROUP /sys/fs/cgroup/fixture.service\n');
  const sandbox = await opening;
  const settled = vi.fn();
  const completion = sandbox.closed.then(() => settled('success'), (error: Error) => settled(error.message));
  try {
    // A descendant can retain stdout after the systemd-run client is killed.
    // Cleanup must start from exit, without waiting for that pipe's close.
    child.emit('exit', null, 'SIGKILL');
    expect(owner.kill).toHaveBeenCalledTimes(1);
    expect(settled).not.toHaveBeenCalled();
    child.emit('close', null, 'SIGKILL');
    await retiring.promise;
    expect(settled).not.toHaveBeenCalled();
    retired.resolve();
    await completion;
    expect(settled).toHaveBeenCalledExactlyOnceWith('converter_failure');
  } finally {
    retired.resolve();
    child.emit('close', null, 'SIGKILL');
    await completion;
    child.stdin.destroy(); child.stdout.destroy(); sandbox.output.destroy();
  }
});

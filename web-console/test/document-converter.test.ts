// @vitest-environment node
import { EventEmitter } from 'node:events';
import { PassThrough } from 'node:stream';
import { existsSync } from 'node:fs';
import { afterEach, expect, it, vi } from 'vitest';
const boundary = vi.hoisted(() => ({ spawn: vi.fn(), kill: vi.fn(), retire: vi.fn() }));
vi.mock('../host/documents/office-cgroup.ts', async original => ({ ...await original<typeof import('../host/documents/office-cgroup.ts')>(), observeOfficeCgroup: async () => ({ kill: boundary.kill, retire: boundary.retire }) }));
vi.mock('node:child_process', () => ({ spawn: boundary.spawn }));
import { convertOffice } from '../host/documents/converter';
function gate() {
  const child = Object.assign(new EventEmitter(), { stdout: new PassThrough(), stdin: new PassThrough(), kill: vi.fn() });
  let spawned!: () => void; const ready = new Promise<void>(resolve => { spawned = resolve; });
  boundary.spawn.mockImplementation(() => { spawned(); return child; });
  boundary.kill.mockResolvedValue(undefined); boundary.retire.mockResolvedValue(undefined);
  return { child, ready };
}
afterEach(() => { vi.useRealTimers(); boundary.spawn.mockReset(); });
it('owns isolation arguments and retains temporary files until process-tree settlement after cancellation', async () => {
  const { child, ready } = gate(), abort = new AbortController();
  const work = convertOffice(Buffer.from('input'), 'docx', abort.signal);
  const rejected = expect(work).rejects.toThrow('obsolete');
  await ready;
  const [exe, args, options] = boundary.spawn.mock.calls[0];
  expect(exe).toBe('/usr/bin/systemd-run');
  child.stdout.write('RUSTX-CGROUP /sys/fs/cgroup/test\n'); await new Promise<void>(resolve => child.stdin.once('finish', resolve));
  for (const required of ['--unshare-all', '--unshare-user', '--disable-userns', '--die-with-parent', '--new-session', '--clearenv', '--size', '67108864', '--property=MemoryMax=536870912', '--property=TasksMax=64', '--property=ExitType=cgroup', '--remount-ro', '--fsize=8388608']) expect(args).toContain(required);
  expect(args).not.toContain('--share-net'); expect(Object.keys(options.env).every(key => ['XDG_RUNTIME_DIR', 'DBUS_SESSION_BUS_ADDRESS'].includes(key))).toBe(true);
  const registry = args.indexOf('/etc/libreoffice/registry');
  expect(args.slice(registry - 1, registry + 2)).toEqual(['--ro-bind-try', '/etc/libreoffice/registry', '/etc/libreoffice/registry']);
  // Only package-owned registry data may cross /etc; never the Host's entire
  // configuration directory. Distros with their registry in /usr need no mount.
  expect(args.filter((arg: string) => arg === '/etc')).toEqual(['/etc']);
  expect(args[args.indexOf('/etc') - 1]).toBe('--dir');
  const directory = args[args.indexOf('/input') - 1];
  expect(existsSync(directory)).toBe(true);
  abort.abort(); expect(boundary.kill).toHaveBeenCalled(); expect(existsSync(directory)).toBe(true);
  child.emit('close', null); await rejected; expect(existsSync(directory)).toBe(false);
});
it('timeout kills and waits for close, while failed output never publishes', async () => {
  vi.useFakeTimers(); const { child, ready } = gate();
  const work = convertOffice(Buffer.from('input'), 'pptx', new AbortController().signal);
  const rejected = expect(work).rejects.toThrow('converter_timeout'); await ready; child.stdout.write('RUSTX-CGROUP /sys/fs/cgroup/test\n'); await new Promise<void>(resolve => child.stdin.once('finish', resolve));
  vi.advanceTimersByTime(15000); expect(boundary.kill).toHaveBeenCalled();
  child.stdout.write('%PDF-late'); child.emit('close', 0); await rejected;
  expect(vi.getTimerCount()).toBe(0);
});
it('cleans up success and malformed output', async () => {
  for (const valid of [true, false]) {
    const { child, ready } = gate(); const work = convertOffice(Buffer.from('input'), 'docx', new AbortController().signal);
    const result = valid ? expect(work).resolves.toEqual(Buffer.from('%PDF-real')) : expect(work).rejects.toThrow('converter_failure');
    await ready; const args = boundary.spawn.mock.calls.filter(call => call[0].endsWith('systemd-run')).at(-1)![1]; const directory = args[args.indexOf('/input') - 1];
    child.stdout.write('RUSTX-CGROUP /sys/fs/cgroup/test\n'); await new Promise<void>(resolve => child.stdin.once('finish', resolve));
    child.stdout.write(valid ? '%PDF-real' : 'invalid'); child.emit('close', 0); await result;
    expect(existsSync(directory)).toBe(false);
  }
});

it('missing cgroup admission fails closed before any permit; cancellation during startup cannot launch payload', async () => {
  for (const cancelled of [false, true]) {
    const { child, ready } = gate(), controller = new AbortController();
    const work = convertOffice(Buffer.from('input'), 'docx', controller.signal);
    const result = expect(work).rejects.toThrow(cancelled ? 'obsolete' : 'converter_unavailable');
    await ready;
    if (cancelled) controller.abort();
    expect(child.stdin.read()).toBeNull();
    child.emit('close', 1); await result;
  }
});
it('oversized output cannot publish a PDF and waits for service closure', async () => {
  const { child, ready } = gate();
  const work = convertOffice(Buffer.from('input'), 'docx', new AbortController().signal);
  const result = expect(work).rejects.toThrow('too_large'); await ready;
  child.stdout.write('RUSTX-CGROUP /sys/fs/cgroup/test\n'); await new Promise<void>(resolve => child.stdin.once('finish', resolve)); child.stdout.write(Buffer.alloc(4 * 1024 * 1024 + 1));
  child.emit('close', 0); await result;
});

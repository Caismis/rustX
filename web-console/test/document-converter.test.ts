// @vitest-environment node
import { EventEmitter } from 'node:events';
import { PassThrough } from 'node:stream';
import { existsSync } from 'node:fs';
import { afterEach, expect, it, vi } from 'vitest';
const boundary = vi.hoisted(() => ({ spawn: vi.fn() }));
vi.mock('node:child_process', () => ({ spawn: boundary.spawn }));
import { convertOffice } from '../host/documents/converter';
function gate() {
  const child = Object.assign(new EventEmitter(), { stdout: new PassThrough(), kill: vi.fn() });
  let spawned!: () => void; const ready = new Promise<void>(resolve => { spawned = resolve; });
  boundary.spawn.mockImplementation(() => { spawned(); return child; });
  return { child, ready };
}
afterEach(() => { vi.useRealTimers(); boundary.spawn.mockReset(); });
it('owns isolation arguments and retains temporary files until process-tree settlement after cancellation', async () => {
  const { child, ready } = gate(), abort = new AbortController();
  const work = convertOffice(Buffer.from('input'), 'docx', abort.signal);
  const rejected = expect(work).rejects.toThrow('obsolete');
  await ready;
  const [exe, args, options] = boundary.spawn.mock.calls[0];
  expect(exe).toBe('/usr/bin/bwrap');
  for (const required of ['--unshare-all', '--unshare-user', '--die-with-parent', '--new-session', '--clearenv', '--size', '67108864', '--as=1073741824', '--fsize=8388608', '--cpu=15']) expect(args).toContain(required);
  expect(args).not.toContain('--share-net'); expect(options.env).toEqual({});
  const registry = args.indexOf('/etc/libreoffice/registry');
  expect(args.slice(registry - 1, registry + 2)).toEqual(['--ro-bind-try', '/etc/libreoffice/registry', '/etc/libreoffice/registry']);
  // Only package-owned registry data may cross /etc; never the Host's entire
  // configuration directory. Distros with their registry in /usr need no mount.
  expect(args.filter((arg: string) => arg === '/etc')).toEqual(['/etc']);
  expect(args[args.indexOf('/etc') - 1]).toBe('--dir');
  const directory = args[args.indexOf('/input') - 1];
  expect(existsSync(directory)).toBe(true);
  abort.abort(); expect(child.kill).toHaveBeenCalledWith('SIGKILL'); expect(existsSync(directory)).toBe(true);
  child.emit('close', null); await rejected; expect(existsSync(directory)).toBe(false);
});
it('timeout kills and waits for close, while failed output never publishes', async () => {
  vi.useFakeTimers(); const { child, ready } = gate();
  const work = convertOffice(Buffer.from('input'), 'pptx', new AbortController().signal);
  const rejected = expect(work).rejects.toThrow('converter_timeout'); await ready;
  vi.advanceTimersByTime(15000); expect(child.kill).toHaveBeenCalledWith('SIGKILL');
  child.stdout.write('%PDF-late'); child.emit('close', 0); await rejected;
  expect(vi.getTimerCount()).toBe(0);
});
it('cleans up success and malformed output', async () => {
  for (const valid of [true, false]) {
    const { child, ready } = gate(); const work = convertOffice(Buffer.from('input'), 'docx', new AbortController().signal);
    const result = valid ? expect(work).resolves.toEqual(Buffer.from('%PDF-real')) : expect(work).rejects.toThrow('converter_failure');
    await ready; const args = boundary.spawn.mock.calls.at(-1)![1]; const directory = args[args.indexOf('/input') - 1];
    child.stdout.write(valid ? '%PDF-real' : 'invalid'); child.emit('close', 0); await result;
    expect(existsSync(directory)).toBe(false);
  }
});

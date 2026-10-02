// @vitest-environment node
import { afterEach, expect, it, vi } from 'vitest';
import { EventEmitter } from 'node:events';
import { mkdtempSync, mkdirSync, realpathSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { DesktopAdapter, macOSDesktop, desktopEnvironment, launchDesktop, type DesktopProcess, type DesktopSystem } from '../host/desktop';
import { LocalWorkspaceHost } from '../host/workspaces';
import type { DesktopTarget } from '../src/workspaces/desktop';
const directories: string[] = [];
afterEach(() => { directories.splice(0).forEach(path => rmSync(path, { recursive: true, force: true })); });
const target: DesktopTarget = { session_id: 'session-one', active_node: 'node-one' };
function gate<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(r => { resolve = r; }); return { promise, resolve }; }
function fixture(read?: (endpoint: string, token: string, target: DesktopTarget) => Promise<string>) {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), 'desktop-'))); directories.push(directory);
  const a = join(directory, '- 工作区 "quotes" ; $(false)\nnext'), b = join(directory, 'other'); mkdirSync(a); mkdirSync(b);
  const launch = vi.fn(async (_spec: DesktopProcess) => ({ status: 'spawned' as const }));
  const executable = vi.fn((path: string): string | undefined => path === '/bin/xdg-open' ? path : undefined);
  const system: DesktopSystem = { platform: 'linux', env: { PATH: '/bin', DISPLAY: ':0', RUSTX_TOKEN: 'secret', API_KEY: 'secret' }, executable, launch };
  const adapter = new DesktopAdapter(system);
  const config = { nativeFilesystem: 'shared' as const, endpoint: 'ws://localhost:8080/', transportToken: 'private', picker: true, metadataFile: join(directory, 'registrations.json'), roots: [{ id: 'a', cwd: a, displayName: 'A' }, { id: 'b', cwd: b, displayName: 'B' }] };
  const readSession = vi.fn(read ?? (async () => a));
  const host = new LocalWorkspaceHost(config, adapter, readSession);
  return { directory, a, b, host, config, readSession, launch, executable, adapter, system };
}
it('uses only exact native Session coordinates and canonical authorized cwd, independent of registration', async () => {
  const f = fixture(), scope = await f.host.listWorkspaces();
  await f.host.removeWorkspace(scope, scope.workspaces[0].id);
  expect(await f.host.desktopCatalog(scope)).toEqual({ available: true, applications: [{ id: 'files', label: 'File manager' }] });
  expect(await f.host.openWorkspace(scope, target, 'files')).toEqual({ status: 'spawned' });
  expect(f.readSession).toHaveBeenCalledExactlyOnceWith(f.config.endpoint, 'private', target);
  expect(f.launch).toHaveBeenCalledExactlyOnceWith({ command: '/bin/xdg-open', args: [f.a], cwd: f.a, env: { PATH: '/bin', DISPLAY: ':0' } });
});
it('native-selected second workspace wins over the first registered workspace and resolves aliases canonically', async () => {
  const f = fixture(); const alias = join(f.directory, 'alias'); symlinkSync(f.b, alias);
  f.readSession.mockResolvedValue(alias);
  await f.host.openWorkspace(await f.host.listWorkspaces(), target, 'files');
  expect(f.launch.mock.calls[0][0].args).toEqual([f.b]);
});
it('missing, unauthorized and replaced directories have no launch effect', async () => {
  const f = fixture(), scope = await f.host.listWorkspaces();
  f.readSession.mockResolvedValue(f.directory);
  await expect(f.host.openWorkspace(scope, target, 'files')).rejects.toThrow('denied');
  f.readSession.mockResolvedValue(f.a); rmSync(f.a, { recursive: true });
  await expect(f.host.openWorkspace(scope, target, 'files')).rejects.toThrow('unavailable');
  writeFileSync(f.a, 'not a directory');
  await expect(f.host.openWorkspace(scope, target, 'files')).rejects.toThrow('unavailable');
  rmSync(f.a);
  symlinkSync(f.b, f.a);
  await expect(f.host.openWorkspace(scope, target, 'files')).rejects.toThrow('denied');
  expect(f.launch).not.toHaveBeenCalled();
});
it('rejects malformed paths, commands, applications, wrong endpoint and stale authority before native work', async () => {
  const f = fixture(), scope = await f.host.listWorkspaces();
  for (const bad of [{ ...target, cwd: f.a }, { session_id: f.a, active_node: 'n' }, { session_id: '', active_node: 'n' }, null]) {
    await expect(f.host.openWorkspace(scope, bad as unknown as DesktopTarget, 'files')).rejects.toThrow('Invalid');
  }
  await expect(f.host.openWorkspace(scope, target, 'sh -c' as 'files')).rejects.toThrow('Invalid');
  await expect(f.host.openWorkspace({ ...scope, endpoint: 'ws://other/' }, target, 'files')).rejects.toThrow('replaced');
  await expect(f.host.openWorkspace({ ...scope, authorityId: 'retired' }, target, 'files')).rejects.toThrow('replaced');
  expect(f.readSession).not.toHaveBeenCalled(); expect(f.launch).not.toHaveBeenCalled();
});
it('authority revoked while exact native resolution is gated rejects with zero launches', async () => {
  const result = gate<string>(), entered = gate<void>();
  const f = fixture(async () => { entered.resolve(); return result.promise; }), scope = await f.host.listWorkspaces();
  const opening = f.host.openWorkspace(scope, target, 'files');
  await entered.promise; expect(f.launch).not.toHaveBeenCalled();
  await expect(f.host.openWorkspace(scope, target, 'files')).rejects.toThrow('pending');
  f.host.close(); result.resolve(f.a);
  await expect(opening).rejects.toThrow('replaced'); expect(f.launch).not.toHaveBeenCalled();
});
it('retired target, adapter failure and disappeared executables propagate without retry', async () => {
  const f = fixture(), scope = await f.host.listWorkspaces();
  f.readSession.mockRejectedValueOnce(new Error('Session target retired'));
  await expect(f.host.openWorkspace(scope, target, 'files')).rejects.toThrow('retired');
  f.launch.mockRejectedValueOnce(new Error('desktop refused'));
  await expect(f.host.openWorkspace(scope, target, 'files')).rejects.toThrow('desktop refused');
  expect(f.launch).toHaveBeenCalledTimes(1);
  f.executable.mockReturnValue(undefined);
  await expect(f.host.openWorkspace(scope, target, 'files')).rejects.toThrow('unavailable');
  expect(f.readSession).toHaveBeenCalledTimes(2); expect(f.launch).toHaveBeenCalledTimes(1);
});
it('executable disappearing during native resolution fails at the final adapter boundary', async () => {
  const result = gate<string>(), f = fixture(async () => result.promise), scope = await f.host.listWorkspaces();
  const opening = f.host.openWorkspace(scope, target, 'files');
  f.executable.mockReturnValue(undefined); result.resolve(f.a);
  await expect(opening).rejects.toThrow('disappeared'); expect(f.launch).not.toHaveBeenCalled();
});
it('missing explicit namespace mapping and headless Hosts never resolve native state or launch', async () => {
  const f = fixture(); const host = new LocalWorkspaceHost({ ...f.config, nativeFilesystem: undefined }, f.adapter, f.readSession);
  const scope = await host.listWorkspaces();
  expect(await host.desktopCatalog(scope)).toEqual({ available: false, reason: 'mapping' });
  await expect(host.openWorkspace(scope, target, 'files')).rejects.toThrow('mapping');
  delete f.system.env.DISPLAY;
  expect(await f.host.desktopCatalog(await f.host.listWorkspaces())).toEqual({ available: false, reason: 'headless' });
  await expect(f.host.openWorkspace(await f.host.listWorkspaces(), target, 'files')).rejects.toThrow('unavailable');
  expect(f.readSession).not.toHaveBeenCalled(); expect(f.launch).not.toHaveBeenCalled();
});
it.each([
  ['linux', 'files', '/bin/xdg-open', ['/tmp/-汉字 "quotes";$(x)\nline']],
  ['linux', 'terminal', '/bin/gnome-terminal', ['--working-directory', '/tmp/-汉字 "quotes";$(x)\nline']],
  ['linux', 'code', '/bin/code', ['--new-window', '--', '/tmp/-汉字 "quotes";$(x)\nline']],
  ['darwin', 'files', '/usr/bin/open', ['--', '/tmp/-汉字 "quotes";$(x)\nline']],
  ['darwin', 'terminal', '/usr/bin/open', ['-a', '/System/Applications/Utilities/Terminal.app', '--', '/tmp/-汉字 "quotes";$(x)\nline']],
] as const)('builds literal safe argv for %s %s', async (platform, id, command, args) => {
  const launch = vi.fn(async () => ({ status: 'spawned' as const }));
  const adapter = new DesktopAdapter({ platform, env: { PATH: '/bin', DISPLAY: ':0' }, macOSDesktop: () => true, executable: path => path, launch });
  const cwd = '/tmp/-汉字 "quotes";$(x)\nline';
  await adapter.prepare(id)(cwd);
  expect(launch).toHaveBeenCalledWith(expect.objectContaining({ command, args: [...args], cwd }));
});
it('discovery has no process probes and repeated menus use a bounded cached catalog', () => {
  const f = fixture(); f.adapter.catalog(); const count = f.executable.mock.calls.length;
  f.adapter.catalog(); expect(f.executable.mock.calls.length - count).toBe(1); expect(f.launch).not.toHaveBeenCalled();
  expect(desktopEnvironment({ API_KEY: 'secret', NODE_OPTIONS: '--require evil', RUSTX_TRANSPORT_TOKEN: 'secret', DISPLAY: ':0' })).toEqual({ DISPLAY: ':0' });
});
it('spawn acknowledgement detaches user app lifetime; spawn errors surface without shell or credentials', async () => {
  const child = Object.assign(new EventEmitter(), { unref: vi.fn(), kill: vi.fn() });
  const spawn = vi.fn(() => child);
  const spec = { command: '/bin/opener', args: ['/a b'], cwd: '/a b', env: { DISPLAY: ':0' } };
  const opening = launchDesktop(spec, spawn as unknown as typeof import('node:child_process').spawn);
  expect(spawn).toHaveBeenCalledWith(spec.command, spec.args, { cwd: spec.cwd, env: spec.env, shell: false, detached: true, stdio: 'ignore' });
  child.emit('spawn'); child.emit('exit', 0, null); expect(await opening).toEqual({ status: 'spawned' });
  expect(child.unref).toHaveBeenCalledOnce(); expect(child.kill).not.toHaveBeenCalled();
  const failed = launchDesktop(spec, spawn as unknown as typeof import('node:child_process').spawn);
  child.emit('error', new Error('private diagnostics'));
  await expect(failed).rejects.toThrow('could not be started'); expect(child.kill).not.toHaveBeenCalled();
});
it('Host retirement after spawn does not turn the handed-off application into a cancellation', async () => {
  const f = fixture(), held = gate<{ status: 'spawned' }>(), scope = await f.host.listWorkspaces();
  const spawned = gate<void>();
  f.launch.mockImplementationOnce(() => { spawned.resolve(); return held.promise; });
  const opening = f.host.openWorkspace(scope, target, 'files'); await spawned.promise;
  f.host.close(); held.resolve({ status: 'spawned' });
  expect(await opening).toEqual({ status: 'spawned' }); expect(f.launch).toHaveBeenCalledOnce();
});
it('HTTP rejects extra path/argv fields and foreign origins before resolution or launch', async () => {
  const { createServer } = await import('node:http');
  const { workspaceHandler } = await import('../host/http');
  const f = fixture(), scope = await f.host.listWorkspaces();
  const server = createServer(workspaceHandler(f.host));
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const address = server.address(); if (!address || typeof address === 'string') throw new Error('No address');
  const url = `http://127.0.0.1:${address.port}/product-host/desktop-open`;
  try {
    for (const extra of [{ path: f.a }, { command: '/bin/sh' }, { args: [f.a] }]) {
      const response = await fetch(url, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ scope, target, application: 'files', ...extra }) });
      expect(response.status).toBe(400);
    }
    expect((await fetch(url, { method: 'POST', headers: { 'Content-Type': 'application/json', Origin: 'https://foreign.example' }, body: JSON.stringify({ scope, target, application: 'files' }) })).status).toBe(400);
    const oversized = await fetch(url, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ scope, target, application: 'files', padding: '汉'.repeat(25_000) }) });
    expect(oversized.status).toBe(400); expect((await oversized.json()).message).toContain('too large');
    expect(f.readSession).not.toHaveBeenCalled(); expect(f.launch).not.toHaveBeenCalled();
  } finally { await new Promise<void>(resolve => server.close(() => resolve())); }
});
it('lost HTTP launch acknowledgement is uncertain and never retried automatically', async () => {
  const { HttpWorkspaceHost } = await import('../src/workspaces/http-host');
  vi.stubGlobal('location', new URL('http://localhost/'));
  const fetch = vi.spyOn(globalThis, 'fetch').mockRejectedValue(new Error('connection lost'));
  const host = new HttpWorkspaceHost();
  await expect(host.openWorkspace({ authorityId: 'host', endpoint: 'ws://localhost/' }, target, 'files')).rejects.toMatchObject({ uncertain: true });
  expect(fetch).toHaveBeenCalledTimes(1);
  vi.unstubAllGlobals();
});

it('early launcher nonzero exits surface; the observation bound never kills a running GUI', async () => {
  vi.useFakeTimers();
  try {
    const child = Object.assign(new EventEmitter(), { unref: vi.fn(), kill: vi.fn() });
    const spawn = vi.fn(() => child) as unknown as typeof import('node:child_process').spawn;
    const spec = { command: '/bin/opener', args: ['/workspace'], cwd: '/workspace', env: {} };
    const failed = launchDesktop(spec, spawn); child.emit('spawn'); child.emit('exit', 7, null);
    await expect(failed).rejects.toThrow('exit 7'); expect(vi.getTimerCount()).toBe(0);
    const alive = launchDesktop(spec, spawn); child.emit('spawn');
    expect(vi.getTimerCount()).toBe(1); expect(child.kill).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1_000);
    expect(await alive).toEqual({ status: 'spawned' }); expect(child.kill).not.toHaveBeenCalled();
  } finally { vi.useRealTimers(); }
});
it('macOS discovery retires Terminal when its verified bundle executable disappears', () => {
  let installed = true;
  const system: DesktopSystem = { platform: 'darwin', env: {}, macOSDesktop: () => true, executable: path => path === '/usr/bin/open' || (installed && path.endsWith('/MacOS/Terminal')) ? path : undefined, launch: vi.fn() };
  const adapter = new DesktopAdapter(system);
  expect(adapter.catalog()).toMatchObject({ applications: [{ id: 'files' }, { id: 'terminal' }] });
  const launch = adapter.prepare('terminal'); installed = false;
  expect(() => launch('/workspace')).toThrow('disappeared');
  expect(adapter.catalog()).toMatchObject({ applications: [{ id: 'files' }] }); expect(system.launch).not.toHaveBeenCalled();
});

it('explicit refresh discovers an installed application exactly once; cached reads never scan for it', async () => {
  const f = fixture(), scope = await f.host.listWorkspaces();
  expect(await f.host.desktopCatalog(scope)).toMatchObject({ applications: [{ id: 'files' }] });
  expect(f.executable).toHaveBeenCalledTimes(4); // three candidates, one verification
  f.executable.mockImplementation(path => ['/bin/xdg-open', '/bin/code'].includes(path) ? path : undefined);
  expect(await f.host.desktopCatalog(scope)).toMatchObject({ applications: [{ id: 'files' }] });
  expect(f.executable).toHaveBeenCalledTimes(5);
  expect(await f.host.desktopCatalog(scope, true)).toMatchObject({ applications: [{ id: 'files' }, { id: 'code' }] });
  expect(f.executable).toHaveBeenCalledTimes(10); // one scan, two verifications
  await f.host.desktopCatalog(scope);
  expect(f.executable).toHaveBeenCalledTimes(12);
  expect(f.executable.mock.calls.filter(([path]) => path === '/bin/code')).toHaveLength(4); // two discovery + two verification
  expect(f.readSession).not.toHaveBeenCalled(); expect(f.launch).not.toHaveBeenCalled();
});
it('an executable removed from the cache returns only after one explicit rediscovery', () => {
  const f = fixture(); f.adapter.catalog(); expect(f.executable).toHaveBeenCalledTimes(4);
  f.executable.mockReturnValue(undefined);
  expect(f.adapter.catalog()).toEqual({ available: false, reason: 'applications' });
  expect(f.executable).toHaveBeenCalledTimes(5);
  f.executable.mockImplementation(path => path === '/bin/xdg-open' ? path : undefined);
  expect(f.adapter.catalog()).toEqual({ available: false, reason: 'applications' });
  expect(f.executable).toHaveBeenCalledTimes(5);
  expect(f.adapter.catalog(true)).toMatchObject({ applications: [{ id: 'files' }] });
  expect(f.executable).toHaveBeenCalledTimes(9);
  f.adapter.catalog(); expect(f.executable).toHaveBeenCalledTimes(10);
  expect(f.launch).not.toHaveBeenCalled();
});
it.each(['win32', 'freebsd'] as const)('unsupported platform %s does no discovery or launch', platform => {
  const f = fixture(); f.system.platform = platform;
  expect(f.adapter.catalog(true)).toEqual({ available: false, reason: 'platform' });
  expect(() => f.adapter.prepare('files')).toThrow('unavailable');
  expect(f.executable).not.toHaveBeenCalled(); expect(f.launch).not.toHaveBeenCalled();
});
it('macOS requires the non-root console owner and Aqua bootstrap; unknown evidence fails closed', () => {
  const owner = vi.fn(() => 501), manager = vi.fn(() => 'Aqua\n');
  expect(macOSDesktop(0, owner, manager)).toBe(false);
  expect(owner).not.toHaveBeenCalled(); expect(manager).not.toHaveBeenCalled();
  expect(macOSDesktop(502, owner, manager)).toBe(false); expect(manager).not.toHaveBeenCalled();
  expect(macOSDesktop(501, owner, manager)).toBe(true); expect(manager).toHaveBeenCalledTimes(1);
  for (const context of ['Background', 'LoginWindow', 'System', '', 'unknown']) {
    expect(macOSDesktop(501, owner, () => context)).toBe(false);
  }
  expect(macOSDesktop(501, () => { throw new Error('no console'); }, manager)).toBe(false);
  expect(macOSDesktop(501, owner, () => { throw new Error('query failed'); })).toBe(false);
  expect(manager).toHaveBeenCalledTimes(1);
});
it('macOS opener existence cannot override unknown, headless or SSH availability', () => {
  const f = fixture(); f.system.platform = 'darwin'; f.executable.mockImplementation(path => path);
  expect(f.adapter.catalog()).toEqual({ available: false, reason: 'headless' });
  const session = vi.fn(() => false); f.system.macOSDesktop = session;
  expect(f.adapter.catalog(true)).toEqual({ available: false, reason: 'headless' });
  expect(f.executable).not.toHaveBeenCalled();
  session.mockReturnValue(true); f.system.env.SSH_CONNECTION = 'remote';
  expect(f.adapter.catalog(true)).toEqual({ available: false, reason: 'headless' });
  expect(session).toHaveBeenCalledTimes(1); expect(f.executable).not.toHaveBeenCalled();
  delete f.system.env.SSH_CONNECTION;
  expect(f.adapter.catalog(true)).toMatchObject({ available: true });
});
it('real Node child receives literal argv, canonical cwd and only filtered environment without a GUI', async () => {
  const { spawn } = await import('node:child_process');
  const { readFileSync } = await import('node:fs');
  const f = fixture(), output = join(f.directory, 'child.json'), closed = gate<void>();
  const literal = '- quotes " 汉字 ; $(false)\nnext';
  const spec = { command: process.execPath, cwd: f.a,
    args: ['-e', 'require("node:fs").writeFileSync(process.argv[1], JSON.stringify({cwd:process.cwd(),arg:process.argv[2],env:process.env}))', '--', output, literal],
    env: desktopEnvironment({ HOME: f.directory, NODE_OPTIONS: '--invalid', API_KEY: 'secret', RUSTX_TRANSPORT_TOKEN: 'secret' }) };
  expect(await launchDesktop(spec, ((...args: Parameters<typeof spawn>) => {
    const child = spawn(...args); child.once('close', () => closed.resolve()); return child;
  }) as typeof spawn)).toEqual({ status: 'spawned' });
  await closed.promise;
  expect(JSON.parse(readFileSync(output, 'utf8'))).toEqual({ cwd: f.a, arg: literal, env: { HOME: f.directory } });
  await expect(launchDesktop({ ...spec, command: join(f.directory, 'missing-opener') })).rejects.toThrow('could not be started');
});
it('executes native eligibility checks fail-closed without opening a GUI', () => {
  expect(typeof macOSDesktop()).toBe('boolean');
  // Exercise the real bounded launchctl query even on a headless macOS runner.
  // Linux has no launchctl and must fail closed. Neither case launches a GUI.
  const result = macOSDesktop(501, () => 501);
  if (process.platform === 'darwin') expect(typeof result).toBe('boolean');
  else expect(result).toBe(false);
  expect(macOSDesktop(0)).toBe(false);
});

it('HTTP catalog refresh reaches the adapter exactly once and rejects malformed refresh inputs', async () => {
  const { createServer } = await import('node:http');
  const { workspaceHandler } = await import('../host/http');
  const { HttpWorkspaceHost } = await import('../src/workspaces/http-host');
  const f = fixture(), scope = await f.host.listWorkspaces(), server = createServer(workspaceHandler(f.host));
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const address = server.address(); if (!address || typeof address === 'string') throw new Error('No address');
  const base = `http://127.0.0.1:${address.port}/product-host`;
  vi.stubGlobal('location', new URL(base));
  try {
    const client = new HttpWorkspaceHost(base);
    await client.desktopCatalog(scope); expect(f.executable).toHaveBeenCalledTimes(4);
    f.executable.mockImplementation(path => ['/bin/xdg-open', '/bin/code'].includes(path) ? path : undefined);
    expect(await client.desktopCatalog(scope)).toMatchObject({ applications: [{ id: 'files' }] });
    expect(f.executable).toHaveBeenCalledTimes(5);
    expect(await client.desktopCatalog(scope, true)).toMatchObject({ applications: [{ id: 'files' }, { id: 'code' }] });
    expect(f.executable).toHaveBeenCalledTimes(10);
    for (const refresh of [undefined, 'true', 1, {}, null]) {
      const response = await fetch(`${base}/desktop-catalog`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ scope, refresh }) });
      expect(response.status).toBe(400);
    }
    expect(f.executable).toHaveBeenCalledTimes(10);
    expect(f.readSession).not.toHaveBeenCalled(); expect(f.launch).not.toHaveBeenCalled();
  } finally { vi.unstubAllGlobals(); await new Promise<void>(resolve => server.close(() => resolve())); }
});

it('each requested discovery probes at most three fixed names in 32 eligible PATH directories', () => {
  const f = fixture(); f.executable.mockReturnValue(undefined);
  f.system.env.PATH = ['relative', '/' + 'x'.repeat(4096), ...Array.from({ length: 40 }, (_, i) => `/dir${i}`)].join(':');
  expect(f.adapter.catalog()).toEqual({ available: false, reason: 'applications' });
  expect(f.executable).toHaveBeenCalledTimes(96);
  f.adapter.catalog(); expect(f.executable).toHaveBeenCalledTimes(96);
  f.adapter.catalog(true); expect(f.executable).toHaveBeenCalledTimes(192);
  expect(f.executable.mock.calls.every(([path]) => /^\/dir(?:[0-9]|[12][0-9]|3[01])\/(xdg-open|gnome-terminal|code)$/.test(path))).toBe(true);
  expect(f.launch).not.toHaveBeenCalled();
});

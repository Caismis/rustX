import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, readdirSync, statSync, existsSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseArguments } from '../src/arguments.ts';
import { appServerEndpoint } from '../src/app-server-readiness.ts';
import { Launcher } from '../src/launcher.ts';
import type { Spawn, ChildSpec } from '../src/process.ts';
import { LocalWorkspaceHost } from '../../web-console/host/workspaces.ts';
import { parseArguments as parseTui } from '../../tui/src/cli.ts';

const root = fileURLToPath(new URL('../../', import.meta.url));
function deferred<T>() { let resolve!: (value: T) => void, reject!: (error: Error) => void; const promise = new Promise<T>((a, b) => { resolve = a; reject = b; }); return { promise, resolve, reject }; }
function harness() {
  const calls: { spec: ChildSpec; ready: ReturnType<typeof deferred<string>>; reap: ReturnType<typeof deferred<void>>; exit: (code: number) => void; ownerShutdown: () => void; stops: number }[] = [];
  const changes: (() => void)[] = [];
  const spawn: Spawn = (spec, exit, ownerShutdown) => {
    const call = { spec, ready: deferred<string>(), reap: deferred<void>(), exit, ownerShutdown, stops: 0 };
    calls.push(call); changes.splice(0).forEach(resolve => resolve());
    return { ready: call.ready.promise, stop: () => { call.stops++; return call.reap.promise; } };
  };
  return { calls, spawn, async count(n: number) { while (calls.length < n) await new Promise<void>(resolve => changes.push(resolve)); } };
}
function fixture() {
  const directory = mkdtempSync(join(tmpdir(), 'rustx-dev-test-'));
  const a = join(directory, 'workspace with spaces'), b = join(directory, 'second');
  mkdirSync(a); mkdirSync(b); writeFileSync(join(a, 'user-owned'), 'keep');
  const args = parseArguments(['web', '--binary', process.execPath, '--config', '/settings with spaces.toml', '--runtime-root', '/runtime with spaces', '--workspace', a, '--workspace', b], root);
  return { directory, a, b, args, remove: () => rmSync(directory, { recursive: true, force: true }) };
}

test('composition grammar consumes only owned options; native values remain opaque', () => {
  const result = parseArguments(['app-server', '--', '--binary', process.execPath, '--config', '/a b', '--model', '--binary', '--runtime-root', '/r'], root);
  assert.equal(result.binary, process.execPath);
  assert.deepEqual(result.forwarded, ['--config', '/a b', '--model', '--binary', '--runtime-root', '/r']);
  assert.equal(parseArguments(['app-server'], root).binary, join(root, 'target/debug/rustx'));
  for (const argv of [[], ['invalid'], ['web'], ['web', '--workspace'], ['web', '--workspace', 'relative'], ['web', '--listen', 'ws://x'], ['web', '--token-file', '/x'], ['app-server', '--binary'], ['app-server', '--binary', '/a', '--binary', '/b']]) {
    assert.throws(() => parseArguments(argv, root));
  }
});

test('App Server invokes the selected native executable with exact configuration arguments and propagates failure', async () => {
  const h = harness(), launcher = new Launcher(root, h.spawn);
  const forwarded = ['--config', '/path with spaces', '--model', 'default', '--runtime-root', '/runtime', '--listen', 'stdio'];
  await launcher.start(parseArguments(['app-server', '--binary', process.execPath, ...forwarded], root));
  const child = h.calls[0];
  assert.equal(child.spec.command, process.execPath);
  assert.deepEqual(child.spec.args, ['app-server', ...forwarded]);
  child.exit(27);
  child.reap.resolve();
  assert.equal(await launcher.done, 27);
  assert.equal(child.stops, 1);
});

test('TUI delegates unchanged arguments to the existing composition root and native host contract', async () => {
  const h = harness(), launcher = new Launcher(root, h.spawn);
  const forwarded = ['--config', '/user/rustx.toml', '--model', 'default', '--runtime-root', '/runtime', '--workspace', '/workspace with spaces', '--resume'];
  await launcher.start(parseArguments(['tui', '--binary', process.execPath, ...forwarded], root));
  const child = h.calls[0];
  assert.equal(child.spec.command, process.execPath);
  assert.deepEqual(child.spec.args, [join(root, 'tui/src/main.ts'), '--binary', process.execPath, ...forwarded]);
  const parsed = parseTui(child.spec.args.slice(1));
  assert.deepEqual(parsed.mode, { kind: 'local', binary: process.execPath, launch: { config: '/user/rustx.toml', runtimeRoot: '/runtime' } });
  assert.equal(parsed.sessionSettings.cwd, '/workspace with spaces');
  assert.deepEqual(parsed.sessionSettings.model, { model: 'default' });
  assert.throws(() => parseTui(['--binary', process.execPath, '--unknown']), /unknown/);
  assert.throws(() => parseTui(['--binary', process.execPath, '--cwd', '/obsolete']), /unknown/);
  const stopping = launcher.settle(0); child.reap.resolve(); await stopping;
});

test('Web creates one token/config, passes the bound endpoint, and cleanup waits for BOTH owned children', async t => {
  const f = fixture(); t.after(f.remove);
  const h = harness(), launcher = new Launcher(root, h.spawn);
  const starting = launcher.start(f.args);
  await h.count(1);
  const app = h.calls[0], tokenFile = app.spec.args.at(-1)!;
  const scratch = dirname(tokenFile);
  t.after(() => rmSync(scratch, { recursive: true, force: true }));
  assert.equal(app.spec.component, 'app-server');
  assert.deepEqual(app.spec.args, ['app-server', ...f.args.forwarded, '--listen', 'ws://127.0.0.1:0', '--token-file', tokenFile]);
  assert.match(readFileSync(tokenFile, 'utf8'), /^[A-Za-z0-9_-]{43}$/);
  assert.equal(statSync(tokenFile).mode & 0o777, 0o600);
  assert.equal(statSync(scratch).mode & 0o777, 0o700);
  assert.deepEqual(readdirSync(scratch), ['transport-token']);
  app.ready.resolve('ws://127.0.0.1:4242/');
  await h.count(2);
  const web = h.calls[1], configFile = web.spec.env!.RUSTX_WORKSPACE_HOST_CONFIG!;
  const config = JSON.parse(readFileSync(configFile, 'utf8'));
  assert.deepEqual(config, { endpoint: 'ws://127.0.0.1:4242/', picker: true, metadataFile: join(scratch, 'workspaces.json'), roots: [
    { id: 'root-1', cwd: f.a, displayName: 'workspace with spaces' }, { id: 'root-2', cwd: f.b, displayName: 'second' },
  ] });
  assert.deepEqual(readdirSync(scratch).sort(), ['host-config.json', 'transport-token']);
  assert.equal(statSync(configFile).mode & 0o777, 0o600);
  const host = new LocalWorkspaceHost(config);
  const nested = join(f.a, 'nested'); mkdirSync(nested);
  assert.deepEqual((await host.classifyLocations([f.a, f.b, nested, f.directory], config.endpoint)).map(row => row.authorized), [true, true, false, false]);
  await assert.rejects(host.adoptWorkspace(nested), /unavailable/);
  assert.equal(web.spec.component, 'web');
  assert.deepEqual(web.spec.args, [join(root, 'web-console/scripts/dev-carrier.ts')]);
  web.ready.resolve('http://127.0.0.1:4243/');
  assert.deepEqual(await starting, { url: 'http://127.0.0.1:4243/', endpoint: config.endpoint, tokenFile, hostConfigFile: configFile, workspaces: [f.a, f.b] });
  assert.equal(h.calls.length, 2); // no provider, fixture, or extra Host writer
  const terminal = launcher.settle(130);
  assert.equal(launcher.settle(143), terminal);
  app.exit(9); web.exit(0); // competing terminal causes preserve the winner
  app.reap.resolve(); await Promise.resolve();
  assert.ok(existsSync(scratch));
  web.reap.resolve();
  assert.equal(await terminal, 130);
  assert.equal(await launcher.done, 130);
  assert.deepEqual(h.calls.map(c => c.stops), [1, 1]);
  assert.equal(existsSync(scratch), false);
  assert.equal(readFileSync(join(f.a, 'user-owned'), 'utf8'), 'keep');
  assert.equal(await launcher.settle(1), 130);
});

test('partial startup failure: A ready, B starts/fails, A reaped before scratch disappears', async t => {
  const f = fixture(); t.after(f.remove);
  const h = harness(), launcher = new Launcher(root, h.spawn);
  const starting = launcher.start(f.args);
  const app = h.calls[0], scratch = dirname(app.spec.args.at(-1)!);
  app.ready.resolve('ws://127.0.0.1:4444/');
  await h.count(2);
  const web = h.calls[1];
  web.exit(42); const terminal = launcher.settle(129); web.ready.reject(new Error('Vite startup failed')); web.reap.resolve();
  await Promise.resolve();
  assert.ok(existsSync(scratch));
  assert.equal(app.stops, 1);
  app.reap.resolve();
  await starting;
  assert.equal(await launcher.done, 42);
  assert.equal(await terminal, 42);
  assert.equal(existsSync(scratch), false);
  assert.equal(readFileSync(join(f.a, 'user-owned'), 'utf8'), 'keep');
});

test('signal fence wins over late readiness: no Host config or second child after settlement', async t => {
  const f = fixture(); t.after(f.remove);
  const h = harness(), launcher = new Launcher(root, h.spawn);
  const starting = launcher.start(f.args), app = h.calls[0];
  const scratch = dirname(app.spec.args.at(-1)!);
  const terminal = launcher.settle(129);
  app.ready.resolve('ws://127.0.0.1:4444/');
  await Promise.resolve();
  assert.equal(h.calls.length, 1);
  assert.deepEqual(readdirSync(scratch), ['transport-token']);
  app.exit(0); app.reap.resolve();
  await starting;
  assert.equal(await terminal, 129);
  assert.equal(existsSync(scratch), false);
  assert.equal(h.calls.length, 1);
});

test('terminal fence before start creates nothing, and missing binary is actionable', async () => {
  const h = harness(), launcher = new Launcher(root, h.spawn);
  await launcher.settle(130);
  await launcher.start(parseArguments(['app-server', '--binary', process.execPath], root));
  assert.equal(h.calls.length, 0);
  const missing = new Launcher(root, h.spawn);
  await missing.start(parseArguments(['app-server', '--binary', '/missing/rustx'], root));
  assert.equal(await missing.done, 1);
  assert.equal(h.calls.length, 0);
});

test('unexpected successful child exit while running still stops its sibling and fails composition', async t => {
  const f = fixture(); t.after(f.remove);
  const h = harness(), launcher = new Launcher(root, h.spawn);
  const starting = launcher.start(f.args), app = h.calls[0];
  app.ready.resolve('ws://127.0.0.1:4444/'); await h.count(2);
  const web = h.calls[1]; web.ready.resolve('http://127.0.0.1:4445/');
  const ready = await starting;
  web.exit(0); web.reap.resolve();
  await Promise.resolve();
  assert.equal(app.stops, 1);
  assert.ok(existsSync(ready!.tokenFile));
  app.reap.resolve();
  assert.equal(await launcher.done, 1);
  assert.equal(existsSync(ready!.tokenFile), false);
});

test('spawn exception after scratch allocation converges on cleanup without starting another child', async t => {
  const f = fixture(); t.after(f.remove);
  let scratch = '';
  const launcher = new Launcher(root, spec => {
    scratch = dirname(spec.args.at(-1)!);
    throw new Error('process boundary failed');
  });
  await launcher.start(f.args);
  assert.equal(await launcher.done, 1);
  assert.equal(existsSync(scratch), false);
});


test('native readiness parser accepts only the bounded bound-loopback announcement', () => {
  assert.equal(appServerEndpoint('rustx app-server listening ws://127.0.0.1:1234'), 'ws://127.0.0.1:1234');
  for (const line of ['listening ws://127.0.0.1:1234', 'rustx app-server listening ws://0.0.0.0:1234', 'rustx app-server listening ws://127.0.0.1:12 unexpected', 'x'.repeat(513)]) assert.equal(appServerEndpoint(line), undefined);
});

for (const [forwarded, expected] of [
  [[], 'shutdown-on-eof'],
  [['--listen', 'stdio'], 'shutdown-on-eof'],
  [['--listen', 'ws://127.0.0.1:8080'], undefined],
  [['--help'], undefined],
  [['--model', '--listen'], 'shutdown-on-eof'],
] as const) test(`standalone transport ownership is explicit for ${JSON.stringify(forwarded)}`, async () => {
  const h = harness(), launcher = new Launcher(root, h.spawn);
  await launcher.start(parseArguments(['app-server', '--binary', process.execPath, ...forwarded], root));
  assert.equal(h.calls[0].spec.ownerStdin, expected);
  assert.equal(h.calls[0].spec.protocolStdio, true);
  if (forwarded.length === 0) assert.deepEqual(h.calls[0].spec.args, ['app-server', '--listen', 'stdio']);
  const terminal = launcher.settle(0); h.calls[0].reap.resolve(); await terminal;
});

for (const first of ['eof', 'child', 'SIGINT', 'SIGHUP', 'SIGTERM'] as const) test(`standalone terminal race preserves ${first} and reaps once`, async () => {
  const h = harness(), launcher = new Launcher(root, h.spawn);
  await launcher.start(parseArguments(['app-server', '--binary', process.execPath], root));
  const child = h.calls[0];
  const codes = { eof: 0, child: 27, SIGINT: 130, SIGHUP: 129, SIGTERM: 143 };
  if (first === 'eof') child.ownerShutdown();
  else if (first === 'child') child.exit(27);
  else void launcher.settle(codes[first]);
  child.ownerShutdown(); child.exit(27);
  const terminal = launcher.settle(129);
  assert.equal(launcher.settle(130), terminal);
  assert.equal(launcher.settle(143), terminal);
  child.reap.resolve();
  assert.equal(await terminal, codes[first]);
  assert.equal(await launcher.done, codes[first]);
  assert.equal(child.stops, 1);
});

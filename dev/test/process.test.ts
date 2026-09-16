import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { mkdtempSync, writeFileSync, readFileSync, existsSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { once } from 'node:events';
import { spawnOwned } from '../src/process.ts';
import { Launcher } from '../src/launcher.ts';
import { parseArguments } from '../src/arguments.ts';
const root = fileURLToPath(new URL('../../', import.meta.url));

test('real executable failure preserves native stdout, stderr, exact argv and exit code', t => {
  const directory = mkdtempSync(join(tmpdir(), 'rustx-launch-test-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const binary = join(directory, 'native binary with spaces');
  writeFileSync(binary, '#!/usr/bin/env node\nconsole.log(JSON.stringify(process.argv.slice(2))); console.error("native configuration failure: original detail"); process.exitCode = 23;\n', { mode: 0o700 });
  const result = spawnSync(process.execPath, [join(root, 'dev/src/main.ts'), 'app-server', '--', '--binary', binary, '--user-settings', '/a b', '--runtime-root', '/c d'], { encoding: 'utf8', timeout: 15_000 });
  assert.equal(result.status, 23, result.stderr);
  assert.deepEqual(JSON.parse(result.stdout), ['app-server', '--user-settings', '/a b', '--runtime-root', '/c d', '--listen', 'stdio']);
  assert.match(result.stderr, /\[app-server\] native configuration failure: original detail/);
});

test('real owned children: partial startup failure waits for native shutdown and removes scratch', { timeout: 20_000 }, async t => {
  const directory = mkdtempSync(join(tmpdir(), 'rustx-launch-test-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const marker = join(directory, 'native-settled');
  const native = join(directory, 'native.mjs');
  writeFileSync(native, `import {createServer} from 'node:net'; import {writeFileSync} from 'node:fs';
const server=createServer(); server.listen(0,'127.0.0.1',()=>console.error('rustx app-server listening ws://127.0.0.1:'+server.address().port));
process.on('SIGTERM',()=>server.close(()=>writeFileSync(${JSON.stringify(marker)},'reaped')));`);
  const pids: number[] = [];
  let scratch = '';
  const launcher = new Launcher(root, (spec, exited) => {
    if (spec.component === 'app-server') scratch = dirname(spec.args.at(-1)!);
    const child = spawnOwned({ ...spec, command: process.execPath,
      args: spec.component === 'app-server' ? [native] : ['-e', 'console.error("carrier boot refused: original cause"); process.exit(37)'] }, exited);
    pids.push(child.pid!);
    return child;
  });
  await launcher.start(parseArguments(['web', '--binary', process.execPath, '--workspace', directory], root));
  assert.equal(await launcher.done, 37);
  assert.equal(readFileSync(marker, 'utf8'), 'reaped');
  assert.equal(existsSync(scratch), false);
  assert.equal(pids.length, 2);
  for (const pid of pids) assert.throws(() => process.kill(pid, 0), { code: 'ESRCH' });
});

test('real SIGTERM during native readiness settles once without launching Web', { timeout: 20_000 }, async t => {
  const directory = mkdtempSync(join(tmpdir(), 'rustx-launch-test-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const script = join(directory, 'signal-test.mjs');
  writeFileSync(script, `import {Launcher} from ${JSON.stringify(new URL('../src/launcher.ts', import.meta.url).href)};
import {parseArguments} from ${JSON.stringify(new URL('../src/arguments.ts', import.meta.url).href)};
const launcher=new Launcher(${JSON.stringify(root)}, (spec)=>{
process.send({owned: spec.args.at(-1)});
return {ready:new Promise(()=>{}),stop:async()=>process.send({stopped:true})};
});
process.on('message',()=>{});
process.on('SIGTERM',()=>void launcher.settle(143));
await launcher.start(parseArguments(['web','--binary',process.execPath,'--workspace',${JSON.stringify(directory)}],${JSON.stringify(root)}));
process.exitCode=await launcher.done; process.disconnect();`);
  const child = spawn(process.execPath, [script], { stdio: ['ignore', 'pipe', 'pipe', 'ipc'] });
  t.after(() => { if (child.exitCode === null) child.kill('SIGKILL'); });
  const messages: unknown[] = [];
  child.on('message', message => messages.push(message));
  const closed = once(child, 'close');
  const [ready] = await once(child, 'message') as [{ owned: string }];
  assert.ok(existsSync(ready.owned));
  child.kill('SIGTERM');
  const [code] = await closed;
  assert.equal(code, 143);
  assert.equal(messages.length, 2);
  assert.deepEqual(messages[1], { stopped: true });
  assert.equal(existsSync(dirname(ready.owned)), false);
});

test('owned process group settles descendants even when the leader exits without reaping them', { timeout: 20_000 }, async t => {
  const directory = mkdtempSync(join(tmpdir(), 'rustx-launch-tree-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const pidFile = join(directory, 'descendant-pid');
  const script = join(directory, 'leader.mjs');
  writeFileSync(script, `import {spawn} from 'node:child_process'; import {writeFileSync} from 'node:fs';
const descendant=spawn(process.execPath,['-e',"const server=require('node:net').createServer();server.listen(0,'127.0.0.1',()=>process.send({ready:true}));"],{stdio:['ignore','ignore','ignore','ipc']});
descendant.once('message',()=>{writeFileSync(${JSON.stringify(pidFile)},String(descendant.pid));console.error('rustx app-server listening ws://127.0.0.1:1234');});
process.stdin.resume();`);
  const child = spawnOwned({ component: 'app-server', command: process.execPath, args: [script], cwd: directory, readiness: 'app-server' }, () => {});
  await child.ready;
  const descendant = Number(readFileSync(pidFile, 'utf8'));
  process.kill(descendant, 0);
  await child.stop();
  assert.throws(() => process.kill(child.pid!, 0), { code: 'ESRCH' });
  assert.throws(() => process.kill(descendant, 0), { code: 'ESRCH' });
});

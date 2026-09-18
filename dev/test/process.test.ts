import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { mkdtempSync, writeFileSync, readFileSync, existsSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { once } from 'node:events';
import { createServer, type Socket } from 'node:net';
import { createInterface } from 'node:readline';
import { spawnOwned } from '../src/process.ts';
import { Launcher } from '../src/launcher.ts';
import { parseArguments } from '../src/arguments.ts';
const root = fileURLToPath(new URL('../../', import.meta.url));

test('real executable failure preserves native stdout, stderr, exact argv and exit code', t => {
  const directory = mkdtempSync(join(tmpdir(), 'rustx-launch-test-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const binary = join(directory, 'native binary with spaces');
  writeFileSync(binary, '#!/usr/bin/env node\nconsole.log(JSON.stringify(process.argv.slice(2))); console.error("native configuration failure: original detail"); process.exitCode = 23;\n', { mode: 0o700 });
  const result = spawnSync(process.execPath, [join(root, 'dev/src/main.ts'), 'app-server', '--', '--binary', binary, '--config', '/a b', '--runtime-root', '/c d', '--listen', 'ws://127.0.0.1:8080'], { encoding: 'utf8', timeout: 15_000 });
  assert.equal(result.status, 23, result.stderr);
  assert.deepEqual(JSON.parse(result.stdout), ['app-server', '--config', '/a b', '--runtime-root', '/c d', '--listen', 'ws://127.0.0.1:8080']);
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
  const launcher = new Launcher(root, (spec, exited, ownerShutdown) => {
    if (spec.component === 'app-server') scratch = dirname(spec.args.at(-1)!);
    const child = spawnOwned({ ...spec, command: process.execPath,
      args: spec.component === 'app-server' ? [native] : ['-e', 'console.error("carrier boot refused: original cause"); process.exit(37)'] }, exited, ownerShutdown);
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
  const child = spawnOwned({ component: 'app-server', command: process.execPath, args: [script], cwd: directory, readiness: 'app-server' }, () => {}, () => {});
  await child.ready;
  const descendant = Number(readFileSync(pidFile, 'utf8'));
  process.kill(descendant, 0);
  await child.stop();
  assert.throws(() => process.kill(child.pid!, 0), { code: 'ESRCH' });
  assert.throws(() => process.kill(descendant, 0), { code: 'ESRCH' });
});

// A socket is the barrier: the fake native process detaches on stdin EOF and
// acknowledges explicit SIGTERM, then holds cleanup until the test releases it.
function lines(stream: NodeJS.ReadableStream) {
  const buffered: string[] = [], waiting: ((line: string) => void)[] = [];
  const reader = createInterface({ input: stream });
  reader.on('line', line => { const resolve = waiting.shift(); if (resolve) resolve(line); else buffered.push(line); });
  return { next: () => buffered.length ? Promise.resolve(buffered.shift()!) : new Promise<string>(resolve => waiting.push(resolve)), close: () => reader.close() };
}

async function nativeFixture(t: import('node:test').TestContext, mode: 'app-server' | 'web', transport?: string) {
  const directory = mkdtempSync(join(tmpdir(), 'rustx-owner-test-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const userState = join(directory, 'user-state'); writeFileSync(userState, 'keep');
  const socketPath = join(directory, 'control.sock');
  const server = createServer(); server.listen(socketPath); await once(server, 'listening');
  t.after(() => server.close());
  const binary = join(directory, 'native.mjs');
  writeFileSync(binary, `#!/usr/bin/env node
import {connect,createServer} from 'node:net';
import {createInterface} from 'node:readline';
const control=connect(${JSON.stringify(socketPath)});
const listener=createServer();
const send=value=>control.write(JSON.stringify(value)+'\\n');
process.stdin.resume(); process.stdin.on('end',()=>send({eof:true}));
process.on('SIGTERM',()=>send({stopping:true}));
createInterface({input:control}).on('line',line=>{
  if(line==='ready') listener.listen(0,'127.0.0.1',()=>console.error('rustx app-server listening ws://127.0.0.1:'+listener.address().port));
  if(line==='release') { listener.close(); control.end(); process.stdin.pause(); }
});
control.on('connect',()=>send({pid:process.pid,argv:process.argv.slice(2)}));
`, { mode: 0o700 });
  const observer = join(directory, 'observe.mjs');
  writeFileSync(observer, `process.channel.unref();
process.on('SIGTERM',()=>queueMicrotask(()=>process.send({signal:'SIGTERM'})));
`);
  const connection = once(server, 'connection');
  const child = spawn(process.execPath, ['--import', observer, join(root, 'dev/src/main.ts'), mode, '--binary', binary,
    ...(mode === 'web' ? ['--no-open', '--workspace', directory] : transport ? ['--listen', transport, '--token-file', userState] : [])],
  { stdio: ['pipe', 'pipe', 'pipe', 'ipc'] });
  let errors = ''; child.stderr!.on('data', chunk => { errors += chunk; });
  const output = lines(child.stdout!);
  const closed = once(child, 'close');
  let exits = 0; child.on('exit', () => exits++);
  t.after(() => { if (child.exitCode === null) child.kill('SIGKILL'); output.close(); });
  const [socket] = await connection as [Socket];
  t.after(() => socket.destroy());
  const messages = lines(socket); t.after(messages.close);
  const owned = JSON.parse(await messages.next()) as { pid: number; argv: string[] };
  t.after(() => { try { process.kill(-owned.pid, 'SIGKILL'); } catch {} });
  return { child, closed, owned, messages, output, userState, socket, errors: () => errors, exits: () => exits };
}

test('stdio parent EOF requests explicit owned shutdown; competing signal cannot replace EOF', { timeout: 20_000 }, async t => {
  const f = await nativeFixture(t, 'app-server');
  assert.deepEqual(f.owned.argv, ['app-server', '--listen', 'stdio']);
  f.child.stdin!.end();
  let stopping = false;
  while (!stopping) stopping = JSON.parse(await f.messages.next()).stopping === true;
  process.kill(f.owned.pid, 0); // drain is still blocked, so settlement cannot finish
  const signal = once(f.child, 'message'); f.child.kill('SIGTERM'); await signal;
  f.socket.write('release\n');
  assert.deepEqual(await f.closed, [0, null], f.errors());
  assert.equal(f.exits(), 1);
  assert.throws(() => process.kill(-f.owned.pid, 0), { code: 'ESRCH' });
});

test('explicit WebSocket stdin EOF detaches without terminating the owner', { timeout: 20_000 }, async t => {
  const f = await nativeFixture(t, 'app-server', 'ws://127.0.0.1:8080');
  f.child.stdin!.end();
  assert.deepEqual(JSON.parse(await f.messages.next()), { eof: true });
  assert.equal(f.child.exitCode, null);
  const signal = once(f.child, 'message'); f.child.kill('SIGTERM'); await signal;
  assert.deepEqual(JSON.parse(await f.messages.next()), { stopping: true });
  f.socket.write('release\n');
  assert.deepEqual(await f.closed, [143, null], f.errors());
  assert.equal(f.exits(), 1);
});

for (const phase of ['startup', 'ready'] as const) test(`real main SIGHUP during Web ${phase} reaps group before scratch and preserves first cause`, { timeout: 30_000 }, async t => {
  const f = await nativeFixture(t, 'web');
  const scratch = dirname(f.owned.argv.at(-1)!);
  t.after(() => rmSync(scratch, { recursive: true, force: true }));
  if (phase === 'ready') {
    f.socket.write('ready\n');
    let line = '';
    while (!line.startsWith('[dev] rustX Web:')) line = await f.output.next();
    assert.ok(existsSync(join(scratch, 'host-config.json')));
  }
  f.child.kill('SIGHUP');
  let stopping = false;
  while (!stopping) stopping = JSON.parse(await f.messages.next()).stopping === true;
  assert.ok(existsSync(scratch)); process.kill(-f.owned.pid, 0);
  const signal = once(f.child, 'message'); f.child.kill('SIGTERM'); await signal;
  assert.ok(existsSync(scratch));
  if (phase === 'startup') assert.equal(existsSync(join(scratch, 'host-config.json')), false);
  f.socket.write('release\n');
  assert.deepEqual(await f.closed, [129, null], f.errors());
  assert.equal(f.exits(), 1);
  assert.throws(() => process.kill(-f.owned.pid, 0), { code: 'ESRCH' });
  assert.equal(existsSync(scratch), false);
  assert.equal(readFileSync(f.userState, 'utf8'), 'keep');
});

test('exact native help delegation has no invented transport and exits successfully', t => {
  const directory = mkdtempSync(join(tmpdir(), 'rustx-help-test-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const binary = join(directory, 'native.mjs');
  writeFileSync(binary, `#!/usr/bin/env node
const args=process.argv.slice(2);
console.log(JSON.stringify(args));
if(JSON.stringify(args)===JSON.stringify(['app-server','--help'])) console.error('native help');
else process.exitCode=2;
`, { mode: 0o700 });
  const result = spawnSync(process.execPath, [join(root, 'dev/src/main.ts'), 'app-server', '--', '--binary', binary, '--help'], { encoding: 'utf8', timeout: 15_000 });
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(JSON.parse(result.stdout), ['app-server', '--help']);
  assert.match(result.stderr, /native help/);
});

test('EOF consumed before ownership still settles and removes stdin listeners across repeated use', { timeout: 20_000 }, async t => {
  const directory = mkdtempSync(join(tmpdir(), 'rustx-ended-test-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const script = join(directory, 'ended.mjs');
  writeFileSync(script, `import {once} from 'node:events';
import {Launcher} from ${JSON.stringify(new URL('../src/launcher.ts', import.meta.url).href)};
import {spawnOwned} from ${JSON.stringify(new URL('../src/process.ts', import.meta.url).href)};
import {parseArguments} from ${JSON.stringify(new URL('../src/arguments.ts', import.meta.url).href)};
process.stdin.resume(); await once(process.stdin,'end');
const before=process.stdin.listenerCount('end');
const results=[];
for(let i=0;i<2;i++) {
  let pid;
  const launcher=new Launcher(${JSON.stringify(root)},(spec,exited,shutdown)=>{
    const child=spawnOwned({...spec,command:process.execPath,args:['-e',"require('node:net').createServer().listen(0)"]},exited,shutdown);
    pid=child.pid; return child;
  });
  await launcher.start(parseArguments(['app-server','--binary',process.execPath],${JSON.stringify(root)}));
  const code=await launcher.done;
  let gone=false; try{process.kill(-pid,0)}catch(error){gone=error.code==='ESRCH'}
  results.push({code,gone,listeners:process.stdin.listenerCount('end')-before});
}
console.log(JSON.stringify(results));`);
  const result = spawnSync(process.execPath, [script], { input: '', encoding: 'utf8', timeout: 15_000 });
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(JSON.parse(result.stdout), Array.from({ length: 2 }, () => ({ code: 0, gone: true, listeners: 0 })));
});

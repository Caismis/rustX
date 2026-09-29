/** The existing native host owns cancellation even before initialize completes. */
import { it } from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:net';
import { once } from 'node:events';
import { mkdtempSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { AppServerHost } from '../src/app-server/host.ts';
import { AppServerChild } from '../src/app-server/child-process.ts';

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(yes => { resolve = yes; });
  return { promise, resolve };
}

it('startup abort after initialize is received reaps the native child', { timeout: 15_000 }, async t => {
  const directory = mkdtempSync(join(tmpdir(), 'rustx-tui-startup-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const barrier = createServer();
  barrier.listen(0, '127.0.0.1'); await once(barrier, 'listening');
  t.after(() => barrier.close());
  const address = barrier.address(); assert.ok(address && typeof address !== 'string');
  const binary = join(directory, 'runtime');
  writeFileSync(binary, `#!/usr/bin/env node
import {connect} from 'node:net';
process.on('SIGTERM',()=>process.exit(0));
process.stdin.once('data',()=>{
 const socket=connect(${address.port},'127.0.0.1',()=>socket.end(String(process.pid)));
});
`, { mode: 0o700 });
  const controller = new AbortController();
  const connected = once(barrier, 'connection');
  const pending = AppServerHost.spawnLocal({ binary, launch: {}, signal: controller.signal });
  const rejected = assert.rejects(pending, /could not start the App Server/);
  const [socket] = await connected;
  let text = ''; socket.setEncoding('utf8'); socket.on('data', (chunk: string) => { text += chunk; });
  await once(socket, 'end');
  controller.abort();
  await rejected;
  assert.throws(() => process.kill(Number(text), 0), { code: 'ESRCH' });
});

it('already aborted startup refuses before creating a native child', async () => {
  const controller = new AbortController(); controller.abort();
  await assert.rejects(AppServerHost.spawnLocal({ binary: '/must-not-be-spawned', launch: {}, signal: controller.signal }), { name: 'AbortError' });
});

// The probe is scoped to this test: it records the grace the host forwards,
// holds the cleanup behind an explicit gate, then delegates to the real
// AppServerChild owner and records the ChildExit that owner observed.
it('initialization failure delegates its startup grace and settles only after the owned child is killed', { timeout: 15_000 }, async t => {
  const directory = mkdtempSync(join(tmpdir(), 'rustx-tui-startup-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const binary = join(directory, 'runtime');
  // Ends the handshake by closing its output, then ignores SIGTERM and stdin EOF.
  writeFileSync(binary, `#!/usr/bin/env node
import {closeSync} from 'node:fs';
process.on('SIGTERM',()=>{});
process.stdin.resume();
setInterval(()=>{},1000);
closeSync(1);
`, { mode: 0o700 });
  const graceMs = 37;
  const events: string[] = [];
  const release = deferred<void>();
  const entered = deferred<{ child: AppServerChild; graceMs: number | undefined }>();
  const waitOrTerminate = AppServerChild.prototype.waitOrTerminate;
  const probe = t.mock.method(AppServerChild.prototype, 'waitOrTerminate', function (this: AppServerChild, grace?: number) {
    entered.resolve({ child: this, graceMs: grace });
    return release.promise
      .then(() => waitOrTerminate.call(this, grace))
      .then(exit => { events.push('cleanup settled'); return exit; });
  });
  let owned: AppServerChild | undefined;
  // Failure-safe: whatever assertion fails, the gate opens and the child is
  // killed and reaped by its own process owner before the test ends.
  t.after(async () => {
    release.resolve();
    if (owned !== undefined && owned.exited === undefined) {
      process.kill(owned.pid!, 'SIGKILL');
      await owned.wait();
    }
  });
  const rejected = assert.rejects(
    AppServerHost.spawnLocal({ binary, launch: {}, terminationGraceMs: graceMs }),
    /could not start the App Server/,
  ).then(() => { events.push('startup rejected'); });

  const call = await entered.promise;
  owned = call.child;
  assert.equal(call.graceMs, graceMs, 'the configured grace reaches the child owner');
  assert.equal(probe.mock.callCount(), 1);
  assert.deepEqual(events, [], 'startup cannot settle while its owned-child cleanup is pending');
  release.resolve();
  await rejected;
  assert.deepEqual(events, ['cleanup settled', 'startup rejected']);
  const exit = await probe.mock.calls[0]!.result!;
  assert.deepEqual(exit, { code: null, signal: 'SIGKILL' }, 'only SIGKILL ends a child that ignores SIGTERM');
  assert.equal(owned.exited, exit);
});

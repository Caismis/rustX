/** The existing native host owns cancellation even before initialize completes. */
import { it } from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:net';
import { once } from 'node:events';
import { mkdtempSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { AppServerHost } from '../src/app-server/host.ts';

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

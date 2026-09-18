import { test } from 'node:test';
import assert from 'node:assert/strict';
import { browserEnvironment, handoff, openBrowser } from '../src/browser.ts';
import { browserHandoff } from '../src/browser-handoff.ts';
import { ChildProcess, type spawn } from 'node:child_process';
import { parseArguments } from '../src/arguments.ts';

test('Web boolean option consumes no native value and is never forwarded', () => {
  const parsed = parseArguments(['web', '--no-open', '--model', 'native', '--workspace', '/workspace'], '/root');
  assert.equal(parsed.noOpen, true); assert.deepEqual(parsed.forwarded, ['--model', 'native']);
  assert.deepEqual(parseArguments(['web', '--model', '--no-open', '--workspace', '/workspace'], '/root').forwarded, ['--model', '--no-open']);
  assert.equal(parseArguments(['web', '--workspace', '/workspace'], '/root').noOpen, false);
});
test('handoff prints once before opening, --no-open suppresses opening, failure is bounded and secret-free', async () => {
  const events: string[] = [], startup = 'http://127.0.0.1:1234/?token=launch';
  const open = async (url: string) => { events.push(`open:${url}`); throw Error('untrusted error containing transport secret'); };
  await handoff(startup, false, open, value => events.push(value), value => events.push(value));
  assert.deepEqual(events, [`[dev] rustX Web: ${startup}`, `open:${startup}`, '[dev] Could not open the browser. Open the startup URL printed above.']);
  events.length = 0;
  await handoff(startup, true, open, value => events.push(value));
  assert.deepEqual(events, [`[dev] rustX Web: ${startup}`]);
});
test('browser receives only explicit desktop environment, never credential or runtime injection variables', () => {
  assert.deepEqual(browserEnvironment({ PATH: '/bin', HOME: '/user', DISPLAY: ':0', OPENAI_API_KEY: 'secret', RUSTX_TOKEN: 'secret', MCP_SECRET: 'secret', NODE_OPTIONS: '--require malicious', LD_PRELOAD: 'malicious', BROWSER: 'malicious' }), { PATH: '/bin', HOME: '/user', DISPLAY: ':0' });
});
test('non-Windows OS acceptance settles without awaiting a never-exiting launcher', async () => {
  for (const platform of ['linux', 'darwin'] as const) {
    const launcher = new ChildProcess();
    launcher.ref = () => { throw Error('must not ref browser lifetime'); };
    await browserHandoff('http://localhost/', platform, async () => launcher);
    assert.equal(launcher.listenerCount('close'), 0);
  }
});
test('Windows waits for only the short-lived launcher and rejects nonzero exit', async () => {
  for (const code of [0, 1]) {
    const launcher = new ChildProcess(); let referenced = false, settled = false;
    launcher.ref = () => { referenced = true; };
    const handoff = browserHandoff('http://localhost/', 'win32', async () => launcher).then(() => { settled = true; });
    await Promise.resolve();
    assert.equal(referenced, true); assert.equal(settled, false);
    launcher.emit('close', code);
    if (code === 0) await handoff; else await assert.rejects(handoff, /launcher failed/);
    assert.equal(launcher.listenerCount('error'), 0);
  }
});
test('cancellation kills and reaps only the rustX helper, never an OS browser process', async () => {
  const helper = new ChildProcess(), abort = new AbortController(); const killed: unknown[] = [];
  helper.kill = signal => { killed.push(signal); return true; };
  const spawnHelper = ((file, args, options) => {
    assert.equal(file, process.execPath); assert.match(args![0], /browser-worker\.ts$/);
    assert.equal(options!.stdio, 'ignore'); return helper;
  }) as typeof spawn;
  const work = openBrowser('http://localhost/', abort.signal, spawnHelper);
  abort.abort(); assert.deepEqual(killed, ['SIGKILL']);
  helper.emit('close', null);
  await assert.rejects(work, /handoff failed/);
});

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { browserEnvironment, handoff } from '../src/browser.ts';
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

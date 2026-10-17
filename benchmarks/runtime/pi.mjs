// Pinned Pi Durable SQLite comparison. All provider responses use Pi's local faux provider.
// node --conditions=source benchmarks/runtime/pi.mjs <pi checkout> <case> <output.json>
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtemp, readFile, readdir, stat, rm, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { performance } from 'node:perf_hooks';
const [checkoutArg, scenario, output] = process.argv.slice(2);
const checkout = resolve(checkoutArg);
const revision = execFileSync('git', ['-C', checkout, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim();
assert.equal(revision, '90d863f7c1183b965f099855715d6e88c7b8a52c');
const load = (path) => import(pathToFileURL(join(checkout, path)).href);
const { chatSetup, openChat, allEntries, textOf } = await load('packages/durable/test/chat-support.ts');
const { context } = await load('packages/durable/test/session-support.ts');
const { fauxAssistantMessage } = await load('packages/ai/src/providers/faux.ts');
const { openNodeSqliteStorage } = await load('packages/durable/src/storage/sqlite/node.ts');
assert(['short-small', 'short-large', 'large', 'stream'].includes(scenario));
const history = scenario === 'short-small' ? 8 : scenario === 'short-large' ? 64 : 0;
const measured = history ? 32 : 1;
const setup = chatSetup(scenario === 'stream' ? { tokensPerSecond: 1200, tokenSize: { min: 12, max: 12 } } : { tokensPerSecond: 1e9 });
let requests = 0;
const reply = (i) => scenario === 'large' ? 'L'.repeat(32768) : scenario === 'stream' ? Array.from({ length: 128 }, (_, n) => `stream-delta-${String(n).padStart(4, '0')} `).join('') : `reply ${String(i).padStart(4, '0')}`;
setup.faux.setResponses(Array.from({ length: history + measured }, (_, i) => (request) => {
  assert.equal(requests++, i);
  assert(JSON.stringify(request.messages).includes(`benchmark turn ${String(i).padStart(4, '0')}`));
  if (i) assert(JSON.stringify(request.messages).includes(reply(i - 1)));
  return fauxAssistantMessage(reply(i));
}));
const rootDir = await mkdtemp(join(dirname(resolve(output)), 'pi-'));
const path = join(rootDir, 'conversation.db');
const footprint = async () => Promise.all((await readdir(rootDir)).map(async (name) => {
  const info = await stat(join(rootDir, name));
  return { name, logical: info.size, allocated: info.blocks * 512 };
}));
const began = performance.now();
const storage = await openNodeSqliteStorage(path);
const { harness, root } = await openChat(storage, setup);
harness.resume();
const open_ms = performance.now() - began;
const initial_cpu = process.cpuUsage();
const initial_memory = process.memoryUsage();
const samples = [];
for (let i = 0; i < history + measured; i++) {
  const start = performance.now();
  const submission = await root.submit({ type: 'input', content: `benchmark turn ${String(i).padStart(4, '0')}` }, context);
  const accepted_ms = performance.now() - start;
  const settled = await submission.wait(context);
  const settled_ms = performance.now() - start;
  assert.equal(settled.status, 'done');
  samples.push({ turn: i, measured: i >= history, accepted_ms, settled_ms,
    cpu: process.cpuUsage(), memory: process.memoryUsage(), footprint: await footprint() });
}
assert.equal(requests, history + measured);
assert.equal(setup.faux.getPendingResponseCount(), 0);
const entries = await allEntries(root);
assert.deepEqual(entries.filter(e => e.kind === 'pi.assistant').map(e => textOf(e.model?.[0])),
  Array.from({ length: history + measured }, (_, i) => reply(i)));
assert.deepEqual(setup.reports, []);
const before_close = await footprint();
const close = performance.now();
await harness.close(context);
const close_ms = performance.now() - close;
const closed_footprint = await footprint();
const reopenedStorage = await openNodeSqliteStorage(path);
const reopened = await openChat(reopenedStorage, setup);
assert.deepEqual(await allEntries(reopened.root), entries);
await reopened.harness.close(context);
const cleanup = performance.now();
await rm(rootDir, { recursive: true });
const cleanup_ms = performance.now() - cleanup;
await writeFile(output, JSON.stringify({ revision, scenario, node: process.version, open_ms, history, measured,
  initial_cpu, initial_memory, samples, requests, before_close, closed_footprint, close_ms, cleanup_ms,
  oracle: 'passed', provider: 'local faux (no HTTP)', allocations: null }, null, 2));

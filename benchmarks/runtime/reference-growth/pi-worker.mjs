// Real Pi Durable Harness, production OpenAI HTTP/SSE adapter, file-backed SQLite.
import assert from 'node:assert/strict';
import { createInterface } from 'node:readline';
import { pathToFileURL } from 'node:url';
import { join } from 'node:path';
import { performance } from 'node:perf_hooks';

const load = relative => import(pathToFileURL(join(process.env.REFERENCE_PI_CHECKOUT, relative)).href);
const { createModels, createProvider } = await load('packages/ai/src/models.ts');
const { openAICompletionsApi } = await load('packages/ai/src/api/openai-completions.lazy.ts');
const { Harness, createRegistry, watchEvents } = await load('packages/durable/src/index.ts');
const { CodingTools } = await load('packages/durable/src/tools/index.ts');
const { NodeExecutionEnv } = await load('packages/durable/src/env/node.ts');
const { BACKGROUND_CONTEXT: context } = await load('packages/chord/src/context/index.ts');
const { openNodeSqliteStorage } = await load('packages/durable/src/storage/sqlite/node.ts');
const [rootDir, baseUrl, scenario] = process.argv.slice(2);
const workspace = join(rootDir, '..', 'workspace');
const emit = value => process.stdout.write(JSON.stringify(value) + '\n');
const models = createModels();
models.setProvider(createProvider({
  id: 'fixture', name: 'Deterministic local provider', baseUrl: baseUrl + '/v1',
  auth: { apiKey: { name: 'Fixture', resolve: async () => ({ auth: { apiKey: 'fixture' } }) } },
  models: [{ id: 'turn-growth', name: 'turn-growth', api: 'openai-completions', provider: 'fixture',
    baseUrl: baseUrl + '/v1', reasoning: false, input: ['text'],
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 }, contextWindow: 1000000, maxTokens: 4096,
    compat: { supportsStore: false, maxTokensField: 'max_completion_tokens' } }],
  api: openAICompletionsApi(),
}));
const registry = createRegistry();
registry.install(CodingTools);
const reports = [];
emit({ type: 'before_open', node: process.version, sqlite: process.versions.sqlite });
const openStart = performance.now();
const harness = await Harness.open(await openNodeSqliteStorage(join(rootDir, 'conversation.db')),
  { models, registry, settings: { stream: { maxRetries: 0, cacheRetention: 'none' } },
    env: () => new NodeExecutionEnv({ cwd: workspace }),
    onReport: error => reports.push(String(error)) }, context);
const root = await harness.root(context, { agent: { model: { provider: 'fixture', modelId: 'turn-growth' },
  cwd: workspace, thinkingLevel: 'off' } });
// Consume the actual committed AgentEvent surface, including streamed partials.
const watch = await watchEvents(harness, root.id, context);
const eventCounts = {};
const completed = new Set();
const waiters = new Map();
let lastAnswer, toolResults = 0;
const textOf = message => typeof message?.content === 'string' ? message.content
  : (message?.content ?? []).filter(block => block.type === 'text').map(block => block.text).join('');
watch.start(async events => {
  for (const event of events) {
    eventCounts[event.type] = (eventCounts[event.type] ?? 0) + 1;
    if (event.type === 'message_end') {
      for (const message of event.entry.model ?? []) {
        if (message.role === 'assistant' && textOf(message)) lastAnswer = textOf(message);
        if (message.role === 'toolResult') {
          assert.equal(textOf(message), 'fixed tool result\n');
          assert(!message.isError);
          toolResults++;
        }
      }
    }
    if (event.type === 'run_end') for (const id of event.inputs) {
      const waiter = waiters.get(id);
      if (waiter) { waiters.delete(id); waiter(); }
      else completed.add(id);
    }
  }
});
harness.resume();
emit({ type: 'ready', open_and_subscribe_ms: performance.now() - openStart,
  snapshot_entries: watch.snapshot.entries.length, root: root.id });
for await (const line of createInterface({ input: process.stdin })) {
  const request = JSON.parse(line);
  if (request.type === 'prompt') {
    lastAnswer = undefined;
    const beforeTools = toolResults;
    const start = performance.now();
    const submission = await root.submit({ type: 'input', content: request.message }, context);
    const accepted_ms = performance.now() - start;
    const settled = await submission.wait(context);
    assert.equal(settled.status, 'done', JSON.stringify(settled));
    if (!completed.delete(submission.id)) await new Promise(resolve => waiters.set(submission.id, resolve));
    assert.equal(lastAnswer, 'ok');
    assert.equal(toolResults - beforeTools, scenario === 'tool' ? 1 : 0);
    assert.deepEqual(reports, []);
    emit({ type: 'terminal', id: request.id, outcome: 'completed', accepted_ms,
      tool_results: toolResults - beforeTools });
  } else if (request.type === 'verify') {
    const entries = [];
    let cursor;
    do {
      const page = await root.entries({}, 256, cursor, context);
      entries.push(...page.items);
      cursor = page.next;
    } while (cursor !== undefined);
    const messages = entries.flatMap(entry => entry.model ?? []);
    const counts = {};
    for (const message of messages) counts[message.role] = (counts[message.role] ?? 0) + 1;
    const turns = request.turns;
    assert.equal(counts.user, turns);
    assert.equal(counts.assistant, turns * (scenario === 'tool' ? 2 : 1));
    assert.equal(counts.toolResult ?? 0, scenario === 'tool' ? turns : 0);
    assert.equal(messages.filter(m => m.role === 'assistant' && textOf(m) === 'ok').length, turns);
    emit({ type: 'verified', id: request.id, counts, entries: entries.length, event_counts: eventCounts });
  } else if (request.type === 'close') {
    await watch.stop();
    await harness.close(context);
    emit({ type: 'closed', id: request.id });
    break;
  } else throw new Error('Unknown command: ' + request.type);
}

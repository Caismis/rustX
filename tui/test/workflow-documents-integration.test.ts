/** Complete documents cross a real stdio process while control records stay small. */
import assert from 'node:assert/strict';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { join } from 'node:path';
import { test } from 'node:test';
import { createHash } from 'node:crypto';
import { AppServerHost } from '../src/app-server/host.ts';
import { AppServerRequestError } from '../src/app-server/client.ts';
import { JSONL_MAX_RECORD_BYTES } from '../src/protocol/jsonl.ts';
import { checkWorkflowDocument, readWorkflowSource, withWorkflowDocument, type WorkflowRequest } from '../../protocol/app-server/workflow-documents.ts';
import type { WorkflowFileDocument } from '../../protocol/app-server/v82.ts';
import { TempFixture } from './support/temp-fixture.ts';

const binary = process.env.RUSTX_BINARY ?? fileURLToPath(new URL('../../target/debug/rustx', import.meta.url));
const skip = existsSync(binary) ? undefined : `Build native binaries at ${binary} before running this integration test`;
if (skip && process.env.RUSTX_REQUIRE_PROVIDER_EMULATOR) throw new Error(skip);

async function largeDocumentFixture(t: import('node:test').TestContext) {
  const fixture = TempFixture.create('rustx-workflow-documents-');
  const home = fixture.path('home');
  const workflows = join(home, 'rustx/.agents/workflows');
  mkdirSync(workflows, { recursive: true });
  const config = fixture.path('rustx.toml');
  writeFileSync(config, '');
  const target = { kind: 'user' as const };
  const code = "def main(): return {'result': 1}\n" + '# native document carrier\n'.repeat(120_000);
  const authored: WorkflowFileDocument = {
    description: 'Large source · 中文 🦀',
    block: { input: { type: 'object', properties: {}, additionalProperties: false },
      nodes: { start: { type: 'start' }, code: { type: 'code', code_language: 'python3', variables: {}, code, outputs: { result: 'number' }, on_error: { type: 'stop' } }, done: { type: 'return', outputs: [] } },
      edges: [{ from: 'start', to: 'code' }, { from: 'code', to: 'done' }] },
  };
  const file = join(workflows, 'large.yaml');
  writeFileSync(file, JSON.stringify(authored));
  assert.ok(readFileSync(file).length > 3 * 1024 * 1024);
  let host: AppServerHost | undefined;
  const started = Date.now();
  let stage = 'bootstrap', sequence = 0, completed = 0, largestRequest = 0;
  const pending = new Map<number, { method: string; started: number }>();
  const mark = (name: string) => { stage = name; };
  const diagnostic = () => t.diagnostic(JSON.stringify({ stage, elapsedMs: Date.now() - started,
    requests: sequence, completed,
    pending: [...pending.values()].map(value => ({ method: value.method, ageMs: Date.now() - value.started })) }));
  t.signal.addEventListener('abort', diagnostic, { once: true });
  t.after(async () => {
    t.signal.removeEventListener('abort', diagnostic);
    diagnostic();
    try { await host?.shutdown(); } finally { fixture.cleanup(); }
  });
  host = await AppServerHost.spawnLocal({ binary, launch: { config, runtimeRoot: fixture.path('runtime') }, env: { ...process.env, HOME: home }, cwd: fixture.root });
  const client = host.client;
  const request: WorkflowRequest = async (operation, expected) => {
    largestRequest = Math.max(largestRequest, Buffer.byteLength(JSON.stringify(operation)));
    const id = ++sequence;
    pending.set(id, { method: operation.method, started: Date.now() });
    try { return await client.call(operation.method, operation.params, expected); }
    finally { completed += 1; pending.delete(id); }
  };
  const assertBounded = () => {
    assert.equal(JSONL_MAX_RECORD_BYTES, 1_048_576);
    assert.ok(largestRequest < JSONL_MAX_RECORD_BYTES);
  };
  mark('initial-read');
  const source = await readWorkflowSource(client, request, target, 'large');
  assert.equal(source.document?.block.nodes.code?.type, 'code');
  const initial = source.document!;
  assert.deepEqual(initial.block.nodes.code, authored.block.nodes.code);
  assert.equal(typeof initial.timeout_ms, 'string', 'content references preserve the generated exact-u64 wire domain');
  return { client, request, target, file, source, initial, mark, assertBounded };
}

test('large Workflow read/check/save/CAS over bounded real stdio records', { skip, timeout: 60_000 }, async t => {
  const { client, request, target, file, source, initial, mark, assertBounded } = await largeDocumentFixture(t);
  const changed = { ...initial, description: 'Saved large source · 中文 🦀' };
  mark('check-three-MiB');
  const report = await checkWorkflowDocument(client, request, target, 'large', changed);
  assert.equal(report.structure.status, 'valid');
  mark('save-three-MiB');
  const { revision } = await withWorkflowDocument(client, request, target, changed, document => request({ method: 'configuration/sourceWrite', params: { target, expected_revision: source.revision, mutation: { kind: 'workflow', id: 'large', edit: { operation: 'replace', document } } } }, 'sources_written'));
  assert.equal(revision, report.revision);
  mark('reread-saved');
  assert.deepEqual((await readWorkflowSource(client, request, target, 'large')).document, changed);
  const committed = readFileSync(file);
  mark('stale-CAS');
  await assert.rejects(withWorkflowDocument(client, request, target, initial, document => request({ method: 'configuration/sourceWrite', params: { target, expected_revision: source.revision, mutation: { kind: 'workflow', id: 'large', edit: { operation: 'replace', document } } } }, 'sources_written')), error => error instanceof AppServerRequestError && error.error.data?.kind === 'source_conflict');
  assert.deepEqual(readFileSync(file), committed);
  assertBounded();
  mark('done');
});

test('large Workflow scratch admission, failed callbacks and incomplete documents stay isolated', { skip, timeout: 60_000 }, async t => {
  const { client, request, target, file, source, initial, mark, assertBounded } = await largeDocumentFixture(t);
  const candidate = structuredClone(initial);
  if (candidate.block.nodes.code?.type !== 'code') throw new Error('Code fixture');
  candidate.block.nodes.code.code = "def main(): return {'result': 1}";
  // Distinct wrappers for one connection share all four scratch permits.
  const alternate: WorkflowRequest = (operation, expected) => request(operation, expected);
  mark('eight-checks-and-read');
  const [concurrent, overlappingRead] = await Promise.all([Promise.all(Array.from({ length: 8 }, (_, index) =>
    checkWorkflowDocument(client, index % 2 ? request : alternate, target, 'large',
      { ...candidate, description: 'Concurrent candidate ' + index }))),
    readWorkflowSource(client, alternate, target, 'large')]);
  assert.ok(concurrent.every(report => report.structure.status === 'valid'));
  assert.deepEqual(overlappingRead.document, initial);
  mark('failed-operation-release');
  await assert.rejects(withWorkflowDocument(client, request, target, candidate, async () => { throw new Error('candidate failed'); }), /candidate failed/);
  assert.deepEqual((await readWorkflowSource(client, alternate, target, 'large')).document, initial, 'failed operation releases admission and cannot mutate source');
  mark('incomplete-document');
  const committed = readFileSync(file);
  const bytes = Buffer.from(JSON.stringify({ ...initial, description: 'Incomplete replacement' }));
  const { reference } = await request({ method: 'workflow/documentBegin', params: { target, byte_length: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex') } }, 'workflow_document');
  try {
    await request({ method: 'workflow/documentWrite', params: { target, reference, offset: 0, data: bytes.subarray(0, 65_536).toString('base64') } }, 'workflow_document_accepted');
    await assert.rejects(request({ method: 'configuration/sourceWrite', params: { target, expected_revision: source.revision, mutation: { kind: 'workflow', id: 'large', edit: { operation: 'replace', document: reference } } } }, 'sources_written'));
    assert.deepEqual(readFileSync(file), committed, 'incomplete document chunks are never partially published');
  } finally { await request({ method: 'workflow/documentRelease', params: { target, reference } }, 'workflow_document_accepted'); }
  assertBounded();
  mark('done');
});

test('large Workflow source accepts exactly sixteen MiB and rejects one byte more', { skip, timeout: 60_000 }, async t => {
  const { client, request, target, file, initial, mark, assertBounded } = await largeDocumentFixture(t);
  mark('read-exact-sixteen-MiB');
  const prefix = Buffer.concat([readFileSync(file), Buffer.from('\n#')]);
  writeFileSync(file, Buffer.concat([prefix, Buffer.alloc(16_777_216 - prefix.length, 'x')]));
  assert.equal(readFileSync(file).length, 16_777_216);
  assert.deepEqual((await readWorkflowSource(client, request, target, 'large')).document, initial, 'the exact 16 MiB source can be read through the same client path');
  mark('reject-over-sixteen-MiB');
  writeFileSync(file, Buffer.concat([readFileSync(file), Buffer.from('x')]));
  await assert.rejects(readWorkflowSource(client, request, target, 'large'), error => error instanceof AppServerRequestError && error.error.data?.kind === 'workflow_invalid' && error.error.data.diagnostic.category === 'source_size');
  assertBounded();
  mark('done');
});

test('complete oversized Tool schemas and Explain reports cross bounded stdio records', { skip, timeout: 60_000 }, async () => {
  const fixture = TempFixture.create('rustx-workflow-reports-');
  const home = fixture.path('home'); mkdirSync(home, { recursive: true });
  const config = fixture.path('rustx.toml'); writeFileSync(config, '');
  const target = { kind: 'user' as const };
  const schema = { type: 'object', properties: Object.fromEntries(Array.from({ length: 300 }, (_, index) => [`field_${index}`, { type: 'string', description: 'd'.repeat(4096) }])), additionalProperties: false };
  const document: WorkflowFileDocument = { description: 'Complete large Tool contract', block: {
    input: { type: 'object', properties: {}, additionalProperties: false },
    nodes: { start: { type: 'start' }, tool: { type: 'tool', selector: { origin: 'builtin', name: 'read' }, arguments: { type: 'object', fields: {} }, result: { type: 'json', schema } }, done: { type: 'return', outputs: [] } },
    edges: [{ from: 'start', to: 'tool' }, { from: 'tool', to: 'done' }],
  } };
  assert.ok(Buffer.byteLength(JSON.stringify(schema)) > JSONL_MAX_RECORD_BYTES);
  let host: AppServerHost | undefined;
  try {
    host = await AppServerHost.spawnLocal({ binary, launch: { config, runtimeRoot: fixture.path('runtime') }, env: { ...process.env, HOME: home }, cwd: fixture.root });
    let largest = 0;
    const request: WorkflowRequest = async (operation, expected) => {
      largest = Math.max(largest, Buffer.byteLength(JSON.stringify(operation)) + 1024);
      const result = await host!.client.call(operation.method, operation.params, expected);
      largest = Math.max(largest, Buffer.byteLength(JSON.stringify(result)) + 1024);
      return result;
    };
    for (const explain of [false, true]) {
      const report = await checkWorkflowDocument(host.client, request, target, 'large', document, explain);
      assert.equal(report.structure.status, 'valid');
      if (report.structure.status !== 'valid') throw new Error('valid compiled structure');
      assert.deepEqual(report.structure.outputs['block.nodes.tool'], schema);
      assert.ok(Buffer.byteLength(JSON.stringify(report)) > JSONL_MAX_RECORD_BYTES);
      assert.equal(!!report.explanation, explain);
      if (report.explanation) assert.equal(typeof report.explanation.configured_timeout_ms, 'string', 'report content uses the ordinary exact-u64 wire codec');
    }
    assert.ok(largest < JSONL_MAX_RECORD_BYTES, `largest envelope allowance: ${largest}`);
  } finally { await host?.shutdown(); fixture.cleanup(); }
});

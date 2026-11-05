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
import { readWorkflowSource, withWorkflowDocument, type WorkflowRequest } from '../../protocol/app-server/workflow-documents.ts';
import type { WorkflowFileDocument } from '../../protocol/app-server/v71.ts';
import { TempFixture } from './support/temp-fixture.ts';

const binary = process.env.RUSTX_BINARY ?? fileURLToPath(new URL('../../target/debug/rustx', import.meta.url));
const skip = existsSync(binary) ? undefined : `Build native binaries at ${binary} before running this integration test`;
if (skip && process.env.RUSTX_REQUIRE_PROVIDER_EMULATOR) throw new Error(skip);

test('large Workflow read/check/save/CAS and incomplete scratch over real stdio', { skip, timeout: 60_000 }, async () => {
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
  try {
    host = await AppServerHost.spawnLocal({ binary, launch: { config, runtimeRoot: fixture.path('runtime') }, env: { ...process.env, HOME: home }, cwd: fixture.root });
    let largestRequest = 0;
    const request: WorkflowRequest = (operation, expected) => {
      largestRequest = Math.max(largestRequest, Buffer.byteLength(JSON.stringify(operation)));
      return host!.client.call(operation.method, operation.params, expected);
    };
    const source = await readWorkflowSource(host!.client, request, target, 'large');
    assert.equal(source.document?.block.nodes.code?.type, 'code');
    const initial = source.document!;
    assert.deepEqual(initial.block.nodes.code, authored.block.nodes.code);
    assert.equal(typeof initial.timeout_ms, 'string', 'content references preserve the generated exact-u64 wire domain');
    const changed = { ...initial, description: 'Saved large source · 中文 🦀' };
    // Distinct wrappers for one connection must share admission. Five uploads
    // used to overflow its four-document scratch even though each RPC fit.
    const alternate: WorkflowRequest = (operation, expected) => request(operation, expected);
    const candidate = structuredClone(changed);
    if (candidate.block.nodes.code?.type !== 'code') throw new Error('Code fixture');
    candidate.block.nodes.code.code = "def main(): return {'result': 1}";
    const [concurrent, overlappingRead] = await Promise.all([Promise.all(Array.from({ length: 8 }, (_, index) =>
      withWorkflowDocument(host!.client, index % 2 ? request : alternate, target,
        { ...candidate, description: `Concurrent candidate ${index}` },
        document => request({ method: 'workflow/check', params: { target, id: 'large', document, explain: false } }, 'workflow_check')))),
      readWorkflowSource(host!.client, alternate, target, 'large')]);
    assert.ok(concurrent.every(result => result.report.structure.status === 'valid'));
    assert.deepEqual(overlappingRead.document, initial);
    await assert.rejects(withWorkflowDocument(host!.client, request, target, candidate, async () => { throw new Error('candidate failed'); }), /candidate failed/);
    assert.deepEqual((await readWorkflowSource(host!.client, alternate, target, 'large')).document, initial, 'failed operation releases admission and cannot mutate source');
    const { report } = await withWorkflowDocument(host!.client, request, target, changed, document => request({ method: 'workflow/check', params: { target, id: 'large', document, explain: false } }, 'workflow_check'));
    assert.equal(report.structure.status, 'valid');
    const { revision } = await withWorkflowDocument(host!.client, request, target, changed, document => request({ method: 'configuration/sourceWrite', params: { target, expected_revision: source.revision, mutation: { kind: 'workflow', id: 'large', edit: { operation: 'replace', document } } } }, 'sources_written'));
    assert.equal(revision, report.revision);
    const reread = await readWorkflowSource(host!.client, request, target, 'large');
    assert.deepEqual(reread.document, changed);
    const committed = readFileSync(file);
    await assert.rejects(withWorkflowDocument(host!.client, request, target, initial, document => request({ method: 'configuration/sourceWrite', params: { target, expected_revision: source.revision, mutation: { kind: 'workflow', id: 'large', edit: { operation: 'replace', document } } } }, 'sources_written')), error => error instanceof AppServerRequestError && error.error.data?.kind === 'source_conflict');
    assert.deepEqual(readFileSync(file), committed);

    const bytes = Buffer.from(JSON.stringify(changed));
    const { reference } = await request({ method: 'workflow/documentBegin', params: { target, byte_length: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex') } }, 'workflow_document');
    try {
      await request({ method: 'workflow/documentWrite', params: { target, reference, offset: 0, data: bytes.subarray(0, 65_536).toString('base64') } }, 'workflow_document_accepted');
      await assert.rejects(request({ method: 'configuration/sourceWrite', params: { target, expected_revision: revision, mutation: { kind: 'workflow', id: 'large', edit: { operation: 'replace', document: reference } } } }, 'sources_written'));
      assert.deepEqual(readFileSync(file), committed, 'incomplete document chunks are never partially published');
    } finally { await request({ method: 'workflow/documentRelease', params: { target, reference } }, 'workflow_document_accepted'); }

    const prefix = Buffer.concat([committed, Buffer.from('\n#')]);
    writeFileSync(file, Buffer.concat([prefix, Buffer.alloc(16_777_216 - prefix.length, 'x')]));
    assert.deepEqual((await readWorkflowSource(host!.client, request, target, 'large')).document, changed, 'the exact 16 MiB source can be read through the same client path');
    writeFileSync(file, Buffer.concat([readFileSync(file), Buffer.from('x')]));
    await assert.rejects(readWorkflowSource(host!.client, request, target, 'large'), error => error instanceof AppServerRequestError && error.error.data?.kind === 'workflow_invalid' && error.error.data.diagnostic.category === 'source_size');
    assert.equal(JSONL_MAX_RECORD_BYTES, 1_048_576);
    assert.ok(largestRequest < JSONL_MAX_RECORD_BYTES);
  } finally { await host?.shutdown(); fixture.cleanup(); }
});

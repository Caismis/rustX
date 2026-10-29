import { duplicateFragment } from '../src/app/workflows/document/clipboard';
import { expect, it } from 'vitest';
import type { WorkflowAnalysisQuery, WorkflowAnalysisReport, WorkflowEditorMetadata } from '../../protocol/app-server/v62';
import { WorkflowAnalysisSession, type AnalysisInput } from '../src/app/workflows/document/analysis';
import { CommandRejected, renameNode } from '../src/app/workflows/document/commands';
import { renameNodePresentation } from '../src/app/workflows/document/presentation';
import { createActor, waitFor } from 'xstate';
import { workflowEditorMachine } from '../src/app/workflows/machines/editor';
import type { Block, BlockPath, WorkflowDocument } from '../src/app/workflows/document/model';
import type { WorkflowScopePort } from '../src/app/workflows/port';
import { exampleSource } from './workflow-fixtures';

function deferred<T>() { let resolve!: (value: T) => void, reject!: (error: unknown) => void; const promise = new Promise<T>((accept, refuse) => { resolve = accept; reject = refuse; }); return { promise, resolve, reject }; }
const empty = { outputs: [], references: [], selected: null, diagnostic: null, incomplete: false, truncated: false };
function scripted() {
  const pending: { id: string; generation: string; query?: WorkflowAnalysisQuery; result: ReturnType<typeof deferred<WorkflowAnalysisReport>> }[] = [];
  const port: WorkflowScopePort = {
    endpoint: 'ws://fixture/', scope: { kind: 'user' }, list: () => new Promise(() => {}), read: () => new Promise(() => {}), check: () => new Promise(() => {}), write: () => new Promise(() => {}), modelChoices: () => new Promise(() => {}),
    analyze: (id, _document, generation, query) => { const result = deferred<WorkflowAnalysisReport>(); pending.push({ id, generation, query, result }); return result.promise; },
  };
  const complete = (index: number, override: Partial<WorkflowAnalysisReport> = {}) => {
    const request = pending[index];
    request.result.resolve({ target: { kind: 'user' }, id: request.id, generation: request.generation, query: request.query ?? null, candidate_revision: `sha256:${'a'.repeat(64)}`, analysis: empty, presentation: [], ...override });
  };
  const input: AnalysisInput = { port, target: { endpoint: port.endpoint, scope: port.scope, id: 'flow' }, document: exampleSource('typed_agent').document!, generation: 7 };
  return { pending, complete, input };
}

it('rejects an older completion after a newer candidate and consumer query complete', async () => {
  const source = scripted(), session = new WorkflowAnalysisSession();
  let current: AnalysisInput = source.input;
  const older = session.request(current, () => current);
  current = { ...current, generation: 8, document: { ...current.document, description: 'New candidate' } };
  const newer = session.request(current, () => current);
  source.complete(1); expect(await newer).toMatchObject({ generation: '8' });
  source.complete(0); expect(await older).toBeUndefined();
});

it('ignores a canceled request rejection after the same query reopens and succeeds', async () => {
  const source = scripted(), session = new WorkflowAnalysisSession();
  const old = session.request(source.input, () => source.input);
  session.cancel();
  const reopened = session.request(source.input, () => source.input);
  source.complete(1); expect(await reopened).toMatchObject({ generation: '7' });
  source.pending[0].result.reject(new Error('old request failed'));
  expect(await old).toBeUndefined();
  const active = session.request(source.input, () => source.input);
  source.pending[2].result.reject(new Error('current failure'));
  await expect(active).rejects.toThrow('current failure');
});

it('fences target, resource authority, consumer field, search and cancellation independently', async () => {
  for (const change of ['target', 'port', 'document', 'consumer', 'search', 'cancel'] as const) {
    const source = scripted(), session = new WorkflowAnalysisSession();
    const consumer = { address: { block: [], node: 'done' }, field: { kind: 'return_output' as const }, path: [] };
    let current: AnalysisInput = { ...source.input, query: { consumer, search: '', limit: 8 } };
    const pending = session.request(current, () => current);
    if (change === 'target') current = { ...current, target: { ...current.target, id: 'another' } };
    if (change === 'port') current = { ...current, port: { ...current.port } };
    if (change === 'document') current = { ...current, document: { ...current.document, editor: { notes: [] } } };
    if (change === 'consumer') current = { ...current, query: { ...current.query!, consumer: { ...consumer, address: { ...consumer.address, node: 'another' } } } };
    if (change === 'search') current = { ...current, query: { ...current.query!, search: 'another' } };
    if (change === 'cancel') session.cancel();
    source.complete(0); expect(await pending, change).toBeUndefined();
  }
});

it('refuses mismatched native correlation rather than accepting a report by executable similarity', async () => {
  for (const override of [{ generation: 'older' }, { id: 'another' }, { candidate_revision: 'program-digest' }, { target: { kind: 'workspace' as const, directory: '/another' } }]) {
    const source = scripted(), session = new WorkflowAnalysisSession();
    const pending = session.request(source.input, () => source.input);
    source.complete(0, override); expect(await pending).toBeUndefined();
  }
});

async function rejectedEdit(document: WorkflowDocument, apply: (document: WorkflowDocument) => WorkflowDocument) {
  const original = structuredClone(document);
  const freeze = (value: unknown) => { if (value && typeof value === 'object') { Object.values(value).forEach(freeze); Object.freeze(value); } };
  freeze(document);
  expect(() => apply(document)).toThrowError(CommandRejected);
  expect(document).toEqual(original);
  const port: WorkflowScopePort = { ...scripted().input.port, read: async () => ({ ...exampleSource('typed_agent'), document }) };
  const editor = createActor(workflowEditorMachine, { input: { port, target: { endpoint: port.endpoint, scope: port.scope, id: 'flow' } } }).start();
  await waitFor(editor, state => state.context.document === document);
  const before = editor.getSnapshot().context;
  editor.send({ type: 'EDIT', apply });
  const after = editor.getSnapshot().context;
  expect(after.rejection).toBe('duplicate_identifier');
  for (const key of ['document', 'past', 'future', 'generation'] as const) expect(after[key]).toBe(before[key]);
  expect(after.document).toEqual(original);
  editor.stop();
}

function orphanNodeDocument(parent: BlockPath, metadata: WorkflowEditorMetadata): WorkflowDocument {
  const empty = { type: 'object', properties: {}, additionalProperties: false };
  const leaf: Block = { input: empty, output: empty,  nodes: {workflow_start:{type:'start' as const}, ...{ done: { type: 'return', output: { type: 'literal', value: {} } } }}, edges: [{from:'workflow_start',to:'done'}, ...[]] };
  const block: Block = parent.length ? { ...leaf,  nodes: {workflow_start:{type:'start' as const}, ...{ ...leaf.nodes, repeat: { type: 'loop', input: { type: 'literal', value: {} }, body: leaf, until: { type: 'boolean', value: { type: 'literal', value: true } }, carry: { type: 'literal', value: {} }, max_iterations: 1 } }}, edges: [{from:'workflow_start',to:'repeat'}, ...[{ from: 'repeat', to: 'done', port: 'satisfied' as const }, { from: 'repeat', to: 'done', port: 'exhausted' as const }]] } : leaf;
  return { description: 'Preserved orphan destination', block, editor: metadata };
}

const orphanDestinations = ([[], [{ node: 'repeat' }]] satisfies BlockPath[]).flatMap(parent => {
  const address = { block: parent, node: 'renamed' }, privateBlock = [...parent, { node: 'renamed' }];
  const position = { x: 20, y: 40 }, dimensions = { width: 240, height: 180 };
  const kinds: [string, WorkflowEditorMetadata][] = [
    ['title', { nodes: [{ address, title: 'old deleted node title' }] }],
    ['position', { nodes: [{ address, position }] }],
    ['input order', { orders: [{ target: { kind: 'node_input', address }, fields: ['x'] }] }],
    ['output order', { orders: [{ target: { kind: 'node_output', address, path: [] }, fields: ['x'] }] }],
    ['private node', { nodes: [{ address: { block: privateBlock, node: 'done' }, title: 'private title' }] }],
    ['private note', { notes: [{ id: 'old', block: privateBlock, text: 'Preserve me', theme: 'blue', position, dimensions }] }],
    ['private order', { orders: [{ target: { kind: 'block_output', block: privateBlock, path: [] }, fields: ['x'] }] }],
  ];
  return kinds.map(([kind, metadata]) => ({ parent, kind, metadata, nested: parent.length > 0 }));
});

it.each(orphanDestinations)('rejects rename into a lone orphan $kind, nested=$nested, without changing history', async ({ parent, metadata }) => {
  const document = orphanNodeDocument(parent, metadata);
  expect(() => renameNodePresentation(document, parent, 'done', 'renamed')).toThrowError(CommandRejected);
  await rejectedEdit(document, current => renameNode(current, parent, 'done', 'renamed'));
});

it.each(orphanDestinations)('allocates a copy beside an orphan $kind, nested=$nested, without adopting its presentation', async ({ parent, metadata }) => {
  const orphan = JSON.parse(JSON.stringify(metadata).replaceAll('"renamed"', '"done_copy"')) as WorkflowEditorMetadata;
  const before = orphanNodeDocument(parent, orphan);
  const result = duplicateFragment(before, parent, ['done'], [], new Map([['done', { x: 360, y: 80 }]]));
  expect(result.nodes).toEqual(['done_copy_2']);
  for (const key of ['nodes', 'notes', 'orders'] as const) for (const entry of orphan[key] ?? []) expect(result.document.editor?.[key]).toContainEqual(entry);
  expect(before.editor).toEqual(orphan);
});

it('keeps identically named orphan destinations in other Blocks separate', () => {
  const parent = [{ node: 'repeat' }];
  const metadata: WorkflowEditorMetadata = { nodes: [{ address: { block: [], node: 'renamed' }, title: 'Root orphan' }, { address: { block: parent, node: 'renamed_long' }, title: 'Different ID' }] };
  const result = renameNode(orphanNodeDocument(parent, metadata), parent, 'done', 'renamed');
  expect(result.editor?.nodes).toEqual(metadata.nodes);
  expect(result.block.nodes.repeat.type).toBe('loop');
});

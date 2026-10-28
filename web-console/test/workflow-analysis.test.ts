import { expect, it } from 'vitest';
import type { WorkflowAnalysisQuery, WorkflowAnalysisReport, WorkflowOrderTarget } from '../../protocol/app-server/v60';
import { WorkflowAnalysisSession, consumerAddress, type AnalysisInput } from '../src/app/workflows/document/analysis';
import { nodeField } from '../src/app/workflows/document/fields';
import { addNode, CommandRejected, deleteNodes, duplicateNode, removeBranch, renameBranch, renameNode } from '../src/app/workflows/document/commands';
import { orderedKeys, positionsIn, setPositions } from '../src/app/workflows/document/presentation';
import { createActor, waitFor } from 'xstate';
import { workflowEditorMachine } from '../src/app/workflows/machines/editor';
import type { WorkflowDocument } from '../src/app/workflows/document/model';
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

it('encodes exact authored positions without inferring availability in TypeScript', () => {
  let document = addNode(exampleSource('typed_agent').document!, [], 'loop', 'repeat');
  const until = consumerAddress(document, nodeField([], 'repeat', ['until', 'value']));
  const carry = consumerAddress(document, nodeField([], 'repeat', ['carry']));
  expect(until).toEqual({ address: { block: [], node: 'repeat' }, field: { kind: 'loop_until' }, path: [{ kind: 'value' }] });
  expect(carry?.field).toEqual({ kind: 'loop_carry' });
  document = addNode(document, [], 'parallel', 'fork');
  expect(consumerAddress(document, nodeField([], 'fork', ['branches', { key: 'main' }, 'input', 'fields', { key: 'a.b' }]))).toEqual({ address: { block: [], node: 'fork' }, field: { kind: 'branch_input', branch: 'main' }, path: [{ kind: 'field', key: 'a.b' }] });
});

function presentationDocument(): WorkflowDocument {
  const document = exampleSource('parallel_review').document!;
  return { ...setPositions(document, [], { reviews: { x: 200, y: 100 } }), editor: {
    ...setPositions(document, [], { reviews: { x: 200, y: 100 } }).editor,
    notes: [
      { id: 'parent_note', block: [], text: 'Keep this Block note', theme: 'blue', position: { x: 0, y: 0 }, dimensions: { width: 240, height: 180 } },
      { id: 'child_note', block: [{ node: 'reviews', branch: 'quality' }], text: '# Private note', theme: 'amber', position: { x: 0, y: 0 }, dimensions: { width: 240, height: 180 } },
    ],
    branches: [{ address: { block: [], node: 'reviews' }, branch: 'quality', position: { x: 0, y: 0 }, dimensions: { width: 450, height: 600 } }],
    orders: [
      { target: { kind: 'branches', address: { block: [], node: 'reviews' } }, fields: ['security', 'quality'] },
      { target: { kind: 'node_output', address: { block: [], node: 'reviews' }, path: [] }, fields: ['security', 'quality'] },
      { target: { kind: 'node_output', address: { block: [], node: 'reviews' }, path: [{ kind: 'property', name: 'quality' }] }, fields: ['acceptable', 'summary'] },
    ],
  } };
}

it('moves metadata by structured identity on node/branch renames and keeps parent-Block notes on deletion', () => {
  const original = presentationDocument();
  const renamed = renameNode(renameBranch(original, [], 'reviews', 'quality', 'clarity'), [], 'reviews', 'checks');
  expect(positionsIn(renamed, [])).toEqual({ checks: { x: 200, y: 100 } });
  expect(renamed.editor?.notes?.[1].block).toEqual([{ node: 'checks', branch: 'clarity' }]);
  expect(renamed.editor?.branches?.[0]).toMatchObject({ address: { node: 'checks' }, branch: 'clarity' });
  expect(renamed.editor?.orders?.[0].fields).toEqual(['security', 'clarity']);
  expect(renamed.editor?.orders?.[1].fields).toEqual(['security', 'clarity']);
  expect(renamed.editor?.orders?.[2].target).toMatchObject({ path: [{ kind: 'property', name: 'clarity' }] });
  const removed = deleteNodes(renamed, [], ['checks']);
  expect(removed.editor?.notes).toEqual([original.editor!.notes![0]]);
  expect(removed.editor?.nodes).toEqual([]);
  expect(removeBranch(original, [], 'reviews', 'quality').editor?.notes).toEqual([original.editor!.notes![0]]);
  const removedBranch = removeBranch(original, [], 'reviews', 'quality');
  expect(removedBranch.editor?.orders).toHaveLength(2);
  expect(removedBranch.editor?.orders?.every(order => order.fields.join() === 'security')).toBe(true);
});

it('copies a node and its private presentation in one document edit, with fresh stable note IDs', () => {
  const original = presentationDocument(), copied = duplicateNode(original, [], 'reviews', 'checks');
  expect(copied.block.nodes.checks).toEqual(original.block.nodes.reviews);
  expect(positionsIn(copied, []).checks).toEqual({ x: 200, y: 100 });
  expect(copied.editor?.notes).toHaveLength(3);
  expect(copied.editor?.notes?.[2]).toMatchObject({ id: 'child_note_copy_2', block: [{ node: 'checks', branch: 'quality' }], text: '# Private note' });
  expect(original.editor?.notes).toHaveLength(2);
});

it.each(['branches', 'node_output'] as const)('atomically refuses a branch rename colliding with an orphaned %s field', async kind => {
  const original = presentationDocument();
  const target: WorkflowOrderTarget = kind === 'branches' ? { kind, address: { block: [], node: 'reviews' } }
    : { kind, address: { block: [], node: 'reviews' }, path: [] };
  const document: WorkflowDocument = { ...original, editor: { ...original.editor, orders: [{ target, fields: ['quality', 'clarity', 'security'] }] } };
  const snapshot = structuredClone(document);
  const fork = document.block.nodes.reviews;
  if (fork.type !== 'parallel') throw new Error('Parallel');
  const branches = Object.keys(fork.branches);
  expect(orderedKeys(document, target, branches)).toEqual(['quality', 'security']);
  try { renameBranch(document, [], 'reviews', 'quality', 'clarity'); throw new Error('rename must reject'); }
  catch (error) { expect(error).toBeInstanceOf(CommandRejected); expect((error as CommandRejected).code).toBe('duplicate_identifier'); }
  expect(document).toEqual(snapshot);
  const port: WorkflowScopePort = { ...scripted().input.port, read: async () => ({ ...exampleSource('parallel_review'), document }) };
  const editor = createActor(workflowEditorMachine, { input: { port, target: { endpoint: port.endpoint, scope: port.scope, id: 'flow' } } }).start();
  await waitFor(editor, snapshot => snapshot.context.document === document);
  const before = editor.getSnapshot().context;
  expect(before.document).toBe(document);
  editor.send({ type: 'EDIT', apply: current => renameBranch(current, [], 'reviews', 'quality', 'clarity') });
  const after = editor.getSnapshot().context;
  expect(after.rejection).toBe('duplicate_identifier');
  expect(after.document).toBe(before.document);
  expect(after.past).toBe(before.past); expect(after.future).toBe(before.future); expect(after.generation).toBe(before.generation);
  expect(after.document).toEqual(snapshot);
  expect(orderedKeys(after.document!, target, branches)).toEqual(['quality', 'security']);
  editor.stop();
});

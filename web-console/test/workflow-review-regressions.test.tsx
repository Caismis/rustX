import { afterEach, describe, expect, it } from 'vitest';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { createActor, waitFor } from 'xstate';
import { addNode, replaceNode } from '../src/app/workflows/document/commands';
import { rootBlock } from './workflow-root';
import { copyFragment, duplicateFragment, parseFragment, pasteFragment } from '../src/app/workflows/document/clipboard';
import { blockField, encodeField, nodeField, setField, type FieldAddress } from '../src/app/workflows/document/fields';
import { putNote, setNodePresentation } from '../src/app/workflows/document/geometry';
import { sameBlockPath, type BlockPath, type NodeDefinition, type WorkflowDocument } from '../src/app/workflows/document/model';
import { renameSchemaField, schemaConsumers, schemaPropertyAddress } from '../src/app/workflows/document/schema';
import { workflowEditorMachine, type WorkflowEditorActor } from '../src/app/workflows/machines/editor';
import type { WorkflowScopePort } from '../src/app/workflows/port';
import { projectScene } from '../src/app/workflows/ui/editor/scene';
import { DraftOwner, FieldCommits } from '../src/app/workflows/ui/editor/forms/drafts';
import { ReferencePicker } from '../src/app/workflows/ui/editor/forms/ReferencePicker';
import { exampleSource } from './workflow-fixtures';
import renameCases from '../../tests/fixtures/workflow/schema-rename.json';
import collisionCases from '../../tests/fixtures/workflow/schema-rename-collisions.json';

afterEach(cleanup);
const fixture = (): WorkflowDocument => ({ description: 'Review regressions', block: rootBlock() });
async function actorFor(document: WorkflowDocument, analyze: WorkflowScopePort['analyze'] = () => new Promise(() => {})) {
  const source = { ...exampleSource('typed_agent'), document };
  const port: WorkflowScopePort = { endpoint: 'ws://fixture/', scope: { kind: 'user' }, enable: () => new Promise(() => {}), analyze, toolCatalog: async () => ({ sources: [] }), modelChoices: () => new Promise(() => {}), list: () => new Promise(() => {}), read: async () => source, check: () => new Promise(() => {}), write: () => new Promise(() => {}) };
  const actor = createActor(workflowEditorMachine, { input: { port, target: { endpoint: port.endpoint, scope: port.scope, id: source.id } } }).start();
  await waitFor(actor, state => !!state.context.document);
  return actor;
}
function oneTransaction(actor: WorkflowEditorActor, before: WorkflowDocument, after: WorkflowDocument) {
  expect(actor.getSnapshot().context.past).toHaveLength(1);
  actor.send({ type: 'UNDO' }); expect(actor.getSnapshot().context.document).toEqual(before);
  expect(actor.getSnapshot().context.past).toHaveLength(0);
  actor.send({ type: 'REDO' }); expect(actor.getSnapshot().context.document).toEqual(after);
}
const positions = (document: WorkflowDocument, path: BlockPath) => new Map(projectScene(document).nodes.filter(node => node.data.view === 'node' && sameBlockPath(node.data.path, path)).map(node => [node.data.local, node.position]));

describe('finite schema renames', () => {
  it.each(renameCases)('keeps $name in one undoable native contract transaction', async example => {
    const before = fixture(); before.block.input = example.before;
    before.block.nodes.done = { type: 'return', outputs: [] };
    const path = example.path as FieldAddress['path'];
    const actor = await actorFor(before);
    actor.send({ type: 'EDIT', apply: document => renameSchemaField(document, blockField([], ['input', ...path]), 'before', 'after') });
    const after = actor.getSnapshot().context.document!;
    expect(after.block.input).toEqual(example.after);
    expect(after.block.nodes.done).toEqual(before.block.nodes.done);
    oneTransaction(actor, before, after); actor.stop();
  });

  it.each(['const', 'enum'] as const)('refuses a conflicting ancestor %s atomically', async constraint => {
    const before = fixture(); before.block.input = { type: 'object', properties: { child: { type: 'object', properties: { before: { type: 'string' } } } }, [constraint]: constraint === 'const' ? { child: { before: 'x', after: 'occupied' } } : [{ child: { before: 'x' } }, { child: { before: 'x', after: 'occupied' } }] };
    const actor = await actorFor(before);
    actor.send({ type: 'EDIT', apply: document => renameSchemaField(document, blockField([], ['input', 'properties', { key: 'child' }]), 'before', 'after') });
    const state = actor.getSnapshot().context;
    expect(state.rejection).toBe('duplicate_identifier'); expect(state.document).toEqual(before); expect(state.past).toHaveLength(0); actor.stop();
  });

  it.each(collisionCases)('refuses $name without changing document/history or existing draft identity', async example => {
    const before = fixture(); before.block.input = example.schema;
    const address = blockField([], ['input', ...example.path as FieldAddress['path']]);
    const actor = await actorFor(before), field = schemaPropertyAddress(address, 'before');
    const pending = nodeField([], 'done', ['output']);
    actor.send({ type: 'DRAFT', address: pending, text: '{ unfinished output' });
    actor.send({ type: 'DRAFT', address: field, text: 'after' });
    const prior = actor.getSnapshot().context, key = encodeField(field), other = encodeField(pending);
    actor.send({ type: 'APPLY_DRAFT', field: key, version: prior.drafts[key].version, command: document => ({ status: 'applied', document: renameSchemaField(document, address, 'before', 'after') }) });
    const after = actor.getSnapshot().context;
    expect(after.document).toBe(prior.document); expect(after.document).toEqual(before);
    expect(after.past).toBe(prior.past); expect(after.future).toBe(prior.future); expect(after.generation).toBe(prior.generation);
    expect(after.drafts[other]).toBe(prior.drafts[other]);
    expect(after.drafts[key]).toEqual({ ...prior.drafts[key], error: { kind: 'rejected', code: 'duplicate_identifier' } });
    actor.stop();
  });

  it('leaves absent keys, scalar enum members and unrelated branches untouched', () => {
    const before = fixture(); before.block.input = { type: 'object', properties: { child: { type: 'object', properties: { before: { type: 'string' } }, enum: [false, { absent: true }] }, other: { const: { before: 'business data' } } }, enum: [false, {}, { child: { absent: true } }, { child: { before: 'x' }, other: { before: 'keep' } }] };
    const after = renameSchemaField(before, blockField([], ['input', 'properties', { key: 'child' }]), 'before', 'after');
    expect(after.block.input).toMatchObject({ enum: [false, {}, { child: { absent: true } }, { child: { after: 'x' }, other: { before: 'keep' } }], properties: { child: { enum: [false, { absent: true }] }, other: { const: { before: 'business data' } } } });
  });
});


describe('one duplicate command for every entry point', () => {

  it.each([{ x: 0.3, y: 0.3 }, { x: 500.3, y: 240.3 }])('separates repeated node and note copies at fractional position %j', point => {
    let document = setNodePresentation(fixture(), [], 'done', { position: point });
    document = putNote(document, { id: 'fractional', block: [], text: '', theme: 'blue', position: point, dimensions: { width: 200, height: 100 } });
    for (const notes of [false, true]) {
      const ids = notes ? [] : ['done'], noteIds = notes ? ['fractional'] : [];
      const first = duplicateFragment(document, [], ids, noteIds, positions(document, []));
      const second = duplicateFragment(first.document, [], ids, noteIds, positions(first.document, []));
      const copiedPosition = notes ? second.document.editor?.notes?.at(-1)?.position : second.document.editor?.nodes?.find(entry => entry.address.node === second.nodes[0])?.position;
      expect(copiedPosition).toEqual({ x: point.x + 64, y: point.y + 64 });
    }
  });

  it('bounds offset discovery for a full adversarial metadata cut before rejecting geometry', () => {
    const count = 64, occupied = 960, base = -1_000_000 + 32 * occupied * (count - 1);
    const source = fixture(); source.editor = { notes: Array.from({ length: count }, (_, index) => ({ id: `note_${index}`, block: [], text: '', theme: 'blue', position: { x: base - 32 * occupied * index, y: base - 32 * occupied * index }, dimensions: { width: 200, height: 100 } })) };
    const destination = fixture(); destination.editor = { nodes: Array.from({ length: occupied }, (_, index) => ({ address: { block: [], node: `orphan_${index}` }, position: { x: base + 32 * (index + 1), y: base + 32 * (index + 1) } })) };
    const before = structuredClone(destination), fragment = parseFragment(copyFragment(source, 'source', [], [], source.editor.notes!.map(note => note.id)));
    const started = performance.now();
    expect(() => pasteFragment(destination, 'destination', [], fragment)).toThrow('invalid_geometry');
    expect(performance.now() - started).toBeLessThan(1000);
    expect(destination).toEqual(before);
  });

  it('returns fresh top-level note identities and atomically refuses bounds overflow', async () => {
    const note = { id: 'note', block: [], text: 'Visible copy', theme: 'blue' as const, position: { x: 20, y: 40 }, dimensions: { width: 220, height: 100 } };
    const document = putNote(fixture(), note), result = duplicateFragment(document, [], [], ['note'], new Map());
    expect(result.nodes).toEqual([]); expect(result.notes).toEqual(['note_2']);
    expect(result.document.editor?.notes?.at(-1)?.position).toEqual({ x: 52, y: 72 });
    const before = setNodePresentation(document, [], 'done', { position: { x: 1_000_000, y: 10 } }), actor = await actorFor(before);
    actor.send({ type: 'EDIT', apply: current => duplicateFragment(current, [], ['done'], [], positions(current, [])).document });
    expect(actor.getSnapshot().context.document).toEqual(before); expect(actor.getSnapshot().context.past).toHaveLength(0); expect(actor.getSnapshot().context.rejection).toBe('invalid_geometry'); actor.stop();
  });
});

it('sends the original search with every titled producer to the bounded native query', async () => {
  const document = fixture(); document.block.nodes.done = { type: 'return', outputs: [{ variable: 'args', value_selector: ['args'] }] };
  const titled = setNodePresentation(setNodePresentation(addNode(addNode(document, [], 'model', 'first'), [], 'model', 'second'), [], 'first', { title: 'First Draft' }), [], 'second', { title: 'Second Draft' });
  const queries: string[] = [], actor = await actorFor(titled, async (_id, _document, _generation, query) => { queries.push(query!.search ?? ''); return new Promise(() => {}); });
  render(<DraftOwner value={{ editor: actor, commits: new FieldCommits() }}><ReferencePicker address={nodeField([], 'done', ['outputs', 0, 'value_selector'])} path={['args']} edit={apply => actor.send({ type: 'EDIT', apply })} /></DraftOwner>);
  fireEvent.click(screen.getByRole('button', { name: 'Choose a variable' }));
  fireEvent.change(screen.getByRole('textbox', { name: 'Search reference paths…' }), { target: { value: 'Draft' } });
  await act(async () => { await Promise.resolve(); });
  expect(queries.at(-1)).toBe('Draft'); actor.stop();
});

it.each(['root Return', 'Loop variable', 'body Tool', 'break condition'])('titles a selected reference by its producer around or inside the %s', async kind => {
  let document = addNode(addNode(fixture(), [], 'model', 'result'), [], 'loop', 'repeat');
  document = setNodePresentation(document, [], 'result', { title: 'Actual result producer' });
  const body = [{ node: 'repeat' }];
  document = addNode(addNode(document, body, 'model', 'inner'), body, 'tool', 'use');
  document = setNodePresentation(document, body, 'inner', { title: 'Private producer' });
  // An End output holds the variable's path itself.
  let address = nodeField([], 'done', ['outputs', 0, 'value_selector']), path = ['result'];
  document = replaceNode(document, [], 'done', { type: 'return', outputs: [{ variable: 'result', value_selector: path }] });
  if (kind === 'Loop variable') {
    document = replaceNode(document, [], 'repeat', { ...document.block.nodes.repeat as Extract<NodeDefinition, { type: 'loop' }>, loop_variables: [{ label: 'text', var_type: 'string', value: { type: 'literal', value: '' } }] });
    address = nodeField([], 'repeat', ['loop_variables', 0, 'value']);
  }
  // As in Dify, a body reads the nodes around it.
  if (kind === 'body Tool') address = nodeField(body, 'use', ['arguments']);
  if (kind === 'break condition') {
    path = ['inner'];
    document = replaceNode(document, [], 'repeat', { ...document.block.nodes.repeat as Extract<NodeDefinition, { type: 'loop' }>, break_condition: { type: 'equal', left: { type: 'reference', path }, right: { type: 'literal', value: '' } } });
    address = nodeField([], 'repeat', ['break_condition', 'left']);
  }
  if (kind !== 'root Return') document = setField(document, address, { type: 'reference', path });
  const actor = await actorFor(document);
  render(<DraftOwner value={{ editor: actor, commits: new FieldCommits() }}><ReferencePicker address={address} path={path} edit={apply => actor.send({ type: 'EDIT', apply })} /></DraftOwner>);
  expect(screen.getByRole('button', { name: 'Choose a variable' }).textContent).toBe(kind === 'break condition' ? 'Private producer' : 'Actual result producer');
  expect(actor.getSnapshot().context.document).toBe(document); expect(actor.getSnapshot().context.past).toHaveLength(0);
  actor.stop();
});

describe('body contract boundaries', () => {
  it.each(['iteration', 'loop'] as const)('renames a body output and its %s round consumer in one undo step', async kind => {
    const before = fixture();
    const body = { input: {}, nodes: { start: { type: 'start' as const }, produce: { type: 'model' as const, model: { mode: 'session' as const }, messages: [], output: { type: 'json_schema' as const, schema: { type: 'object', properties: { old: { type: 'boolean' } }, required: ['old'], additionalProperties: false } } } }, edges: [{ from: 'start', to: 'produce' }] };
    const reference = { type: 'reference' as const, path: ['produce', 'structured_output', 'old'] };
    before.block.nodes.repeat = kind === 'iteration' ? { type: kind, iterator: { type: 'reference', path: ['args', 'items'] }, output: reference, body }
      : { type: kind, loop_count: 2, loop_variables: [], break_condition: { type: 'boolean', value: reference }, body };
    const address = nodeField([{ node: 'repeat' }], 'produce', ['output', 'schema']);
    expect(schemaConsumers(before, { ...address, path: [...address.path, 'properties', { key: 'old' }] })).toContainEqual({ block: [], node: 'repeat' });
    const actor = await actorFor(before);
    actor.send({ type: 'EDIT', apply: document => renameSchemaField(document, address, 'old', 'new') });
    const after = actor.getSnapshot().context.document!;
    const container = after.block.nodes.repeat;
    expect(container).toMatchObject(kind === 'iteration' ? { output: { path: ['produce', 'structured_output', 'new'] } } : { break_condition: { value: { path: ['produce', 'structured_output', 'new'] } } });
    oneTransaction(actor, before, after); actor.stop();
  });

  it('allocates a copied container away from its copied descendants and destination ancestors', () => {
    const before = fixture();
    before.block.nodes.repeat = { type: 'iteration', iterator: { type: 'reference', path: ['args', 'items'] }, output: { type: 'reference', path: ['repeat_copy', 'text'] },
      body: { input: {}, nodes: { start: { type: 'start' }, repeat_copy: { type: 'tool', selector: { origin: 'builtin', name: 'read' }, arguments: { type: 'object', fields: {} }, result: { type: 'text' } } }, edges: [{ from: 'start', to: 'repeat_copy' }] } };
    const fragment = parseFragment(copyFragment(before, 'same', [], ['repeat'], []));
    const pasted = pasteFragment(before, 'same', [], fragment);
    expect(pasted.nodes).toEqual(['repeat_copy_2']);
    expect(pasted.document.block.nodes.repeat_copy_2).toMatchObject({ output: { path: ['repeat_copy', 'text'] } });

  });
  it('reserves copied subtree IDs across documents and ancestor IDs inside a destination body', () => {
    const source = fixture();
    source.block.nodes.source = { type: 'tool', selector: { origin: 'builtin', name: 'read' }, arguments: { type: 'object', fields: {} }, result: { type: 'json', schema: { type: 'object', properties: { items: { type: 'array', items: { type: 'string' } } } } } };
    source.block.nodes.repeat = { type: 'iteration', iterator: { type: 'reference', path: ['source', 'items'] }, output: { type: 'reference', path: ['repeat_copy', 'text'] }, body: { input: {}, nodes: { start: { type: 'start' }, repeat_copy: { type: 'tool', selector: { origin: 'builtin', name: 'read' }, arguments: { type: 'object', fields: {} }, result: { type: 'text' } } }, edges: [{ from: 'start', to: 'repeat_copy' }] } };
    const fragment = parseFragment(copyFragment(source, 'source', [], ['source', 'repeat'], []));
    const pasted = pasteFragment(fixture(), 'destination', [], fragment);
    expect(pasted.nodes).toContain('repeat_copy_2');
    expect(pasted.document.block.nodes.repeat_copy_2).toMatchObject({ iterator: { path: ['source_copy', 'items'] }, output: { path: ['repeat_copy', 'text'] } });
    const destination = fixture();
    destination.block.nodes.source_copy = { type: 'loop', loop_count: 2, loop_variables: [], body: { input: {}, nodes: { start: { type: 'start' } }, edges: [] } };
    const leaf = parseFragment(copyFragment(source, 'source', [], ['source'], []));
    expect(pasteFragment(destination, 'destination', [{ node: 'source_copy' }], leaf).nodes).toEqual(['source_copy_2']);
  });

});

describe('Agent and Model declaration paths', () => {
  for (const kind of ['agent', 'model'] as const) it.each([false, true])(`${kind} nested array=%s keeps ancestor finite constraints and references in one transaction`, async array => {
    const contract = (key: string) => {
      const finite = { [key]: 'value' };
      const leaf = { type: 'object', properties: { [key]: { type: 'string' } }, required: [key], additionalProperties: false, const: finite, enum: [finite] };
      const parentValue = array ? { rows: [finite] } : finite;
      const parent = array ? { type: 'object', properties: { rows: { type: 'array', items: leaf, const: [finite], enum: [[finite]] } }, required: ['rows'], additionalProperties: false, const: parentValue, enum: [parentValue] } : leaf;
      return { type: 'object', properties: { parent }, required: ['parent'], additionalProperties: false, const: { parent: parentValue }, enum: [{ parent: parentValue }] };
    };
    const before = fixture();
    before.block.nodes.produce = kind === 'agent' ? { type: 'agent', profile: 'reviewer', task: [], output: { type: 'json_schema', schema: contract('old') } }
      : { type: 'model', model: { mode: 'session' }, messages: [], output: { type: 'json_schema', schema: contract('old') } };
    const prefix = kind === 'model' ? ['produce', 'structured_output'] : ['produce'];
    const reference = [...prefix, 'parent', ...array ? ['rows'] : ['old']];
    before.block.nodes.done = { type: 'return', outputs: [{ variable: 'result', value_selector: reference }] };
    const address = nodeField([], 'produce', ['output', 'schema', 'properties', { key: 'parent' }, ...array ? ['properties', { key: 'rows' }, 'items'] : []]);
    const actor = await actorFor(before);
    actor.send({ type: 'EDIT', apply: document => renameSchemaField(document, address, 'old', 'new') });
    const after = actor.getSnapshot().context.document!;
    expect(after.block.nodes.produce).toMatchObject({ output: { schema: contract('new') } });
    expect(after.block.nodes.done).toMatchObject({ outputs: [{ value_selector: array ? reference : [...prefix, 'parent', 'new'] }] });
    oneTransaction(actor, before, after); actor.stop();
  });
});

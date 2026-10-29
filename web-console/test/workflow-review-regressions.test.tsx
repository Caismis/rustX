import { afterEach, describe, expect, it } from 'vitest';
import { act, cleanup, fireEvent, render, screen, within as uiWithin } from '@testing-library/react';
import { useSelector } from '@xstate/react';
import { createActor, waitFor } from 'xstate';
import { addBranch, addNode, emptyBlock, renameBranch } from '../src/app/workflows/document/commands';
import { copyFragment, duplicateFragment, parseFragment, pasteFragment } from '../src/app/workflows/document/clipboard';
import { blockField, encodeField, fieldValue, nodeField, setField, within, type FieldAddress } from '../src/app/workflows/document/fields';
import { putNote, setLane, setNodePresentation, setOrder } from '../src/app/workflows/document/geometry';
import { blockAt, sameBlockPath, type BlockPath, type Value, type WorkflowDocument } from '../src/app/workflows/document/model';
import { renameSchemaField, schemaPropertyAddress } from '../src/app/workflows/document/schema';
import { workflowEditorMachine, type WorkflowEditorActor } from '../src/app/workflows/machines/editor';
import type { WorkflowScopePort } from '../src/app/workflows/port';
import { organizeDocument } from '../src/app/workflows/ui/editor/layout';
import { projectScene } from '../src/app/workflows/ui/editor/scene';
import { viewId } from '../src/app/workflows/ui/editor/selection';
import { DraftOwner, FieldCommits } from '../src/app/workflows/ui/editor/forms/drafts';
import { ReferencePicker } from '../src/app/workflows/ui/editor/forms/ReferencePicker';
import { SchemaEditor } from '../src/app/workflows/ui/editor/forms/SchemaEditor';
import { exampleSource } from './workflow-fixtures';
import renameCases from '../../tests/fixtures/workflow/schema-rename.json';
import collisionCases from '../../tests/fixtures/workflow/schema-rename-collisions.json';
import constructionCollision from '../../tests/fixtures/workflow/schema-construction-collision.json';

afterEach(cleanup);
const fixture = (): WorkflowDocument => ({ description: 'Review regressions', block: emptyBlock() });
async function actorFor(document: WorkflowDocument, analyze: WorkflowScopePort['analyze'] = () => new Promise(() => {})) {
  const source = { ...exampleSource('typed_agent'), document };
  const port: WorkflowScopePort = { endpoint: 'ws://fixture/', scope: { kind: 'user' }, analyze, modelChoices: () => new Promise(() => {}), list: () => new Promise(() => {}), read: async () => source, check: () => new Promise(() => {}), write: () => new Promise(() => {}) };
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

describe('owned Object construction collisions', () => {
  const owners = ['Parallel output', 'Loop output', 'nested Object', 'Parallel input', 'Loop initial input', 'Loop carry', 'root output'] as const;
  const cases = owners.flatMap(owner => [true, false].flatMap(source => [false, true].map(literal => ({ owner, source, literal }))));
  it.each(cases)('rejects $owner with source present=$source literal=$literal before rebinding any consumer or draft', async ({ owner, source, literal }) => {
    let document = structuredClone(constructionCollision) as WorkflowDocument;
    let block: BlockPath = [{ node: 'fork', branch: 'main' }], contract = 'output';
    let construction = nodeField(block, 'done', ['output']);
    if (owner.startsWith('Loop')) {
      document = addNode(fixture(), [], 'loop', 'repeat'); block = [{ node: 'repeat' }];
      construction = owner === 'Loop output' ? nodeField(block, 'done', ['output']) : nodeField([], 'repeat', [owner === 'Loop carry' ? 'carry' : 'input']);
      contract = owner === 'Loop output' ? 'output' : 'input';
      // The earlier construction can be changed locally before a later carry
      // collision throws; the actor must reject that whole candidate too.
      document = setField(document, nodeField([], 'repeat', ['input']), { type: 'object', fields: { before: { type: 'literal', value: 'wanted' } } });
    } else if (owner === 'Parallel input') {
      contract = 'input'; construction = nodeField([], 'fork', ['branches', { key: 'main' }, 'input']);
    } else if (owner === 'root output') {
      document = fixture(); block = []; construction = nodeField([], 'done', ['output']);
    }
    const schema = { type: 'object', properties: { before: { type: 'string' } }, ...(source ? { required: ['before'] } : {}), additionalProperties: true };
    const fields: Record<string, Value> = { ...(source ? { before: { type: 'literal' as const, value: 'wanted' } } : {}), after: { type: 'literal', value: 'unrelated' } };
    const data = { ...(source ? { before: 'wanted' } : {}), after: 'unrelated' };
    const value: Value = literal ? { type: 'literal', value: data } : { type: 'object', fields }, nested = owner === 'nested Object';
    const address = blockField(block, [contract, ...(nested ? ['properties', { key: 'wrapper' }] : [])]);
    document = setField(document, blockField(block, [contract]), nested ? { type: 'object', properties: { wrapper: schema } } : schema);
    document = setField(document, construction, nested ? literal ? { type: 'literal', value: { wrapper: data } } : { type: 'object', fields: { wrapper: value } } : value);
    const actor = await actorFor(document);
    actor.send({ type: 'EDIT', apply: current => ({ ...current, description: 'Earlier accepted edit' }) });
    actor.send({ type: 'EDIT', apply: current => ({ ...current, description: 'Redo must survive rejection' }) });
    actor.send({ type: 'UNDO' });
    const pending = within(construction, literal ? ['value'] : [...(nested ? ['fields', { key: 'wrapper' }] : []), 'fields', { key: source ? 'before' : 'after' }, 'value']);
    const rename = schemaPropertyAddress(address, 'before'), key = encodeField(rename), other = encodeField(pending);
    actor.send({ type: 'DRAFT', address: pending, text: '"unfinished construction' });
    actor.send({ type: 'DRAFT', address: rename, text: 'after' });
    const prior = actor.getSnapshot().context;
    expect(prior.past).toHaveLength(1); expect(prior.future).toHaveLength(1);
    actor.send({ type: 'APPLY_DRAFT', field: key, version: prior.drafts[key].version, command: current => ({ status: 'applied', document: renameSchemaField(current, address, 'before', 'after') }) });
    const after = actor.getSnapshot().context;
    expect(after.document).toBe(prior.document); expect(after.document).toEqual(prior.document);
    expect(after.past).toBe(prior.past); expect(after.future).toBe(prior.future); expect(after.generation).toBe(prior.generation);
    expect(after.drafts[other]).toBe(prior.drafts[other]);
    expect(after.drafts[key]).toEqual({ ...prior.drafts[key], error: { kind: 'rejected', code: 'duplicate_identifier' } });
    actor.stop();
  });

  it('renames the exact nested construction while leaving an unrelated occupied key alone', () => {
    let document = fixture();
    document.block.output = { type: 'object', properties: { wrapper: { type: 'object', properties: { before: { type: 'string' } } } } };
    document = setField(document, nodeField([], 'done', ['output']), { type: 'object', fields: { wrapper: { type: 'object', fields: { before: { type: 'literal', value: 'wanted' } } }, unrelated: { type: 'object', fields: { after: { type: 'literal', value: 'keep' } } } } });
    const after = renameSchemaField(document, blockField([], ['output', 'properties', { key: 'wrapper' }]), 'before', 'after');
    expect(after.block.nodes.done).toMatchObject({ output: { fields: { wrapper: { fields: { after: { value: 'wanted' } } }, unrelated: { fields: { after: { value: 'keep' } } } } } });
  });

  it('rejects a Literal leaf under an Object AST without rewriting unrelated business literals', () => {
    let document = fixture();
    document.block.output = { type: 'object', properties: { wrapper: { type: 'object', properties: { before: { type: 'string' } } } } };
    document = setField(document, nodeField([], 'done', ['output']), { type: 'object', fields: { wrapper: { type: 'literal', value: { before: 'wanted', after: 'unrelated' } }, unrelated: { type: 'literal', value: { after: 'keep' } } } });
    expect(() => renameSchemaField(document, blockField([], ['output', 'properties', { key: 'wrapper' }]), 'before', 'after')).toThrow('duplicate_identifier');
    document = setField(document, nodeField([], 'done', ['output', 'fields', { key: 'wrapper' }]), { type: 'literal', value: { before: 'wanted' } });
    const after = renameSchemaField(document, blockField([], ['output', 'properties', { key: 'wrapper' }]), 'before', 'after');
    expect(after.block.nodes.done).toEqual(document.block.nodes.done);
  });

  it.each([false, true])('refuses an opaque whole-value construction with nested=%s without guessing its upstream data', async nested => {
    let document = structuredClone(constructionCollision) as WorkflowDocument;
    const block = [{ node: 'fork', branch: 'main' }];
    const data = { type: 'object', fields: { before: { type: 'literal', value: 'wanted' }, after: { type: 'literal', value: 'unrelated' } } };
    document = setField(document, blockField(block, ['input']), { type: 'object', properties: { before: { type: 'string' }, after: { type: 'string' } }, required: ['before', 'after'], additionalProperties: false });
    document = setField(document, nodeField([], 'fork', ['branches', { key: 'main' }, 'input']), data);
    const output = blockAt(document, block)!.output;
    document = setField(document, blockField(block, ['output']), nested ? { type: 'object', properties: { wrapper: output } } : output);
    document = setField(document, nodeField(block, 'done', ['output']), nested ? { type: 'object', fields: { wrapper: { type: 'reference', path: ['args'] } } } : { type: 'reference', path: ['args'] });
    const address = blockField(block, ['output', ...(nested ? ['properties', { key: 'wrapper' }] : [])]);
    const actor = await actorFor(document), field = schemaPropertyAddress(address, 'before'), key = encodeField(field);
    actor.send({ type: 'DRAFT', address: field, text: 'after' });
    const prior = actor.getSnapshot().context;
    actor.send({ type: 'APPLY_DRAFT', field: key, version: prior.drafts[key].version, command: current => ({ status: 'applied', document: renameSchemaField(current, address, 'before', 'after') }) });
    const after = actor.getSnapshot().context;
    expect(after.document).toBe(prior.document); expect(after.past).toBe(prior.past); expect(after.future).toBe(prior.future); expect(after.generation).toBe(prior.generation);
    expect(after.drafts[key]).toEqual({ ...prior.drafts[key], error: { kind: 'rejected', code: 'unsupported_schema_rename' } });
    actor.stop();
  });

  it('renames explicit Object keys whose values are references, and refuses an opaque Loop carry', () => {
    const document = fixture();
    document.block.output = { type: 'object', properties: { before: { type: 'object' } } };
    document.block.nodes.done = { type: 'return', output: { type: 'object', fields: { before: { type: 'reference', path: ['args'] } } } };
    expect(renameSchemaField(document, blockField([], ['output']), 'before', 'after').block.nodes.done).toMatchObject({ output: { fields: { after: { type: 'reference', path: ['args'] } } } });
    let loop = addNode(fixture(), [], 'loop', 'repeat');
    loop = setField(loop, blockField([{ node: 'repeat' }], ['input']), { type: 'object', properties: { before: { type: 'string' } } });
    loop = setField(loop, nodeField([], 'repeat', ['input']), { type: 'object', fields: { before: { type: 'literal', value: 'wanted' } } });
    expect(() => renameSchemaField(loop, blockField([{ node: 'repeat' }], ['input']), 'before', 'after')).toThrow('unsupported_schema_rename');
    expect(loop.block.nodes.repeat).toMatchObject({ input: { fields: { before: { value: 'wanted' } } }, carry: { type: 'reference', path: ['result'] } });
  });
});

describe('native branch keys in clipboard fragments', () => {
  it.each(['main', 'args'].flatMap(branch => [false, true].map(child => ({ branch, child }))))('copies, pastes and duplicates branch $branch child=$child with qualified presentation', async ({ branch, child }) => {
    let document = structuredClone(constructionCollision) as WorkflowDocument;
    if (branch !== 'main') document = renameBranch(document, [], 'fork', 'main', branch);
    const privatePath = [{ node: 'fork', branch }];
    document = setNodePresentation(document, privatePath, 'done', { title: 'Private return', position: { x: 402, y: 120 } });
    document = setLane(document, { address: { block: [], node: 'fork' }, branch, position: { x: 24, y: 72 }, dimensions: { width: 640, height: 256 } });
    document = putNote(document, { id: 'private', block: privatePath, text: 'Private note', theme: 'blue', position: { x: 35, y: 300 }, dimensions: { width: 200, height: 100 } });
    const path = child ? privatePath : [], id = child ? 'done' : 'fork';
    const fragment = parseFragment(copyFragment(document, 'owner', path, [id]));
    expect(fragment.nodes[id]).toEqual(blockAt(document, path)!.nodes[id]);
    const pasted = pasteFragment(document, 'owner', path, fragment), duplicated = duplicateFragment(document, path, [id], [], positions(document, path));
    for (const result of [pasted, duplicated]) {
      const at = child ? privatePath : [{ node: 'fork_copy', branch }];
      expect(blockAt(result.document, at)!.nodes[child ? 'done_copy' : 'done']).toEqual(blockAt(document, privatePath)!.nodes.done);
      expect(result.document.editor?.nodes).toContainEqual(expect.objectContaining({ address: { block: at, node: child ? 'done_copy' : 'done' }, title: 'Private return' }));
      if (!child) {
        expect(result.document.editor?.branches).toContainEqual(expect.objectContaining({ address: { block: [], node: 'fork_copy' }, branch }));
        expect(result.document.editor?.notes).toContainEqual(expect.objectContaining({ block: at, position: { x: 35, y: 300 } }));
      }
      const actor = await actorFor(document); actor.send({ type: 'EDIT', apply: () => result.document });
      oneTransaction(actor, document, result.document); actor.stop();
    }
  });

  it('allows args as an authored branch name while keeping it reserved for executable node IDs', () => {
    const document = addBranch(addNode(fixture(), [], 'parallel', 'fork'), [], 'fork', 'args');
    expect(blockAt(document, [{ node: 'fork', branch: 'args' }])).toBeDefined();
    expect(() => addNode(document, [], 'return', 'args')).toThrow('reserved_identifier');
    const fragment = JSON.parse(copyFragment(document, 'owner', [], ['fork']));
    const illegalChild = structuredClone(fragment);
    illegalChild.nodes.fork.branches.args.block.nodes = { args: illegalChild.nodes.fork.branches.args.block.nodes.done };
    const bad = [{ ...fragment, nodes: { args: document.block.nodes.fork } }, illegalChild];
    for (const value of bad) expect(() => parseFragment(JSON.stringify(value))).toThrow('invalid_fragment');
  });

  it('retains legal args orphan metadata below a copied container without treating it as an executable node', () => {
    const privatePath = [{ node: 'fork', branch: 'main' }], orphan = [...privatePath, { node: 'args', branch: 'args' }];
    const document = structuredClone(constructionCollision) as WorkflowDocument;
    document.editor = {
      nodes: [{ address: { block: privatePath, node: 'args' }, title: 'Retained orphan' }],
      entries: [{ block: orphan, position: { x: 24, y: 80 } }],
      branches: [{ address: { block: privatePath, node: 'args' }, branch: 'args', position: { x: 24, y: 72 }, dimensions: { width: 640, height: 256 } }],
      notes: [{ id: 'orphan', block: orphan, text: 'Retained', theme: 'blue', position: { x: 24, y: 80 }, dimensions: { width: 200, height: 100 } }],
      orders: [{ target: { kind: 'block_output', block: orphan, path: [] }, fields: ['keep'] }],
    };
    const fragment = parseFragment(copyFragment(document, 'owner', [], ['fork']));
    expect(fragment.editor).toEqual(document.editor);
    const after = pasteFragment(document, 'owner', [], fragment).document;
    expect(after.editor?.nodes).toContainEqual({ address: { block: [{ node: 'fork_copy', branch: 'main' }], node: 'args' }, title: 'Retained orphan' });
    expect(after.editor?.orders).toContainEqual({ target: { kind: 'block_output', block: [{ node: 'fork_copy', branch: 'main' }, { node: 'args', branch: 'args' }], path: [] }, fields: ['keep'] });
    expect(blockAt(after, privatePath)!.nodes).not.toHaveProperty('args');
  });

  it.each(['', 'with.dot', 'x'.repeat(65), 'args\n'])('still rejects invalid local key %j in every identity decoder entry point', branch => {
    const document = structuredClone(constructionCollision) as WorkflowDocument;
    const fragment = JSON.parse(copyFragment(document, 'owner', [], ['fork']));
    const fork = fragment.nodes.fork;
    const bad = [
      { ...fragment, nodes: { fork: { ...fork, branches: { [branch]: fork.branches.main } } } },
      { ...fragment, block: [{ node: 'fork', branch }] },
      { ...fragment, editor: { branches: [{ address: { block: [], node: 'fork' }, branch, position: { x: 24, y: 72 }, dimensions: { width: 640, height: 256 } }] } },
      { ...fragment, nodes: { [branch]: { type: 'return', output: { type: 'literal', value: {} } } } },
      { ...fragment, editor: { notes: [{ id: branch, block: [], text: '', theme: 'blue', position: { x: 24, y: 80 }, dimensions: { width: 200, height: 100 } }] } },
    ];
    for (const value of bad) expect(() => parseFragment(JSON.stringify(value))).toThrow('invalid_fragment');
    expect(() => putNote(document, { id: branch, block: [], text: '', theme: 'blue', position: { x: 24, y: 80 }, dimensions: { width: 200, height: 100 } })).toThrow('invalid_identifier');
  });
});

describe('finite schema renames', () => {
  it.each(renameCases)('keeps $name in one undoable native contract transaction', async example => {
    const before = fixture(); before.block.input = example.before;
    before.block.nodes.done = { type: 'return', output: { type: 'literal', value: { before: 'Do not rewrite business literals' } } };
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

function disjointLanes(document: WorkflowDocument, path: BlockPath, id = 'fork') {
  const frames = projectScene(document).nodes.filter(node => node.data.view === 'lane' && node.parentId === viewId(path, 'node', id));
  for (let i = 0; i < frames.length; i++) for (const other of frames.slice(i + 1)) {
    const frame = frames[i];
    expect(frame.position.x + frame.width! <= other.position.x || other.position.x + other.width! <= frame.position.x || frame.position.y + frame.height! <= other.position.y || other.position.y + other.height! <= frame.position.y).toBe(true);
  }
  return frames;
}

describe('saved Parallel lane growth', () => {
  it.each(([[], [{ node: 'outer' }]] satisfies BlockPath[]).map(path => ({ path })))('separates new and enlarged lanes in Block $path before and after organize', async ({ path }) => {
    let document = fixture(); if (path.length) document = addNode(document, [], 'loop', 'outer');
    document = addNode(document, path, 'parallel', 'fork');
    document = await organizeDocument(document);
    document = addBranch(document, path, 'fork', 'alpha');
    disjointLanes(document, path);
    const order = { kind: 'branches' as const, address: { block: path, node: 'fork' } };
    document = setOrder(document, order, ['alpha', 'main']);
    const first = await organizeDocument(document);
    const frames = disjointLanes(first, path);
    expect(frames.find(frame => frame.data.local === 'alpha')!.position).toEqual({ x: 24, y: 72 });
    expect(frames.find(frame => frame.data.local === 'main')!.position.y).toBeGreaterThan(72);
    const privatePath = [...path, { node: 'fork', branch: 'alpha' }];
    let grown = addNode(first, privatePath, 'loop', 'larger');
    grown = putNote(grown, { id: 'lane_note', block: privatePath, text: 'Relative note', theme: 'blue', position: { x: 30, y: 600 }, dimensions: { width: 220, height: 110 } });
    disjointLanes(grown, path);
    const organized = await organizeDocument(grown);
    disjointLanes(organized, path);
    expect(organized.block).toBe(grown.block); expect(organized.editor?.notes).toEqual(grown.editor?.notes);
    for (const lane of organized.editor?.branches ?? []) expect(lane.dimensions.width <= 8192 && lane.dimensions.height <= 8192).toBe(true);
    expect(await organizeDocument(grown)).toEqual(organized);
    const actor = await actorFor(grown); actor.send({ type: 'EDIT', apply: () => organized });
    oneTransaction(actor, grown, organized); actor.stop();
  });

  it('keeps noncolliding authored lane positions and resolves collisions after child measurement', () => {
    let document = addBranch(addNode(fixture(), [], 'parallel', 'fork'), [], 'fork', 'alpha');
    document = setLane(document, { address: { block: [], node: 'fork' }, branch: 'main', position: { x: 500, y: 1000 }, dimensions: { width: 640, height: 232 } });
    const frames = disjointLanes(document, []);
    expect(frames.find(frame => frame.data.local === 'main')!.position).toEqual({ x: 500, y: 1000 });
  });
});

describe('one duplicate command for every entry point', () => {
  it.each(['return', 'loop', 'parallel'] as const)('offsets and selects identities for positioned and unpositioned %s copies', async kind => {
    let document = fixture(); if (kind !== 'return') document = addNode(document, [], kind, 'owner');
    const id = kind === 'return' ? 'done' : 'owner';
    if (kind !== 'return') {
      const child = [{ node: id, ...(kind === 'parallel' ? { branch: 'main' } : {}) }];
      document = setNodePresentation(document, child, 'done', { position: { x: 402, y: 120 }, title: 'Private return' });
      document = putNote(document, { id: 'private', block: child, text: 'Keep relative', theme: 'green', position: { x: 35, y: 320 }, dimensions: { width: 240, height: 120 } });
    }
    for (const positioned of [false, true]) {
      const before = positioned ? setNodePresentation(document, [], id, { position: { x: 500, y: 240 }, title: 'Finish', dimensions: { width: 256, height: 128 } }) : document;
      const original = positions(before, []).get(id)!;
      const actor = await actorFor(before);
      actor.send({ type: 'EDIT', apply: current => duplicateFragment(current, [], [id], [], positions(current, [])).document });
      const after = actor.getSnapshot().context.document!;
      expect(after.editor?.nodes?.find(entry => entry.address.node === `${id}_copy` && !entry.address.block.length)?.position).toEqual({ x: original.x + 32, y: original.y + 32 });
      if (kind !== 'return') {
        const child = [{ node: `${id}_copy`, ...(kind === 'parallel' ? { branch: 'main' } : {}) }];
        expect(after.editor?.nodes?.find(entry => sameBlockPath(entry.address.block, child))?.position).toEqual({ x: 402, y: 120 });
        expect(after.editor?.notes?.at(-1)).toMatchObject({ id: 'private_2', block: child, position: { x: 35, y: 320 } });
      }
      oneTransaction(actor, before, after);
      const repeated = duplicateFragment(after, [], [id], [], positions(after, []));
      expect(repeated.nodes).toEqual([`${id}_copy_2`]);
      expect(repeated.document.editor?.nodes?.find(entry => entry.address.node === repeated.nodes[0] && !entry.address.block.length)?.position).toEqual({ x: original.x + 64, y: original.y + 64 });
      expect(document.block.nodes[id]).toEqual(before.block.nodes[id]); actor.stop();
    }
  });

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

function SchemaHarness({ actor }: { actor: WorkflowEditorActor }) {
  const document = useSelector(actor, state => state.context.document)!;
  return <DraftOwner value={{ editor: actor, commits: new FieldCommits() }}><SchemaEditor label="Input contract" value={document.block.input} address={blockField([], ['input'])} edit={apply => actor.send({ type: 'EDIT', apply })} /></DraftOwner>;
}
it('explicitly creates/omits optional array items and preserves exact detached item drafts', async () => {
  const document = fixture(); document.block.input = { type: 'array' };
  const actor = await actorFor(document), ui = render(<SchemaHarness actor={actor} />);
  expect(actor.getSnapshot().context.document).toEqual(document); expect(actor.getSnapshot().context.past).toHaveLength(0);
  expect(screen.queryByText('Array item contract', { selector: 'h3 span' })).toBeNull();
  fireEvent.click(screen.getByRole('checkbox', { name: 'Constrain array items' }));
  expect(actor.getSnapshot().context.document!.block.input).toEqual({ type: 'array', items: { type: 'string' } });
  const items = ui.container.querySelector('[data-schema-label="Array item contract"]')!;
  fireEvent.change(uiWithin(items as HTMLElement).getByLabelText('Type', { exact: true }), { target: { value: 'object' } });
  fireEvent.click(uiWithin(items as HTMLElement).getByRole('button', { name: 'Advanced JSON' }));
  const text = '{ "type": "object",\n "properties": { unfinished';
  fireEvent.change(uiWithin(items as HTMLElement).getByLabelText('Array item contract', { exact: true }), { target: { value: text } });
  const key = encodeField(within(blockField([], ['input']), ['items']));
  expect(actor.getSnapshot().context.drafts[key].text).toBe(text);
  fireEvent.click(screen.getByRole('checkbox', { name: 'Constrain array items' }));
  expect(actor.getSnapshot().context.document!.block.input).toEqual({ type: 'array' });
  const detached = Object.entries(actor.getSnapshot().context.drafts).find(([, draft]) => draft.text === text)!;
  expect(detached[0]).not.toBe(key); expect(detached[1]).toMatchObject({ text, conflict: 'missing' });
  await act(async () => actor.send({ type: 'UNDO' }));
  expect(actor.getSnapshot().context.drafts[detached[0]]).toMatchObject({ text, conflict: 'missing' });
  expect(actor.getSnapshot().context.document!.block.input).toMatchObject({ type: 'array', items: { type: 'object' } });
  expect(fieldValue(actor.getSnapshot().context.document!, within(blockField([], ['input']), ['items']))?.value).toMatchObject({ type: 'object' });
  await act(async () => actor.send({ type: 'REDO' }));
  expect(actor.getSnapshot().context.drafts[detached[0]].text).toBe(text);
  ui.unmount(); actor.stop();
});

it('sends the original search with every titled producer to the bounded native query', async () => {
  const document = fixture(); document.block.nodes.done = { type: 'return', output: { type: 'reference', path: ['args'] } };
  const titled = setNodePresentation(setNodePresentation(addNode(addNode(document, [], 'model', 'first'), [], 'model', 'second'), [], 'first', { title: 'First Draft' }), [], 'second', { title: 'Second Draft' });
  const queries: string[] = [], actor = await actorFor(titled, async (_id, _document, _generation, query) => { queries.push(query!.search ?? ''); return new Promise(() => {}); });
  render(<DraftOwner value={{ editor: actor, commits: new FieldCommits() }}><ReferencePicker address={nodeField([], 'done', ['output'])} path={['args']} edit={apply => actor.send({ type: 'EDIT', apply })} /></DraftOwner>);
  fireEvent.click(screen.getByRole('button', { name: 'Choose a variable' }));
  fireEvent.change(screen.getByRole('textbox', { name: 'Search reference paths…' }), { target: { value: 'Draft' } });
  await act(async () => { await Promise.resolve(); });
  expect(queries.at(-1)).toBe('Draft'); actor.stop();
});

it.each(['root Return', 'private Return', 'Loop input', 'Loop until', 'Loop carry'])('labels a selected result reference by its exact %s consumer scope', async kind => {
  let document = addNode(addNode(fixture(), [], 'model', 'result'), [], 'loop', 'repeat');
  document = setNodePresentation(document, [], 'result', { title: 'Actual result producer' });
  const body = [{ node: 'repeat' }];
  document = addNode(document, body, 'model', 'result');
  document = setNodePresentation(document, body, 'result', { title: 'Private result producer' });
  let address = nodeField([], 'done', ['output']);
  if (kind === 'private Return') address = nodeField(body, 'done', ['output']);
  if (kind === 'Loop input') address = nodeField([], 'repeat', ['input']);
  if (kind === 'Loop until') {
    document = setField(document, nodeField([], 'repeat', ['until']), { type: 'equal', left: { type: 'reference', path: ['result'] }, right: { type: 'literal', value: {} } });
    address = nodeField([], 'repeat', ['until', 'left']);
  }
  if (kind === 'Loop carry') {
    document = setField(document, nodeField([], 'repeat', ['carry']), { type: 'object', fields: { echo: { type: 'reference', path: ['result'] } } });
    address = nodeField([], 'repeat', ['carry', 'fields', { key: 'echo' }]);
  }
  document = setField(document, address, { type: 'reference', path: ['result'] });
  const actor = await actorFor(document);
  render(<DraftOwner value={{ editor: actor, commits: new FieldCommits() }}><ReferencePicker address={address} path={['result']} edit={apply => actor.send({ type: 'EDIT', apply })} /></DraftOwner>);
  expect(screen.getByRole('button', { name: 'Choose a variable' }).textContent).toBe(kind === 'Loop until' || kind === 'Loop carry' ? 'Committed body result' : kind === 'private Return' ? 'Private result producer' : 'Actual result producer');
  expect(actor.getSnapshot().context.document).toBe(document); expect(actor.getSnapshot().context.past).toHaveLength(0);
  actor.stop();
});

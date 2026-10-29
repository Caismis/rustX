import { afterEach, describe, expect, it } from 'vitest';
import { act, cleanup, fireEvent, render, screen, within as uiWithin } from '@testing-library/react';
import { useSelector } from '@xstate/react';
import { createActor, waitFor } from 'xstate';
import { addBranch, addNode, emptyBlock } from '../src/app/workflows/document/commands';
import { copyFragment, duplicateFragment, parseFragment, pasteFragment } from '../src/app/workflows/document/clipboard';
import { blockField, encodeField, fieldValue, nodeField, within, type FieldAddress } from '../src/app/workflows/document/fields';
import { putNote, setLane, setNodePresentation, setOrder } from '../src/app/workflows/document/geometry';
import { sameBlockPath, type BlockPath, type WorkflowDocument } from '../src/app/workflows/document/model';
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

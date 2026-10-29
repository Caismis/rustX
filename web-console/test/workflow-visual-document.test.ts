import { describe, expect, it } from 'vitest';
import { createActor } from 'xstate';
import { addNode, connectAddressed, emptyBlock, insertNode, nodeTemplate, renameNode } from '../src/app/workflows/document/commands';
import { copyFragment, fragmentOwnsField, parseFragment, pasteFragment } from '../src/app/workflows/document/clipboard';
import { blockField, encodeField, nodeField, renameAddress } from '../src/app/workflows/document/fields';
import { nodePresentation, putNote, setNodePresentation } from '../src/app/workflows/document/geometry';
import { blockAt, sameDocument, type WorkflowDocument } from '../src/app/workflows/document/model';
import { renameSchemaField, schemaConsumers, schemaPropertyAddress } from '../src/app/workflows/document/schema';
import { workflowEditorMachine } from '../src/app/workflows/machines/editor';
import type { WorkflowScopePort } from '../src/app/workflows/port';
import { LayoutSession } from '../src/app/workflows/ui/editor/layout';
import { exampleSource } from './workflow-fixtures';

const fixture = (): WorkflowDocument => ({ description: 'Visual document fixture', block: emptyBlock() });
const deferred = <T>() => { let resolve!: (value: T) => void; let reject!: (reason: Error) => void; const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };

describe('qualified scene ownership', () => {

  it('moves a container without changing child-relative geometry or lexical ownership', () => {
    const path = [{ node: 'repeat' }];
    let document = addNode(fixture(), [], 'loop', 'repeat');
    document = setNodePresentation(document, path, 'done', { position: { x: 100, y: 180 } });
    const moved = setNodePresentation(document, [], 'repeat', { position: { x: 700, y: 300 } });
    expect(nodePresentation(moved, path, 'done')?.position).toEqual({ x: 100, y: 180 });
    expect(moved.block).toBe(document.block);
    expect(() => connectAddressed(moved, { block: [], node: 'repeat', port: 'satisfied' }, { block: path, node: 'done' })).toThrow('cross_scope');
  });
});

describe('atomic insertion and bounded clipboard', () => {
  it('preserves an edge continuation in one transaction and refuses multi-exit or terminal guesses', () => {
    const document = fixture();
    document.block.nodes = { read: nodeTemplate('tool'), done: nodeTemplate('return') };
    document.block.edges = [{ from: 'read', to: 'done' }];
    const next = insertNode(document, [], 0, 'model', 'summarize');
    expect(next.block.edges).toEqual([{ from: 'read', to: 'summarize' }, { from: 'summarize', to: 'done' }]);
    for (const kind of ['branch', 'loop', 'return'] as const) expect(() => insertNode(document, [], 0, kind, kind)).toThrow('unsupported_insertion');
    expect(document.block.edges).toEqual([{ from: 'read', to: 'done' }]);
    expect(Object.keys(document.block.nodes)).toEqual(['read', 'done']);
  });

  it('remaps selected producers and owned metadata deterministically while preserving private locals', () => {
    let document = addNode(fixture(), [], 'model', 'first');
    document = addNode(document, [], 'loop', 'repeat');
    const repeat = document.block.nodes.repeat;
    if (repeat.type !== 'loop') throw new Error('loop fixture');
    repeat.input = { type: 'object', fields: { text: { type: 'reference', path: ['first'] } } };
    document = setNodePresentation(document, [], 'repeat', { title: 'Refine', position: { x: 500, y: 100 } });
    document = putNote(document, { id: 'explain', block: [{ node: 'repeat' }], text: '**Private**', theme: 'blue', position: { x: 32, y: 300 }, dimensions: { width: 240, height: 120 } });
    const fragment = parseFragment(copyFragment(document, 'file-owner', [], ['first', 'repeat']));
    const next = pasteFragment(document, 'file-owner', [], fragment).document;
    expect(next.block.nodes.repeat_copy).toMatchObject({ input: { fields: { text: { path: ['first_copy'] } } } });
    expect(blockAt(next, [{ node: 'repeat_copy' }])!.nodes.done).toEqual(repeat.body.nodes.done);
    expect(nodePresentation(next, [], 'repeat_copy')).toMatchObject({ title: 'Refine', position: { x: 532, y: 132 } });
    expect(next.editor?.notes).toHaveLength(2);
    expect(next.editor?.notes?.[1]).toMatchObject({ id: 'explain_2', block: [{ node: 'repeat_copy' }], text: '**Private**', position: { x: 32, y: 300 } });
    const twice = pasteFragment(next, 'file-owner', [], parseFragment(copyFragment(next, 'file-owner', [], ['repeat_copy']))).document;
    expect(twice.editor?.notes?.[2]).toMatchObject({ block: [{ node: 'repeat_copy_copy' }], position: { x: 32, y: 300 } });
    expect(sameDocument(pasteFragment(document, 'file-owner', [], fragment).document, next)).toBe(true);
    // A copied fragment can move scopes only when all of its external root
    // references are supplied by copied producers. Private args stay private.
    expect(() => pasteFragment(document, 'other-file', [], fragment)).not.toThrow();
    const partial = parseFragment(copyFragment(document, 'file-owner', [], ['repeat']));
    expect(() => pasteFragment(document, 'other-file', [], partial)).toThrow('cross_scope');
    expect(() => pasteFragment(document, 'file-owner', [{ node: 'repeat' }], partial)).toThrow('cross_scope');
  });

  it('refuses missing selections, destination Block adoption and out-of-bounds paste offsets atomically', () => {
    const original = fixture();
    expect(() => copyFragment(original, 'owner', [], ['absent'])).toThrow('missing_node');
    const fragment = parseFragment(copyFragment(original, 'owner', [], ['done']));
    expect(() => pasteFragment(original, 'owner', [], { ...fragment, editor: { orders: [{ target: { kind: 'block_input', block: [], path: [] }, fields: [] }] } })).toThrow('invalid_fragment');
    const positioned = setNodePresentation(original, [], 'done', { position: { x: 1_000_000, y: 0 } });
    expect(() => pasteFragment(original, 'owner', [], parseFragment(copyFragment(positioned, 'owner', [], ['done'])))).toThrow('invalid_geometry');
    expect(Object.keys(original.block.nodes)).toEqual(['start', 'done']);
    const note = putNote(original, { id: 'args', block: [], text: '', theme: 'neutral', position: { x: 0, y: 0 }, dimensions: { width: 200, height: 100 } });
    expect(parseFragment(copyFragment(note, 'owner', [], [], ['args'])).editor.notes?.[0].id).toBe('args');
    expect(fragmentOwnsField([], ['loop'], [], blockField([{ node: 'loop' }], ['input']))).toBe(true);
    expect(fragmentOwnsField([], ['loop'], [], nodeField([{ node: 'other' }], 'loop', ['input']))).toBe(false);
  });

  it('rejects malformed shapes, runtime payloads, non-finite or oversized geometry and rounded JSON before mutation', () => {
    const fragment = JSON.parse(copyFragment(fixture(), 'owner', [], ['done']));
    const invalid = [
      { ...fragment, nodes: { done: { type: 'return', output: { type: 'eval', source: 'alert(1)' } } } },
      { ...fragment, runtime: { credentials: 'not accepted' } },
      { ...fragment, editor: { nodes: [{ address: { block: [], node: 'done' }, title: 'x'.repeat(257) }] } },
      { ...fragment, editor: { nodes: [{ address: { block: [], node: 'done' } }, { address: { node: 'done', block: [] } }] } },
      { ...fragment, editor: { entries: [{ block: [], position: { x: 0, y: 0, runtime: 1 } }] } },
      { ...fragment, editor: { orders: [{ target: { kind: 'node_input', address: { block: [], node: 'done' }, path: [] }, fields: [] }] } },
      { ...fragment, nodes: { done: { ...fragment.nodes.done, execution: { result: 1 } } } },
      { ...fragment, editor: { notes: [{ id: 'bad', block: [], text: '', theme: 'amber', position: { x: 0, y: 0 }, dimensions: { width: 9000, height: 120 } }] } },
    ];
    for (const value of invalid) expect(() => parseFragment(JSON.stringify(value))).toThrow();
    expect(() => parseFragment(JSON.stringify(fragment).replace('"value":{}', '"value":9007199254740993'))).toThrow();
    expect(() => parseFragment(' '.repeat(768 * 1024 + 1))).toThrow('limit_exceeded');
    expect(() => parseFragment('{"a":'.repeat(140) + '0' + '}'.repeat(140))).toThrow('limit_exceeded');
  });
});

describe('schema contract identity', () => {

  it('renames root arguments, required/order metadata and only corresponding structured paths', () => {
    const document = fixture();
    document.block.input = { type: 'object', properties: { question: { type: 'string', description: 'User question' } }, required: ['question'], additionalProperties: false };
    document.block.nodes = { model: { type: 'model', messages: [{ role: 'system', content: [{ type: 'literal', value: 'Keep question in literal text.' }] }, { role: 'user', content: [{ type: 'reference', path: ['args', 'question'] }] }], output: { type: 'text' } }, done: { type: 'return', output: { type: 'reference', path: ['args'] } } };
    const next = renameSchemaField(document, blockField([], ['input']), 'question', 'prompt');
    expect(next.block.input).toMatchObject({ properties: { prompt: { description: 'User question' } }, required: ['prompt'] });
    expect(next.block.nodes.model).toMatchObject({ messages: [{ content: [{ value: 'Keep question in literal text.' }] }, { content: [{ path: ['args', 'prompt'] }] }] });
    expect(next.block.nodes.done).toMatchObject({ output: { path: ['args'] } });
    expect(next.editor?.orders?.[0].fields).toEqual(['prompt']);
    expect(schemaConsumers(next, blockField([], ['input'])).map(use => use.node).sort()).toEqual(['done', 'model']);
    const before = schemaPropertyAddress(blockField([], ['input']), 'question');
    const address = blockField([], ['input', 'properties', { key: 'question' }, 'description']);
    expect(renameAddress(address, before, 'prompt').path).toEqual(['input', 'properties', { key: 'prompt' }, 'description']);
  });

  it('refuses schema property rename into retained orphan list fields or descendant order targets', () => {
    const document = fixture();
    document.block.input = { type: 'object', properties: { from: { type: 'object', properties: { leaf: { type: 'string' } } } }, additionalProperties: false };
    const target = { kind: 'block_input' as const, block: [], path: [] };
    for (const orders of [
      [{ target, fields: ['to', 'from'] }],
      [{ target: { ...target, path: [{ kind: 'property' as const, name: 'to' }] }, fields: ['leaf'] }],
    ]) {
      const original = { ...document, editor: { orders } }, before = structuredClone(original);
      expect(() => renameSchemaField(original, blockField([], ['input']), 'from', 'to')).toThrow('duplicate_identifier');
      expect(original).toEqual(before);
    }
    const separate = { ...document, editor: { orders: [{ target: { kind: 'block_output' as const, block: [], path: [] }, fields: ['to'] }] } };
    expect(() => renameSchemaField(separate, blockField([], ['input']), 'from', 'to')).not.toThrow();
  });

  it('renames body outputs in Return constructions, post-body result scope and parent aggregate consumers', () => {
    let document = addNode(fixture(), [], 'loop', 'repeat');
    const repeat = document.block.nodes.repeat;
    if (repeat.type !== 'loop') throw new Error('loop fixture');
    repeat.body.output = { type: 'object', properties: { ready: { type: 'boolean' } }, required: ['ready'], additionalProperties: false };
    repeat.body.nodes.done = { type: 'return', output: { type: 'object', fields: { ready: { type: 'literal', value: true } } } };
    repeat.until = { type: 'boolean', value: { type: 'reference', path: ['result', 'ready'] } };
    document.block.nodes.done = { type: 'return', output: { type: 'reference', path: ['repeat', 'result', 'ready'] } };
    document = renameSchemaField(document, blockField([{ node: 'repeat' }], ['output']), 'ready', 'accepted');
    expect(document.block.nodes.repeat).toMatchObject({ until: { value: { path: ['result', 'accepted'] } }, body: { nodes: { done: { output: { fields: { accepted: { value: true } } } } } } });
    expect(document.block.nodes.done).toMatchObject({ output: { path: ['repeat', 'result', 'accepted'] } });
  });
});

describe('organize generation and native capacity', () => {

  it('discards results after a newer request, drag, target document change or cancellation with controlled promises', async () => {
    const session = new LayoutSession(), document = fixture();
    const owner = { document, generation: 0 };
    let current = owner;
    const old = deferred<WorkflowDocument>(), fresh = deferred<WorkflowDocument>();
    const first = session.run(owner, () => current, () => old.promise), second = session.run(owner, () => current, () => fresh.promise);
    fresh.resolve(document); expect(await second).toBe(document);
    old.resolve(document); expect(await first).toBeUndefined();
    const drag = deferred<WorkflowDocument>(), request = session.run(owner, () => current, () => drag.promise);
    current = { document: setNodePresentation(document, [], 'done', { position: { x: 900, y: 400 } }), generation: 1 };
    drag.resolve(document); expect(await request).toBeUndefined();
    current = owner;
    const closed = deferred<WorkflowDocument>(), closing = session.run(owner, () => current, () => closed.promise);
    session.cancel(); closed.resolve(document); expect(await closing).toBeUndefined();
  });

  it('discards both completion and failure when Edit then Undo restores the exact same document object', async () => {
    const session = new LayoutSession(), document = fixture(), owner = { document, generation: 0 };
    let current = owner;
    const result = deferred<WorkflowDocument>(), request = session.run(owner, () => current, () => result.promise);
    current = { document: setNodePresentation(document, [], 'done', { position: { x: 900, y: 400 } }), generation: 1 };
    current = { document, generation: 2 };
    expect(current.document).toBe(owner.document);
    result.resolve(document); expect(await request).toBeUndefined();
    const failure = deferred<WorkflowDocument>(), failed = session.run(current, () => current, () => failure.promise);
    current = { document, generation: 4 };
    failure.reject(new Error('obsolete layout failed')); expect(await failed).toBeUndefined();
  });
});

it('keeps bounded metadata text drafts and their identity through node rename Undo/Redo', async () => {
  const source = exampleSource('typed_agent');
  const port: WorkflowScopePort = { endpoint: 'ws://fixture', scope: { kind: 'user' }, read: async () => source, list: async () => { throw new Error('unused'); }, analyze: () => new Promise(() => {}), modelChoices: () => new Promise(() => {}), check: () => new Promise(() => {}), write: () => { throw new Error('pending drafts must not write'); } };
  const actor = createActor(workflowEditorMachine, { input: { port, target: { endpoint: port.endpoint, scope: port.scope, id: source.id } } }).start();
  await Promise.resolve(); await Promise.resolve();
  const id = Object.keys(actor.getSnapshot().context.document!.block.nodes)[0];
  const address = { ...nodeField([], id, ['description']), domain: { kind: 'node_presentation' as const } };
  actor.send({ type: 'DRAFT', address, text: JSON.stringify('x'.repeat(4097)) });
  const identity = nodeField([], id, [], 'key');
  actor.send({ type: 'DRAFT', address: identity, text: 'renamed' });
  actor.send({ type: 'APPLY_DRAFT', field: encodeField(identity), version: actor.getSnapshot().context.drafts[encodeField(identity)].version, command: document => ({ status: 'applied', document: renameNode(document, [], id, 'renamed') }) });
  const renamed = { ...address, node: 'renamed' };
  expect(actor.getSnapshot().context.drafts[encodeField(renamed)].text).toBe(JSON.stringify('x'.repeat(4097)));
  actor.send({ type: 'UNDO' });
  expect(actor.getSnapshot().context.drafts[encodeField(address)].conflict).toBeUndefined();
  actor.send({ type: 'REDO' });
  expect(actor.getSnapshot().context.drafts[encodeField(renamed)].conflict).toBeUndefined();
  actor.send({ type: 'SAVE' });
  expect(actor.getSnapshot().context.notice).toEqual({ kind: 'pending_fields', count: 1 });
  const draft = actor.getSnapshot().context.drafts[encodeField(renamed)];
  expect(draft.text.length).toBe(4099);
  actor.stop();
});

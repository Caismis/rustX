import { describe, expect, it } from 'vitest';
import { EXAMPLES, exampleSource } from './workflow-fixtures';
import {
  addBranch, addNode, CommandRejected, connect, deleteNodes, disconnect, reconnect, removeBranch, renameBranch,
  renameNode, replaceNode, setEntry, setWorkflow,
} from '../src/app/workflows/document/commands';
import { locate } from '../src/app/workflows/document/diagnostics';
import {
  blockAt, blockKey, childSteps, existingPath, nativeBlockPath, sameDocument, ROOT, type BlockPath, type WorkflowDocument,
} from '../src/app/workflows/document/model';
import { blockField, encodeField, fieldText, nodeField, renameAddress, setField } from '../src/app/workflows/document/fields';
import { after, defaultLayout, layout } from '../src/app/workflows/ui/editor/layout';
import { edgePort } from '../src/app/workflows/document/ports';
import { blockReferences } from '../src/app/workflows/document/references';
import { readJson } from '../src/app/workflows/ui/editor/forms/drafts';

/** The six native examples exactly as the App Server returns them. */
function example(id: string): WorkflowDocument {
  return exampleSource(id).document!;
}

function rejection(run: () => unknown) {
  try { run(); } catch (cause) { if (cause instanceof CommandRejected) return cause.code; throw cause; }
  return undefined;
}

/** Every lexical block of a document, depth first. */
function blocks(document: WorkflowDocument, path: BlockPath = ROOT): BlockPath[] {
  const block = blockAt(document, path)!;
  return [path, ...Object.entries(block.nodes).flatMap(([id, node]) => childSteps(id, node).flatMap(step => blocks(document, [...path, step])))];
}

describe('native Workflow editing document', () => {
  it('keeps every example semantically unchanged by projection and inverse edits', () => {
    for (const id of EXAMPLES) {
      const original = example(id);
      for (const path of blocks(original)) {
        const block = blockAt(original, path)!;
        for (const node of Object.keys(block.nodes)) {
          // Rename there and back: identities, edges, entry and references
          // return exactly; nothing else in the program moved.
          const renamed = renameNode(original, path, node, `${node}_renamed`);
          expect(sameDocument(renamed, original)).toBe(false);
          expect(blockAt(renamed, path)!.nodes[node]).toBeUndefined();
          expect(sameDocument(renameNode(renamed, path, `${node}_renamed`, node), original)).toBe(true);
        }
      }
      // Untouched advanced fields survive every structural edit elsewhere.
      const edited = setWorkflow(original, { description: 'changed', timeout_ms: original.timeout_ms });
      expect(sameDocument({ ...edited, description: original.description }, original)).toBe(true);
    }
  });

  it('renames update structured references in the owning lexical scope only, never text', () => {
    const document = example('implement_and_review');
    const renamed = renameNode(document, ROOT, 'plan', 'proposal');
    const root = blockAt(renamed, ROOT)!;
    const review = root.nodes.review_plan;
    expect(review.type === 'review' && review.subject.value).toEqual({ type: 'reference', path: ['proposal'] });
    const loop = root.nodes.repair;
    expect(loop.type === 'loop' && loop.input.type === 'object' && loop.input.fields.plan).toEqual({ type: 'reference', path: ['proposal'] });
    expect(root.edges!.some(edge => edge.from === 'proposal' && edge.to === 'review_plan')).toBe(true);
    expect(root.edges!.some(edge => edge.to === 'proposal')).toBe(true);
    // Task prose that mentions the word is program data, not a reference.
    const plan = root.nodes.proposal;
    expect(plan.type === 'agent' && plan.task).toContain('implementation plan');
    // The Loop body's private `args.plan` is a different lexical value.
    const body = blockAt(renamed, [{ node: 'repair' }])!;
    expect(blockReferences(body)).toContainEqual(['args', 'plan']);
    expect(sameDocument(body, blockAt(document, [{ node: 'repair' }]))).toBe(true);

    // Renaming the entry node moves the entry marker with it.
    expect(blockAt(renameNode(document, ROOT, 'intake', 'gather'), ROOT)!.entry).toBe('gather');

    // Branch keys rewrite references into the Parallel's keyed output.
    const reviews = renameBranch(example('parallel_review'), ROOT, 'reviews', 'quality', 'clarity');
    const decision = blockAt(reviews, ROOT)!.nodes.decision;
    expect(decision.type === 'branch' && decision.condition.type === 'and' && decision.condition.predicates[0]).toEqual({ type: 'boolean', value: { type: 'reference', path: ['reviews', 'clarity', 'acceptable'] } });
    expect(blockAt(reviews, [{ node: 'reviews', branch: 'clarity' }])).toBeDefined();
    expect(blockAt(reviews, [{ node: 'reviews', branch: 'quality' }])).toBeUndefined();
  });

  it('refuses invalid, reserved and colliding identities without changing the document', () => {
    const document = example('parallel_review');
    expect(rejection(() => renameNode(document, ROOT, 'decision', 'ready'))).toBe('duplicate_identifier');
    expect(rejection(() => renameNode(document, ROOT, 'decision', 'args'))).toBe('reserved_identifier');
    expect(rejection(() => renameNode(document, ROOT, 'decision', 'has.dot'))).toBe('invalid_identifier');
    expect(rejection(() => renameNode(document, ROOT, 'decision', ''))).toBe('invalid_identifier');
    expect(rejection(() => renameBranch(document, ROOT, 'reviews', 'quality', 'security'))).toBe('duplicate_identifier');
    expect(rejection(() => addNode(document, ROOT, 'agent', 'decision'))).toBe('duplicate_identifier');
    expect(rejection(() => removeBranch(addBranch(document, ROOT, 'reviews', 'extra'), ROOT, 'reviews', 'missing'))).toBe('missing_branch');
  });

  it('enforces native ports and the single-successor rule as canvas ergonomics', () => {
    let document = example('parallel_review');
    expect(rejection(() => connect(document, ROOT, 'ready', 'next', 'revise'))).toBe('return_has_no_successor');
    expect(rejection(() => connect(document, ROOT, 'decision', 'next', 'ready'))).toBe('invalid_port');
    expect(rejection(() => connect(document, ROOT, 'decision', 'true', 'revise'))).toBe('port_occupied');
    expect(rejection(() => connect(document, ROOT, 'reviews', 'next', 'reviews'))).toBe('self_edge');
    const edges = blockAt(document, ROOT)!.edges!;
    const trueEdge = edges.findIndex(edge => edgePort(edge) === 'true');
    document = disconnect(document, ROOT, [trueEdge]);
    document = connect(document, ROOT, 'decision', 'true', 'revise');
    expect(blockAt(document, ROOT)!.edges!.at(-1)).toEqual({ from: 'decision', to: 'revise', port: 'true' });
    const moved = reconnect(document, ROOT, blockAt(document, ROOT)!.edges!.length - 1, 'decision', 'true', 'ready');
    expect(blockAt(moved, ROOT)!.edges!.at(-1)).toEqual({ from: 'decision', to: 'ready', port: 'true' });
    // Ordinary edges omit the port, exactly as authored natively.
    expect(rejection(() => addNode(moved, ROOT, 'return', 'finish', { from: 'ready', port: 'next' }))).toBe('return_has_no_successor');
    expect(rejection(() => addNode(moved, ROOT, 'return', 'finish', { from: 'reviews', port: 'next' }))).toBe('port_occupied');
    const fresh = disconnect(moved, ROOT, [blockAt(moved, ROOT)!.edges!.findIndex(edge => edge.from === 'reviews')]);
    expect(blockAt(addNode(fresh, ROOT, 'return', 'finish', { from: 'reviews', port: 'next' }), ROOT)!.edges!.at(-1)).toEqual({ from: 'reviews', to: 'finish' });
  });

  it('deletes without rewiring: dangling entries and references remain for the native check', () => {
    const document = example('typed_agent');
    const root = blockAt(document, ROOT)!;
    const entry = root.entry;
    const deleted = deleteNodes(document, ROOT, [entry]);
    expect(blockAt(deleted, ROOT)!.entry).toBe(entry);
    expect(blockAt(deleted, ROOT)!.edges!.some(edge => edge.from === entry || edge.to === entry)).toBe(false);
    expect(rejection(() => deleteNodes(document, ROOT, Object.keys(root.nodes)))).toBe('last_node');
  });

  it('edits nested blocks one lexical scope at a time', () => {
    let document = example('parallel_review');
    const quality: BlockPath = [{ node: 'reviews', branch: 'quality' }];
    expect(blockKey(quality)).toBe('reviews/quality');
    expect(blockKey([{ node: 'repair' }])).toBe('repair/body');
    expect(nativeBlockPath(quality)).toBe('block.nodes.reviews.branches.quality.block');
    document = addNode(document, quality, 'return', 'early');
    expect(blockAt(document, quality)!.nodes.early.type).toBe('return');
    expect(blockAt(document, ROOT)!.nodes.early).toBeUndefined();
    expect(blockAt(document, [{ node: 'reviews', branch: 'security' }])!.nodes.early).toBeUndefined();
    // A nested node cannot connect to a sibling block's node.
    expect(rejection(() => connect(document, quality, 'early', 'next', 'review'))).toBe('return_has_no_successor');
    expect(rejection(() => connect(document, quality, 'review', 'next', 'decision'))).toBe('missing_node');
    document = setEntry(document, quality, 'refine');
    expect(blockAt(document, quality)!.entry).toBe('refine');
    document = setField(document, blockField(quality, ['output']), { type: 'object', properties: {}, additionalProperties: false });
    expect(blockAt(document, quality)!.output).toEqual({ type: 'object', properties: {}, additionalProperties: false });
    // Removing the owning branch leaves the editor at the nearest live block.
    const removed = removeBranch(document, ROOT, 'reviews', 'quality');
    expect(existingPath(removed, quality)).toEqual([]);
  });

  it('replaces a node only with a definition of the same native kind', () => {
    const document = example('typed_agent');
    const [id, node] = Object.entries(blockAt(document, ROOT)!.nodes).find(([, node]) => node.type === 'agent')!;
    expect(node.type).toBe('agent');
    expect(rejection(() => replaceNode(document, ROOT, id, { type: 'return', output: { type: 'literal', value: {} } }))).toBe('missing_node');
    const edited = replaceNode(document, ROOT, id, { ...node, task: 'Different task' } as typeof node);
    const replaced = blockAt(edited, ROOT)!.nodes[id];
    expect(replaced.type === 'agent' && replaced.task).toBe('Different task');
  });

  it('maps native diagnostic paths to their block, node, edge and field', () => {
    expect(locate('$')).toEqual({ block: [], field: undefined });
    expect(locate('description')).toEqual({ block: [], field: 'description' });
    expect(locate('block.entry')).toEqual({ block: [], field: 'entry' });
    expect(locate('block.nodes.review.input.proposal')).toEqual({ block: [], node: 'review', field: 'input.proposal' });
    expect(locate('block.edges.2.to')).toEqual({ block: [], edge: 2, field: 'to' });
    expect(locate('block.nodes.reviews.branches.quality.block.edges.0.to')).toEqual({ block: [{ node: 'reviews', branch: 'quality' }], edge: 0, field: 'to' });
    expect(locate('block.nodes.repair.body.nodes.check.result.schema')).toEqual({ block: [{ node: 'repair' }], node: 'check', field: 'result.schema' });
    expect(locate('block.nodes.repair.body.entry')).toEqual({ block: [{ node: 'repair' }], field: 'entry' });
    expect(locate('block.nodes.repair.max_iterations')).toEqual({ block: [], node: 'repair', field: 'max_iterations' });
    expect(locate('block.nodes.reviews.branches.quality.input')).toEqual({ block: [], node: 'reviews', field: 'branches.quality.input' });
  });
});

describe('authored maps and the browser number contract', () => {
  it('treats a valid identifier that names an inherited property as free, and still rejects real duplicates', () => {
    const document = example('parallel_review');
    const added = addNode(document, ROOT, 'return', 'constructor');
    expect(Object.hasOwn(blockAt(added, ROOT)!.nodes, 'constructor')).toBe(true);
    expect(rejection(() => addNode(added, ROOT, 'return', 'constructor'))).toBe('duplicate_identifier');
    const renamed = renameNode(document, ROOT, 'ready', 'constructor');
    expect(Object.keys(blockAt(renamed, ROOT)!.nodes)).toContain('constructor');
    expect(rejection(() => renameNode(renamed, ROOT, 'revise', 'constructor'))).toBe('duplicate_identifier');
    expect(rejection(() => setEntry(document, ROOT, 'constructor'))).toBe('missing_node');
    const branched = addBranch(document, ROOT, 'reviews', 'constructor');
    expect(Object.hasOwn((blockAt(branched, ROOT)!.nodes.reviews as { branches: object }).branches, 'constructor')).toBe(true);
    expect(rejection(() => renameBranch(document, ROOT, 'reviews', 'constructor', 'quality_2'))).toBe('missing_branch');
    expect(blockAt(branched, [{ node: 'reviews', branch: 'constructor' }])).toBeDefined();
    expect(blockAt(document, [{ node: 'reviews', branch: 'constructor' }])).toBeUndefined();
  });

  it('accepts exactly the numbers a JSON round trip keeps, judged from the authored token', () => {
    expect(readJson('{"n": 9007199254740991, "f": 1.5, "e": 1e21, "z": 0}')).toEqual({ ok: true, value: { n: 9007199254740991, f: 1.5, e: 1e21, z: 0 } });
    for (const text of ['9007199254740993', '[9007199254740992]', '1.0', '{"a": 2.50e1}', '-0.0', '1e400']) {
      expect(readJson(text), text).toEqual({ ok: false, reason: 'number' });
    }
    expect(readJson('{"a": ')).toEqual({ ok: false, reason: 'syntax' });
    // What is accepted serializes back to the very token native receives.
    const accepted = readJson('[2.5, -0.25, 9007199254740991, 1e21]');
    expect(accepted.ok && JSON.stringify(accepted.value)).toBe('[2.5,-0.25,9007199254740991,1e+21]');
  });

  it('refuses negative zero in every spelling and position before an integer rule accepts it', () => {
    // `-0` is an integer token and `Number.isSafeInteger(-0)` holds, yet it
    // would be written as `0`.
    for (const text of ['-0', '-0.0', '-0e0', '-0E+5', '-0.000e-3', '[1, -0]', '{"a": {"b": [2, -0]}}', '{"n": -0, "ok": 1}']) {
      expect(readJson(text), text).toEqual({ ok: false, reason: 'number' });
    }
    for (const [text, value] of [['0', 0], ['-1', -1], ['-9007199254740991', -9007199254740991], ['-0.5', -0.5], ['3.14', 3.14], ['1e-7', 1e-7], ['{"a": [0, -2.5]}', { a: [0, -2.5] }]] as const) {
      const reading = readJson(text);
      expect(reading, text).toEqual({ ok: true, value });
      expect(reading.ok && Object.is(reading.value, -0)).toBe(false);
    }
    for (const text of ['9007199254740992', '-9007199254740992', '1.0', '1e3', 'NaN', '1e400']) expect(readJson(text).ok, text).toBe(false);
  });

  it('places a node named like an inherited property from its own cache entry or its default', () => {
    const document = addNode(example('parallel_review'), ROOT, 'return', 'constructor');
    const block = blockAt(document, ROOT)!;
    // A cache written before the node existed holds other nodes only.
    const stored = { ready: { x: 900, y: 40 } };
    const positions = layout(block, stored);
    expect(positions.ready).toEqual({ x: 900, y: 40 });
    for (const id of ['constructor', ...Object.keys(block.nodes)]) {
      expect(Object.hasOwn(positions, id), id).toBe(true);
      expect(Number.isFinite(positions[id].x) && Number.isFinite(positions[id].y), id).toBe(true);
    }
    const own = positions['constructor' as string];
    expect(own).toEqual(defaultLayout(block)['constructor' as string]);
    const next = after(block, positions, 'constructor', 'next');
    expect(next.x).toBe(own.x + 320);
    expect(Number.isFinite(next.y)).toBe(true);
  });
});

describe('field addresses', () => {
  const fixture = (): WorkflowDocument => {
    const document = example('parallel_review');
    return replaceNode(document, ROOT, 'ready', {
      type: 'return',
      output: { type: 'object', fields: {
        'a.fields.b': { type: 'literal', value: 1 },
        a: { type: 'object', fields: { b: { type: 'literal', value: 2 } } },
      } },
    });
  };
  const direct = nodeField(ROOT, 'ready', ['output', 'fields', { key: 'a.fields.b' }, 'value']);
  const nested = nodeField(ROOT, 'ready', ['output', 'fields', { key: 'a' }, 'fields', { key: 'b' }, 'value']);

  it('keeps a direct key containing dots apart from the nested path it spells', () => {
    expect(encodeField(direct)).not.toBe(encodeField(nested));
    // An authored key is never a structural component of the same name.
    expect(encodeField(nodeField(ROOT, 'n', ['output', 'fields', { key: 'fields' }]))).not.toBe(encodeField(nodeField(ROOT, 'n', ['output', 'fields', 'fields'])));
    const document = fixture();
    expect(fieldText(document, direct)).toBe('1');
    expect(fieldText(document, nested)).toBe('2');
    const edited = setField(document, direct, 3);
    expect(fieldText(edited, direct)).toBe('3');
    expect(fieldText(edited, nested)).toBe('2');
  });

  it('resolves only own entries and existing items, and follows renames', () => {
    const document = fixture();
    expect(fieldText(document, nodeField(ROOT, 'constructor', ['output']))).toBeUndefined();
    expect(fieldText(document, nodeField(ROOT, 'ready', ['output', 'fields', { key: 'toString' }]))).toBeUndefined();
    expect(fieldText(document, nodeField(ROOT, 'ready', [], 'key'))).toBe('ready');
    expect(rejection(() => setField(document, nodeField(ROOT, 'ready', ['output', 'items', 0]), 1))).toBe('missing_field');
    const quality: BlockPath = [{ node: 'reviews', branch: 'quality' }];
    const inQuality = nodeField(quality, 'done', ['output']);
    expect(renameAddress(inQuality, nodeField(ROOT, 'reviews', ['branches', { key: 'quality' }], 'key'), 'clarity')).toEqual(nodeField([{ node: 'reviews', branch: 'clarity' }], 'done', ['output']));
    expect(renameAddress(inQuality, nodeField(ROOT, 'reviews', [], 'key'), 'checks')).toEqual(nodeField([{ node: 'checks', branch: 'quality' }], 'done', ['output']));
    expect(renameAddress(direct, nodeField(ROOT, 'ready', [], 'key'), 'done_now')).toEqual(nodeField(ROOT, 'done_now', direct.path));
    // Another block's same-named node is another node.
    expect(renameAddress(inQuality, nodeField(ROOT, 'done', [], 'key'), 'other')).toBe(inQuality);
  });
});

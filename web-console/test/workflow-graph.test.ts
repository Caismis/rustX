import { expect, it } from 'vitest';
import { addNode, connect, disconnect, emptyBlock, insertNode, reconnect, renameNode } from '../src/app/workflows/document/commands';
import { copyFragment, parseFragment, pasteFragment } from '../src/app/workflows/document/clipboard';
import { sameDocument, type WorkflowDocument } from '../src/app/workflows/document/model';
import { applySceneConnection } from '../src/app/workflows/ui/editor/connections';
import { organizeDocument } from '../src/app/workflows/ui/editor/layout';
import { projectScene } from '../src/app/workflows/ui/editor/scene';
import { viewId } from '../src/app/workflows/ui/editor/selection';

function graph(): WorkflowDocument {
  let document: WorkflowDocument = { description: 'Implicit parallelism', block: emptyBlock() };
  document = disconnect(document, [], [0]);
  document = addNode(document, [], 'model', 'left', { from: 'start', port: 'next' });
  document = addNode(document, [], 'model', 'right', { from: 'start', port: 'next' });
  document = connect(document, [], 'left', 'next', 'done');
  return connect(document, [], 'right', 'next', 'done');
}

it('allows distinct targets on one port and refuses only duplicate connections', () => {
  const document = graph(), before = structuredClone(document);
  expect(document.block.edges?.filter(edge => edge.from === 'start')).toHaveLength(2);
  expect(() => connect(document, [], 'start', 'next', 'left')).toThrow('duplicate_edge');
  expect(() => reconnect(document, [], 1, 'start', 'next', 'left')).toThrow('duplicate_edge');
  expect(() => connect(document, [], 'left', 'next', 'start')).toThrow('invalid_port');
  expect(() => connect(document, [], 'done', 'next', 'left')).toThrow('return_has_no_successor');
  expect(() => connect(document, [], 'left', 'next', 'left')).toThrow('self_edge');
  expect(document).toEqual(before);
  const branched = addNode(document, [], 'branch', 'choose', { from: 'left', port: 'next' });
  const fanout = connect(connect(branched, [], 'choose', 'true', 'left'), [], 'choose', 'true', 'right');
  expect(fanout.block.edges?.filter(edge => edge.from === 'choose' && edge.port === 'true')).toHaveLength(2);
});

it('projects the real Start and every native edge without synthetic entry connections', () => {
  const document = graph(), scene = projectScene(document);
  expect(scene.nodes.filter(node => node.data.view === 'start')).toHaveLength(1);
  expect(scene.nodes.find(node => node.data.view === 'start')?.data.local).toBe('start');
  expect(scene.nodes).toHaveLength(4); expect(scene.edges).toHaveLength(4);
  expect(scene.edges.filter(edge => edge.source === viewId([], 'start'))).toHaveLength(2);
  expect(scene.edges.map(edge => edge.data?.index)).toEqual([0, 1, 2, 3]);
});

it('appends and reconnects Start edges using the same native connection command', () => {
  let document = graph(); document = addNode(document, [], 'review', 'other');
  const scene = projectScene(document), nodes = new Map(scene.nodes.map(node => [node.id, node]));
  const connection = { source: viewId([], 'start'), sourceHandle: 'next', target: viewId([], 'node', 'other'), targetHandle: 'in' };
  const added = applySceneConnection(document, nodes, connection);
  expect(added.block.edges?.filter(edge => edge.from === 'start').map(edge => edge.to)).toEqual(['left', 'right', 'other']);
  const moved = applySceneConnection(document, nodes, connection, scene.edges[0]);
  expect(moved.block.edges?.filter(edge => edge.from === 'start').map(edge => edge.to)).toEqual(['other', 'right']);
  expect(document.block.edges?.[0]).toEqual({ from: 'start', to: 'left' });
});

it('inserts into one chosen edge and preserves every parallel sibling', () => {
  const document = graph(), next = insertNode(document, [], 0, 'review', 'inspect');
  expect(next.block.edges).toEqual([{ from: 'start', to: 'right' }, { from: 'left', to: 'done' }, { from: 'right', to: 'done' }, { from: 'start', to: 'inspect' }, { from: 'inspect', to: 'left' }]);
  expect(document.block.edges?.[0]).toEqual({ from: 'start', to: 'left' });
});

it('renames a fanout producer in edges and typed references while preserving business text', () => {
  const document = graph(); document.block.nodes.done = { type: 'return', output: { type: 'object', fields: { result: { type: 'reference', path: ['left'] }, text: { type: 'literal', value: 'left' } } } };
  const renamed = renameNode(document, [], 'left', 'draft');
  expect(renamed.block.edges?.filter(edge => edge.from === 'start').map(edge => edge.to)).toEqual(['draft', 'right']);
  expect(renamed.block.nodes.done).toMatchObject({ output: { fields: { result: { path: ['draft'] }, text: { value: 'left' } } } });
});

it('copies selected graph edges and Loop scopes while protecting the one Start per block', () => {
  let document = graph(); document = addNode(document, [], 'loop', 'repeat');
  const copied = pasteFragment(document, 'owner', [], parseFragment(copyFragment(document, 'owner', [], ['left', 'right', 'done', 'repeat']))).document;
  expect(copied.block.edges).toContainEqual({ from: 'left_copy', to: 'done_copy' });
  expect(copied.block.edges).toContainEqual({ from: 'right_copy', to: 'done_copy' });
  expect(copied.block.nodes.repeat_copy).toMatchObject({ body: { nodes: { start: { type: 'start' } }, edges: [{ from: 'start', to: 'done' }] } });
  expect(() => copyFragment(document, 'owner', [], ['start'])).toThrow('invalid_port');
});

it('organizes sibling paths through ELK without changing execution', async () => {
  const document = graph(), organized = await organizeDocument(document);
  expect(sameDocument(organized.block, document.block)).toBe(true);
  const scene = projectScene(organized), left = scene.nodes.find(node => node.data.local === 'left')!, right = scene.nodes.find(node => node.data.local === 'right')!;
  expect(Math.abs(left.position.y - right.position.y)).toBeGreaterThanOrEqual(128);
});

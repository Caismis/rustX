import { expect, it } from 'vitest';
import { addBranchCase, removeBranchCase, addNode, connect, disconnect, insertNode, reconnect, renameNode } from '../src/app/workflows/document/commands';
import { rootBlock } from './workflow-root';
import { copyFragment, parseFragment, pasteFragment } from '../src/app/workflows/document/clipboard';
import { sameDocument, type WorkflowDocument } from '../src/app/workflows/document/model';
import { applySceneConnection } from '../src/app/workflows/ui/editor/connections';
import { organizeDocument } from '../src/app/workflows/ui/editor/layout';
import { branchGeometry } from '../src/app/workflows/document/branch';
import { portsOf } from '../src/app/workflows/document/ports';
import { projectScene } from '../src/app/workflows/ui/editor/scene';
import { setNodePresentation } from '../src/app/workflows/document/geometry';
import { viewId } from '../src/app/workflows/ui/editor/selection';

function graph(): WorkflowDocument {
  let document: WorkflowDocument = { description: 'Implicit parallelism', block: rootBlock() };
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

it('organizes a described Start using its measured content height', () => {
  const document = setNodePresentation(graph(), [], 'start', { description: 'Workflow input.' });
  const id = viewId([], 'start');
  const scene = projectScene(document, new Map([[id, { width: 256, height: 103 }]]));
  expect(scene.nodes.find(node => node.id === id)?.height).toBe(103);
});

it.each(['agent', 'model', 'tool', 'branch', 'review', 'return'] as const)('uses the measured %s card height instead of its description estimate', kind => {
  const document = setNodePresentation(addNode(graph(), [], kind, 'card'), [], 'card', { description: 'One line.' });
  const id = viewId([], 'node', 'card');
  const scene = projectScene(document, new Map([[id, { width: 256, height: 103 }]]));
  expect(scene.nodes.find(node => node.id === id)?.height).toBe(103);
});

it('keeps a Loop container at its authored dimensions', () => {
  const document = setNodePresentation(addNode(graph(), [], 'loop', 'repeat'), [], 'repeat', { dimensions: { width: 900, height: 800 } });
  const container = projectScene(document).nodes.find(node => node.id === viewId([], 'node', 'repeat'))!;
  expect(container.width).toBe(900); expect(container.height).toBe(800);
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
  expect(copied.block.nodes.repeat_copy).toMatchObject({ body: { nodes: { repeat_start: { type: 'start' } }, edges: [] } });
  expect(() => copyFragment(document, 'owner', [], ['start'])).toThrow('invalid_port');
});

it('organizes sibling paths through ELK without changing execution', async () => {
  const document = graph(), organized = await organizeDocument(document);
  expect(sameDocument(organized.block, document.block)).toBe(true);
  const scene = projectScene(organized), left = scene.nodes.find(node => node.data.local === 'left')!, right = scene.nodes.find(node => node.data.local === 'right')!;
  expect(Math.abs(left.position.y - right.position.y)).toBeGreaterThanOrEqual(128);
});


it('keeps surviving CASE exits stable when an earlier case is removed', () => {
  let document = addNode(graph(), [], 'branch', 'choose');
  document = addBranchCase(addBranchCase(document, [], 'choose'), [], 'choose');
  document = connect(connect(connect(document, [], 'choose', 'true', 'left'), [], 'choose', 'case_2', 'right'), [], 'choose', 'case_3', 'done');
  document = connect(document, [], 'choose', 'false', 'done');
  const before = document;
  const fragment = parseFragment(copyFragment(document, 'fixture', [], ['choose', 'right', 'done']));
  const pasted = pasteFragment(document, 'fixture', [], fragment);
  expect(pasted.document.block.edges).toContainEqual({ from: 'choose_copy', to: 'right_copy', port: 'case_2' });
  const removed = removeBranchCase(document, [], 'choose', 'true');
  const node = removed.block.nodes.choose;
  if (node.type !== 'branch') throw new Error('branch');
  expect(portsOf(node)).toEqual(['case_2', 'case_3', 'false']);
  expect(removed.block.edges?.filter(edge => edge.from === 'choose')).toEqual([
    { from: 'choose', to: 'right', port: 'case_2' }, { from: 'choose', to: 'done', port: 'case_3' }, { from: 'choose', to: 'done', port: 'false' },
  ]);
  expect(() => connect(removed, [], 'choose', 'true', 'left')).toThrow('invalid_port');
  expect(before.block.nodes.choose).toMatchObject({ cases: [{ id: 'true' }, { id: 'case_2' }, { id: 'case_3' }] });
  const one = removeBranchCase(removed, [], 'choose', 'case_2');
  expect(() => removeBranchCase(one, [], 'choose', 'case_3')).toThrow('missing_field');
  const geometry = branchGeometry(node);
  expect(geometry.cases.map(item => item.top)).toEqual([65, 89]);
  expect(geometry.elseTop).toBe(113);
  expect(projectScene(removed).nodes.find(item => item.data.local === 'choose')?.height).toBe(geometry.height);
});

it('renames a producer inside a Tool argument template and copies the template as a fragment', () => {
  let document: WorkflowDocument = { description: 'Template', block: rootBlock() };
  document = addNode(document, [], 'model', 'search');
  document = addNode(document, [], 'tool', 'read', { from: 'search', port: 'next' });
  const tool = document.block.nodes.read;
  if (tool.type !== 'tool') throw new Error('tool');
  document.block.nodes.read = { ...tool, arguments: { type: 'object', fields: { path: { type: 'template', parts: [{ type: 'literal', value: 'notes/' }, { type: 'reference', path: ['search', 'text'] }] } } } };
  const renamed = renameNode(document, [], 'search', 'lookup').block.nodes.read;
  expect(renamed).toMatchObject({ arguments: { fields: { path: { type: 'template', parts: [{ type: 'literal', value: 'notes/' }, { type: 'reference', path: ['lookup', 'text'] }] } } } });
  expect(parseFragment(copyFragment(document, 'test', [], ['read']))).toBeTruthy();
});

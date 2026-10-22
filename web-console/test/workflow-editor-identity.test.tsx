import { afterEach, expect, it } from 'vitest';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { createActor } from 'xstate';
import { exampleSource } from './workflow-fixtures';
import { addNode, deleteNodes, renameBranch, renameNode } from '../src/app/workflows/document/commands';
import { blockAt, blockKey, type BlockPath, type WorkflowDocument } from '../src/app/workflows/document/model';
import { encodeField, nodeField, type FieldAddress } from '../src/app/workflows/document/fields';
import { isDirty, workflowEditorMachine, type CheckResult, type WorkflowEditorActor } from '../src/app/workflows/machines/editor';
import type { WorkflowScopePort } from '../src/app/workflows/port';
import { NodeForm } from '../src/app/workflows/ui/editor/forms/NodeForms';
import { DraftOwner, FieldCommits } from '../src/app/workflows/ui/editor/forms/drafts';
import { CheckResults } from '../src/app/workflows/ui/editor/Inspector';
import { blockView, readViews, renameViews, viewKey, writeBlockView } from '../src/app/workflows/ui/editor/view-store';

afterEach(() => { cleanup(); localStorage.clear(); });

it('refuses an old Agent form command when its ID now belongs to a Tool, without adding history', async () => {
  const source = exampleSource('typed_agent');
  const id = Object.entries(source.document!.block.nodes).find(([, node]) => node.type === 'agent')![0];
  const port: WorkflowScopePort = {
    endpoint: 'ws://fixture/', scope: { kind: 'user' },
    list: () => new Promise(() => {}), read: async () => source,
    check: () => new Promise(() => {}), write: () => new Promise(() => {}),
  };
  const editor = createActor(workflowEditorMachine, { input: { port, target: { endpoint: port.endpoint, scope: port.scope, id: source.id } } }).start();
  await act(async () => { await Promise.resolve(); });
  const commands: ((document: WorkflowDocument) => WorkflowDocument)[] = [];
  render(<DraftOwner value={{ editor, commits: new FieldCommits() }}><NodeForm document={editor.getSnapshot().context.document!} block={[]} id={id} tab="config"
    edit={apply => commands.push(apply)} onRenamed={() => {}} navigate={() => {}} /></DraftOwner>);
  fireEvent.change(screen.getByLabelText('Task'), { target: { value: 'Retained Agent callback' } });
  expect(commands).toHaveLength(1);
  act(() => editor.send({ type: 'EDIT', apply: current => addNode(deleteNodes(current, [], [id]), [], 'tool', id) }));
  const before = editor.getSnapshot().context;
  expect(before.document!.block.nodes[id].type).toBe('tool');
  act(() => editor.send({ type: 'EDIT', apply: commands[0] }));
  const after = editor.getSnapshot().context;
  expect(after.rejection).toBe('missing_node');
  expect(after.document).toBe(before.document);
  expect(after.past).toBe(before.past);
  expect(after.future).toBe(before.future);
  expect(after.generation).toBe(before.generation);
  expect(after.document!.block.nodes[id]).not.toHaveProperty('task');
  expect(after.document!.block.nodes[id]).not.toHaveProperty('profile');
  editor.stop();
});

it('keeps structure results but hides dependencies from another selected Workspace', () => {
  const check: CheckResult = { generation: 0, explain: false, context: 'a', report: {
    structure: { status: 'valid', outputs: {}, revision: 'sha256:candidate' },
    dependencies: { status: 'admitted', context: '/a' }, explanation: null,
  } };
  const props = { check, generation: 0, global: true, workspaces: [{ id: 'a', displayName: 'Workspace A', displayPath: '/a' }, { id: 'b', displayName: 'Workspace B', displayPath: '/b' }], setContext: () => {}, run: () => {}, go: () => {} };
  const ui = render(<CheckResults {...props} context="a" />);
  expect(screen.getByText('Agents and Tools admit in Workspace A.')).toBeTruthy();
  ui.rerender(<CheckResults {...props} context="b" />);
  expect(screen.queryByText('Agents and Tools admit in Workspace A.')).toBeNull();
  expect(screen.getByText('Not checked', { selector: 'p' })).toBeTruthy();
  expect(screen.getByText('Structure check passed')).toBeTruthy();
  ui.rerender(<CheckResults {...props} context="a" />);
  expect(screen.getByText('Agents and Tools admit in Workspace A.')).toBeTruthy();
});

function applyRename(editor: WorkflowEditorActor, address: FieldAddress, to: string, apply: (document: WorkflowDocument, to: string) => WorkflowDocument) {
  editor.send({ type: 'DRAFT', address, text: to });
  const field = encodeField(address), draft = editor.getSnapshot().context.drafts[field];
  editor.send({ type: 'APPLY_DRAFT', field, version: draft.version, command: (document, text) => ({ status: 'applied', document: apply(document, text.trim()) }) });
}

it('moves nested layouts on accepted branch/node renames and preserves them through batched undo/redo with dragging', async () => {
  const source = exampleSource('typed_agent');
  let document = addNode(source.document!, [], 'loop', 'repeat_view');
  const body: BlockPath = [{ node: 'repeat_view' }];
  const branch: BlockPath = [...body, { node: 'fork_view', branch: 'main' }];
  document = addNode(addNode(document, body, 'parallel', 'fork_view'), branch, 'loop', 'inner_view');
  const deep: BlockPath = [...branch, { node: 'inner_view' }];
  const returnId = blockAt(document, deep)!.entry;
  const port: WorkflowScopePort = { endpoint: 'ws://view/', scope: { kind: 'user' }, read: async () => ({ ...source, document }), list: () => new Promise(() => {}), check: () => new Promise(() => {}), write: () => { throw new Error('layout must not write'); } };
  const editor = createActor(workflowEditorMachine, { input: { port, target: { endpoint: port.endpoint, scope: port.scope, id: source.id } } }).start();
  await act(async () => { await Promise.resolve(); });
  const key = viewKey(port.endpoint, 'user', source.id), other = viewKey(port.endpoint, 'user', 'other');
  writeBlockView(key, 'root', () => ({ positions: { repeat_view: { x: 500, y: 200 } }, viewport: { x: 1, y: 2, zoom: 0.8 } }));
  writeBlockView(key, blockKey(body), () => ({ positions: { fork_view: { x: 50, y: 70 } } }));
  writeBlockView(key, blockKey(branch), () => ({ positions: { inner_view: { x: 80, y: 90 } } }));
  writeBlockView(key, blockKey(deep), () => ({ positions: { [returnId]: { x: 100, y: 110 } }, viewport: { x: 3, y: 4, zoom: 0.7 } }));
  // Orphaned preferences of a formerly deleted destination cannot win.
  writeBlockView(key, 'root/body', () => ({ positions: { stale: { x: -1, y: -1 } } }));
  writeBlockView(other, 'root', () => ({ positions: { repeat_view: { x: 9, y: 9 } } }));
  const unrelated = localStorage.getItem(other);
  const renamed = editor.on('view.rename', event => renameViews(key, event.rename));
  const restored = editor.on('view.restore', event => writeBlockView(key, event.block, view => ({ ...view, positions: { ...view.positions, ...event.positions } })));
  const address = nodeField(body, 'fork_view', ['branches', { key: 'main' }], 'key');
  const original = localStorage.getItem(key);
  applyRename(editor, address, 'bad branch', (current, to) => renameBranch(current, body, 'fork_view', 'main', to));
  expect(editor.getSnapshot().context.applied?.result.status).toBe('rejected');
  expect(editor.getSnapshot().context.past).toHaveLength(0);
  expect(localStorage.getItem(key)).toBe(original);
  editor.send({ type: 'DISCARD_DRAFTS' });
  // No render or scheduling gap between identity changes: each event must be
  // delivered, including every descendant block and its current viewport.
  applyRename(editor, address, 'quality', (current, to) => renameBranch(current, body, 'fork_view', 'main', to));
  applyRename(editor, nodeField([], 'repeat_view', [], 'key'), 'root', (current, to) => renameNode(current, [], 'repeat_view', to));
  expect(editor.getSnapshot().context.past).toHaveLength(2);
  expect(blockView(readViews(key), 'root')?.positions).toEqual({ root: { x: 500, y: 200 } });
  expect(blockView(readViews(key), 'root')?.viewport).toEqual({ x: 1, y: 2, zoom: 0.8 });
  expect(blockView(readViews(key), 'root/body')?.positions).toEqual({ fork_view: { x: 50, y: 70 } });
  const movedPath: BlockPath = [{ node: 'root' }, { node: 'fork_view', branch: 'quality' }, { node: 'inner_view' }];
  const movedKey = blockKey(movedPath);
  expect(blockView(readViews(key), movedKey)?.positions).toEqual({ [returnId]: { x: 100, y: 110 } });
  expect(readViews(key)).not.toHaveProperty(blockKey(body));
  writeBlockView(key, movedKey, view => ({ ...view, positions: { [returnId]: { x: 300, y: 400 } }, viewport: { x: 30, y: 40, zoom: 0.6 } }));
  editor.send({ type: 'MOVE', path: movedPath, block: movedKey, before: { [returnId]: { x: 100, y: 110 } }, after: { [returnId]: { x: 300, y: 400 } } });
  editor.send({ type: 'UNDO' });
  editor.send({ type: 'UNDO' });
  editor.send({ type: 'UNDO' });
  expect(editor.getSnapshot().context.document).toBe(document);
  expect(isDirty(editor.getSnapshot().context)).toBe(false);
  expect(blockView(readViews(key), 'root')?.positions).toEqual({ repeat_view: { x: 500, y: 200 } });
  expect(blockView(readViews(key), blockKey(deep))).toEqual({ positions: { [returnId]: { x: 100, y: 110 } }, viewport: { x: 30, y: 40, zoom: 0.6 } });
  editor.send({ type: 'REDO' });
  editor.send({ type: 'REDO' });
  editor.send({ type: 'REDO' });
  expect(blockView(readViews(key), movedKey)).toEqual({ positions: { [returnId]: { x: 300, y: 400 } }, viewport: { x: 30, y: 40, zoom: 0.6 } });
  expect(editor.getSnapshot().context.block).toEqual(movedPath);
  expect(localStorage.getItem(other)).toBe(unrelated);
  renamed.unsubscribe(); restored.unsubscribe(); editor.stop();
});

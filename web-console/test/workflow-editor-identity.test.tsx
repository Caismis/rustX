import { afterEach, expect, it } from 'vitest';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { createActor } from 'xstate';
import { exampleSource } from './workflow-fixtures';
import { addNode, deleteNodes } from '../src/app/workflows/document/commands';
import { type WorkflowDocument } from '../src/app/workflows/document/model';
import { workflowEditorMachine, type CheckResult } from '../src/app/workflows/machines/editor';
import type { WorkflowScopePort } from '../src/app/workflows/port';
import { AgentPanel } from '../src/app/workflows/ui/editor/panels/InferencePanels';
import { DraftOwner, FieldCommits } from '../src/app/workflows/ui/editor/forms/drafts';
import { CheckResults } from '../src/app/workflows/ui/editor/Inspector';

afterEach(() => { cleanup(); localStorage.clear(); });

it('refuses an old Agent form command when its ID now belongs to a Tool, without adding history', async () => {
  const source = exampleSource('typed_agent');
  const id = Object.entries(source.document!.block.nodes).find(([, node]) => node.type === 'agent')![0];
  const port: WorkflowScopePort = {
    endpoint: 'ws://fixture/', scope: { kind: 'user' },
    analyze: () => new Promise(() => {}), modelChoices: () => new Promise(() => {}), list: () => new Promise(() => {}), read: async () => source,
    check: () => new Promise(() => {}), write: () => new Promise(() => {}),
  };
  const editor = createActor(workflowEditorMachine, { input: { port, target: { endpoint: port.endpoint, scope: port.scope, id: source.id } } }).start();
  await act(async () => { await Promise.resolve(); });
  const commands: ((document: WorkflowDocument) => WorkflowDocument)[] = [];
  render(<DraftOwner value={{ editor, commits: new FieldCommits() }}><AgentPanel port={port} document={editor.getSnapshot().context.document!} block={[]} id={id}
    vocabulary={{ profiles: ['planner'], builtin_tools: [] }} edit={apply => commands.push(apply)} navigate={() => {}} onSelect={() => {}} /></DraftOwner>);
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Agent profile' })));
  await act(async () => fireEvent.click(await screen.findByRole('menuitemradio', { name: 'planner' })));
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
  expect(screen.getByText('Agents, Models and Tools admit in Workspace A.')).toBeTruthy();
  ui.rerender(<CheckResults {...props} context="b" />);
  expect(screen.queryByText('Agents, Models and Tools admit in Workspace A.')).toBeNull();
  expect(screen.getByText('Not checked', { selector: 'p' })).toBeTruthy();
  expect(screen.getByText('Structure check passed')).toBeTruthy();
  ui.rerender(<CheckResults {...props} context="a" />);
  expect(screen.getByText('Agents, Models and Tools admit in Workspace A.')).toBeTruthy();
});

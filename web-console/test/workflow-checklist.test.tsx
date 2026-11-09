import { afterEach, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import type { WorkflowCheckReport } from '../../protocol/app-server/v76';
import { changeNode, nodeTemplate } from '../src/app/workflows/document/commands';
import { checklist } from '../src/app/workflows/document/checklist';
import { toggleAggregateGroups } from '../src/app/workflows/document/aggregate';
import type { WorkflowDocument } from '../src/app/workflows/document/model';
import { Checklist } from '../src/app/workflows/ui/editor/Checklist';

afterEach(cleanup);

const document: WorkflowDocument = {
  description: 'Checklist',
  block: {
    nodes: {
      start: { type: 'start' },
      llm: nodeTemplate('model'),
      each: nodeTemplate('iteration', 'each'),
      done: { type: 'return', outputs: [{ variable: 'answer', value_selector: [] }] },
      stray: { type: 'return', outputs: [] },
    },
    edges: [{ from: 'start', to: 'llm' }, { from: 'llm', to: 'each' }, { from: 'each', to: 'done' }],
  },
  editor: { nodes: [{ address: { block: [], node: 'llm' }, title: 'LLM 2' }] },
};
const invalid = (path: string, reason: string): WorkflowCheckReport => ({ revision: 'sha256:x', structure: { status: 'invalid', diagnostic: { path, category: 'compile', reason, correction: null, detail: null, line: null, column: null } }, dependencies: { status: 'not_checked' } });

it('lists each node once with its first empty required field, as Dify\'s checklist', () => {
  expect(checklist(document).map(item => [item.node, item.title, item.messages.map(message => message.text)])).toEqual([
    ['llm', 'LLM 2', [{ kind: 'required', field: 'prompt' }]],
    ['each', undefined, [{ kind: 'required', field: 'input' }]],
    ['done', undefined, [{ kind: 'required', field: 'value' }]],
    ['stray', undefined, [{ kind: 'unconnected' }]],
  ]);
});

it('adds the native check\'s problem only where no message names one already', () => {
  expect(checklist(document, invalid('block.nodes.llm.messages.1.content', 'Model "llm" message 1 is empty')).find(item => item.node === 'llm')!.messages).toHaveLength(1);
  const items = checklist(document, invalid('block.nodes.start', 'input schema is invalid'));
  expect(items.find(item => item.node === 'start')).toMatchObject({ type: 'start', messages: [{ text: { kind: 'native', reason: 'input schema is invalid' } }] });
  expect(checklist(document, invalid('description', 'description is empty')).at(-1)).toMatchObject({ node: undefined, messages: [{ text: { kind: 'native', reason: 'description is empty' }, field: 'description' }] });
});

it('requires sources only for active aggregate groups', () => {
  const grouped: WorkflowDocument = { description: 'Aggregate groups', block: {
    nodes: {
      start: { type: 'start' },
      merge: { type: 'aggregate', grouped: true, groups: [
        { name: 'Group1', output_type: 'string', variables: [['args', 'text']] },
        { name: 'Group2', variables: [] },
      ] },
      done: { type: 'return', outputs: [] },
    },
    edges: [{ from: 'start', to: 'merge' }, { from: 'merge', to: 'done' }],
  } };
  expect(checklist(grouped)).toMatchObject([{ node: 'merge', messages: [{ text: { kind: 'required', field: 'input' } }] }]);
  const ungrouped = toggleAggregateGroups(grouped, [], 'merge', false);
  expect(checklist(ungrouped)).toEqual([]);
  expect(checklist(toggleAggregateGroups(ungrouped, [], 'merge', true))).toHaveLength(1);
});

it('words why the native admission blocks a node', () => {
  const blocked: WorkflowCheckReport = { revision: 'sha256:x', structure: { status: 'valid', outputs: {} }, dependencies: { status: 'blocked', context: 'workspace', diagnostics: [
    { path: 'block.nodes.start', reason: { kind: 'tool', detail: { kind: 'source_unavailable', selector: 'exa/web_fetch_exa', source: 'exa', reason: { kind: 'unprepared' } } } },
  ] } };
  const start = checklist(document, blocked).find(item => item.node === 'start')!;
  expect(start.messages).toEqual([{ text: { kind: 'dependency', failure: blocked.dependencies.status === 'blocked' && blocked.dependencies.diagnostics[0].reason }, field: undefined }]);
  render(<Checklist items={[start]} onGo={() => {}} />);
  fireEvent.click(screen.getByRole('button', { name: /Checklist/ }));
  expect(screen.getByText('Tool source exa is not connected yet')).toBeTruthy();
});

it('changes a node in place, keeping its incoming connection and its ordinary exit', () => {
  const branched: WorkflowDocument = { ...document, block: { ...document.block, edges: [...document.block.edges!, { from: 'llm', port: 'fail', to: 'stray' }] } };
  const changed = changeNode(branched, [], 'llm', 'branch');
  expect(changed.block.nodes.llm).toEqual(nodeTemplate('branch', 'llm'));
  expect(changed.block.edges).toEqual([{ from: 'start', to: 'llm' }, { from: 'llm', port: 'true', to: 'each' }, { from: 'each', to: 'done' }]);
  expect(changeNode(document, [], 'llm', 'return').block.edges).toEqual([{ from: 'start', to: 'llm' }, { from: 'each', to: 'done' }]);
  expect(() => changeNode(document, [], 'start', 'model')).toThrow();
});

it('counts the problems on its button and goes to the one chosen', async () => {
  const go = vi.fn(), items = checklist(document);
  render(<Checklist items={items} onGo={go} />);
  fireEvent.click(screen.getByRole('button', { name: 'Checklist (4)' }));
  expect(await screen.findByText('Prompt is required')).toBeTruthy();
  expect(screen.getByText('This step is not connected to anything')).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: /Input is required/ }));
  expect(go).toHaveBeenCalledWith(items[1], items[1].messages[0]);
});

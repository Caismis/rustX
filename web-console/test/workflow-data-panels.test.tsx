import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { createActor } from 'xstate';
import { workflowEditorMachine } from '../src/app/workflows/machines/editor';
import { workflowSurfaceMachine } from '../src/app/workflows/machines/surface';
import WorkflowEditor from '../src/app/workflows/ui/editor/WorkflowEditor';
import type { WorkflowScopePort } from '../src/app/workflows/port';
import type { WorkflowDocument } from '../src/app/workflows/document/model';
import { viewId } from '../src/app/workflows/ui/editor/selection';
import { nodeTemplate } from '../src/app/workflows/document/commands';
import { dataNodesDocument } from './workflow-data-document';
import { statusSource } from './workflow-fixtures';

beforeEach(() => {
  vi.stubGlobal(
    'ResizeObserver',
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  );
  vi.stubGlobal(
    'DOMMatrixReadOnly',
    class {
      m22 = 1;
    },
  );
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  localStorage.clear();
});
async function open(id: string, document: WorkflowDocument = dataNodesDocument()) {
  const source = { ...statusSource(), document };
  const port: WorkflowScopePort = {
    endpoint: 'ws://127.0.0.1:1/',
    scope: { kind: 'user' },
    enable: () => new Promise(() => {}),
    toolCatalog: async () => ({ sources: [] }),
    modelChoices: () => new Promise(() => {}),
    analyze: async (id, _document, generation, query) => ({
      id,
      generation,
      query,
      target: { kind: 'user' },
      candidate_revision: `sha256:${'a'.repeat(64)}`,
      presentation: [],
      analysis: {
        incomplete: false,
        truncated: false,
        outputs: [],
        references: [
          {
            path: ['args', 'query'],
            schema: { type: 'string' },
            availability: 'available',
            compatibility: 'compatible',
          },
        ],
        ...(query?.reference?.[1] === 'numbers'
          ? {
              selected: {
                path: query.reference,
                schema: { type: 'array', items: { type: 'integer' } },
                availability: 'available' as const,
                compatibility: 'compatible' as const,
              },
            }
          : {}),
      },
    }),
    list: () => new Promise(() => {}),
    check: () => new Promise(() => {}),
    read: async (id) => ({ ...source, id, revision: 'sha256:base' }),
    write: () => new Promise(() => {}),
  };
  const surface = createActor(workflowSurfaceMachine, { input: { ports: () => port } }).start(),
    editor = createActor(workflowEditorMachine, {
      input: { port, target: { endpoint: port.endpoint, scope: port.scope, id: 'flow' } },
    }).start();
  await act(async () => {
    await Promise.resolve();
  });
  render(<WorkflowEditor surface={surface} editor={editor} scopeLabel="Global" />);
  const card = window.document.querySelector<HTMLElement>(`.react-flow__node[data-id='${viewId([], 'node', id)}']`)!;
  act(() => card.focus());
  fireEvent.keyDown(card, { key: 'Enter' });
  return {
    editor,
    document: () => editor.getSnapshot().context.document!,
    panel: within(screen.getByRole('complementary', { name: 'Inspector' })),
  };
}

it('starts aggregation without invented inputs or a fixed output type and infers from selection', async () => {
  const initial = nodeTemplate('aggregate');
  expect(initial).toEqual({ type: 'aggregate', grouped: false, groups: [{ name: 'Group1', variables: [] }] });
  const document = dataNodesDocument();
  document.block.nodes.merge = initial;
  const view = await open('merge', document);
  fireEvent.click(view.panel.getByRole('button', { name: 'Add variable to output' }));
  const picker = await screen.findByRole('dialog', { name: 'Choose a variable' });
  fireEvent.click(await within(picker).findByRole('button', { name: /args \/ query/ }));
  expect(view.document().block.nodes.merge).toMatchObject({
    groups: [{ output_type: 'string', variables: [['args', 'query']] }],
  });
  fireEvent.click(view.panel.getByRole('button', { name: 'Remove source 1' }));
  expect(view.document().block.nodes.merge).toEqual(initial);
});

it('toggles grouping and renames its real nested output references through Undo', async () => {
  const view = await open('merge');
  fireEvent.click(view.panel.getByRole('switch', { name: 'Aggregation groups' }));
  expect(view.document().block.nodes.done).toMatchObject({
    outputs: [{ value_selector: ['merge', 'Group1', 'output'] }, { value_selector: ['list', 'result'] }],
  });
  const name = view.panel.getByRole('textbox', { name: 'Output name' });
  fireEvent.change(name, { target: { value: 'answer' } });
  fireEvent.keyDown(name, { key: 'Enter' });
  expect(view.document().block.nodes.done).toMatchObject({
    outputs: [{ value_selector: ['merge', 'answer', 'output'] }, {}],
  });
  fireEvent.click(view.panel.getByRole('switch', { name: 'Aggregation groups' }));
  expect(view.document().block.nodes.done).toMatchObject({ outputs: [{ value_selector: ['merge', 'output'] }, {}] });
  fireEvent.click(screen.getByRole('button', { name: 'Undo' }));
  expect(view.document().block.nodes.merge).toMatchObject({ grouped: true, groups: [{ name: 'answer' }] });
  expect(view.document().block.nodes.done).toMatchObject({
    outputs: [{ value_selector: ['merge', 'answer', 'output'] }, {}],
  });
});

it('confirms loss of referenced groups and restores removed declarations and readers together', async () => {
  const document = dataNodesDocument();
  document.block.nodes.merge = {
    type: 'aggregate',
    grouped: true,
    groups: [
      {
        name: 'Group1',
        output_type: 'string',
        variables: [
          ['left', 'output'],
          ['right', 'output'],
        ],
      },
      {
        name: 'Group2',
        output_type: 'string',
        variables: [
          ['left', 'output'],
          ['right', 'output'],
        ],
      },
    ],
  };
  document.block.nodes.done = {
    type: 'return',
    outputs: [
      { variable: 'one', value_selector: ['merge', 'Group1', 'output'] },
      { variable: 'two', value_selector: ['merge', 'Group2', 'output'] },
    ],
  };
  const view = await open('merge', document);
  fireEvent.click(view.panel.getByRole('switch', { name: 'Aggregation groups' }));
  const dialog = within(screen.getByRole('dialog', { name: 'Remove referenced outputs?' }));
  expect(view.document().block.nodes.merge).toMatchObject({ grouped: true });
  fireEvent.click(dialog.getByRole('button', { name: 'Remove and clear references' }));
  expect(view.document().block.nodes.done).toMatchObject({
    outputs: [{ value_selector: ['merge', 'output'] }, { value_selector: [] }],
  });
  expect(view.document().block.nodes.merge).toMatchObject({
    grouped: false,
    groups: [{ name: 'Group1' }, { name: 'Group2' }],
  });
  fireEvent.click(screen.getByRole('button', { name: 'Undo' }));
  expect(view.document()).toEqual(document);
  fireEvent.click(view.panel.getByRole('button', { name: 'Remove output group Group2' }));
  fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
  expect(view.document()).toEqual(document);
});

it('uses typed list operators, extraction, limit bounds and radio sorting', async () => {
  const view = await open('list');
  await act(async () => {
    await Promise.resolve();
  });
  fireEvent.click(view.panel.getByRole('switch', { name: 'Filter' }));
  const operator = view.panel.getByRole('combobox', { name: 'Operator' });
  expect(
    within(operator)
      .getAllByRole('option')
      .map((option) => option.getAttribute('value')),
  ).toEqual(['equal', 'not_equal', 'greater', 'greater_equal', 'less', 'less_equal']);
  expect(view.panel.queryByRole('combobox', { name: 'Value type' })).toBeNull();
  fireEvent.click(view.panel.getByRole('switch', { name: 'Extract by serial number' }));
  expect(view.document().block.nodes.list).toMatchObject({ extract: { type: 'literal', value: 1 } });
  fireEvent.click(view.panel.getByRole('switch', { name: 'Limit' }));
  const slider = view.panel.getByRole('slider', { name: 'Maximum results' });
  expect(slider.getAttribute('max')).toBe('20');
  fireEvent.change(slider, { target: { value: '3' } });
  fireEvent.click(view.panel.getByRole('switch', { name: 'Sort' }));
  fireEvent.click(view.panel.getByRole('radio', { name: 'Descending' }));
  expect(view.document().block.nodes.list).toMatchObject({ limit: 3, sort: { order: 'desc' } });
  fireEvent.click(view.panel.getByRole('button', { name: 'Output variables' }));
  expect(view.panel.getByText('The first record, or null when the list is empty.')).toBeTruthy();
});

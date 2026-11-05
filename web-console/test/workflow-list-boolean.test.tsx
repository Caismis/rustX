import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { createActor } from 'xstate';
import type { WorkflowEdit, WorkflowFileDocument } from '../../protocol/app-server/v71';
import type { WorkflowDocument } from '../src/app/workflows/document/model';
import type { WorkflowScopePort } from '../src/app/workflows/port';
import { workflowEditorMachine } from '../src/app/workflows/machines/editor';
import { workflowSurfaceMachine } from '../src/app/workflows/machines/surface';
import WorkflowEditor from '../src/app/workflows/ui/editor/WorkflowEditor';
import { viewId } from '../src/app/workflows/ui/editor/selection';
import { rootBlock } from './workflow-root';
import { statusSource } from './workflow-fixtures';

const actors: { stop: () => void }[] = [];
beforeEach(() => {
  vi.stubGlobal('ResizeObserver', class { observe() {} unobserve() {} disconnect() {} });
  vi.stubGlobal('DOMMatrixReadOnly', class { m22 = 1; });
});
afterEach(() => { cleanup(); actors.splice(0).forEach(actor => actor.stop()); vi.unstubAllGlobals(); localStorage.clear(); });

function booleanList(): WorkflowDocument {
  const document: WorkflowDocument = { description: 'Boolean reference filter', block: rootBlock() };
  document.block.input = { type: 'object', properties: {
    flags: { type: 'array', items: { type: 'boolean' } }, keep: { type: 'boolean' }, other: { type: 'boolean' },
  }, required: ['flags', 'keep', 'other'], additionalProperties: false };
  document.block.nodes.list = { type: 'list', input: { type: 'reference', path: ['args', 'flags'] },
    filter: { path: [], operator: 'equal', value: { type: 'reference', path: ['args', 'keep'] } } };
  document.block.nodes.done = { type: 'return', outputs: [{ variable: 'flags', value_selector: ['list', 'result'] }] };
  document.block.edges = [{ from: 'start', to: 'list' }, { from: 'list', to: 'done' }];
  return document;
}

async function open(document: WorkflowDocument) {
  const writes: WorkflowEdit[] = [];
  const properties = document.block.input!.properties as Record<string, Record<string, unknown>>;
  const port: WorkflowScopePort = {
    endpoint: 'ws://127.0.0.1:1/', scope: { kind: 'user' }, enable: () => new Promise(() => {}),
    list: () => new Promise(() => {}), toolCatalog: async () => ({ sources: [] }), modelChoices: () => new Promise(() => {}),
    check: () => new Promise(() => {}), read: async id => ({ ...statusSource(), id, document, revision: 'sha256:base' }),
    write: (_id, _revision, edit) => { writes.push(edit); return new Promise(() => {}); },
    analyze: async (id, _document, generation, query) => ({
      id, generation, query, target: { kind: 'user' }, candidate_revision: `sha256:${'a'.repeat(64)}`, presentation: [],
      analysis: { incomplete: false, truncated: false, outputs: [],
        references: ['keep', 'other'].map(name => ({ path: ['args', name], schema: properties[name], availability: 'available', compatibility: 'compatible' })),
        ...(query?.reference ? { selected: { path: query.reference, schema: properties[query.reference[1]], availability: 'available', compatibility: 'compatible' } } : {}),
      },
    }),
  };
  const surface = createActor(workflowSurfaceMachine, { input: { ports: () => port } }).start();
  const editor = createActor(workflowEditorMachine, { input: { port, target: { endpoint: port.endpoint, scope: port.scope, id: 'flow' } } }).start();
  actors.push(surface, editor);
  await act(async () => { await Promise.resolve(); });
  render(<WorkflowEditor surface={surface} editor={editor} scopeLabel="Global" />);
  const card = window.document.querySelector<HTMLElement>(`.react-flow__node[data-id='${viewId([], 'node', 'list')}']`)!;
  act(() => card.focus());
  await act(async () => fireEvent.keyDown(card, { key: 'Enter' }));
  await waitFor(() => expect(screen.getByRole('group', { name: 'Comparison value' })).toBeTruthy());
  return {
    comparison: () => within(screen.getByRole('group', { name: 'Comparison value' })),
    node: () => editor.getSnapshot().context.document!.block.nodes.list,
    async save() {
      fireEvent.click(screen.getByRole('button', { name: 'Save File' }));
      await waitFor(() => expect(writes).toHaveLength(1));
      return structuredClone((writes[0] as Extract<WorkflowEdit, { operation: 'replace' }>).document) as WorkflowFileDocument;
    },
  };
}

it('displays and edits Boolean list references through save and reopen, and creates a literal only after an explicit mode change', async () => {
  const source = booleanList(), view = await open(source);
  expect(view.comparison().getByRole('radio', { name: 'Variable', checked: true })).toBeTruthy();
  expect(view.comparison().getByRole('button', { name: 'Choose a variable' }).textContent).toContain('keep');
  expect(view.comparison().queryByRole('combobox', { name: 'Comparison value' })).toBeNull();
  expect(view.node()).toEqual(source.block.nodes.list);
  fireEvent.click(view.comparison().getByRole('radio', { name: 'Variable' }));
  expect(view.node()).toEqual(source.block.nodes.list);
  fireEvent.click(view.comparison().getByRole('button', { name: 'Choose a variable' }));
  const picker = await screen.findByRole('dialog', { name: 'Choose a variable' });
  fireEvent.click(await within(picker).findByRole('button', { name: /^args \/ other / }));
  const saved = await view.save();
  expect(saved.block.nodes.list).toMatchObject({ filter: { value: { type: 'reference', path: ['args', 'other'] } } });
  cleanup();
  const reopened = await open(saved as WorkflowDocument);
  expect(reopened.comparison().getByRole('radio', { name: 'Variable', checked: true })).toBeTruthy();
  expect(reopened.comparison().getByRole('button', { name: 'Choose a variable' }).textContent).toContain('other');
  expect(reopened.comparison().queryByRole('combobox')).toBeNull();
  fireEvent.click(reopened.comparison().getByRole('radio', { name: 'Constant' }));
  expect(reopened.node()).toMatchObject({ filter: { value: { type: 'literal', value: false } } });
  const constant = reopened.comparison().getByRole('combobox', { name: 'Comparison value' });
  fireEvent.change(constant, { target: { value: 'true' } });
  fireEvent.click(reopened.comparison().getByRole('radio', { name: 'Constant' }));
  expect((await reopened.save()).block.nodes.list).toMatchObject({ filter: { value: { type: 'literal', value: true } } });
});

it('changes a Boolean constant into a reference only through the explicit Variable mode', async () => {
  const source = booleanList(), list = source.block.nodes.list;
  if (list.type !== 'list') throw Error('Expected List node');
  list.filter!.value = { type: 'literal', value: true };
  const view = await open(source);
  expect(view.comparison().getByRole('radio', { name: 'Constant', checked: true })).toBeTruthy();
  expect((view.comparison().getByRole('combobox', { name: 'Comparison value' }) as HTMLSelectElement).value).toBe('true');
  fireEvent.click(view.comparison().getByRole('radio', { name: 'Variable' }));
  expect(view.node()).toMatchObject({ filter: { value: { type: 'reference', path: [] } } });
  expect(view.comparison().queryByRole('combobox')).toBeNull();
  fireEvent.click(view.comparison().getByRole('button', { name: 'Choose a variable' }));
  const picker = await screen.findByRole('dialog', { name: 'Choose a variable' });
  fireEvent.click(await within(picker).findByRole('button', { name: /^args \/ keep / }));
  expect((await view.save()).block.nodes.list).toMatchObject({ filter: { value: { type: 'reference', path: ['args', 'keep'] } } });
});

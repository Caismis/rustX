import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { createActor } from 'xstate';
import { $getRoot, $isTextNode, getNearestEditorFromDOMNode } from 'lexical';
import type { WorkflowFileDocument } from '../../protocol/app-server/v76';
import type { WorkflowEdit } from '../../protocol/app-server/workflow-documents.ts';
import type { Predicate, Value, WorkflowDocument } from '../src/app/workflows/document/model';
import { workflowEditorMachine } from '../src/app/workflows/machines/editor';
import { workflowSurfaceMachine } from '../src/app/workflows/machines/surface';
import type { WorkflowScopePort } from '../src/app/workflows/port';
import WorkflowEditor from '../src/app/workflows/ui/editor/WorkflowEditor';
import { viewId } from '../src/app/workflows/ui/editor/selection';
import { statusSource } from './workflow-fixtures';
import { rootBlock } from './workflow-root';

const actors: { stop: () => void }[] = [];
beforeEach(() => {
  vi.stubGlobal('ResizeObserver', class { observe() {} unobserve() {} disconnect() {} });
  vi.stubGlobal('DOMMatrixReadOnly', class { m22 = 1; });
});
afterEach(() => {
  cleanup();
  actors.splice(0).forEach(actor => actor.stop());
  vi.unstubAllGlobals();
  localStorage.clear();
});

const reference: Value = { type: 'reference', path: ['args', 'records'] };
const objectItem = { type: 'object', properties: { id: { type: 'string' } }, required: ['id'], additionalProperties: false };
const objects = { type: 'array', items: objectItem };

/** Open the real inspector and capture the document sent by its Save action. */
async function open(condition: Predicate, schema: Record<string, unknown>, properties: Record<string, Record<string, unknown>> = { records: schema }) {
  const document: WorkflowDocument = { description: 'Condition values', block: rootBlock() };
  document.block.input = { type: 'object', properties, required: Object.keys(properties), additionalProperties: false };
  document.block.nodes.choose = { type: 'branch', cases: [{ id: 'true', condition }] };
  document.block.edges = [{ from: 'start', to: 'choose' }, { from: 'choose', to: 'done', port: 'true' }, { from: 'choose', to: 'done', port: 'false' }];
  const writes: WorkflowEdit[] = [];
  const port: WorkflowScopePort = {
    endpoint: 'ws://127.0.0.1:1/', scope: { kind: 'user' }, enable: () => new Promise(() => {}),
    toolCatalog: async () => ({ sources: [] }), modelChoices: () => new Promise(() => {}), list: () => new Promise(() => {}),
    check: () => new Promise(() => {}), read: async id => ({ ...statusSource(), id, document, revision: 'sha256:base' }),
    write: (_id, _expected, edit) => { writes.push(edit); return new Promise(() => {}); },
    analyze: async (id, _document, generation, query) => ({
      id, generation, query, target: { kind: 'user' }, candidate_revision: `sha256:${'a'.repeat(64)}`, presentation: [],
      analysis: {
        incomplete: false, truncated: false, outputs: [], references: [],
        ...(query?.reference ? { selected: { path: query.reference, schema: properties[query.reference[1]] ?? schema, availability: 'available', compatibility: 'compatible' } } : {}),
      },
    }),
  };
  const surface = createActor(workflowSurfaceMachine, { input: { ports: () => port } }).start();
  const editor = createActor(workflowEditorMachine, { input: { port, target: { endpoint: port.endpoint, scope: port.scope, id: 'flow' } } }).start();
  actors.push(surface, editor);
  await act(async () => { await Promise.resolve(); });
  render(<WorkflowEditor surface={surface} editor={editor} scopeLabel="Global" />);
  const card = window.document.querySelector<HTMLElement>(`.react-flow__node[data-id='${viewId([], 'node', 'choose')}']`)!;
  act(() => card.focus());
  await act(async () => { fireEvent.keyDown(card, { key: 'Enter' }); });
  return {
    panel: within(screen.getByRole('complementary', { name: 'Inspector' })),
    condition: () => {
      const node = editor.getSnapshot().context.document!.block.nodes.choose;
      if (node.type !== 'branch') throw new Error('expected branch');
      return node.cases[0].condition;
    },
    save: () => {
      fireEvent.click(screen.getByRole('button', { name: 'Save File' }));
      expect(writes).toHaveLength(1);
      const node = (writes[0] as { document: WorkflowFileDocument }).document.block.nodes.choose;
      if (node.type !== 'branch') throw new Error('expected saved branch');
      return node.cases[0].condition;
    },
  };
}

it('opens and edits array-object containment without presenting its object as null or changing the predicate', async () => {
  const condition: Predicate = { type: 'contains', left: reference, right: { type: 'object', fields: { id: { type: 'literal', value: 'x' } } } };
  const view = await open(condition, objects);
  expect((view.panel.getByRole('combobox', { name: 'Predicate' }) as HTMLSelectElement).value).toBe('contains');
  const right = within(view.panel.getByRole('group', { name: 'Right' }));
  expect(right.getByRole('radio', { name: 'Object', checked: true })).toBeTruthy();
  const id = within(right.getByRole('group', { name: 'id' })).getByRole('textbox', { name: 'Literal JSON' }) as HTMLTextAreaElement;
  expect(id.value).toBe('"x"');
  expect(view.condition()).toEqual(condition);
  fireEvent.change(id, { target: { value: '"updated"' } });
  expect(view.save()).toEqual({ ...condition, right: { type: 'object', fields: { id: { type: 'literal', value: 'updated' } } } });
});

it.each([
  { name: 'constructed array', schema: { type: 'array', items: { type: 'array', items: { type: 'string' } } }, right: { type: 'array', items: [{ type: 'literal', value: 'x' }] } as Value, group: '[0]', text: '"x"', edited: '"y"', expected: { type: 'array', items: [{ type: 'literal', value: 'y' }] } },
  { name: 'literal object', schema: objects, right: { type: 'literal', value: { id: 'x' } } as Value, text: '{\n  "id": "x"\n}', edited: '{"id":"y"}', expected: { type: 'literal', value: { id: 'y' } } },
  { name: 'literal array', schema: { type: 'array', items: { type: 'array', items: { type: 'string' } } }, right: { type: 'literal', value: ['x'] } as Value, text: '[\n  "x"\n]', edited: '["y"]', expected: { type: 'literal', value: ['y'] } },
  { name: 'null item', schema: { type: 'array', items: { type: 'null' } }, right: { type: 'literal', value: null } as Value, text: 'null', edited: 'null', expected: { type: 'literal', value: null } },
])('uses the native value editor for $name', async ({ schema, right, group, text, edited, expected }) => {
  const condition: Predicate = { type: 'contains', left: reference, right };
  const view = await open(condition, schema);
  expect((view.panel.getByRole('combobox', { name: 'Predicate' }) as HTMLSelectElement).value).toBe('contains');
  const rhs = within(view.panel.getByRole('group', { name: 'Right' }));
  const field = (group ? within(rhs.getByRole('group', { name: group })) : rhs).getByRole('textbox', { name: 'Literal JSON' }) as HTMLTextAreaElement;
  expect(field.value).toBe(text);
  expect(view.condition()).toEqual(condition);
  if (edited !== text) {
    fireEvent.change(field, { target: { value: edited } });
    expect(view.save()).toEqual({ ...condition, right: expected });
  }
});

it.each([
  { name: 'number', schema: { type: 'number' }, type: 'greater' as const, value: 2, edited: '3', expected: 3 },
  { name: 'number array item', schema: { type: 'array', items: { type: 'number' } }, type: 'contains' as const, value: 2, edited: '3', expected: 3 },
  { name: 'boolean', schema: { type: 'boolean' }, type: 'equal' as const, value: true, edited: 'false', expected: false },
])('keeps the compact $name editor and saves its scalar type', async ({ schema, type, value, edited, expected }) => {
  const condition: Predicate = { type, left: reference, right: { type: 'literal', value } };
  const view = await open(condition, schema);
  expect(view.panel.queryByRole('combobox', { name: 'Predicate' })).toBeNull();
  const field = view.panel.getByLabelText('Value');
  fireEvent.change(field, { target: { value: edited } });
  expect(view.save()).toEqual({ ...condition, right: { type: 'literal', value: expected } });
});

it.each([
  { name: 'empty string', right: { type: 'literal', value: '' } as Value, text: '' },
  { name: 'string', right: { type: 'literal', value: 'x' } as Value, text: 'x' },
  { name: 'flat string template', right: { type: 'template', parts: [{ type: 'literal', value: 'before ' }, { type: 'reference', path: ['args', 'records'] }, { type: 'literal', value: ' after' }] } as Value, text: 'before ', trailing: ' after' },
  { name: 'reference', right: reference, text: undefined },
])('keeps the compact $name without mutating the source value on open', async ({ right, text, trailing }) => {
  const condition: Predicate = { type: 'contains', left: reference, right };
  const view = await open(condition, { type: 'string' });
  expect(view.panel.queryByRole('combobox', { name: 'Predicate' })).toBeNull();
  const value = view.panel.getByRole('textbox', { name: 'Value' });
  if (text !== undefined) expect(value.textContent).toContain(text);
  if (trailing) expect(value.textContent).toContain(trailing);
  if (right.type === 'reference' || right.type === 'template') expect(within(value).getByRole('button')).toBeTruthy();
  expect(view.condition()).toEqual(condition);
});

it('preserves a native unary boolean predicate instead of presenting an editable equality', async () => {
  const condition: Predicate = { type: 'boolean', value: reference };
  const view = await open(condition, { type: 'boolean' });
  expect((view.panel.getByRole('combobox', { name: 'Predicate' }) as HTMLSelectElement).value).toBe('boolean');
  expect(view.panel.queryByRole('button', { name: 'Operator' })).toBeNull();
  expect(view.condition()).toEqual(condition);
});

it('keeps string template semantics when deleting ordinary text leaves only an integer reference token', async () => {
  const count: Value = { type: 'reference', path: ['args', 'count'] };
  const condition: Predicate = {
    type: 'contains', left: { type: 'reference', path: ['args', 'text'] },
    right: { type: 'template', parts: [{ type: 'literal', value: 'n=' }, count] },
  };
  const view = await open(condition, { type: 'string' }, { text: { type: 'string' }, count: { type: 'integer' } });
  const value = view.panel.getByRole('textbox', { name: 'Value' });
  expect(value.textContent).toContain('n=');
  const token = within(value).getByRole('button');
  expect(token.textContent).toContain('count');
  const lexical = getNearestEditorFromDOMNode(value)!;
  await act(async () => {
    value.focus();
    lexical.update(() => {
      const text = $getRoot().getFirstDescendant();
      if (!$isTextNode(text)) throw new Error('expected the ordinary template prefix');
      text.select(0, text.getTextContentSize());
    }, { discrete: true });
    fireEvent.keyDown(value, { key: 'Backspace', code: 'Backspace' });
  });
  const expected = { ...condition, right: { type: 'template', parts: [count] } };
  await waitFor(() => expect(view.condition()).toEqual(expected));
  expect(value.textContent).not.toContain('n=');
  expect(within(value).getByRole('button')).toBe(token);
  expect(view.save()).toEqual(expected);
});

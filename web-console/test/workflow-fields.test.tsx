import { showSchemaJson } from './workflow-editor-actions';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { createActor } from 'xstate';
import { exampleSource } from './workflow-fixtures';
import type { WorkflowFileDocument, WorkflowEdit } from '../../protocol/app-server/v62';
import type { WorkflowCommit } from '../src/workspaces/host';
import type { WorkspaceAuthority } from '../src/workspaces/authority';
import { workflowEditorMachine } from '../src/app/workflows/machines/editor';
import { editorHolds, workflowSurfaceMachine } from '../src/app/workflows/machines/surface';
import type { WorkflowScopePort } from '../src/app/workflows/port';
import WorkflowEditor from '../src/app/workflows/ui/editor/WorkflowEditor';

/** The real editor: its fields, its Ctrl/Cmd+S handler and its machine,
 * over a port whose writes the test observes. */
const writes: { expected: string; edit: WorkflowEdit }[] = [];
const port: WorkflowScopePort = {
  endpoint: 'ws://127.0.0.1:1/', scope: { kind: 'user' },
  analyze: () => new Promise(() => {}), toolCatalog: async () => ({ sources: [] }), modelChoices: () => new Promise(() => {}), list: () => new Promise(() => {}),
  read: async id => ({ ...exampleSource('parallel_review'), id, revision: 'sha256:base' }),
  check: () => new Promise(() => {}),
  write: (_id, expected, edit) => { writes.push({ expected, edit }); return new Promise<WorkflowCommit>(() => {}); },
};
const authority = { subscribe: () => () => {}, getCatalog: () => undefined } as unknown as WorkspaceAuthority;

beforeEach(() => {
  writes.length = 0;
  vi.stubGlobal('ResizeObserver', class { observe() {} unobserve() {} disconnect() {} });
  vi.stubGlobal('DOMMatrixReadOnly', class { m22 = 1; constructor() {} });
});
afterEach(() => { cleanup(); vi.unstubAllGlobals(); localStorage.clear(); });

async function mountSettings() {
  const surface = createActor(workflowSurfaceMachine, { input: { ports: () => port } }).start();
  const editor = createActor(workflowEditorMachine, { input: { port, target: { endpoint: port.endpoint, scope: port.scope, id: 'flow' } } }).start();
  await act(async () => { await Promise.resolve(); });
  render(<WorkflowEditor surface={surface} editor={editor} authority={authority} scopeLabel="Global" />);
  fireEvent.click(screen.getByRole('button', { name: 'Block settings' }));
  return editor;
}
async function openBlockSettings() {
  const editor = await mountSettings();
  const field = showSchemaJson('Output schema');
  field.focus();
  return { editor, field };
}
const saveShortcut = (target: Element) => fireEvent.keyDown(target, { key: 's', ctrlKey: true });

it('saves the focused field\'s valid text: Ctrl/Cmd+S applies it before writing', async () => {
  const { editor, field } = await openBlockSettings();
  const schema = { type: 'object', properties: { proposal: { type: 'string' }, note: { type: 'string' } }, required: ['proposal'], additionalProperties: false };
  fireEvent.change(field, { target: { value: JSON.stringify(schema) } });
  expect(document.activeElement).toBe(field);
  expect(editorHolds(editor)).toBe(true);
  saveShortcut(field);
  expect(writes).toHaveLength(1);
  expect((writes[0].edit as { document: WorkflowFileDocument }).document.block.output).toEqual(schema);
  // What was written is what the field shows, and no text is left unapplied.
  expect(JSON.parse(field.value)).toEqual(schema);
  expect(editor.getSnapshot().context.drafts).toEqual({});
});

it('never writes focused invalid text, keeps it visible and protects it from silent loss', async () => {
  const { editor, field } = await openBlockSettings();
  fireEvent.change(field, { target: { value: '{ "type": "object", ' } });
  saveShortcut(field);
  expect(writes).toHaveLength(0);
  expect(field.value).toBe('{ "type": "object", ');
  expect(field.getAttribute('aria-invalid')).toBe('true');
  expect(screen.getByText(/Not valid JSON/)).toBeTruthy();
  expect(screen.getByText(/not applied yet/)).toBeTruthy();
  expect(editorHolds(editor)).toBe(true);
  expect(editor.getSnapshot().context.notice).toEqual({ kind: 'pending_fields', count: 1 });
});

it('never writes a number the browser would round, and says why', async () => {
  const { editor, field } = await openBlockSettings();
  fireEvent.change(field, { target: { value: '{"type": "object", "properties": {"n": {"const": 9007199254740993}}, "additionalProperties": false}' } });
  saveShortcut(field);
  expect(writes).toHaveLength(0);
  expect(Object.values(editor.getSnapshot().context.drafts).map(draft => draft.error)).toEqual([{ kind: 'number' }]);
  expect(screen.getByText(/would change in the browser editor/)).toBeTruthy();
});

it('adds an optional input field through the dialog, validating it before one native schema edit', async () => {
  await mountSettings();
  const inspector = screen.getByRole('complementary', { name: 'Inspector' });
  fireEvent.click(within(inspector).getByRole('button', { name: 'Add input field' }));
  const dialog = within(screen.getByRole('dialog', { name: 'Add input field' }));
  const save = () => fireEvent.click(dialog.getByRole('button', { name: 'Save' }));
  fireEvent.change(dialog.getByLabelText('Variable name'), { target: { value: '1tone' } });
  save();
  expect(dialog.getByRole('alert').textContent).toMatch(/letters, digits and underscores/);
  fireEvent.change(dialog.getByLabelText('Variable name'), { target: { value: 'tone' } });
  fireEvent.change(dialog.getByLabelText('Max length'), { target: { value: '8' } });
  fireEvent.click(dialog.getByRole('button', { name: 'Add option' }));
  fireEvent.click(dialog.getByRole('button', { name: 'Add option' }));
  fireEvent.change(dialog.getByLabelText('Option 1'), { target: { value: 'formal' } });
  fireEvent.change(dialog.getByLabelText('Option 2'), { target: { value: 'casual' } });
  fireEvent.click(dialog.getByRole('radio', { name: /^Optional/ }));
  save();
  expect(dialog.getByRole('alert').textContent).toBe('Choose one of the options as the default.');
  fireEvent.change(dialog.getByLabelText('Default value'), { target: { value: 'casual' } });
  save();
  expect(screen.queryByRole('dialog')).toBeNull();
  // The new field is listed last with its requirement and type.
  const rows = within(within(inspector).getByRole('list', { name: 'Input fields' })).getAllByRole('listitem');
  expect(rows.at(-1)!.textContent).toContain('tone');
  expect(rows.at(-1)!.textContent).toContain('OptionalString');
  saveShortcut(inspector);
  const input = (writes[0].edit as { document: WorkflowFileDocument }).document.block.input as { properties: Record<string, unknown>; required: string[] };
  expect(input.properties.tone).toEqual({ type: 'string', maxLength: 8, enum: ['formal', 'casual'], default: 'casual' });
  expect(input.required).not.toContain('tone');
});

it('titles a new node by its kind, numbered after the first, with the kind\'s description', async () => {
  const editor = await mountSettings();
  const add = () => {
    fireEvent.click(within(screen.getByRole('toolbar')).getByRole('button', { name: 'Add node' }));
    fireEvent.click(screen.getByRole('option', { name: 'Model' }));
  };
  add(); add();
  const nodes = (editor.getSnapshot().context.document!.editor!.nodes ?? []).filter(node => node.title?.startsWith('Model'));
  expect(nodes.map(node => [node.title, node.description])).toEqual([
    ['Model', 'Call a language model once to answer, summarize or extract from the input.'],
    ['Model 2', 'Call a language model once to answer, summarize or extract from the input.'],
  ]);
});

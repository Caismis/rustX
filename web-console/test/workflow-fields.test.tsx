import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { createActor } from 'xstate';
import { exampleSource } from './workflow-fixtures';
import type { WorkflowDefinition, WorkflowEdit } from '../../protocol/app-server/v57';
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
  modelChoices: () => new Promise(() => {}), list: () => new Promise(() => {}),
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

async function openBlockSettings() {
  const surface = createActor(workflowSurfaceMachine, { input: { ports: () => port } }).start();
  const editor = createActor(workflowEditorMachine, { input: { port, target: { endpoint: port.endpoint, scope: port.scope, id: 'flow' } } }).start();
  await act(async () => { await Promise.resolve(); });
  render(<WorkflowEditor surface={surface} editor={editor} authority={authority} scopeLabel="Global" />);
  fireEvent.click(screen.getByRole('button', { name: 'Block settings' }));
  const field = screen.getByLabelText('Input schema') as HTMLTextAreaElement;
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
  expect((writes[0].edit as { document: WorkflowDefinition }).document.block.input).toEqual(schema);
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

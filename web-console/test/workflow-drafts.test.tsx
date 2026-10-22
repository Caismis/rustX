import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { createActor } from 'xstate';
import { exampleSource } from './workflow-fixtures';
import type { WorkflowDefinition, WorkflowEdit, WorkflowSource } from '../../protocol/app-server/v51';
import type { WorkflowCommit } from '../src/workspaces/host';
import type { WorkspaceAuthority } from '../src/workspaces/authority';
import { blockAt } from '../src/app/workflows/document/model';
import { isDirty, workflowEditorMachine } from '../src/app/workflows/machines/editor';
import { workflowSurfaceMachine } from '../src/app/workflows/machines/surface';
import type { WorkflowScopePort } from '../src/app/workflows/port';
import WorkflowEditor from '../src/app/workflows/ui/editor/WorkflowEditor';
import { collidingFields } from './workflow-colliding-document';

/** The real editor, its inspector forms, its Ctrl/Cmd+S handler and its
 * machine, over a port whose writes the test captures. */
const writes: { expected: string; edit: WorkflowEdit }[] = [];
function portFor(source: WorkflowSource): WorkflowScopePort {
  return {
    endpoint: 'ws://127.0.0.1:1/', scope: { kind: 'user' },
    list: () => new Promise(() => {}),
    read: async id => ({ ...source, id, revision: 'sha256:base' }),
    check: () => new Promise(() => {}),
    write: (_id, expected, edit) => { writes.push({ expected, edit }); return new Promise<WorkflowCommit>(() => {}); },
  };
}
const authority = { subscribe: () => () => {}, getCatalog: () => undefined } as unknown as WorkspaceAuthority;

beforeEach(() => {
  writes.length = 0;
  vi.stubGlobal('ResizeObserver', class { observe() {} unobserve() {} disconnect() {} });
  vi.stubGlobal('DOMMatrixReadOnly', class { m22 = 1; constructor() {} });
});
afterEach(() => { cleanup(); vi.unstubAllGlobals(); localStorage.clear(); });

async function open(source: WorkflowSource) {
  const port = portFor(source);
  const surface = createActor(workflowSurfaceMachine, { input: { ports: () => port } }).start();
  const editor = createActor(workflowEditorMachine, { input: { port, target: { endpoint: port.endpoint, scope: port.scope, id: 'flow' } } }).start();
  await act(async () => { await Promise.resolve(); });
  render(<WorkflowEditor surface={surface} editor={editor} authority={authority} scopeLabel="Global" />);
  return editor;
}
/** Select a canvas node the way a keyboard user does: focus it, press Enter. */
function select(label: string) {
  const node = document.querySelector<HTMLElement>(`.react-flow__node[aria-label="${label}"]`);
  if (!node) throw new Error(`no canvas node ${label}: ${[...document.querySelectorAll('.react-flow__node')].map(item => item.getAttribute('aria-label')).join(', ')}`);
  act(() => { node.focus(); });
  fireEvent.keyDown(node, { key: 'Enter' });
}
const inspector = () => within(screen.getByRole('complementary', { name: 'Inspector' }));
const literals = () => inspector().getAllByLabelText('Literal JSON') as HTMLTextAreaElement[];
const save = () => fireEvent.keyDown(document.activeElement ?? document.body, { key: 's', ctrlKey: true });
const written = () => (writes[0].edit as { document: WorkflowDefinition }).document;

describe('field identity (F1)', () => {
  it('edits and saves a direct key spelled like a nested path without touching the nested field', async () => {
    const editor = await open(collidingFields());
    select('Return node done');
    const [direct, nested] = literals();
    // Both fields show their own values.
    expect([direct.value, nested.value]).toEqual(['1', '2']);
    fireEvent.change(direct, { target: { value: '3' } });
    expect(literals().map(field => field.value)).toEqual(['3', '2']);
    // Each field registered its own commit and holds its own text.
    expect(Object.values(editor.getSnapshot().context.drafts).map(draft => draft.text)).toEqual(['3']);
    act(() => { direct.focus(); });
    save();
    expect(writes).toHaveLength(1);
    const output = blockAt(written(), [])!.nodes.done;
    expect(output).toEqual({ type: 'return', output: { type: 'object', fields: {
      'a.fields.b': { type: 'literal', value: 3 },
      a: { type: 'object', fields: { b: { type: 'literal', value: 2 } } },
    } } });
    expect(editor.getSnapshot().context.drafts).toEqual({});
  });

  it('edits the nested field alone', async () => {
    await open(collidingFields());
    select('Return node done');
    fireEvent.change(literals()[1], { target: { value: '5' } });
    expect(literals().map(field => field.value)).toEqual(['1', '5']);
    save();
    const fields = (blockAt(written(), [])!.nodes.done as { output: { fields: Record<string, unknown> } }).output.fields;
    expect(fields['a.fields.b']).toEqual({ type: 'literal', value: 1 });
    expect(fields.a).toEqual({ type: 'object', fields: { b: { type: 'literal', value: 5 } } });
  });
});

describe('drafts across selection (F3)', () => {
  it('keeps invalid text of one node across selecting another node with a different value, and back', async () => {
    const editor = await open(exampleSource('parallel_review'));
    select('Return node ready');
    const status = literals()[0];
    expect(status.value).toBe('"ready"');
    fireEvent.change(status, { target: { value: '"unfinished' } });
    fireEvent.blur(status);
    expect(isDirty(editor.getSnapshot().context)).toBe(true);
    select('Return node revise');
    expect(literals()[0].value).toBe('"needs_revision"');
    select('Return node ready');
    const retained = literals()[0];
    expect(retained.value).toBe('"unfinished');
    expect(retained.getAttribute('aria-invalid')).toBe('true');
    expect(inspector().getByText(/Not valid JSON/)).toBeTruthy();
    expect(isDirty(editor.getSnapshot().context)).toBe(true);
    // Save and Check refuse while the text is pending, and say where it is.
    select('Return node revise');
    save();
    fireEvent.click(screen.getByRole('button', { name: 'Check' }));
    expect(writes).toHaveLength(0);
    const pending = screen.getByRole('list', { name: 'Fields with unapplied text' });
    expect(pending.textContent).toContain('ready · output.fields["status"].value');
    fireEvent.click(within(pending).getByRole('button', { name: 'Show' }));
    expect(screen.getByRole('complementary', { name: 'Inspector' }).textContent).toContain('ready');
    expect(literals()[0].value).toBe('"unfinished');
    // Fixing it applies on Save.
    fireEvent.change(literals()[0], { target: { value: '"needs_revision"' } });
    save();
    expect(writes).toHaveLength(1);
    expect(blockAt(written(), [])!.nodes.ready).toMatchObject({ output: { fields: { status: { type: 'literal', value: 'needs_revision' } } } });
  });
});

describe('field application (F4)', () => {
  it('keeps a rejected branch rename with its error, sends nothing and leaves the document unchanged', async () => {
    const editor = await open(exampleSource('parallel_review'));
    const before = editor.getSnapshot().context.document;
    select('Parallel node reviews');
    const key = inspector().getByLabelText('Rename branch quality') as HTMLInputElement;
    fireEvent.change(key, { target: { value: 'security' } });
    act(() => { key.focus(); });
    save();
    expect(writes).toHaveLength(0);
    expect(editor.getSnapshot().context.document).toBe(before);
    expect((inspector().getByLabelText('Rename branch quality') as HTMLInputElement).value).toBe('security');
    expect(inspector().getByText('That ID is already used in this block.')).toBeTruthy();
    expect(Object.values(editor.getSnapshot().context.drafts)).toMatchObject([{ text: 'security', error: { kind: 'rejected', code: 'duplicate_identifier' } }]);
    // An accepted rename applies, follows its fields and clears exactly its draft.
    fireEvent.change(inspector().getByLabelText('Rename branch quality'), { target: { value: 'clarity' } });
    save();
    expect(writes).toHaveLength(1);
    expect(Object.keys((blockAt(written(), [])!.nodes.reviews as { branches: object }).branches).sort()).toEqual(['clarity', 'security']);
    expect(editor.getSnapshot().context.drafts).toEqual({});
  });

  it('renames a node through the same boundary and the inspector follows it', async () => {
    const editor = await open(exampleSource('parallel_review'));
    select('Return node ready');
    const id = inspector().getByLabelText('Node ID') as HTMLInputElement;
    fireEvent.change(id, { target: { value: 'revise' } });
    fireEvent.keyDown(id, { key: 'Enter' });
    expect(inspector().getByText('That ID is already used in this block.')).toBeTruthy();
    expect(Object.keys(blockAt(editor.getSnapshot().context.document!, [])!.nodes)).toContain('ready');
    fireEvent.change(inspector().getByLabelText('Node ID'), { target: { value: 'accepted' } });
    fireEvent.keyDown(inspector().getByLabelText('Node ID'), { key: 'Enter' });
    expect((inspector().getByLabelText('Node ID') as HTMLInputElement).value).toBe('accepted');
    expect(editor.getSnapshot().context.drafts).toEqual({});
  });
});

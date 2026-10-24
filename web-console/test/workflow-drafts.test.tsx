import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { createActor } from 'xstate';
import { exampleSource } from './workflow-fixtures';
import type { WorkflowDefinition, WorkflowEdit, WorkflowSource } from '../../protocol/app-server/v57';
import type { WorkflowCommit } from '../src/workspaces/host';
import type { WorkspaceAuthority } from '../src/workspaces/authority';
import { blockAt } from '../src/app/workflows/document/model';
import { blockField, describeField, encodeField, nodeField } from '../src/app/workflows/document/fields';
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
    modelChoices: () => new Promise(() => {}), list: () => new Promise(() => {}),
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

const status = (node: string) => nodeField([], node, ['output', 'fields', { key: 'status' }, 'value']);
/** Every retained draft as [text, owner, detached]. */
const retained = (editor: Awaited<ReturnType<typeof open>>) => Object.entries(editor.getSnapshot().context.drafts)
  .map(([key, draft]) => [draft.text, describeField(draft.address), key !== encodeField(draft.address)] as const)
  .sort(([a], [b]) => a.localeCompare(b));
function type(field: HTMLTextAreaElement | HTMLInputElement, text: string) {
  fireEvent.change(field, { target: { value: text } });
  fireEvent.blur(field);
}

describe('rename with retained drafts (repair B)', () => {
  it.each([['the deleted node\'s draft first', false], ['the live node\'s draft first', true]])('keeps a deleted node\'s text apart when another node takes its name, %s', async (_order, liveFirst) => {
    const editor = await open(exampleSource('parallel_review'));
    const leave = (node: string, text: string) => { select(`Return node ${node}`); type(literals()[0], text); };
    if (liveFirst) leave('revise', '"live_revise');
    leave('ready', '"orphan_ready');
    if (!liveFirst) leave('revise', '"live_revise');
    select('Return node ready');
    fireEvent.click(inspector().getByRole('button', { name: 'Delete node' }));
    select('Return node revise');
    const id = inspector().getByLabelText('Node ID') as HTMLInputElement;
    fireEvent.change(id, { target: { value: 'ready' } });
    fireEvent.keyDown(id, { key: 'Enter' });
    // The rename is accepted; the inspector follows the node, which shows
    // its own kept text and never the deleted node's.
    const nodes = blockAt(editor.getSnapshot().context.document!, [])!.nodes;
    expect(Object.keys(nodes).sort()).toEqual(['decision', 'ready', 'reviews']);
    expect(nodes.ready).toMatchObject({ output: { fields: { status: { type: 'literal', value: 'needs_revision' } } } });
    expect((inspector().getByLabelText('Node ID') as HTMLInputElement).value).toBe('ready');
    expect(literals()[0].value).toBe('"live_revise');
    expect(retained(editor)).toEqual([
      ['"live_revise', 'ready · output.fields["status"].value', false],
      ['"orphan_ready', 'ready · output.fields["status"].value', true],
    ]);
    expect(editor.getSnapshot().context.drafts[encodeField(status('ready'))].text).toBe('"live_revise');
    save();
    expect(writes).toHaveLength(0);
    const pending = within(screen.getByRole('list', { name: 'Fields with unapplied text' }));
    expect(pending.getAllByRole('listitem')).toHaveLength(2);
    expect(pending.getByText('This field no longer exists.')).toBeTruthy();
  });

  it.each([['the deleted branch\'s draft first', false], ['the live branch\'s draft first', true]])('keeps a deleted branch\'s text apart when another branch takes its key, %s', async (_order, liveFirst) => {
    const editor = await open(exampleSource('parallel_review'));
    const leave = (branch: 'quality' | 'security', text: string) => {
      select('Parallel node reviews');
      fireEvent.click(inspector().getAllByRole('button', { name: /Edit subgraph/ })[branch === 'quality' ? 0 : 1]);
      fireEvent.click(screen.getByRole('button', { name: 'Block settings' }));
      type(inspector().getByLabelText('Output schema') as HTMLTextAreaElement, text);
      fireEvent.click(screen.getByRole('button', { name: 'Root block' }));
    };
    if (liveFirst) leave('security', '{"live_security"');
    leave('quality', '{"orphan_quality"');
    if (!liveFirst) leave('security', '{"live_security"');
    select('Parallel node reviews');
    fireEvent.click(inspector().getByRole('button', { name: 'Remove branch quality' }));
    const key = inspector().getByLabelText('Rename branch security') as HTMLInputElement;
    fireEvent.change(key, { target: { value: 'quality' } });
    fireEvent.keyDown(key, { key: 'Enter' });
    const reviews = blockAt(editor.getSnapshot().context.document!, [])!.nodes.reviews as { branches: Record<string, { block: { nodes: Record<string, { task?: string }> } }> };
    expect(Object.keys(reviews.branches)).toEqual(['quality']);
    expect(reviews.branches.quality.block.nodes.review.task).toContain('for security');
    const quality = [{ node: 'reviews', branch: 'quality' }];
    expect(editor.getSnapshot().context.drafts[encodeField(blockField(quality, ['output']))].text).toBe('{"live_security"');
    expect(retained(editor)).toEqual([
      ['{"live_security"', 'reviews / quality · output', false],
      ['{"orphan_quality"', 'reviews / quality · output', true],
    ]);
    // The renamed branch's settings show its own kept text.
    fireEvent.click(inspector().getByRole('button', { name: /Edit subgraph/ }));
    fireEvent.click(screen.getByRole('button', { name: 'Block settings' }));
    expect((inspector().getByLabelText('Output schema') as HTMLTextAreaElement).value).toBe('{"live_security"');
    save();
    expect(writes).toHaveLength(0);
  });

  it('moves the drafts of a renamed node to its new name when nothing collides', async () => {
    const editor = await open(exampleSource('parallel_review'));
    select('Return node revise');
    type(literals()[0], '"still typing');
    const id = inspector().getByLabelText('Node ID') as HTMLInputElement;
    fireEvent.change(id, { target: { value: 'rework' } });
    fireEvent.keyDown(id, { key: 'Enter' });
    expect(Object.keys(blockAt(editor.getSnapshot().context.document!, [])!.nodes)).toContain('rework');
    expect(retained(editor)).toEqual([['"still typing', 'rework · output.fields["status"].value', false]]);
    expect(literals()[0].value).toBe('"still typing');
    fireEvent.change(literals()[0], { target: { value: '"reworked"' } });
    save();
    expect(writes).toHaveLength(1);
    expect(blockAt(written(), [])!.nodes.rework).toMatchObject({ output: { fields: { status: { type: 'literal', value: 'reworked' } } } });
  });
});

describe('array edits with retained drafts (repair C)', () => {
  const items = (first: unknown, second: unknown) => {
    const source = collidingFields();
    (source.document!.block.nodes as Record<string, unknown>).done = { type: 'return', output: { type: 'object', fields: { items: { type: 'array', items: [{ type: 'literal', value: first }, { type: 'literal', value: second }] } } } };
    return source;
  };
  const item = (index: number) => within(inspector().getByRole('group', { name: `[${index}]` })).getByLabelText('Literal JSON') as HTMLTextAreaElement;
  const saved = () => (blockAt(written(), [])!.nodes.done as unknown as { output: { fields: { items: { items: unknown[] } } } }).output.fields.items.items;

  it('keeps item 0\'s draft through an append and saves its corrected text there', async () => {
    await open(items({ n: 1 }, { n: 2 }));
    select('Return node done');
    type(item(0), '{"n":');
    fireEvent.click(inspector().getByRole('button', { name: 'Add item' }));
    expect(item(0).value).toBe('{"n":');
    fireEvent.change(item(0), { target: { value: '{"n":3}' } });
    save();
    expect(writes).toHaveLength(1);
    expect(saved()).toEqual([{ type: 'literal', value: { n: 3 } }, { type: 'literal', value: { n: 2 } }, { type: 'reference', path: ['args'] }]);
  });

  it('keeps item 0\'s draft when a later item is removed, and through undo and redo of it', async () => {
    const editor = await open(items({ n: 1 }, { n: 2 }));
    select('Return node done');
    type(item(0), '{"n":');
    fireEvent.click(inspector().getByRole('button', { name: 'Remove [1]' }));
    expect(item(0).value).toBe('{"n":');
    act(() => { editor.send({ type: 'UNDO' }); });
    expect(item(0).value).toBe('{"n":');
    expect(item(1).value).toBe(JSON.stringify({ n: 2 }, null, 2));
    act(() => { editor.send({ type: 'REDO' }); });
    expect(item(0).value).toBe('{"n":');
    fireEvent.change(item(0), { target: { value: '{"n":3}' } });
    save();
    expect(saved()).toEqual([{ type: 'literal', value: { n: 3 } }]);
  });

  it('moves a later item\'s draft with its item when an earlier identical item is removed', async () => {
    const editor = await open(items({ n: 1 }, { n: 1 }));
    select('Return node done');
    type(item(1), '{"n":');
    fireEvent.click(inspector().getByRole('button', { name: 'Remove [0]' }));
    // The surviving item is now [0], and its text went with it.
    expect(item(0).value).toBe('{"n":');
    expect(retained(editor)).toEqual([['{"n":', 'done · output.fields["items"].items[0].value', false]]);
    fireEvent.change(item(0), { target: { value: '{"n":4}' } });
    save();
    expect(saved()).toEqual([{ type: 'literal', value: { n: 4 } }]);
  });

  it('detaches the text of a removed item instead of giving it to the identical item now at its index', async () => {
    const editor = await open(items({ n: 1 }, { n: 1 }));
    select('Return node done');
    type(item(0), '{"n":');
    fireEvent.click(inspector().getByRole('button', { name: 'Remove [0]' }));
    expect(item(0).value).toBe(JSON.stringify({ n: 1 }, null, 2));
    expect(retained(editor)).toEqual([['{"n":', 'done · output.fields["items"].items[0].value', true]]);
    save();
    expect(writes).toHaveLength(0);
    expect(within(screen.getByRole('list', { name: 'Fields with unapplied text' })).getByText('This field no longer exists.')).toBeTruthy();
  });

  const undo = () => fireEvent.keyDown(document.body, { key: 'z', ctrlKey: true });
  const redo = () => fireEvent.keyDown(document.body, { key: 'z', ctrlKey: true, shiftKey: true });
  const itemGroup = (index: number) => within(inspector().getByRole('group', { name: `[${index}]` }));
  const pretty = (value: unknown) => JSON.stringify(value, null, 2);
  /** Item 0 changed to {n:10} and applied, then its incomplete text left;
   * Undo of the applied change marks that text `changed`. */
  const changedOnFirst = (survivorFirst: boolean) => {
    if (survivorFirst) type(item(1), '{"n":202');
    type(item(0), '{"n":10}');
    type(item(0), '{"n":101');
    if (!survivorFirst) type(item(1), '{"n":202');
    undo();
    expect(item(0).value).toBe('{"n":101');
    expect(itemGroup(0).getByRole('button', { name: 'Keep my text' })).toBeTruthy();
  };

  it.each([['the deleted item\'s draft first', false], ['the surviving item\'s draft first', true]])('detaches a changed draft with its removed item and moves the survivor\'s draft with it, %s', async (_order, survivorFirst) => {
    const editor = await open(items({ n: 1 }, { n: 2 }));
    select('Return node done');
    changedOnFirst(survivorFirst);
    fireEvent.click(inspector().getByRole('button', { name: 'Remove [0]' }));
    // The survivor is [0] and shows its own text; the removed item's text
    // is kept apart, still marked by its content conflict as detached.
    expect(item(0).value).toBe('{"n":202');
    expect(itemGroup(0).queryByRole('button', { name: 'Keep my text' })).toBeNull();
    expect(retained(editor)).toEqual([
      ['{"n":101', 'done · output.fields["items"].items[0].value', true],
      ['{"n":202', 'done · output.fields["items"].items[0].value', false],
    ]);
    // Undo restores the removed item; its text stays detached, and the
    // survivor's text follows the survivor back to [1].
    undo();
    expect(item(0).value).toBe(pretty({ n: 1 }));
    expect(item(1).value).toBe('{"n":202');
    expect(retained(editor)).toEqual([
      ['{"n":101', 'done · output.fields["items"].items[0].value', true],
      ['{"n":202', 'done · output.fields["items"].items[1].value', false],
    ]);
    redo();
    expect(item(0).value).toBe('{"n":202');
    fireEvent.change(item(0), { target: { value: '{"n":22}' } });
    save();
    expect(writes).toHaveLength(0);
    const pending = within(screen.getByRole('list', { name: 'Fields with unapplied text' }));
    fireEvent.click(within(pending.getAllByRole('listitem').find(entry => entry.textContent!.includes('no longer exists'))!).getByRole('button', { name: 'Discard' }));
    save();
    expect(writes).toHaveLength(1);
    expect(saved()).toEqual([{ type: 'literal', value: { n: 22 } }]);
  });

  it('never gives a removed item\'s changed draft to the item now at its index', async () => {
    const editor = await open(items({ n: 1 }, { n: 2 }));
    select('Return node done');
    type(item(0), '{"n":10}');
    type(item(0), '{"n":101');
    undo();
    fireEvent.click(inspector().getByRole('button', { name: 'Remove [0]' }));
    expect(item(0).value).toBe(pretty({ n: 2 }));
    expect(inspector().queryByRole('button', { name: 'Keep my text' })).toBeNull();
    expect(retained(editor)).toEqual([['{"n":101', 'done · output.fields["items"].items[0].value', true]]);
    save();
    expect(writes).toHaveLength(0);
    fireEvent.click(within(screen.getByRole('list', { name: 'Fields with unapplied text' })).getByRole('button', { name: 'Discard' }));
    save();
    expect(writes).toHaveLength(1);
    expect(saved()).toEqual([{ type: 'literal', value: { n: 2 } }]);
  });

  it('moves a later item\'s changed draft back to it when Undo restores an earlier item, and with Redo', async () => {
    const editor = await open(items({ n: 1 }, { n: 2 }));
    select('Return node done');
    fireEvent.click(inspector().getByRole('button', { name: 'Remove [0]' }));
    type(item(0), '{"n":20}');
    type(item(0), '{"n":203');
    undo();
    expect(itemGroup(0).getByRole('button', { name: 'Keep my text' })).toBeTruthy();
    undo();
    // A is back at [0]; B and its text are at [1].
    expect(item(0).value).toBe(pretty({ n: 1 }));
    expect(item(1).value).toBe('{"n":203');
    expect(itemGroup(0).queryByRole('button', { name: 'Keep my text' })).toBeNull();
    expect(retained(editor)).toEqual([['{"n":203', 'done · output.fields["items"].items[1].value', false]]);
    redo();
    expect(item(0).value).toBe('{"n":203');
    expect(retained(editor)).toEqual([['{"n":203', 'done · output.fields["items"].items[0].value', false]]);
    undo();
    expect(item(1).value).toBe('{"n":203');
    fireEvent.click(itemGroup(1).getByRole('button', { name: 'Keep my text' }));
    fireEvent.change(item(1), { target: { value: '{"n":203}' } });
    save();
    expect(writes).toHaveLength(1);
    expect(saved()).toEqual([{ type: 'literal', value: { n: 1 } }, { type: 'literal', value: { n: 203 } }]);
  });
});

describe('recovering detached text (F4)', () => {
  /** Incomplete JSON with inner whitespace and newlines, exactly as typed. */
  const kept = '"half typed\n  {  \n\tstill';
  const location = 'ready · output.fields["status"].value';
  const undo = () => fireEvent.keyDown(document.body, { key: 'z', ctrlKey: true });
  const pending = () => within(screen.getByRole('list', { name: 'Fields with unapplied text' }));
  const detachedEntry = () => within(pending().getAllByRole('listitem').find(entry => entry.textContent!.includes('no longer exists'))!);
  let clipboard: string[];
  let refuse: boolean;
  beforeEach(() => {
    clipboard = []; refuse = false;
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async (text: string) => { if (refuse) throw new Error('denied'); clipboard.push(text); } } });
  });
  afterEach(() => { Reflect.deleteProperty(navigator, 'clipboard'); });
  const reveal = () => {
    save();
    fireEvent.click(detachedEntry().getByRole('button', { name: 'Show kept text' }));
    return screen.getByRole('textbox', { name: `Kept text of ${location}` }) as HTMLTextAreaElement;
  };

  it('shows and copies the exact text kept from a deleted node after Undo, beside the live field\'s own text, and discards only it', async () => {
    const editor = await open(exampleSource('parallel_review'));
    select('Return node ready');
    type(literals()[0], kept);
    fireEvent.click(inspector().getByRole('button', { name: 'Delete node' }));
    undo();
    // The node is back; its field shows its value, not the kept text, and
    // takes its own new draft.
    select('Return node ready');
    expect(literals()[0].value).toBe('"ready"');
    type(literals()[0], '"live');
    const text = reveal();
    expect(writes).toHaveLength(0);
    expect(text.value).toBe(kept);
    expect(text.readOnly).toBe(true);
    const before = editor.getSnapshot().context.drafts;
    await act(async () => { fireEvent.click(detachedEntry().getByRole('button', { name: 'Copy' })); });
    expect(clipboard).toEqual([kept]);
    expect(detachedEntry().getByRole('status').textContent).toMatch(/Copied/);
    // A refused clipboard leaves the text shown for manual selection.
    refuse = true;
    await act(async () => { fireEvent.click(detachedEntry().getByRole('button', { name: 'Copy' })); });
    expect(detachedEntry().getByRole('status').textContent).toMatch(/Could not copy/);
    expect((screen.getByRole('textbox', { name: `Kept text of ${location}` }) as HTMLTextAreaElement).value).toBe(kept);
    // Showing and copying changed nothing and sent nothing.
    expect(editor.getSnapshot().context.drafts).toBe(before);
    expect(writes).toHaveLength(0);
    expect(literals()[0].value).toBe('"live');
    fireEvent.click(detachedEntry().getByRole('button', { name: 'Discard' }));
    expect(retained(editor)).toEqual([['"live', location, false]]);
    expect(screen.queryByRole('textbox', { name: `Kept text of ${location}` })).toBeNull();
    fireEvent.change(literals()[0], { target: { value: '"live"' } });
    save();
    expect(writes).toHaveLength(1);
    expect(blockAt(written(), [])!.nodes.ready).toMatchObject({ output: { fields: { status: { type: 'literal', value: 'live' } } } });
  });

  it('shows the exact text a rename took along after Undo of the rename, and clears it only on discard', async () => {
    const editor = await open(exampleSource('parallel_review'));
    select('Return node ready');
    type(literals()[0], kept);
    const id = inspector().getByLabelText('Node ID') as HTMLInputElement;
    fireEvent.change(id, { target: { value: 'accepted' } });
    fireEvent.keyDown(id, { key: 'Enter' });
    undo();
    select('Return node ready');
    expect(literals()[0].value).toBe('"ready"');
    expect(retained(editor)).toEqual([[kept, 'accepted · output.fields["status"].value', true]]);
    save();
    fireEvent.click(detachedEntry().getByRole('button', { name: 'Show kept text' }));
    expect((screen.getByRole('textbox', { name: 'Kept text of accepted · output.fields["status"].value' }) as HTMLTextAreaElement).value).toBe(kept);
    expect(isDirty(editor.getSnapshot().context)).toBe(true);
    expect(writes).toHaveLength(0);
    fireEvent.click(detachedEntry().getByRole('button', { name: 'Discard' }));
    expect(editor.getSnapshot().context.drafts).toEqual({});
    expect(isDirty(editor.getSnapshot().context)).toBe(false);
  });
});

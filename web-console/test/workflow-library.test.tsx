import { afterEach, describe, expect, it } from 'vitest';
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { createActor } from 'xstate';
import type { WorkflowEdit, WorkflowLibrary } from '../../protocol/app-server/v57';
import type { WorkflowCommit } from '../src/workspaces/host';
import type { WorkspaceAuthority } from '../src/workspaces/authority';
import { workflowSurfaceMachine } from '../src/app/workflows/machines/surface';
import { WorkflowOperationError, type WorkflowScope, type WorkflowScopePort } from '../src/app/workflows/port';
import { WorkflowLibrary as Library } from '../src/app/workflows/ui/library/Library';

/** Every ordering below is established by a deferred promise the test
 * releases itself: no sleeps, timers or scheduling assumptions. */
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
  promise.catch(() => {});
  return { promise, resolve, reject };
}
const flush = () => act(async () => { for (let turn = 0; turn < 16; turn++) await Promise.resolve(); });

/** The Global scope served by one Product Host/App Server authority. Once
 * replaced, its ports refuse every operation, as the real port does. */
interface Authority {
  readonly name: string;
  replaced: boolean;
  lists: ReturnType<typeof deferred<WorkflowLibrary>>[];
  writes: { id: string; expected: string; edit: WorkflowEdit; result: ReturnType<typeof deferred<WorkflowCommit>> }[];
}
function authorityNamed(name: string): Authority {
  return { name, replaced: false, lists: [], writes: [] };
}
function portOf(authority: Authority, scope: WorkflowScope): WorkflowScopePort {
  const refuse = () => Promise.reject(new WorkflowOperationError({ kind: 'unavailable', message: 'product_host_replaced' }));
  return {
    endpoint: `ws://${authority.name}/`, scope,
    modelChoices: () => new Promise(() => {}), list: () => { if (authority.replaced) return refuse(); const result = deferred<WorkflowLibrary>(); authority.lists.push(result); return result.promise; },
    read: () => new Promise(() => {}),
    check: () => new Promise(() => {}),
    write: (id, expected, edit) => {
      if (authority.replaced) return refuse();
      const result = deferred<WorkflowCommit>();
      authority.writes.push({ id, expected, edit, result });
      return result.promise;
    },
  };
}
const listing = (authority: Authority): WorkflowLibrary => ({
  target: { kind: 'user' }, directory: `/home/${authority.name}/.agents/workflows`, absent_revision: `missing-${authority.name}`, diagnostic: null,
  files: [{ file_name: 'flow.yaml', id: 'flow', relative_path: '.agents/workflows/flow.yaml', revision: `sha256:${authority.name}`, byte_length: '1', modified_ms: null, description: `${authority.name} flow`, nodes: 1, diagnostic: null, shadows_global: false }],
});
const host = { subscribe: () => () => {}, getCatalog: () => undefined, observe: () => new Promise(() => {}) } as unknown as WorkspaceAuthority;

function mount() {
  const a = authorityNamed('a'), b = authorityNamed('b');
  let current = a;
  const surface = createActor(workflowSurfaceMachine, { input: { ports: scope => portOf(current, scope) } }).start();
  const library = () => surface.getSnapshot().context.library;
  const show = () => render(<Library surface={surface} authority={host} />);
  /** Authority B replaces A behind the same address. */
  const replace = () => { a.replaced = true; current = b; };
  return { a, b, surface, library, show, replace };
}
afterEach(cleanup);

const dialog = () => within(screen.getByRole('dialog'));
const newButton = () => screen.getByRole('button', { name: 'New Workflow' }) as HTMLButtonElement;
function fill(id: string, description: string) {
  fireEvent.change(dialog().getByLabelText(/^Workflow ID/), { target: { value: id } });
  fireEvent.change(dialog().getByLabelText(/^Description/), { target: { value: description } });
}

describe('accepted listing observations (F2)', () => {
  it('keeps a confirmation opened before a refresh bound to its own observation, and never writes to the replacement', async () => {
    const { a, b, library, show, replace } = mount();
    show();
    a.lists[0].resolve(listing(a));
    await flush();
    fireEvent.click(newButton());
    expect(dialog().getByText('/home/a/.agents/workflows')).toBeTruthy();
    // A refresh starts after authority B replaced A, and completes.
    replace();
    act(() => library().send({ type: 'REFRESH' }));
    expect(newButton().disabled).toBe(true);
    b.lists[0].resolve(listing(b));
    await flush();
    expect(screen.getByText('b flow')).toBeTruthy();
    // The open confirmation still shows and targets A.
    expect(dialog().getByText('/home/a/.agents/workflows')).toBeTruthy();
    fill('fresh', 'Fresh');
    fireEvent.click(dialog().getByRole('button', { name: 'Create' }));
    await flush();
    // A's port refuses; nothing reaches B, and the dialog reports it.
    expect(b.writes).toHaveLength(0);
    expect(a.writes).toHaveLength(0);
    expect(screen.getByRole('dialog')).toBeTruthy();
    expect(dialog().getByRole('alert').textContent).toBeTruthy();
    expect(library().getSnapshot().context.operationFailure?.failure).toEqual({ kind: 'unavailable', message: 'product_host_replaced' });
  });

  it('starts no confirmation from an old listing while its replacement read is pending', async () => {
    const { a, b, library, show, replace } = mount();
    const view = show();
    a.lists[0].resolve(listing(a));
    await flush();
    // Leave the library, B replaces A, and return: B's listing is pending
    // while A's is still shown.
    view.unmount();
    replace();
    show();
    expect(b.lists).toHaveLength(1);
    expect(screen.getByText('a flow')).toBeTruthy();
    expect(newButton().disabled).toBe(true);
    fireEvent.click(newButton());
    // The machine enforces it too, for any sender.
    act(() => library().send({ type: 'CONFIRM', kind: 'create' }));
    act(() => library().send({ type: 'CONFIRM', kind: 'delete', file: 'flow.yaml' }));
    act(() => library().send({ type: 'OPEN', file: 'flow.yaml' }));
    expect(library().getSnapshot().context.confirmation).toBeUndefined();
    expect(screen.queryByRole('dialog')).toBeNull();
    b.lists[0].resolve(listing(b));
    await flush();
    fireEvent.click(newButton());
    expect(dialog().getByText('/home/b/.agents/workflows')).toBeTruthy();
    fill('fresh', 'Fresh');
    fireEvent.click(dialog().getByRole('button', { name: 'Create' }));
    expect(b.writes).toMatchObject([{ id: 'fresh', expected: 'missing-b', edit: { operation: 'create', description: 'Fresh' } }]);
  });

  it('copies and deletes the file and revision of the confirmation\'s own observation', async () => {
    const { a, b, library, show } = mount();
    show();
    a.lists[0].resolve(listing(a));
    await flush();
    act(() => library().send({ type: 'CONFIRM', kind: 'delete', file: 'flow.yaml' }));
    // A refresh lists a newer revision of the same file meanwhile.
    act(() => library().send({ type: 'REFRESH' }));
    a.lists[1].resolve({ ...listing(a), files: [{ ...listing(a).files[0], revision: 'sha256:newer' }] });
    await flush();
    fireEvent.click(dialog().getByRole('button', { name: 'Delete' }));
    expect(a.writes).toMatchObject([{ id: 'flow', expected: 'sha256:a', edit: { operation: 'delete' } }]);
    expect(b.writes).toHaveLength(0);
  });

  it('keeps a dispatched write and its outcome through refresh and scope navigation, and never resends it', async () => {
    const { a, surface, library, show } = mount();
    show();
    a.lists[0].resolve(listing(a));
    await flush();
    fireEvent.click(newButton());
    fill('fresh', 'Fresh');
    fireEvent.click(dialog().getByRole('button', { name: 'Create' }));
    expect(a.writes).toHaveLength(1);
    const port = library().getSnapshot().context.pending!.port;
    act(() => {
      library().send({ type: 'REFRESH' });
      library().send({ type: 'SCOPE', scope: { kind: 'workspace', workspaceId: 'w' } });
      library().send({ type: 'ACTIVATE' });
    });
    expect(library().getSnapshot().matches('operating')).toBe(true);
    expect(library().getSnapshot().context.pending?.port).toBe(port);
    expect(a.writes).toHaveLength(1);
    a.writes[0].result.resolve({ acknowledgement: { revision: 'sha256:fresh', configuration_revision: null }, reread: { status: 'failed', error: 'x' } });
    await flush();
    // The acknowledged create opens exactly the created file through its port.
    const editor = surface.getSnapshot().context.editor!;
    expect(editor.getSnapshot().context.target).toEqual({ endpoint: 'ws://a/', scope: { kind: 'user' }, id: 'fresh' });
    expect(editor.getSnapshot().context.port).toBe(port);
    // The scope chosen meanwhile is selected after the outcome.
    expect(library().getSnapshot().context.scope).toEqual({ kind: 'workspace', workspaceId: 'w' });
    expect(a.writes).toHaveLength(1);
  });

  it('observes an uncertain write with the next listing, without claiming success', async () => {
    const { a, library, show } = mount();
    show();
    a.lists[0].resolve(listing(a));
    await flush();
    fireEvent.click(newButton());
    fill('fresh', 'Fresh');
    fireEvent.click(dialog().getByRole('button', { name: 'Create' }));
    a.writes[0].result.reject(new WorkflowOperationError({ kind: 'uncertain', message: 'outcome_unknown' }));
    await flush();
    expect(library().getSnapshot().context.operationFailure?.failure.kind).toBe('uncertain');
    expect(a.lists).toHaveLength(2);
    expect(a.writes).toHaveLength(1);
    // The dialog stays open with the failure; it never reads as success.
    expect(screen.getByRole('dialog')).toBeTruthy();
  });

  it.each(['create', 'copy', 'delete'] as const)('requires an explicit new transaction after an uncertain %s, before and after observation', async kind => {
    const { a, library, show, surface } = mount();
    show();
    a.lists[0].resolve(listing(a));
    await flush();
    act(() => library().send({ type: 'CONFIRM', kind, file: kind === 'create' ? undefined : 'flow.yaml' }));
    if (kind === 'create') fill('fresh', 'Fresh');
    else if (kind === 'copy') fireEvent.change(dialog().getByLabelText(/^Workflow ID/), { target: { value: 'fresh' } });
    const label = { create: 'Create', copy: 'Copy', delete: 'Delete' }[kind];
    fireEvent.click(dialog().getByRole('button', { name: label }));
    const pending = library().getSnapshot().context.pending!;
    a.writes[0].result.reject(new WorkflowOperationError({ kind: 'uncertain', message: 'outcome_unknown' }));
    await flush();
    expect(library().getSnapshot().context.pending).toBeUndefined();
    expect(library().getSnapshot().context.confirmation?.uncertain).toBe(pending);
    const cannotReplay = () => {
      const button = dialog().getByRole('button', { name: label }) as HTMLButtonElement;
      expect(button.disabled).toBe(true);
      fireEvent.click(button);
      const form = screen.getByRole('dialog').querySelector('form');
      if (form) fireEvent.submit(form);
      act(() => library().send({ type: 'SUBMIT', seq: pending.confirmation, id: 'fresh', description: 'Fresh' }));
      expect(a.writes).toHaveLength(1);
    };
    // The observation is held pending, then released with the write's apparent
    // effect. Neither phase authorizes replay or proves who changed the file.
    expect(library().getSnapshot().matches('loading')).toBe(true);
    cannotReplay();
    const observed = listing(a);
    observed.files = kind === 'delete' ? [] : [...observed.files, { ...observed.files[0], id: 'fresh', file_name: 'fresh.yaml' }];
    a.lists[1].resolve(observed);
    await flush();
    expect(library().getSnapshot().matches('ready')).toBe(true);
    cannotReplay();
    expect(library().getSnapshot().context.confirmation?.uncertain).toBe(pending);
    expect(surface.getSnapshot().context.editor).toBeUndefined();
    // A failed read is no stronger evidence than a successful one. The same
    // confirmation remains blocked through failure and another refresh.
    act(() => library().send({ type: 'REFRESH' }));
    a.lists[2].reject(new Error('observation unavailable'));
    await flush();
    expect(library().getSnapshot().matches('failed')).toBe(true);
    cannotReplay();
    act(() => library().send({ type: 'REFRESH' }));
    a.lists[3].resolve(observed);
    await flush();
    cannotReplay();
    fireEvent.click(dialog().getByRole('button', { name: 'Cancel' }));
    fireEvent.click(newButton());
    fill('another', 'Another transaction');
    expect(library().getSnapshot().context.confirmation!.seq).toBeGreaterThan(pending.confirmation);
    fireEvent.click(dialog().getByRole('button', { name: 'Create' }));
    expect(a.writes).toHaveLength(2);
    expect(a.writes[1]).toMatchObject({ id: 'another', expected: 'missing-a', edit: { operation: 'create', description: 'Another transaction' } });
  });

  it('never lets a superseded listing overwrite a newer accepted read', async () => {
    const { a, library, show } = mount();
    show();
    const older = a.lists[0];
    act(() => library().send({ type: 'REFRESH' }));
    a.lists[1].resolve({ ...listing(a), directory: '/newer' });
    await flush();
    older.resolve({ ...listing(a), directory: '/older' });
    await flush();
    expect(library().getSnapshot().context.observation?.listing.directory).toBe('/newer');
  });
});

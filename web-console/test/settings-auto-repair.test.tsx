// @vitest-environment jsdom
/** Issue #464: native automatic configuration repair in the Web console —
 * source-scoped client observations, one nonblocking notification per
 * receipt identity, Settings → Advanced → Auto Repair, exact owner
 * navigation, and the Settings transaction fences a receipt never crosses. */
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createActor } from 'xstate';
import type { ConfigRepairReceipt, ConfigurationApplication, Request, SourceSettings, SourceTarget } from '../../protocol/app-server/v47';
import { RepairNotice, repairNoticeText } from '../src/app/RepairNotice';
import { repairChangeLabel } from '../src/app/settings/advanced/AutoRepair';
import { inspectionAcknowledger, RepairAcknowledgements } from '../src/app/repair-notices';
import { settingsNavigationMachine, type OwnerResolution, type RepairInspection } from '../src/app/settings/machines/navigation';
import { RpcFailure } from '../src/client/app-server';
import type { ProductHostWorkspaces } from '../src/workspaces/host';
import { userSettingsTarget, workspaceSettingsTarget } from '../src/app/settings/projection';
import { translator } from '../src/locale/translation';
import { cfg3Source } from './cfg3-data';
import { endpoint, Server, TOKEN } from './fixture';
import { openSettingsPage, settingsReady, SettingsSurface } from './settings-harness';

beforeEach(() => localStorage.clear());
afterEach(cleanup);

const workspace: SourceTarget = { kind: 'workspace', directory: '/workspace/A' };
function receipt(overrides: Partial<ConfigRepairReceipt> = {}): ConfigRepairReceipt {
  return {
    id: 'receipt-user-1', target: { kind: 'user' }, status: 'repaired',
    before_revision: 'sha256:before', after_revision: 'sha256:after', changed_count: 3,
    changes: [
      { kind: 'updated_schema_version', from: 8, to: 10 },
      { kind: 'removed_field', object_path: 'agent.plugins.todo' },
      { kind: 'removed_native_tool', position: 1, name: null, reason: 'unknown' },
    ],
    truncated: false, backup_available: true, diagnostic: null, ...overrides,
  };
}
/** Native ledger sequences of announced observations, in announcement order. */
let announcedSequence = 0;
async function announce(s: Server, target: SourceTarget, value: ConfigRepairReceipt | null) {
  await act(async () => { s.socket.deliver({ jsonrpc: '2.0', method: 'configuration/sourceRepairChanged', params: { target, receipt: value, sequence: String(++announcedSequence) } }); });
}
const sent = (s: Server, method: Request['method']) => s.requests.filter(item => item.request.method === method).map(item => item.request);

it('keeps source repair observations source-scoped, deduplicated and per connection', async () => {
  const s = new Server(); await s.connect();
  const listener = vi.fn(); s.client.subscribe(listener);
  await announce(s, { kind: 'user' }, receipt());
  const observed = s.client.getSnapshot().sourceRepairs;
  expect(observed).toEqual({ user: { authorityId: s.authorityId, target: { kind: 'user' }, receipt: receipt() } });
  const publications = listener.mock.calls.length;
  // The same receipt again is no change.
  await announce(s, { kind: 'user' }, receipt());
  expect(listener.mock.calls.length).toBe(publications);
  expect(s.client.getSnapshot().sourceRepairs).toBe(observed);
  await announce(s, workspace, receipt({ id: 'w', target: workspace, status: 'unresolved', after_revision: null }));
  await announce(s, workspace, null);
  expect(s.client.getSnapshot().sourceRepairs?.['workspace:/workspace/A']).toEqual({ authorityId: s.authorityId, target: workspace, receipt: undefined });
  // Never Session, application or conversation state.
  expect(s.client.getSnapshot().configuration).toEqual({});
  expect(Object.keys(s.client.getSnapshot().views)).toEqual([]);
  // A new connection starts empty; its server announces again.
  s.client.disconnect(); await s.connect();
  expect(s.client.getSnapshot().sourceRepairs).toEqual({});
  s.client.disconnect();
});

it('words the notification for repaired, combined, unresolved and uncertain receipts', () => {
  const tx = translator('en');
  expect(repairNoticeText(tx, [receipt()])).toBe('Configuration automatically repaired · 3 changes in User configuration');
  expect(repairNoticeText(tx, [receipt(), receipt({ id: 'w', target: workspace, changed_count: 1 })]))
    .toBe('Configuration automatically repaired · 3 changes in User configuration and 1 change in Workspace configuration');
  expect(repairNoticeText(tx, [receipt(), receipt({ id: 'w', target: workspace, status: 'unresolved' })]))
    .toBe('Workspace configuration could not be repaired automatically');
  expect(repairNoticeText(tx, [receipt({ status: 'commit_uncertain' })])).toBe('User configuration repair was not durably confirmed');
});

const notice = () => screen.queryByRole('status', { name: 'Configuration repair' }) ?? screen.queryByRole('alert', { name: 'Configuration repair' });
const identity = (receiptId: string, authorityId = 'authority-1') => ({ authorityId, receiptId });
async function replaceProcess(s: Server, authorityId: string) {
  s.client.disconnect();
  s.authorityId = authorityId;
  await s.connect();
}

/** What native currently answers for each source's authoritative read. */
interface NativeSources {
  user?: ConfigRepairReceipt;
  workspace?: ConfigRepairReceipt;
  /** The authoritative read fails. */
  unreadable?: boolean;
}

/** Settings, the repair notice and the shared acknowledgements composed as
 * `App` composes them: the real navigation machine — here with a controlled
 * owner lookup — completes a View details inspection through
 * `inspectionAcknowledger`, and the real Settings surface reads each source
 * authoritatively from the fixture App Server. */
async function inspectionShell(native: NativeSources = {}) {
  const s = new Server();
  s.handlers.set('configuration/sourcesRead', request => {
    if (native.unreadable) throw new RpcFailure({ code: -32000, message: 'unavailable', data: { kind: 'invalid_state' } });
    const target = (request.params as { target: SourceTarget }).target;
    const projection = cfg3Source();
    projection.target = target;
    projection.user_repair = native.user ?? null;
    projection.workspace_repair = target.kind === 'workspace' ? native.workspace ?? null : null;
    return { type: 'source_settings', projection };
  });
  const lookups: { directory: string; answer: (resolution: OwnerResolution) => void }[] = [];
  const directory = (id: string) => id === 'wA' ? '/workspace/A' : '/workspace/B';
  // The Product Host performs Workspace source I/O; here it reads through the
  // fixture App Server, exactly as the Host does.
  const host: ProductHostWorkspaces = { ...s.workspaceHost, resolveWorkspace: async id => ({ cwd: directory(id) }),
    configureWorkspace: async (id, _endpoint, operation) => {
      if (operation.kind !== 'read') throw new Error('read only');
      const { projection } = await s.client.request({ method: 'configuration/sourcesRead', params: { target: { kind: 'workspace', directory: directory(id) } } }, 'source_settings');
      return { kind: 'read', projection };
    } };
  await s.connect();
  const acknowledgements = new RepairAcknowledgements(localStorage);
  const navigation = createActor(settingsNavigationMachine, { input: {
    lookup: directory => new Promise<OwnerResolution>(answer => lookups.push({ directory, answer })),
    inspected: inspectionAcknowledger(s.client, acknowledgements),
  } });
  navigation.start();
  const viewDetails = vi.fn(({ source, authorityId, receiptId }: RepairInspection) => navigation.send({ type: 'OPEN.REPAIRS', source, inspection: { authorityId, receiptId } }));
  render(<>
    <RepairNotice client={s.client} acknowledgements={acknowledgements} viewDetails={viewDetails} />
    <SettingsSurface client={s.client} host={host} target={userSettingsTarget} navigation={navigation} />
  </>);
  if (native.user) await announce(s, { kind: 'user' }, native.user);
  if (native.workspace) await announce(s, workspace, native.workspace);
  /** Answer the owner lookup in flight for `directory`. */
  const resolve = async (resolution: OwnerResolution) => {
    const lookup = lookups.shift()!;
    expect(lookup.directory).toBe('/workspace/A');
    await act(async () => lookup.answer(resolution));
  };
  /** The Auto Repair details on screen, once their authoritative read lands. */
  const details = () => screen.findByRole('region', { name: 'Auto Repair' });
  const acknowledged = (receiptId: string) => acknowledgements.has({ authorityId: s.client.getSnapshot().authorityId!, receiptId });
  return { s, navigation, acknowledgements, viewDetails, lookups, resolve, details, acknowledged };
}

const workspaceReceipt = (overrides: Partial<ConfigRepairReceipt> = {}) => receipt({ id: 'w', target: workspace, changed_count: 1, ...overrides });
const unresolved = { status: 'unresolved', after_revision: null, backup_available: false, diagnostic: 'invalid value' } as const;
const viewDetails = () => fireEvent.click(within(notice()!).getByRole('button', { name: 'View details' }));

it('notifies once per receipt identity and is silent when healthy', async () => {
  const shell = await inspectionShell();
  // A healthy source has no receipt: nothing is presented.
  expect(notice()).toBeNull();
  await announce(shell.s, { kind: 'user' }, receipt());
  await announce(shell.s, workspace, workspaceReceipt());
  const presented = screen.getByRole('status', { name: 'Configuration repair' });
  expect(presented.textContent).toMatch(/3 changes in User configuration and 1 change in Workspace configuration/);
  // Nonblocking: no dialog, nothing modal, no navigation until asked.
  expect(screen.queryByRole('dialog')).toBeNull();
  expect(shell.viewDetails).not.toHaveBeenCalled();
  expect(sent(shell.s, 'configuration/sourcesRead')).toHaveLength(0);
  shell.s.client.disconnect();
});

describe('View details acknowledges a receipt only once its exact details are presented', () => {
  it('User details opened: only the User receipt is acknowledged, after they are presented', async () => {
    const shell = await inspectionShell({ user: receipt(), workspace: workspaceReceipt() });
    viewDetails();
    expect(shell.viewDetails).toHaveBeenCalledWith({ authorityId: 'fixture-app-server-authority', receiptId: 'receipt-user-1', source: { kind: 'user' } });
    // Navigating is not inspecting: nothing is acknowledged before the read.
    expect(shell.acknowledged('receipt-user-1')).toBe(false);
    const details = await shell.details();
    expect(within(details).getByRole('status').textContent).toBe('Automatically repaired: 3 changes committed.');
    await waitFor(() => expect(shell.acknowledged('receipt-user-1')).toBe(true));
    expect(shell.acknowledged('w')).toBe(false);
    expect(notice()?.textContent).toMatch(/^Configuration automatically repaired · 1 change in Workspace configuration/);
    expect(shell.navigation.getSnapshot().context.inspection).toBeUndefined();
    // Reconnecting to the same authority: the User stays acknowledged, the
    // Workspace stays presented.
    shell.s.client.disconnect(); await shell.s.connect();
    await announce(shell.s, { kind: 'user' }, receipt());
    await announce(shell.s, workspace, workspaceReceipt());
    expect(notice()?.textContent).toMatch(/^Configuration automatically repaired · 1 change in Workspace configuration/);
    expect(sent(shell.s, 'configuration/sourceWrite')).toHaveLength(0);
    shell.s.client.disconnect();
  });

  it('Workspace owner resolved: acknowledged only once its details are presented', async () => {
    const shell = await inspectionShell({ user: receipt(), workspace: workspaceReceipt(unresolved) });
    // Failure first: the Workspace is the subject.
    expect(screen.getByRole('alert', { name: 'Configuration repair' }).textContent).toMatch(/Workspace configuration could not be repaired automatically/);
    shell.s.held.add('configuration/sourcesRead');
    const reading = shell.s.waitFor('configuration/sourcesRead');
    viewDetails();
    expect(shell.lookups).toHaveLength(1);
    expect(shell.navigation.getSnapshot().context.pendingInspection).toMatchObject({ receiptId: 'w' });
    expect(shell.acknowledged('w')).toBe(false);
    await shell.resolve({ kind: 'resolved', id: 'wA', displayName: 'Workspace A' });
    // Landed at the exact owner, but its details are not presented yet.
    const read = await reading;
    expect(shell.navigation.getSnapshot().context).toMatchObject({ page: 'advanced', inspection: { receiptId: 'w' } });
    expect(shell.acknowledged('w')).toBe(false);
    shell.s.held.delete('configuration/sourcesRead');
    await act(async () => shell.s.reply(read));
    const details = await shell.details();
    expect(within(details).getByRole('alert').textContent).toBe('Automatic repair could not fix this source. It is unchanged.');
    await waitFor(() => expect(shell.acknowledged('w')).toBe(true));
    expect(shell.acknowledged('receipt-user-1')).toBe(false);
    expect(screen.getByRole('status', { name: 'Configuration repair' }).textContent).toMatch(/^Configuration automatically repaired · 3 changes in User configuration/);
    shell.s.client.disconnect();
  });

  for (const [name, resolution] of [
    ['an unregistered Workspace', { kind: 'unregistered', directory: '/workspace/A' }],
    ['a failed owner lookup', { kind: 'failed', message: 'catalog unavailable' }],
    ['a retired owner lookup', { kind: 'retired' }],
  ] as const) {
    it(`${name} acknowledges nothing`, async () => {
      const shell = await inspectionShell({ workspace: workspaceReceipt() });
      viewDetails();
      await shell.resolve(resolution);
      expect(shell.navigation.getSnapshot().context.pendingInspection).toBeUndefined();
      expect(shell.navigation.getSnapshot().context.inspection).toBeUndefined();
      expect(shell.acknowledged('w')).toBe(false);
      expect(notice()?.textContent).toMatch(/1 change in Workspace configuration/);
      shell.s.client.disconnect();
    });
  }

  for (const [name, abandon] of [
    ['Settings closed', (shell: Awaited<ReturnType<typeof inspectionShell>>) => shell.navigation.send({ type: 'CLOSE' })],
    ['another Settings target opened', (shell: Awaited<ReturnType<typeof inspectionShell>>) => shell.navigation.send({ type: 'OPEN', target: workspaceSettingsTarget('wB', 'Workspace B') })],
    ['the lookup superseded by another owner lookup', (shell: Awaited<ReturnType<typeof inspectionShell>>) => shell.navigation.send({ type: 'OPEN.OWNER', directory: '/workspace/A' })],
    ['the App Server authority retired', (shell: Awaited<ReturnType<typeof inspectionShell>>) => shell.navigation.send({ type: 'RETIRE' })],
  ] as const) {
    it(`${name} before the owner lookup completes acknowledges nothing`, async () => {
      const shell = await inspectionShell({ workspace: workspaceReceipt() });
      viewDetails();
      const cancelled = shell.lookups[0];
      act(() => abandon(shell));
      expect(shell.navigation.getSnapshot().context.pendingInspection).toBeUndefined();
      // The stopped lookup completing later has no path to land or inspect.
      await act(async () => cancelled.answer({ kind: 'resolved', id: 'wA', displayName: 'Workspace A' }));
      expect(shell.navigation.getSnapshot().context.inspection).toBeUndefined();
      expect(shell.acknowledged('w')).toBe(false);
      expect(notice()?.textContent).toMatch(/1 change in Workspace configuration/);
      shell.s.client.disconnect();
    });
  }

  it('Settings closed after the owner landed, before the details were presented, acknowledges nothing', async () => {
    const shell = await inspectionShell({ workspace: workspaceReceipt() });
    shell.s.held.add('configuration/sourcesRead');
    const reading = shell.s.waitFor('configuration/sourcesRead');
    viewDetails();
    await shell.resolve({ kind: 'resolved', id: 'wA', displayName: 'Workspace A' });
    const read = await reading;
    act(() => shell.navigation.send({ type: 'CLOSE' }));
    shell.s.held.delete('configuration/sourcesRead');
    await act(async () => shell.s.reply(read));
    expect(shell.acknowledged('w')).toBe(false);
    expect(notice()?.textContent).toMatch(/1 change in Workspace configuration/);
    shell.s.client.disconnect();
  });

  it('a details read that fails acknowledges nothing', async () => {
    const shell = await inspectionShell({ workspace: workspaceReceipt(), unreadable: true });
    viewDetails();
    await shell.resolve({ kind: 'resolved', id: 'wA', displayName: 'Workspace A' });
    await waitFor(() => expect(sent(shell.s, 'configuration/sourcesRead').length).toBeGreaterThan(0));
    await waitFor(() => expect(document.querySelector('[data-lifecycle]')?.getAttribute('data-lifecycle')).not.toBe('loading'));
    expect(screen.queryByRole('region', { name: 'Auto Repair' })).toBeNull();
    expect(shell.acknowledged('w')).toBe(false);
    expect(notice()?.textContent).toMatch(/1 change in Workspace configuration/);
    shell.s.client.disconnect();
  });

  it('a newer receipt presented in place of the inspected one acknowledges neither', async () => {
    const native: NativeSources = { workspace: workspaceReceipt() };
    const shell = await inspectionShell(native);
    viewDetails();
    // Native repairs the source again while the owner lookup is in flight.
    native.workspace = workspaceReceipt({ id: 'w-2', changed_count: 2 });
    await announce(shell.s, workspace, native.workspace);
    await shell.resolve({ kind: 'resolved', id: 'wA', displayName: 'Workspace A' });
    const details = await shell.details();
    expect(within(details).getByRole('status').textContent).toBe('Automatically repaired: 2 changes committed.');
    expect(shell.acknowledged('w')).toBe(false);
    expect(shell.acknowledged('w-2')).toBe(false);
    expect(notice()?.textContent).toMatch(/2 changes in Workspace configuration/);
    // An explicit View details of the newer receipt inspects it.
    viewDetails();
    await shell.resolve({ kind: 'resolved', id: 'wA', displayName: 'Workspace A' });
    await waitFor(() => expect(shell.acknowledged('w-2')).toBe(true));
    expect(notice()).toBeNull();
    shell.s.client.disconnect();
  });

  it('an inspection of a replaced authority acknowledges nothing under the new one', async () => {
    const shell = await inspectionShell({ user: receipt() });
    shell.s.held.add('configuration/sourcesRead');
    const reading = shell.s.waitFor('configuration/sourcesRead');
    viewDetails();
    const read = await reading;
    // The App Server process is replaced at the same endpoint mid-navigation.
    await act(async () => { await replaceProcess(shell.s, 'replacement-authority'); });
    act(() => shell.navigation.send({ type: 'RETIRE' }));
    shell.s.held.delete('configuration/sourcesRead');
    await act(async () => shell.s.reply(read));
    expect(shell.navigation.getSnapshot().context.inspection).toBeUndefined();
    expect(shell.acknowledgements.has({ authorityId: 'fixture-app-server-authority', receiptId: 'receipt-user-1' })).toBe(false);
    expect(shell.acknowledgements.has({ authorityId: 'replacement-authority', receiptId: 'receipt-user-1' })).toBe(false);
    shell.s.client.disconnect();
  });

  it('a completion that outlives its authority is refused by the acknowledger', () => {
    const acknowledgements = new RepairAcknowledgements(localStorage);
    let authorityId = 'authority-1';
    const complete = inspectionAcknowledger({ getSnapshot: () => ({ authorityId }) as ReturnType<Server['client']['getSnapshot']> }, acknowledgements);
    authorityId = 'authority-2';
    complete(identity('r'));
    expect(acknowledgements.has(identity('r'))).toBe(false);
    authorityId = 'authority-1';
    complete(identity('r'));
    expect(acknowledgements.has(identity('r'))).toBe(true);
  });

  it('two unresolved sources: viewing one leaves the other failure presented', async () => {
    const shell = await inspectionShell({ user: receipt(unresolved), workspace: workspaceReceipt(unresolved) });
    expect(screen.getByRole('alert', { name: 'Configuration repair' }).textContent).toMatch(/User configuration could not be repaired automatically/);
    viewDetails();
    await shell.details();
    await waitFor(() => expect(shell.acknowledged('receipt-user-1')).toBe(true));
    expect(screen.getByRole('alert', { name: 'Configuration repair' }).textContent).toMatch(/Workspace configuration could not be repaired automatically/);
    expect(shell.acknowledged('w')).toBe(false);
    shell.s.client.disconnect();
  });

  it('Dismiss acknowledges the complete presented group for its authority, immediately', async () => {
    const shell = await inspectionShell({ user: receipt(), workspace: workspaceReceipt(unresolved) });
    fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
    expect(notice()).toBeNull();
    expect(shell.acknowledged('receipt-user-1') && shell.acknowledged('w')).toBe(true);
    expect(shell.viewDetails).not.toHaveBeenCalled();
    shell.s.client.disconnect(); await shell.s.connect();
    await announce(shell.s, { kind: 'user' }, receipt());
    await announce(shell.s, workspace, workspaceReceipt(unresolved));
    expect(notice()).toBeNull();
    // The same receipts from a replacement process are presented again.
    await replaceProcess(shell.s, 'replacement-authority');
    await announce(shell.s, { kind: 'user' }, receipt());
    await announce(shell.s, workspace, workspaceReceipt(unresolved));
    expect(screen.getByRole('alert', { name: 'Configuration repair' }).textContent).toMatch(/Workspace configuration could not be repaired automatically/);
    expect(sent(shell.s, 'configuration/sourceWrite')).toHaveLength(0);
    shell.s.client.disconnect();
  });
});

it('keeps acknowledgements bounded, merged across tabs and in memory when storage is denied', async () => {
  // Normal persistent storage: acknowledgements survive a fresh ledger, and
  // only opaque authority and receipt identities are stored.
  new RepairAcknowledgements(localStorage).acknowledge([identity('kept')]);
  expect(new RepairAcknowledgements(localStorage).has(identity('kept'))).toBe(true);
  expect(new RepairAcknowledgements(localStorage).has(identity('kept', 'authority-2'))).toBe(false);
  expect(JSON.parse(localStorage.getItem(RepairAcknowledgements.KEY)!)).toEqual([['authority-1', 'kept']]);
  // Entries of any other shape are ignored, never trusted.
  localStorage.setItem(RepairAcknowledgements.KEY, JSON.stringify(['kept', ['authority-1'], ['authority-1', 7], ['authority-1', 'kept']]));
  expect(new RepairAcknowledgements(localStorage).has(identity('kept'))).toBe(true);
  // getItem and setItem throwing: the ledger reads empty and stays in memory.
  const failing = { getItem: () => { throw new DOMException('denied', 'SecurityError'); }, setItem: () => { throw new DOMException('full', 'QuotaExceededError'); } };
  const memory = new RepairAcknowledgements(failing, undefined);
  expect(memory.has(identity('kept'))).toBe(false);
  for (let index = 0; index < RepairAcknowledgements.LIMIT + 2; index++) memory.acknowledge([identity(`r${index}`)]);
  expect(memory.has(identity('r0'))).toBe(false);
  expect(memory.has(identity(`r${RepairAcknowledgements.LIMIT + 1}`))).toBe(true);
  // Persistent storage stays bounded too.
  localStorage.clear();
  const bounded = new RepairAcknowledgements(localStorage);
  for (let index = 0; index < RepairAcknowledgements.LIMIT + 2; index++) bounded.acknowledge([identity(`b${index}`)]);
  expect(JSON.parse(localStorage.getItem(RepairAcknowledgements.KEY)!)).toHaveLength(RepairAcknowledgements.LIMIT);
  // Reading the storage property itself throwing must not stop the notice.
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, 'localStorage')!;
  Object.defineProperty(globalThis, 'localStorage', { configurable: true, get: () => { throw new DOMException('denied', 'SecurityError'); } });
  const s = new Server(); await s.connect();
  try {
    render(<RepairNotice client={s.client} viewDetails={vi.fn()} acknowledgements={new RepairAcknowledgements()} />);
    await announce(s, { kind: 'user' }, receipt());
    fireEvent.click(within(notice()!).getByRole('button', { name: 'Dismiss' }));
    expect(notice()).toBeNull();
    // The in-memory acknowledgement still deduplicates the same receipt.
    await announce(s, { kind: 'user' }, receipt());
    expect(notice()).toBeNull();
  } finally {
    Object.defineProperty(globalThis, 'localStorage', descriptor);
    s.client.disconnect();
  }
});

it('merges another tab’s acknowledgements before writing and presents them when observed', async () => {
  // Two tabs, each with its own ledger over the shared browser storage.
  const first = new RepairAcknowledgements(localStorage);
  const second = new RepairAcknowledgements(localStorage);
  first.acknowledge([identity('a')]);
  // The second tab's snapshot predates `a`; its write merges, never erases it.
  second.acknowledge([identity('b')]);
  expect(JSON.parse(localStorage.getItem(RepairAcknowledgements.KEY)!)).toEqual([['authority-1', 'a'], ['authority-1', 'b']]);
  first.acknowledge([identity('c')]);
  expect(JSON.parse(localStorage.getItem(RepairAcknowledgements.KEY)!)).toEqual([['authority-1', 'a'], ['authority-1', 'b'], ['authority-1', 'c']]);
  // The merge stays bounded, newest last.
  for (let index = 0; index < RepairAcknowledgements.LIMIT; index++) second.acknowledge([identity(`n${index}`)]);
  const stored = JSON.parse(localStorage.getItem(RepairAcknowledgements.KEY)!) as unknown[];
  expect(stored).toHaveLength(RepairAcknowledgements.LIMIT);
  expect(stored.at(-1)).toEqual(['authority-1', `n${RepairAcknowledgements.LIMIT - 1}`]);

  // A presented notice disappears when another tab's acknowledgement of the
  // same receipt is observed through the browser's `storage` event.
  localStorage.clear();
  const s = new Server(); await s.connect();
  const here = new RepairAcknowledgements(localStorage);
  render(<RepairNotice client={s.client} viewDetails={vi.fn()} acknowledgements={here} />);
  await announce(s, { kind: 'user' }, receipt());
  expect(notice()).not.toBeNull();
  // Another key, or an unchanged list, changes nothing.
  const version = here.version();
  act(() => { window.dispatchEvent(new StorageEvent('storage', { key: 'unrelated' })); });
  expect(here.version()).toBe(version);
  const elsewhere = new RepairAcknowledgements(localStorage, undefined);
  elsewhere.acknowledge([{ authorityId: 'fixture-app-server-authority', receiptId: 'receipt-user-1' }]);
  expect(notice()).not.toBeNull();
  act(() => { window.dispatchEvent(new StorageEvent('storage', { key: RepairAcknowledgements.KEY })); });
  expect(notice()).toBeNull();
  expect(here.has({ authorityId: 'fixture-app-server-authority', receiptId: 'receipt-user-1' })).toBe(true);
  s.client.disconnect();
});

it('acknowledges a receipt only for the App Server authority that produced it', async () => {
  const s = new Server(); await s.connect();
  const view = render(<RepairNotice client={s.client} viewDetails={vi.fn()} acknowledgements={new RepairAcknowledgements(localStorage)} />);
  await announce(s, { kind: 'user' }, receipt());
  fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
  expect(notice()).toBeNull();
  // Reconnecting to the same authority re-announces the receipt: suppressed.
  s.client.disconnect(); await s.connect();
  await announce(s, { kind: 'user' }, receipt());
  expect(notice()).toBeNull();
  // A reload against the same authority reads the same browser storage.
  view.unmount();
  render(<RepairNotice client={s.client} viewDetails={vi.fn()} acknowledgements={new RepairAcknowledgements(localStorage)} />);
  expect(notice()).toBeNull();
  // A new App Server process at the same endpoint is another authority: its
  // equal receipt id is a repair of its own.
  await replaceProcess(s, 'replacement-authority');
  await announce(s, { kind: 'user' }, receipt());
  expect(notice()?.textContent).toMatch(/3 changes in User configuration/);
  fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
  expect(notice()).toBeNull();
  // Another App Server, sharing this browser's storage, announcing the same id.
  const other = new Server(); other.authorityId = 'other-authority'; await other.connect();
  cleanup();
  render(<RepairNotice client={other.client} viewDetails={vi.fn()} acknowledgements={new RepairAcknowledgements(localStorage)} />);
  await announce(other, { kind: 'user' }, receipt());
  expect(notice()?.textContent).toMatch(/3 changes in User configuration/);
  expect(JSON.parse(localStorage.getItem(RepairAcknowledgements.KEY)!)).toEqual([
    ['fixture-app-server-authority', 'receipt-user-1'],
    ['replacement-authority', 'receipt-user-1'],
  ]);
  s.client.disconnect(); other.client.disconnect();
});

it('OCC-WEB acknowledging one occurrence never suppresses a second of identical changes', async () => {
  const s = new Server(); await s.connect();
  const acknowledgements = new RepairAcknowledgements(localStorage);
  render(<RepairNotice client={s.client} viewDetails={vi.fn()} acknowledgements={acknowledgements} />);
  // R1: A repaired to B.
  await announce(s, { kind: 'user' }, receipt({ id: 'repair-1' }));
  fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
  expect(notice()).toBeNull();
  // R2: A restored externally and repaired to B again — identical content,
  // another native occurrence of the same authority.
  await announce(s, { kind: 'user' }, receipt({ id: 'repair-2' }));
  expect(notice()?.textContent).toMatch(/3 changes in User configuration/);
  fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
  // Delivered again, or after a reconnect to the same authority: still one.
  await announce(s, { kind: 'user' }, receipt({ id: 'repair-2' }));
  s.client.disconnect(); await s.connect();
  await announce(s, { kind: 'user' }, receipt({ id: 'repair-2' }));
  expect(notice()).toBeNull();
  expect(JSON.parse(localStorage.getItem(RepairAcknowledgements.KEY)!)).toEqual([
    ['fixture-app-server-authority', 'repair-1'],
    ['fixture-app-server-authority', 'repair-2'],
  ]);
  s.client.disconnect();
});

it('words every change from native vocabulary alone', () => {
  const en = translator('en');
  expect(repairChangeLabel(en, { kind: 'removed_field', object_path: '' })).toBe('Removed an unknown top-level field');
  expect(repairChangeLabel(en, { kind: 'removed_field', object_path: 'models.*.profiles.*' })).toBe('Removed an unknown field from models.*.profiles.*');
  expect(repairChangeLabel(en, { kind: 'removed_native_tool', position: 4, name: null, reason: 'unknown' })).toBe('Removed agent.tools.builtin[4]: no such Native Tool');
  expect(repairChangeLabel(en, { kind: 'removed_native_tool', position: 2, name: 'read', reason: 'duplicate' })).toBe('Removed agent.tools.builtin[2], a repeated read');
  expect(repairChangeLabel(en, { kind: 'removed_native_tool', position: 0, name: 'todo', reason: 'plugin_owned' })).toBe('Removed todo (agent.tools.builtin[0]): it is provided by a Plugin');
  const zh = translator('zh');
  expect(repairChangeLabel(zh, { kind: 'removed_field', object_path: '' })).toBe('已删除一个未知的顶层字段');
  expect(repairChangeLabel(zh, { kind: 'removed_native_tool', position: 4, name: null, reason: 'unknown' })).toBe('已删除 agent.tools.builtin[4]：不存在此原生工具');
});

it('attributes a receipt delivered during initialization to the authority that connection negotiates', async () => {
  const s = new Server(); await s.connect();
  render(<RepairNotice client={s.client} viewDetails={vi.fn()} acknowledgements={new RepairAcknowledgements(localStorage)} />);
  await announce(s, { kind: 'user' }, receipt());
  fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
  const duringInitialize = async (authorityId: string) => {
    s.client.disconnect();
    s.authorityId = authorityId;
    s.held.add('initialize');
    const initialize = s.waitFor('initialize');
    const connecting = s.client.connect(endpoint, TOKEN);
    const request = await initialize;
    // The client still holds the previous connection's authority here.
    await announce(s, { kind: 'user' }, receipt());
    expect(s.client.getSnapshot().sourceRepairs).toEqual({});
    expect(notice()).toBeNull();
    s.held.delete('initialize');
    await act(async () => { s.reply(request); await connecting; });
    expect(s.client.getSnapshot().sourceRepairs?.user).toEqual({ authorityId, target: { kind: 'user' }, receipt: receipt() });
  };
  // The same authority: the early receipt is the acknowledged one.
  await duringInitialize('fixture-app-server-authority');
  expect(notice()).toBeNull();
  // A replacement process: the early receipt belongs to it, not to the
  // authority the client knew when the notification arrived.
  await duringInitialize('replacement-authority');
  expect(notice()?.textContent).toMatch(/3 changes in User configuration/);
  // A connection that ends before initializing attributes nothing.
  s.client.disconnect();
  s.held.add('initialize');
  const initialize = s.waitFor('initialize');
  const abandoned = s.client.connect(endpoint, TOKEN).catch(() => undefined);
  await initialize;
  await announce(s, workspace, receipt({ id: 'w', target: workspace }));
  s.client.disconnect(); await abandoned;
  s.held.delete('initialize');
  await s.connect();
  expect(s.client.getSnapshot().sourceRepairs).toEqual({});
  s.client.disconnect();
});

it('opens the exact source owner at Advanced → Auto Repair', async () => {
  let resolve!: (value: { kind: 'resolved'; id: string; displayName: string }) => void;
  const lookup = vi.fn(() => new Promise<{ kind: 'resolved'; id: string; displayName: string }>(done => { resolve = done; }));
  const navigation = createActor(settingsNavigationMachine, { input: { lookup } });
  navigation.start();
  navigation.send({ type: 'OPEN.REPAIRS', source: { kind: 'user' } });
  expect(navigation.getSnapshot().context).toMatchObject({ target: userSettingsTarget, page: 'advanced', focus: { advanced: { kind: 'auto_repair' } } });
  navigation.send({ type: 'OPEN.REPAIRS', source: workspace });
  expect(lookup).toHaveBeenCalledWith('/workspace/A');
  await act(async () => resolve({ kind: 'resolved', id: 'wA', displayName: 'Workspace A' }));
  expect(navigation.getSnapshot().context).toMatchObject({ target: workspaceSettingsTarget('wA', 'Workspace A'), page: 'advanced', focus: { advanced: { kind: 'auto_repair' } } });
  // An ordinary owner opening lands on the owner's landing page again.
  navigation.send({ type: 'OPEN.OWNER', directory: '/workspace/A' });
  await act(async () => resolve({ kind: 'resolved', id: 'wA', displayName: 'Workspace A' }));
  expect(navigation.getSnapshot().context.page).toBe('models');
  expect(navigation.getSnapshot().context.focus).toEqual({});
});

/** One User source whose native revision, receipt and description advance
 * independently, as the App Server owns them. */
class Native {
  revision = 'user-1';
  repair: ConfigRepairReceipt | undefined;
  version = 1;
  projection(): SourceSettings {
    const source = cfg3Source();
    source.user.revision = this.revision;
    source.user_repair = this.repair ?? null;
    return source;
  }
  application(): ConfigurationApplication {
    return { scope: 'source:user', sources: [{ kind: 'user' }], version: String(this.version), desired: { input_revision: this.revision, attempt: String(this.version) }, units: { execution_policy: { status: 'applied' } }, candidate: null };
  }
}

it('renders Auto Repair from the authoritative read and keeps an unsaved draft fenced', async () => {
  const native = new Native();
  const s = new Server();
  s.handlers.set('configuration/sourcesRead', () => ({ type: 'source_settings', projection: native.projection() }));
  await s.connect();
  render(<SettingsSurface client={s.client} target={userSettingsTarget} />);
  await settingsReady();
  await openSettingsPage('Advanced');
  const section = await screen.findByRole('region', { name: 'Auto Repair' });
  expect(within(section).getByText('No automatic repair was needed for this source in this App Server process.')).toBeTruthy();
  // An unsaved draft on another page.
  await openSettingsPage('Agent');
  fireEvent.change(screen.getByRole('textbox', { name: 'Root description' }), { target: { value: 'unsaved draft' } });
  // Native repairs the source: a new revision, a receipt, its notice, and the
  // separate application publication of the committed bytes.
  native.revision = 'user-2'; native.repair = receipt({ after_revision: 'user-2' }); native.version = 2;
  await announce(s, { kind: 'user' }, native.repair);
  await act(async () => { s.socket.deliver({ jsonrpc: '2.0', method: 'configuration/changed', params: { application: native.application() } }); });
  await waitFor(() => expect(sent(s, 'configuration/sourcesRead').length).toBeGreaterThan(1));
  // The draft is neither erased, submitted nor silently rebased: the moved
  // source requires the explicit review gesture.
  expect((screen.getByRole('textbox', { name: 'Root description' }) as HTMLInputElement).value).toBe('unsaved draft');
  expect(await screen.findByRole('button', { name: 'Use reviewed revision' })).toBeTruthy();
  expect(sent(s, 'configuration/sourceWrite')).toHaveLength(0);
  await openSettingsPage('Advanced');
  const repaired = await screen.findByRole('region', { name: 'Auto Repair' });
  expect(within(repaired).getByRole('status').textContent).toBe('Automatically repaired: 3 changes committed.');
  for (const line of ['Updated schema_version 8 → 10', 'Removed an unknown field from agent.plugins.todo', 'Removed agent.tools.builtin[1]: no such Native Tool'])
    expect(within(repaired).getByText(line)).toBeTruthy();
  expect(within(repaired).getByText('An exact private copy of the original file is kept in rustX storage.')).toBeTruthy();
  expect(within(repaired).getByText('Revision sha256:before → user-2')).toBeTruthy();
  await openSettingsPage('Agent');
  expect((screen.getByRole('textbox', { name: 'Root description' }) as HTMLInputElement).value).toBe('unsaved draft');
  expect(sent(s, 'configuration/sourceWrite')).toHaveLength(0);
  s.client.disconnect();
});

it('shows an unresolved repair as a failure next to the manual repair', async () => {
  const s = new Server();
  const source = cfg3Source();
  source.user.authored = null;
  source.user.diagnostic = 'invalid rustx.toml; source was not loaded';
  source.user_repair = receipt({ status: 'unresolved', after_revision: null, backup_available: false, diagnostic: 'malformed TOML at line 3, column 1; syntax is never repaired automatically', changes: [] , changed_count: 0 });
  s.handlers.set('configuration/sourcesRead', () => ({ type: 'source_settings', projection: source }));
  await s.connect();
  render(<SettingsSurface client={s.client} target={userSettingsTarget} initialPage="advanced" />);
  const section = await screen.findByRole('region', { name: 'Auto Repair' });
  expect(within(section).getByRole('alert').textContent).toBe('Automatic repair could not fix this source. It is unchanged.');
  expect(within(section).getByText(/malformed TOML at line 3, column 1/)).toBeTruthy();
  expect(within(section).getByText('No backup copy was made.')).toBeTruthy();
  expect(within(section).getByText('Use Repair malformed source below to correct the document by hand.')).toBeTruthy();
  expect(screen.getByRole('button', { name: /Save Repair malformed source/ })).toBeTruthy();
  s.client.disconnect();
});

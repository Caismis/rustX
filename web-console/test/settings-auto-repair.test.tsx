import { materializeSource } from '../../protocol/app-server/source-observation';
// @vitest-environment jsdom
/** Issue #464: native automatic configuration repair in the Web console —
 * source-scoped client observations, one nonblocking notification per
 * receipt identity, Settings → Advanced → Auto Repair, exact owner
 * navigation, and the Settings transaction fences a receipt never crosses. */
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createActor } from 'xstate';
import type { ConfigRepairReceipt, ConfigurationApplication, Request, SourceSettings, SourceTarget } from '../../protocol/app-server/v57';
import { RepairNotice, repairNoticeText } from '../src/app/RepairNotice';
import { repairChangeLabel } from '../src/app/settings/advanced/AutoRepair';
import { inspectionAcknowledger, RepairAcknowledgements } from '../src/app/repair-notices';
import { settingsNavigationMachine, type OwnerResolution, type RepairInspection } from '../src/app/settings/machines/navigation';
import { RpcFailure } from '../src/client/errors';
import type { ProductHostWorkspaces } from '../src/workspaces/host';
import { userSettingsTarget, workspaceSettingsTarget } from '../src/app/settings/projection';
import { translator } from '../src/locale/translation';
import { cfg3Source } from './cfg3-data';
import { applicationReceipt, boundedSettings, sourceReceipt, settingsReceipt, endpoint, Server, TOKEN } from './fixture';
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
  expect(observed).toEqual({ user: { authorityId: s.authorityId, target: { kind: 'user' }, receipt: receipt(), sequence: String(announcedSequence) } });
  const publications = listener.mock.calls.length;
  // The same level again, at the same sequence, is no change.
  await act(async () => { s.socket.deliver({ jsonrpc: '2.0', method: 'configuration/sourceRepairChanged', params: { target: { kind: 'user' }, receipt: receipt(), sequence: String(announcedSequence) } }); });
  expect(listener.mock.calls.length).toBe(publications);
  expect(s.client.getSnapshot().sourceRepairs).toBe(observed);
  await announce(s, workspace, receipt({ id: 'w', target: workspace, status: 'unresolved', after_revision: null }));
  await announce(s, workspace, null);
  expect(s.client.getSnapshot().sourceRepairs?.['workspace:/workspace/A']).toEqual({ authorityId: s.authorityId, target: workspace, receipt: undefined, sequence: String(announcedSequence) });
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
    return { type: 'source_settings', projection: boundedSettings(projection) };
  });
  const lookups: { directory: string; answer: (resolution: OwnerResolution) => void }[] = [];
  const directory = (id: string) => id === 'wA' ? '/workspace/A' : '/workspace/B';
  // The Product Host performs Workspace source I/O; here it reads through the
  // fixture App Server, exactly as the Host does.
  const host: ProductHostWorkspaces = { ...s.workspaceHost, resolveWorkspace: async id => ({ cwd: directory(id) }),
    configureWorkspace: async (id, _endpoint, operation) => {
      if (operation.kind !== 'read') throw new Error('read only');
      const { projection } = await s.client.request({ method: 'configuration/sourcesRead', params: { target: { kind: 'workspace', directory: directory(id) } } }, 'source_settings');
      const source = await materializeSource(projection, async (reference, offset) => (await s.client.request({ method: 'configuration/sourceValueRead', params: { target: projection.target, reference, offset, max_bytes: 65_536 } }, 'source_value_chunk')).chunk, () => s.client.getSnapshot().connection === 'connected');
      return { kind: 'read', projection: source };
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

  it('P2-03 a combined notice is about its most severe receipt, whatever the source order', async () => {
    const statuses = { repaired: {}, unresolved, commit_uncertain: { status: 'commit_uncertain', diagnostic: 'durability not confirmed' } } as const;
    const rank = { repaired: 0, unresolved: 1, commit_uncertain: 2 } as const;
    const wording = { unresolved: 'could not be repaired automatically', commit_uncertain: 'repair was not durably confirmed' } as const;
    type Status = keyof typeof statuses;
    const pairs: [Status, Status][] = [['repaired', 'unresolved'], ['repaired', 'commit_uncertain'], ['unresolved', 'commit_uncertain'], ['commit_uncertain', 'unresolved'], ['repaired', 'repaired']];
    for (const [a, b] of pairs) for (const [userStatus, workspaceStatus] of [[a, b], [b, a]]) for (const userFirst of [true, false]) {
      const label = `User ${userStatus}, Workspace ${workspaceStatus}, ${userFirst ? 'User' : 'Workspace'} announced first`;
      const s = new Server(); await s.connect();
      const viewDetailsOf = vi.fn();
      const { unmount } = render(<RepairNotice client={s.client} acknowledgements={new RepairAcknowledgements(undefined, undefined)} viewDetails={viewDetailsOf} />);
      const user = receipt(statuses[userStatus]);
      const other = workspaceReceipt(statuses[workspaceStatus]);
      const announced: [SourceTarget, ConfigRepairReceipt][] = [[{ kind: 'user' }, user], [workspace, other]];
      for (const [target, value] of userFirst ? announced : announced.reverse()) await announce(s, target, value);
      const subject = rank[workspaceStatus] > rank[userStatus] ? other : user;
      const presented = notice()!;
      if (subject.status === 'repaired') {
        expect(presented.getAttribute('role'), label).toBe('status');
        expect(presented.textContent, label).toMatch(/3 changes in User configuration and 1 change in Workspace configuration/);
      } else {
        expect(presented.getAttribute('role'), label).toBe('alert');
        expect(presented.textContent, label).toContain(`${subject.target.kind === 'user' ? 'User' : 'Workspace'} configuration ${wording[subject.status]}`);
      }
      viewDetails();
      expect(viewDetailsOf, label).toHaveBeenCalledWith({ authorityId: s.authorityId, receiptId: subject.id, source: subject.target });
      unmount(); s.client.disconnect();
    }
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
    expect(s.client.getSnapshot().sourceRepairs?.user).toEqual({ authorityId, target: { kind: 'user' }, receipt: receipt(), sequence: String(announcedSequence) });
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
  s.handlers.set('configuration/sourcesRead', () => ({ type: 'source_settings', projection: boundedSettings(native.projection()) }));
  await s.connect();
  render(<SettingsSurface client={s.client} target={userSettingsTarget} />);
  await settingsReady();
  await openSettingsPage('Advanced');
  const section = await screen.findByRole('region', { name: 'Auto Repair' });
  expect(within(section).getByText('No automatic repair is currently reported for this source in this App Server process.')).toBeTruthy();
  // An unsaved draft on another page.
  await openSettingsPage('Agent');
  fireEvent.change(screen.getByRole('textbox', { name: 'Root description' }), { target: { value: 'unsaved draft' } });
  // Native repairs the source: a new revision, a receipt, its notice, and the
  // separate application publication of the committed bytes.
  native.revision = 'user-2'; native.repair = receipt({ after_revision: 'user-2' }); native.version = 2;
  await announce(s, { kind: 'user' }, native.repair);
  await act(async () => { s.socket.deliver({ jsonrpc: '2.0', method: 'configuration/changed', params: { application: applicationReceipt(native.application()) } }); });
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
  s.handlers.set('configuration/sourcesRead', () => ({ type: 'source_settings', projection: boundedSettings(source) }));
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

// P1-02: the client retains a finite set of source observations — the User
// and SOURCE_REPAIR_WORKSPACES Workspaces — ordered by native sequence, and
// recovers what it forgot from native authority.

const workspaceAt = (index: number): SourceTarget => ({ kind: 'workspace', directory: `/workspace/w${index}` });
const receiptOf = (target: SourceTarget, id: string, overrides: Partial<ConfigRepairReceipt> = {}) => receipt({ id, target, changed_count: 1, ...overrides });
/** Announces a receipt for each of Workspaces `from` up to `to` (exclusive). */
async function announceWorkspaces(s: Server, from: number, to: number) {
  for (let index = from; index < to; index++) await announce(s, workspaceAt(index), receiptOf(workspaceAt(index), `w${index}`));
}
const observedKeys = (s: Server) => Object.keys(s.client.getSnapshot().sourceRepairs ?? {});
/** A native read of `target`: its repair at `sequence`, and the User's. */
function nativeRead(target: SourceTarget, sequence: string, workspaceRepair: ConfigRepairReceipt | null, userRepair: ConfigRepairReceipt | null = null): SourceSettings {
  const projection = cfg3Source();
  projection.target = target;
  projection.repair_sequence = sequence;
  projection.user_repair = userRepair;
  projection.workspace_repair = workspaceRepair;
  return projection;
}

it('WEB-BOUND-01/02 retains the User and at most 64 Workspaces, forgetting the least recently observed Workspace', async () => {
  const s = new Server(); await s.connect();
  await announce(s, { kind: 'user' }, receipt());
  await announceWorkspaces(s, 0, 70);
  const retained = s.client.getSnapshot().sourceRepairs!;
  // The User, never evicted although observed first, and the 64 newest.
  expect(observedKeys(s)).toEqual(['user', ...Array.from({ length: 64 }, (_, index) => `workspace:/workspace/w${index + 6}`)]);
  // A Workspace observed again becomes the most recent; the next one forgets
  // the least recent remaining, never the User.
  await announce(s, workspaceAt(10), receiptOf(workspaceAt(10), 'w10-again'));
  await announce(s, workspaceAt(70), receiptOf(workspaceAt(70), 'w70'));
  const keys = observedKeys(s);
  expect(keys).toHaveLength(65);
  expect(keys[0]).toBe('user');
  expect(keys).not.toContain('workspace:/workspace/w6');
  expect(keys.slice(-2)).toEqual(['workspace:/workspace/w10', 'workspace:/workspace/w70']);
  // Published immutably: an earlier snapshot is never changed.
  expect(Object.keys(retained)).toHaveLength(65);
  expect(retained['workspace:/workspace/w6']).toBeDefined();
  // A forgotten source is retired from what the notice presents.
  render(<RepairNotice client={s.client} acknowledgements={new RepairAcknowledgements(undefined, undefined)} viewDetails={vi.fn()} />);
  expect(notice()!.textContent).not.toMatch(/w6\b/);
  s.client.disconnect();
});

it('WEB-BOUND-03 one Dismiss acknowledges a presented User and 64 Workspaces without recreating the notice', async () => {
  const s = new Server(); await s.connect();
  const acknowledgements = new RepairAcknowledgements(localStorage);
  render(<RepairNotice client={s.client} acknowledgements={acknowledgements} viewDetails={vi.fn()} />);
  await announce(s, { kind: 'user' }, receipt());
  await announceWorkspaces(s, 0, 64);
  expect(observedKeys(s)).toHaveLength(65);
  fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
  expect(notice()).toBeNull();
  for (const [, observation] of Object.entries(s.client.getSnapshot().sourceRepairs!)) {
    expect(acknowledgements.has({ authorityId: observation.authorityId, receiptId: observation.receipt!.id })).toBe(true);
  }
  // Delivered again by the same authority: still acknowledged, so nothing.
  await announce(s, { kind: 'user' }, receipt());
  await announceWorkspaces(s, 0, 64);
  expect(notice()).toBeNull();
  s.client.disconnect();
});

describe('WEB-BOUND-04 a revisited Workspace the client forgot is recovered from native authority', () => {
  for (const [name, current] of [['a newer receipt', 'w-current'], ['a withdrawal', null]] as const) {
    it(name, async () => {
      const s = new Server();
      const directory = '/workspace/A';
      const target: SourceTarget = { kind: 'workspace', directory };
      // Both fixture Sessions belong to Workspace A.
      s.handlers.set('session/settings', () => settingsReceipt({ cwd: directory }, '0'));
      // Native's current answer for the Workspace, once it is read.
      s.handlers.set('configuration/sourcesRead', request => {
        const asked = (request.params as { target: SourceTarget }).target;
        return { type: 'source_settings', projection: boundedSettings(nativeRead(asked, '1000', current === null ? null : receiptOf(asked, current))) };
      });
      await s.connect();
      render(<RepairNotice client={s.client} acknowledgements={new RepairAcknowledgements(undefined, undefined)} viewDetails={vi.fn()} />);
      await announce(s, target, receiptOf(target, 'w-stale'));
      // 64 newer Workspaces: the client forgets Workspace A.
      await announceWorkspaces(s, 0, 64);
      expect(observedKeys(s)).not.toContain(`workspace:${directory}`);
      expect(sent(s, 'configuration/sourcesRead')).toHaveLength(0);
      // Attaching a Session of Workspace A reads it again, read-only.
      await s.client.attach('A');
      await waitFor(() => expect(sent(s, 'configuration/sourcesRead')).toHaveLength(1));
      expect(sent(s, 'configuration/sourcesRead')[0].params).toEqual({ target });
      await waitFor(() => expect(observedKeys(s)).toContain(`workspace:${directory}`));
      const recovered = s.client.getSnapshot().sourceRepairs![`workspace:${directory}`];
      expect(recovered.sequence).toBe('1000');
      expect(recovered.receipt?.id).toBe(current ?? undefined);
      // The forgotten receipt is never resurrected, even delivered late.
      await act(async () => { s.socket.deliver({ jsonrpc: '2.0', method: 'configuration/sourceRepairChanged', params: { target, receipt: receiptOf(target, 'w-stale'), sequence: '1' } }); });
      expect(s.client.getSnapshot().sourceRepairs![`workspace:${directory}`].receipt?.id).toBe(current ?? undefined);
      expect(Object.values(s.client.getSnapshot().sourceRepairs!).some(observation => observation.receipt?.id === 'w-stale')).toBe(false);
      // Read-only: nothing was written or reconciled.
      expect(sent(s, 'configuration/sourceWrite')).toHaveLength(0);
      expect(sent(s, 'configuration/reconcile')).toHaveLength(0);
      // Another Session of the same Workspace, while the client holds it,
      // reads nothing more.
      await s.client.attach('B');
      expect(s.client.getSnapshot().views.B?.settings?.cwd).toBe(directory);
      expect(sent(s, 'configuration/sourcesRead')).toHaveLength(1);
      s.client.disconnect();
    });
  }
});

describe('WEB-BOUND-05 a delayed read never overwrites a newer native notification', () => {
  const deliverAt = (s: Server, target: SourceTarget, value: ConfigRepairReceipt | null, sequence: number) =>
    act(async () => { s.socket.deliver({ jsonrpc: '2.0', method: 'configuration/sourceRepairChanged', params: { target, receipt: value, sequence: String(sequence) } }); });
  for (const [name, evict] of [['held source', false], ['source forgotten while the read was in flight', true]] as const) {
    it(name, async () => {
      const s = new Server();
      const target: SourceTarget = { kind: 'workspace', directory: '/workspace/A' };
      s.handlers.set('configuration/sourcesRead', request => ({ type: 'source_settings', projection: boundedSettings(nativeRead((request.params as { target: SourceTarget }).target, '90', receiptOf(target, 'older'))) }));
      await s.connect();
      // 65 Workspaces: this connection has forgotten one, so an absent
      // Workspace is read when one of its Sessions is attached.
      for (let index = 0; index < 65; index++) await deliverAt(s, workspaceAt(index), receiptOf(workspaceAt(index), `w${index}`), 1 + index);
      s.held.add('configuration/sourcesRead');
      const reading = s.waitFor('configuration/sourcesRead');
      await s.client.attach('A');
      const read = await reading;
      // Native announces a newer receipt while the read (answering at 90) is
      // in flight.
      await deliverAt(s, target, receiptOf(target, 'newer'), 100);
      if (evict) {
        for (let index = 100; index < 164; index++) await deliverAt(s, workspaceAt(index), receiptOf(workspaceAt(index), `w${index}`), 1 + index);
        expect(observedKeys(s)).not.toContain('workspace:/workspace/A');
      }
      s.held.delete('configuration/sourcesRead');
      await act(async () => { s.reply(read); await Promise.resolve(); });
      expect(sent(s, 'configuration/sourcesRead')).toHaveLength(1);
      const held = s.client.getSnapshot().sourceRepairs!['workspace:/workspace/A'];
      if (evict) expect(held).toBeUndefined();
      else expect(held).toMatchObject({ sequence: '100', receipt: { id: 'newer' } });
      expect(Object.values(s.client.getSnapshot().sourceRepairs!).some(observation => observation.receipt?.id === 'older')).toBe(false);
      s.client.disconnect();
    });
  }
});

it('WEB-BOUND-06 reconnecting to the same authority keeps acknowledgements and a bounded cache', async () => {
  const s = new Server(); await s.connect();
  const acknowledgements = new RepairAcknowledgements(localStorage);
  render(<RepairNotice client={s.client} acknowledgements={acknowledgements} viewDetails={vi.fn()} />);
  await announce(s, { kind: 'user' }, receipt());
  await announceWorkspaces(s, 0, 64);
  fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
  for (let reconnect = 0; reconnect < 3; reconnect++) {
    s.client.disconnect(); await s.connect();
    expect(s.client.getSnapshot().sourceRepairs).toEqual({});
    // The same authority announces every latest receipt again, and more.
    await announce(s, { kind: 'user' }, receipt());
    await announceWorkspaces(s, 0, 64);
    expect(notice()).toBeNull();
    // Six new occurrences beyond the bound.
    for (let index = 64; index < 70; index++) await announce(s, workspaceAt(index), receiptOf(workspaceAt(index), `w${index}-${reconnect}`));
    expect(observedKeys(s)).toHaveLength(65);
    expect(notice()!.textContent).toMatch(/6 Workspace|Workspace configuration/);
    fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
  }
  expect(JSON.parse(localStorage.getItem(RepairAcknowledgements.KEY)!).length).toBeLessThanOrEqual(RepairAcknowledgements.LIMIT);
  s.client.disconnect();
});

it('WEB-BOUND-07 a replacement authority presents a distinct receipt with an equal occurrence id', async () => {
  const s = new Server(); await s.connect();
  const acknowledgements = new RepairAcknowledgements(localStorage);
  render(<RepairNotice client={s.client} acknowledgements={acknowledgements} viewDetails={vi.fn()} />);
  await announce(s, { kind: 'user' }, receipt());
  await announceWorkspaces(s, 0, 64);
  fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
  await replaceProcess(s, 'replacement-authority');
  expect(s.client.getSnapshot().sourceRepairs).toEqual({});
  await announce(s, workspaceAt(3), receiptOf(workspaceAt(3), 'w3'));
  expect(notice()!.textContent).toMatch(/1 change in Workspace configuration/);
  expect(s.client.getSnapshot().sourceRepairs!['workspace:/workspace/w3'].authorityId).toBe('replacement-authority');
  s.client.disconnect();
});

it('WEB-BOUND-08 repair observations never touch a Settings draft, its CAS base, an in-flight Save or Session state', async () => {
  const native = new Native();
  const s = new Server();
  s.handlers.set('configuration/sourcesRead', () => ({ type: 'source_settings', projection: boundedSettings(native.projection()) }));
  s.handlers.set('configuration/sourceWrite', () => ({ type: 'sources_written', revision: sourceReceipt(native.projection()).revision, configuration_revision: null }));
  await s.attached('B');
  render(<SettingsSurface client={s.client} target={userSettingsTarget} />);
  await settingsReady();
  await openSettingsPage('Agent');
  fireEvent.change(screen.getByRole('textbox', { name: 'Root description' }), { target: { value: 'unsaved draft' } });
  // An in-flight Save of another unit, held at native.
  await openSettingsPage('Advanced');
  s.held.add('configuration/sourceWrite');
  fireEvent.change(screen.getByLabelText('max_connections'), { target: { value: '19' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save App Server policy' }));
  const [write] = sent(s, 'configuration/sourceWrite');
  expect(write.params).toMatchObject({ expected_revision: 'user-1' });
  const before = s.client.getSnapshot();
  const reads = sent(s, 'configuration/sourcesRead').length;
  // Observations of every source, past the retention bound.
  await announce(s, { kind: 'user' }, receipt());
  await announceWorkspaces(s, 0, 70);
  await announce(s, { kind: 'user' }, null);
  const after = s.client.getSnapshot();
  expect(after.views).toBe(before.views);
  expect(after.configuration).toBe(before.configuration);
  expect(after.sessions).toBe(before.sessions);
  expect(sent(s, 'configuration/sourceWrite')).toEqual([write]);
  expect(sent(s, 'configuration/sourcesRead')).toHaveLength(reads);
  expect(sent(s, 'configuration/reconcile')).toHaveLength(0);
  expect((screen.getByLabelText('max_connections') as HTMLInputElement).value).toBe('19');
  await openSettingsPage('Agent');
  expect((screen.getByRole('textbox', { name: 'Root description' }) as HTMLInputElement).value).toBe('unsaved draft');
  s.client.disconnect();
});

// P1-04 / P2-05: native ledger sequence alone orders a source's observations;
// an empty level is a state; native re-admission converges a stale receipt.

/** Delivers one native observation of `target` at exactly `sequence`. */
const deliverAt = (s: Server, target: SourceTarget, value: ConfigRepairReceipt | null, sequence: number) =>
  act(async () => { s.socket.deliver({ jsonrpc: '2.0', method: 'configuration/sourceRepairChanged', params: { target, receipt: value, sequence: String(sequence) } }); });
const held = (s: Server, target: SourceTarget) => s.client.getSnapshot().sourceRepairs?.[target.kind === 'user' ? 'user' : `workspace:${target.directory}`];
const unresolvedAt = (target: SourceTarget, id: string) => receiptOf(target, id, unresolved);
/** Native answers every Workspace read with `current()` at `sequence()`. */
function answering(s: Server, sequence: () => number, current: () => ConfigRepairReceipt | null) {
  s.handlers.set('configuration/sourcesRead', request => {
    const asked = (request.params as { target: SourceTarget }).target;
    return { type: 'source_settings', projection: boundedSettings(nativeRead(asked, String(sequence()), asked.kind === 'workspace' ? current() : null)) };
  });
}

it('WEB-SEQ-01 a higher sequence of the same receipt advances its observation without a second notice', async () => {
  const s = new Server(); await s.connect();
  const acknowledgements = new RepairAcknowledgements(localStorage);
  render(<RepairNotice client={s.client} acknowledgements={acknowledgements} viewDetails={vi.fn()} />);
  await deliverAt(s, workspace, unresolvedAt(workspace, 'repair-12'), 20);
  const presented = notice();
  expect(presented!.textContent).toMatch(/Workspace configuration could not be repaired automatically/);
  await deliverAt(s, workspace, unresolvedAt(workspace, 'repair-12'), 100);
  expect(held(s, workspace)).toMatchObject({ sequence: '100', receipt: { id: 'repair-12' } });
  // The same occurrence: the same notice element, not a new one.
  expect(notice()).toBe(presented);
  fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
  await deliverAt(s, workspace, unresolvedAt(workspace, 'repair-12'), 150);
  expect(held(s, workspace)!.sequence).toBe('150');
  expect(notice()).toBeNull();
  s.client.disconnect();
});

it('WEB-SEQ-02 a higher empty level advances an empty observation', async () => {
  const s = new Server(); await s.connect();
  render(<RepairNotice client={s.client} acknowledgements={new RepairAcknowledgements(undefined, undefined)} viewDetails={vi.fn()} />);
  await deliverAt(s, workspace, null, 20);
  expect(held(s, workspace)).toMatchObject({ sequence: '20', receipt: undefined });
  await deliverAt(s, workspace, null, 100);
  expect(held(s, workspace)).toMatchObject({ sequence: '100', receipt: undefined });
  // An empty level is a state, never a notice.
  expect(notice()).toBeNull();
  s.client.disconnect();
});

describe('WEB-SEQ-03/04 an empty level at a higher sequence withdraws a receipt, and nothing older restores it', () => {
  for (const [name, byRead] of [['withdrawn by a notice', false], ['withdrawn by an authoritative read', true]] as const) {
    it(name, async () => {
      const s = new Server();
      answering(s, () => 100, () => null);
      await s.connect();
      render(<RepairNotice client={s.client} acknowledgements={new RepairAcknowledgements(undefined, undefined)} viewDetails={vi.fn()} />);
      await deliverAt(s, workspace, unresolvedAt(workspace, 'w-20'), 20);
      expect(notice()!.textContent).toMatch(/could not be repaired automatically/);
      if (byRead) {
        // An inconsistent notice at the held sequence is never applied; the
        // source is read from native authority instead.
        await deliverAt(s, workspace, null, 20);
        await waitFor(() => expect(held(s, workspace)!.sequence).toBe('100'));
        expect(sent(s, 'configuration/sourcesRead').map(request => request.params)).toEqual([{ target: workspace }]);
      } else {
        await deliverAt(s, workspace, null, 100);
      }
      expect(held(s, workspace)).toMatchObject({ sequence: '100', receipt: undefined });
      expect(notice()).toBeNull();
      // WEB-SEQ-04: a delayed notice ordered before the withdrawal.
      await deliverAt(s, workspace, unresolvedAt(workspace, 'w-90'), 90);
      expect(held(s, workspace)).toMatchObject({ sequence: '100', receipt: undefined });
      expect(notice()).toBeNull();
      s.client.disconnect();
    });
  }
});

it('WEB-SEQ-05 a delayed read cannot overwrite a newer notice of a held source', async () => {
  const s = new Server();
  answering(s, () => 90, () => unresolvedAt(workspace, 'w-older'));
  await s.connect();
  await deliverAt(s, workspace, unresolvedAt(workspace, 'w-20'), 20);
  // A conflicting notice at 20 sends a recovery read, held at native.
  s.held.add('configuration/sourcesRead');
  const reading = s.waitFor('configuration/sourcesRead');
  await deliverAt(s, workspace, null, 20);
  const read = await reading;
  await deliverAt(s, workspace, unresolvedAt(workspace, 'w-newer'), 100);
  s.held.delete('configuration/sourcesRead');
  await act(async () => { s.reply(read); await Promise.resolve(); });
  expect(held(s, workspace)).toMatchObject({ sequence: '100', receipt: { id: 'w-newer' } });
  s.client.disconnect();
});

it('WEB-SEQ-06 a newer observation of the same receipt refreshes its retention without presenting it again', async () => {
  const s = new Server(); await s.connect();
  const acknowledgements = new RepairAcknowledgements(localStorage);
  render(<RepairNotice client={s.client} acknowledgements={acknowledgements} viewDetails={vi.fn()} />);
  const w0 = workspaceAt(0);
  await deliverAt(s, w0, unresolvedAt(w0, 'w0'), 1);
  for (let index = 1; index < 64; index++) await deliverAt(s, workspaceAt(index), null, 1 + index);
  fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
  // W0's receipt observed again, later: it is now the most recently observed.
  await deliverAt(s, w0, unresolvedAt(w0, 'w0'), 70);
  expect(notice()).toBeNull();
  await deliverAt(s, workspaceAt(64), null, 71);
  expect(observedKeys(s)).toContain('workspace:/workspace/w0');
  expect(observedKeys(s)).not.toContain('workspace:/workspace/w1');
  expect(held(s, w0)).toMatchObject({ sequence: '70', receipt: { id: 'w0' } });
  expect(notice()).toBeNull();
  s.client.disconnect();
});

describe('WEB-SEQ-07 an equal-sequence conflict never overwrites the held observation', () => {
  for (const [name, conflicting] of [['a withdrawal', null], ['another receipt', unresolvedAt(workspace, 'w-other')]] as const) {
    it(name, async () => {
      const s = new Server();
      answering(s, () => 20, () => unresolvedAt(workspace, 'w-20'));
      await s.connect();
      await deliverAt(s, workspace, unresolvedAt(workspace, 'w-20'), 20);
      const before = held(s, workspace);
      await deliverAt(s, workspace, conflicting, 20);
      expect(held(s, workspace)).toBe(before);
      // Recovered from native authority: one read, whose answer at the same
      // sequence agrees with what is held and changes nothing.
      await waitFor(() => expect(sent(s, 'configuration/sourcesRead')).toHaveLength(1));
      await act(async () => { await Promise.resolve(); });
      expect(held(s, workspace)).toMatchObject({ sequence: '20', receipt: { id: 'w-20' } });
      // A conflicting read is never applied either, and reads nothing more.
      answering(s, () => 20, () => null);
      await deliverAt(s, workspace, conflicting, 20);
      await waitFor(() => expect(sent(s, 'configuration/sourcesRead')).toHaveLength(2));
      await act(async () => { await Promise.resolve(); });
      expect(held(s, workspace)).toMatchObject({ sequence: '20', receipt: { id: 'w-20' } });
      expect(sent(s, 'configuration/sourcesRead')).toHaveLength(2);
      s.client.disconnect();
    });
  }
});

describe('WEB-SEQ-08 an in-flight recovery read never publishes into a later connection', () => {
  for (const [name, replacement] of [['reconnect to the same authority', undefined], ['replacement authority', 'replacement-authority']] as const) {
    it(name, async () => {
      const s = new Server();
      answering(s, () => 500, () => unresolvedAt(workspace, 'w-stale-read'));
      await s.connect();
      await deliverAt(s, workspace, unresolvedAt(workspace, 'w-20'), 20);
      s.held.add('configuration/sourcesRead');
      const reading = s.waitFor('configuration/sourcesRead');
      await deliverAt(s, workspace, null, 20);
      const read = await reading;
      const socket = s.socket;
      if (replacement) await replaceProcess(s, replacement);
      else { s.client.disconnect(); await s.connect(); }
      await deliverAt(s, workspace, null, 3);
      s.held.delete('configuration/sourcesRead');
      await act(async () => { s.reply(read, socket); await Promise.resolve(); });
      expect(held(s, workspace)).toMatchObject({ sequence: '3', receipt: undefined });
      expect(Object.values(s.client.getSnapshot().sourceRepairs!).some(observation => observation.receipt?.id === 'w-stale-read')).toBe(false);
      s.client.disconnect();
    });
  }
});

/** The repair notice, also while open Settings hides the page behind it. */
const shownNotice = () => screen.queryByRole('status', { name: 'Configuration repair', hidden: true }) ?? screen.queryByRole('alert', { name: 'Configuration repair', hidden: true });

/** Every persisted browser entry, acknowledgements included. */
const stored = () => Object.fromEntries(Array.from({ length: localStorage.length }, (_, index) => {
  const key = localStorage.key(index)!;
  return [key, localStorage.getItem(key)];
}));

it('LEDGER-EVICT-08 a warning whose receipt native forgets retires while W0 stays observed, with no side effect', async () => {
  const native = new Native();
  const s = new Server();
  const w0 = workspace;
  s.handlers.set('configuration/sourcesRead', () => ({ type: 'source_settings', projection: boundedSettings(native.projection()) }));
  s.handlers.set('configuration/sourceWrite', () => ({ type: 'sources_written', revision: sourceReceipt(native.projection()).revision, configuration_revision: null }));
  await s.connect();
  const acknowledgements = new RepairAcknowledgements(localStorage);
  render(<>
    <RepairNotice client={s.client} acknowledgements={acknowledgements} viewDetails={vi.fn()} />
    <SettingsSurface client={s.client} target={userSettingsTarget} />
  </>);
  await settingsReady();
  await openSettingsPage('Agent');
  fireEvent.change(screen.getByRole('textbox', { name: 'Root description' }), { target: { value: 'unsaved draft' } });
  // 1. W0's unresolved receipt R0, delivered at 1, warns.
  await deliverAt(s, w0, unresolvedAt(w0, 'w0-1'), 1);
  expect(shownNotice()!.textContent).toMatch(/Workspace configuration could not be repaired automatically/);
  const requests = s.requests.length;
  const before = s.client.getSnapshot();
  const persisted = stored();
  // 2. Another connection records 64 receipts of Workspaces this connection
  //    never addressed (sequences 2…65), so the finite ledger forgets W0.
  //    Native interest still holds W0, so native delivers W0's absence once
  //    at the ledger sequence it read it at — with no request and no
  //    re-admission.
  await deliverAt(s, w0, null, 65);
  expect(held(s, w0)).toMatchObject({ sequence: '65', receipt: undefined });
  expect(shownNotice()).toBeNull();
  // 3. Older notices of R0 never restore it.
  for (const older of [64, 1]) await deliverAt(s, w0, unresolvedAt(w0, 'w0-1'), older);
  expect(held(s, w0)).toMatchObject({ sequence: '65', receipt: undefined });
  expect(shownNotice()).toBeNull();
  // Observation alone: no request, nothing presented as a success, nothing
  // acknowledged or forgotten in the browser, no Session or application
  // state changed.
  expect(s.requests.length).toBe(requests);
  expect(stored()).toEqual(persisted);
  const authorityId = s.client.getSnapshot().authorityId;
  expect(authorityId).toBeTruthy();
  expect(acknowledgements.has({ authorityId: authorityId!, receiptId: 'w0-1' })).toBe(false);
  const after = s.client.getSnapshot();
  expect(after.views).toBe(before.views);
  expect(after.configuration).toBe(before.configuration);
  expect(after.sessions).toBe(before.sessions);
  expect(sent(s, 'configuration/sourceWrite')).toHaveLength(0);
  expect(sent(s, 'configuration/reconcile')).toHaveLength(0);
  // The Settings draft and its CAS base are untouched.
  expect((screen.getByRole('textbox', { name: 'Root description' }) as HTMLInputElement).value).toBe('unsaved draft');
  await openSettingsPage('Advanced');
  fireEvent.change(screen.getByLabelText('max_connections'), { target: { value: '19' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save App Server policy' }));
  await waitFor(() => expect(sent(s, 'configuration/sourceWrite')).toHaveLength(1));
  expect(sent(s, 'configuration/sourceWrite')[0].params).toMatchObject({ expected_revision: 'user-1' });
  s.client.disconnect();
});

it('CONVERGE-WEB a stale warning retires when native re-admits its forgotten Workspace, with no side effect', async () => {
  const native = new Native();
  const s = new Server();
  const w0 = workspace;
  // Native's state of W0, which another connection withdraws.
  let sequence = 0;
  let w0Receipt: ConfigRepairReceipt | null = null;
  s.handlers.set('configuration/sourcesRead', request => {
    const asked = (request.params as { target: SourceTarget }).target;
    if (asked.kind === 'user') return { type: 'source_settings', projection: boundedSettings(native.projection()) };
    return { type: 'source_settings', projection: boundedSettings(nativeRead(asked, String(sequence), asked.directory === w0.directory ? w0Receipt : null)) };
  });
  s.handlers.set('configuration/sourceWrite', () => ({ type: 'sources_written', revision: sourceReceipt(native.projection()).revision, configuration_revision: null }));
  s.handlers.set('session/settings', () => settingsReceipt({ cwd: w0.directory }, '0'));
  await s.connect();
  const acknowledgements = new RepairAcknowledgements(localStorage);
  render(<>
    <RepairNotice client={s.client} acknowledgements={acknowledgements} viewDetails={vi.fn()} />
    <SettingsSurface client={s.client} target={userSettingsTarget} />
  </>);
  await settingsReady();
  await openSettingsPage('Agent');
  fireEvent.change(screen.getByRole('textbox', { name: 'Root description' }), { target: { value: 'unsaved draft' } });
  // 1. W0 is unresolved, and the notice warns.
  w0Receipt = unresolvedAt(w0, 'w0-1');
  await deliverAt(s, w0, w0Receipt, ++sequence);
  // 2. This connection addresses 63 more Workspaces: native announces each
  //    empty level, and its interest is full.
  for (let index = 1; index < 64; index++) await deliverAt(s, workspaceAt(index), null, ++sequence);
  // 3. W0's unresolved document changes: a newer level, observed most
  //    recently by the client, while native interest still orders W0 first.
  w0Receipt = unresolvedAt(w0, 'w0-2');
  await deliverAt(s, w0, w0Receipt, ++sequence);
  // 4. A 65th Workspace: native forgets W0; the client forgets W1, keeps W0.
  await deliverAt(s, workspaceAt(64), null, ++sequence);
  expect(observedKeys(s)).toContain(`workspace:${w0.directory}`);
  expect(observedKeys(s)).not.toContain('workspace:/workspace/w1');
  // 5. Another connection withdraws W0; native tells this one nothing.
  w0Receipt = null; ++sequence;
  expect(shownNotice()!.textContent).toMatch(/Workspace configuration could not be repaired automatically/);
  const requests = s.requests.length;
  const before = s.client.getSnapshot();
  // 6. Re-admission: native announces W0's current level, which is empty.
  await deliverAt(s, w0, null, sequence);
  expect(held(s, w0)).toMatchObject({ sequence: String(sequence), receipt: undefined });
  expect(shownNotice()).toBeNull();
  // Observation alone: no request, and no Session, application or summary
  // state changed.
  expect(s.requests.length).toBe(requests);
  const after = s.client.getSnapshot();
  expect(after.views).toBe(before.views);
  expect(after.configuration).toBe(before.configuration);
  expect(after.sessions).toBe(before.sessions);
  // 7. Older notices never restore it; attaching a Session of W0 reads
  //    nothing while W0 is held; a reconnect to the same authority converges
  //    again from native levels.
  await deliverAt(s, w0, unresolvedAt(w0, 'w0-2'), sequence - 2);
  await deliverAt(s, w0, unresolvedAt(w0, 'w0-1'), 1);
  await s.client.attach('A');
  expect(sent(s, 'configuration/sourcesRead').filter(request => (request.params as { target: SourceTarget }).target.kind === 'workspace')).toHaveLength(0);
  s.client.disconnect(); await s.connect();
  await deliverAt(s, w0, null, sequence);
  expect(held(s, w0)!.receipt).toBeUndefined();
  expect(shownNotice()).toBeNull();
  expect(sent(s, 'configuration/sourceWrite')).toHaveLength(0);
  expect(sent(s, 'configuration/reconcile')).toHaveLength(0);
  // The Settings draft and its CAS base are untouched.
  expect((screen.getByRole('textbox', { name: 'Root description' }) as HTMLInputElement).value).toBe('unsaved draft');
  await openSettingsPage('Advanced');
  fireEvent.change(screen.getByLabelText('max_connections'), { target: { value: '19' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save App Server policy' }));
  await waitFor(() => expect(sent(s, 'configuration/sourceWrite')).toHaveLength(1));
  expect(sent(s, 'configuration/sourceWrite')[0].params).toMatchObject({ expected_revision: 'user-1' });
  s.client.disconnect();
});

// @vitest-environment jsdom
/** Issue #464: native automatic configuration repair in the Web console —
 * source-scoped client observations, one nonblocking notification per
 * receipt identity, Settings → Advanced → Auto Repair, exact owner
 * navigation, and the Settings transaction fences a receipt never crosses. */
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createActor } from 'xstate';
import type { ConfigRepairReceipt, ConfigurationApplication, Request, SourceSettings, SourceTarget } from '../../protocol/app-server/v46';
import { RepairNotice, repairNoticeText } from '../src/app/RepairNotice';
import { RepairAcknowledgements } from '../src/app/repair-notices';
import { settingsNavigationMachine } from '../src/app/settings/machines/navigation';
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
      { kind: 'removed_field', field_path: 'agent.plugins.todo.unknown_permission' },
      { kind: 'removed_native_tool', name: 'old_tool', reason: 'unknown' },
    ],
    truncated: false, backup_available: true, diagnostic: null, ...overrides,
  };
}
async function announce(s: Server, target: SourceTarget, value: ConfigRepairReceipt | null) {
  await act(async () => { s.socket.deliver({ jsonrpc: '2.0', method: 'configuration/sourceRepairChanged', params: { target, receipt: value } }); });
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

it('notifies once per receipt identity, opens the exact owner, and is silent when healthy', async () => {
  const s = new Server(); await s.connect();
  const viewDetails = vi.fn();
  const acknowledgements = new RepairAcknowledgements(localStorage);
  const view = render(<RepairNotice client={s.client} viewDetails={viewDetails} acknowledgements={acknowledgements} />);
  // A healthy source has no receipt: nothing is presented.
  expect(screen.queryByRole('status')).toBeNull();
  await announce(s, { kind: 'user' }, receipt());
  await announce(s, workspace, receipt({ id: 'w', target: workspace, changed_count: 1 }));
  const notice = screen.getByRole('status', { name: 'Configuration repair' });
  expect(notice.textContent).toMatch(/3 changes in User configuration and 1 change in Workspace configuration/);
  // Nonblocking: no dialog, nothing modal, no navigation until asked.
  expect(screen.queryByRole('dialog')).toBeNull();
  expect(viewDetails).not.toHaveBeenCalled();
  fireEvent.click(within(notice).getByRole('button', { name: 'View details' }));
  expect(viewDetails).toHaveBeenCalledWith({ kind: 'user' });
  // Only the inspected source is acknowledged; the Workspace stays presented.
  expect(screen.getByRole('status', { name: 'Configuration repair' }).textContent).toMatch(/^Configuration automatically repaired · 1 change in Workspace configuration/);
  fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
  expect(screen.queryByRole('status', { name: 'Configuration repair' })).toBeNull();
  // A reconnect announces the same receipts again; they stay acknowledged,
  // also for a fresh presentation reading the same browser storage.
  s.client.disconnect(); await s.connect();
  await announce(s, { kind: 'user' }, receipt());
  await announce(s, workspace, receipt({ id: 'w', target: workspace, changed_count: 1 }));
  view.unmount();
  render(<RepairNotice client={s.client} viewDetails={viewDetails} acknowledgements={new RepairAcknowledgements(localStorage)} />);
  expect(screen.queryByRole('status', { name: 'Configuration repair' })).toBeNull();
  // A failure is an alert that opens the failing owner.
  await announce(s, workspace, receipt({ id: 'w-2', target: workspace, status: 'unresolved', after_revision: null, diagnostic: 'malformed TOML at line 1, column 2' }));
  const alert = screen.getByRole('alert', { name: 'Configuration repair' });
  expect(alert.textContent).toMatch(/Workspace configuration could not be repaired automatically/);
  fireEvent.click(within(alert).getByRole('button', { name: 'View details' }));
  expect(viewDetails).toHaveBeenLastCalledWith(workspace);
  // Dismiss acknowledges without navigating.
  await announce(s, { kind: 'user' }, receipt({ id: 'receipt-user-2' }));
  fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
  expect(screen.queryByRole('status', { name: 'Configuration repair' })).toBeNull();
  expect(viewDetails).toHaveBeenCalledTimes(2);
  expect(sent(s, 'configuration/sourceWrite')).toHaveLength(0);
  s.client.disconnect();
});

it('presents and acknowledges receipts when browser storage is denied or failing', async () => {
  const presented = (id: string, authorityId = 'authority-1') => ({ authorityId, receipt: receipt({ id }) });
  // Normal persistent storage: acknowledgements survive a fresh ledger, and
  // only opaque authority and receipt identities are stored.
  new RepairAcknowledgements(localStorage).acknowledge([presented('kept')]);
  expect(new RepairAcknowledgements(localStorage).has(presented('kept'))).toBe(true);
  expect(new RepairAcknowledgements(localStorage).has(presented('kept', 'authority-2'))).toBe(false);
  expect(JSON.parse(localStorage.getItem(RepairAcknowledgements.KEY)!)).toEqual([['authority-1', 'kept']]);
  // Entries of any other shape are ignored, never trusted.
  localStorage.setItem(RepairAcknowledgements.KEY, JSON.stringify(['kept', ['authority-1'], ['authority-1', 7], ['authority-1', 'kept']]));
  expect(new RepairAcknowledgements(localStorage).has(presented('kept'))).toBe(true);
  // getItem and setItem throwing: the ledger reads empty and stays in memory.
  const failing = { getItem: () => { throw new DOMException('denied', 'SecurityError'); }, setItem: () => { throw new DOMException('full', 'QuotaExceededError'); } };
  const memory = new RepairAcknowledgements(failing);
  expect(memory.has(presented('kept'))).toBe(false);
  for (let index = 0; index < RepairAcknowledgements.LIMIT + 2; index++) memory.acknowledge([presented(`r${index}`)]);
  expect(memory.has(presented('r0'))).toBe(false);
  expect(memory.has(presented(`r${RepairAcknowledgements.LIMIT + 1}`))).toBe(true);
  // Persistent storage stays bounded too.
  const bounded = new RepairAcknowledgements(localStorage);
  for (let index = 0; index < RepairAcknowledgements.LIMIT + 2; index++) bounded.acknowledge([presented(`b${index}`)]);
  expect(JSON.parse(localStorage.getItem(RepairAcknowledgements.KEY)!)).toHaveLength(RepairAcknowledgements.LIMIT);
  // Reading the storage property itself throwing must not stop the notice.
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, 'localStorage')!;
  Object.defineProperty(globalThis, 'localStorage', { configurable: true, get: () => { throw new DOMException('denied', 'SecurityError'); } });
  const s = new Server(); await s.connect();
  const viewDetails = vi.fn();
  try {
    render(<RepairNotice client={s.client} viewDetails={viewDetails} />);
    await announce(s, { kind: 'user' }, receipt());
    const notice = screen.getByRole('status', { name: 'Configuration repair' });
    fireEvent.click(within(notice).getByRole('button', { name: 'View details' }));
    expect(viewDetails).toHaveBeenCalledWith({ kind: 'user' });
    expect(screen.queryByRole('status', { name: 'Configuration repair' })).toBeNull();
    // The in-memory acknowledgement still deduplicates the same receipt.
    await announce(s, { kind: 'user' }, receipt());
    expect(screen.queryByRole('status', { name: 'Configuration repair' })).toBeNull();
    expect(sent(s, 'configuration/sourceWrite')).toHaveLength(0);
  } finally {
    Object.defineProperty(globalThis, 'localStorage', descriptor);
    s.client.disconnect();
  }
});

const notice = () => screen.queryByRole('status', { name: 'Configuration repair' }) ?? screen.queryByRole('alert', { name: 'Configuration repair' });
async function replaceProcess(s: Server, authorityId: string) {
  s.client.disconnect();
  s.authorityId = authorityId;
  await s.connect();
}

it('acknowledges a receipt only for the App Server authority that produced it', async () => {
  const s = new Server(); await s.connect();
  const viewDetails = vi.fn();
  const view = render(<RepairNotice client={s.client} viewDetails={viewDetails} acknowledgements={new RepairAcknowledgements(localStorage)} />);
  await announce(s, { kind: 'user' }, receipt());
  fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
  expect(notice()).toBeNull();
  // Reconnecting to the same authority re-announces the receipt: suppressed.
  s.client.disconnect(); await s.connect();
  await announce(s, { kind: 'user' }, receipt());
  expect(notice()).toBeNull();
  // A reload against the same authority reads the same browser storage.
  view.unmount();
  render(<RepairNotice client={s.client} viewDetails={viewDetails} acknowledgements={new RepairAcknowledgements(localStorage)} />);
  expect(notice()).toBeNull();
  // A new App Server process at the same endpoint is another authority: its
  // equal content-derived receipt id is a repair of its own.
  await replaceProcess(s, 'replacement-authority');
  await announce(s, { kind: 'user' }, receipt());
  expect(notice()?.textContent).toMatch(/3 changes in User configuration/);
  fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
  expect(notice()).toBeNull();
  // Another App Server, sharing this browser's storage, announcing the same id.
  const other = new Server(); other.authorityId = 'other-authority'; await other.connect();
  cleanup();
  render(<RepairNotice client={other.client} viewDetails={viewDetails} acknowledgements={new RepairAcknowledgements(localStorage)} />);
  await announce(other, { kind: 'user' }, receipt());
  expect(notice()?.textContent).toMatch(/3 changes in User configuration/);
  expect(JSON.parse(localStorage.getItem(RepairAcknowledgements.KEY)!)).toEqual([
    ['fixture-app-server-authority', 'receipt-user-1'],
    ['replacement-authority', 'receipt-user-1'],
  ]);
  expect(viewDetails).not.toHaveBeenCalled();
  s.client.disconnect(); other.client.disconnect();
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

describe('View details acknowledges only the inspected source', () => {
  const workspaceReceipt = (overrides: Partial<ConfigRepairReceipt> = {}) => receipt({ id: 'w', target: workspace, changed_count: 1, ...overrides });
  const unresolved = { status: 'unresolved', after_revision: null, backup_available: false, diagnostic: 'invalid value' } as const;
  async function presented(user: ConfigRepairReceipt, work: ConfigRepairReceipt) {
    const s = new Server(); await s.connect();
    const viewDetails = vi.fn();
    render(<RepairNotice client={s.client} viewDetails={viewDetails} acknowledgements={new RepairAcknowledgements(localStorage)} />);
    await announce(s, { kind: 'user' }, user);
    await announce(s, workspace, work);
    return { s, viewDetails };
  }

  it('two repaired sources: the User is viewed, the Workspace stays pending across reconnects', async () => {
    const { s, viewDetails } = await presented(receipt(), workspaceReceipt());
    expect(notice()?.textContent).toMatch(/3 changes in User configuration and 1 change in Workspace configuration/);
    fireEvent.click(screen.getByRole('button', { name: 'View details' }));
    expect(viewDetails).toHaveBeenCalledWith({ kind: 'user' });
    expect(notice()?.textContent).toMatch(/^Configuration automatically repaired · 1 change in Workspace configuration/);
    s.client.disconnect(); await s.connect();
    await announce(s, { kind: 'user' }, receipt());
    await announce(s, workspace, workspaceReceipt());
    expect(notice()?.textContent).toMatch(/^Configuration automatically repaired · 1 change in Workspace configuration/);
    fireEvent.click(screen.getByRole('button', { name: 'View details' }));
    expect(viewDetails).toHaveBeenLastCalledWith(workspace);
    expect(notice()).toBeNull();
    s.client.disconnect();
  });

  it('two unresolved sources: viewing one leaves the other failure presented', async () => {
    const { s, viewDetails } = await presented(receipt(unresolved), workspaceReceipt(unresolved));
    expect(screen.getByRole('alert').textContent).toMatch(/User configuration could not be repaired automatically/);
    fireEvent.click(screen.getByRole('button', { name: 'View details' }));
    expect(viewDetails).toHaveBeenCalledWith({ kind: 'user' });
    expect(screen.getByRole('alert').textContent).toMatch(/Workspace configuration could not be repaired automatically/);
    s.client.disconnect(); await s.connect();
    await announce(s, { kind: 'user' }, receipt(unresolved));
    await announce(s, workspace, workspaceReceipt(unresolved));
    expect(screen.getByRole('alert').textContent).toMatch(/Workspace configuration could not be repaired automatically/);
    s.client.disconnect();
  });

  it('User repaired, Workspace unresolved: viewing the failing Workspace leaves the User pending', async () => {
    const { s, viewDetails } = await presented(receipt(), workspaceReceipt(unresolved));
    expect(screen.getByRole('alert').textContent).toMatch(/Workspace configuration could not be repaired automatically/);
    fireEvent.click(screen.getByRole('button', { name: 'View details' }));
    expect(viewDetails).toHaveBeenCalledWith(workspace);
    expect(screen.queryByRole('alert')).toBeNull();
    expect(screen.getByRole('status').textContent).toMatch(/^Configuration automatically repaired · 3 changes in User configuration/);
    s.client.disconnect(); await s.connect();
    await announce(s, { kind: 'user' }, receipt());
    await announce(s, workspace, workspaceReceipt(unresolved));
    expect(screen.getByRole('status').textContent).toMatch(/^Configuration automatically repaired · 3 changes in User configuration/);
    s.client.disconnect();
  });

  it('Dismiss acknowledges the complete presented group for its authority', async () => {
    const { s, viewDetails } = await presented(receipt(), workspaceReceipt(unresolved));
    fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
    expect(notice()).toBeNull();
    expect(viewDetails).not.toHaveBeenCalled();
    s.client.disconnect(); await s.connect();
    await announce(s, { kind: 'user' }, receipt());
    await announce(s, workspace, workspaceReceipt(unresolved));
    expect(notice()).toBeNull();
    // The same receipts from a replacement process are presented again.
    await replaceProcess(s, 'replacement-authority');
    await announce(s, { kind: 'user' }, receipt());
    await announce(s, workspace, workspaceReceipt(unresolved));
    expect(screen.getByRole('alert').textContent).toMatch(/Workspace configuration could not be repaired automatically/);
    expect(sent(s, 'configuration/sourceWrite')).toHaveLength(0);
    s.client.disconnect();
  });
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
  for (const line of ['Updated schema_version 8 → 10', 'Removed unknown field agent.plugins.todo.unknown_permission', 'Removed old_tool from agent.tools.builtin: no such Native Tool'])
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

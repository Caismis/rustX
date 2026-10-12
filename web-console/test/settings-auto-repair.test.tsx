// @vitest-environment jsdom
/** Issue #464: native automatic configuration repair in the Web console —
 * source-scoped client observations, one nonblocking notification per
 * receipt identity, Settings → Advanced → Auto Repair, exact owner
 * navigation, and the Settings transaction fences a receipt never crosses. */
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { createActor } from 'xstate';
import type { ConfigRepairReceipt, ConfigurationApplication, Request, SourceSettings, SourceTarget } from '../../protocol/app-server/v46';
import { RepairNotice, repairNoticeText } from '../src/app/RepairNotice';
import { RepairAcknowledgements } from '../src/app/repair-notices';
import { settingsNavigationMachine } from '../src/app/settings/machines/navigation';
import { userSettingsTarget, workspaceSettingsTarget } from '../src/app/settings/projection';
import { translator } from '../src/locale/translation';
import { cfg3Source } from './cfg3-data';
import { Server } from './fixture';
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
  expect(observed).toEqual({ user: { target: { kind: 'user' }, receipt: receipt() } });
  const publications = listener.mock.calls.length;
  // The same receipt again is no change.
  await announce(s, { kind: 'user' }, receipt());
  expect(listener.mock.calls.length).toBe(publications);
  expect(s.client.getSnapshot().sourceRepairs).toBe(observed);
  await announce(s, workspace, receipt({ id: 'w', target: workspace, status: 'unresolved', after_revision: null }));
  await announce(s, workspace, null);
  expect(s.client.getSnapshot().sourceRepairs?.['workspace:/workspace/A']).toEqual({ target: workspace, receipt: undefined });
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

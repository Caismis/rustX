// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it } from 'vitest';
import { App } from '../src/app/App';
import { AppServerClient } from '../src/client/app-server';
import type { ConfigurationApplication, SourceTarget } from '../../protocol/app-server/v21';
import type { ProductHostWorkspaces, WorkspaceCatalog } from '../src/workspaces/host';
import { Server, TOKEN, endpoint } from './fixture';
import { ConnectionController } from '../src/connection/controller';
import { userSettingsTarget, workspaceSettingsTarget } from '../src/app/settings/projection';
import { openSettingsNavigation, SettingsSurface } from './settings-harness';

// Deterministic owner-navigation fencing. `listWorkspaces` is the only
// asynchronous preparation; every test drives it with an explicit deferred
// promise. No sleep, timer or scheduling guess establishes an ordering.
let server: Server;
beforeEach(() => { localStorage.clear(); server = new Server(); });
afterEach(() => { cleanup(); server.client.disconnect(); });

function deferred<T>() {
  let resolve!: (value: T) => void, reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}
function catalog(): WorkspaceCatalog {
  return { endpoint, workspaces: [
    { id: 'wA', displayName: 'Workspace A', location: 'a', displayPath: '/workspace/A' },
    { id: 'wB', displayName: 'Workspace B', location: 'b', displayPath: '/workspace/B' },
  ], picker: { kind: 'unavailable', reason: 'test' } };
}
function failing(sources: SourceTarget[]): ConfigurationApplication {
  return { eligibility: { status: 'unavailable' }, scope: 'ses', version: '1', desired: { input_revision: 'i', attempt: '1' },
    sources, units: { capabilities: { status: 'failed', diagnostic: 'boom' } }, candidate: null };
}
function hostWith(list: () => Promise<WorkspaceCatalog>): ProductHostWorkspaces {
  return {
    listWorkspaces: list,
    classifyLocations: async cwds => cwds.map(cwd => ['/workspace/A', '/workspace/B'].includes(cwd) ? { authorized: true as const, workspaceId: cwd === '/workspace/A' ? 'wA' : 'wB' } : { authorized: false as const }),
    resolveWorkspace: async () => ({ cwd: '/workspace/A' }),
    adoptWorkspace: async () => {}, renameWorkspace: async () => {}, reorderWorkspace: async () => {}, removeWorkspace: async () => {},
    configureWorkspace: async () => { throw new Error('not used'); },
  };
}
/** Mount the product shell with a Session configuration failure that names the
 * Workspace author as one owner, then hand back the held catalog lookup. */
async function mountOwnerFailure() {
  server.handlers.set('session/configuration', () => ({ type: 'session_configuration', application: failing([{ kind: 'workspace', directory: '/workspace/A' }]) }));
  let hold = false;
  let held = deferred<WorkspaceCatalog>();
  const host = hostWith(async () => { if (hold) { hold = false; return held.promise; } return catalog(); });
  await server.connect();
  render(<App client={server.client} workspaceHost={host} />);
  await screen.findByRole('button', { name: 'Open Session A' });
  fireEvent.click(screen.getByRole('button', { name: 'Open Session A' }));
  await screen.findByText(/Some configuration preparation failed/);
  return {
    startLookup() {
      hold = true; held = deferred<WorkspaceCatalog>();
      // Some cases take this decision while the Settings modal covers the
      // Session surface, which the modal hides from the accessibility tree.
      fireEvent.click(screen.getByRole('button', { name: 'Open Workspace Settings — /workspace/A', hidden: true }));
      return held;
    },
  };
}
const release = async (held: ReturnType<typeof deferred<WorkspaceCatalog>>, success = false) => {
  await act(async () => {
    if (success) held.resolve(catalog()); else held.reject(new Error('stale owner lookup failed'));
    await held.promise.catch(() => {});
  });
};

it('navigation A: a newer User Settings decision fences a stale owner-lookup failure', async () => {
  const { startLookup } = await mountOwnerFailure();
  const held = startLookup();
  fireEvent.click(screen.getByRole('button', { name: 'Settings' }));
  await screen.findByRole('heading', { name: 'User Settings' });
  await release(held);
  // The newer target stands, and the obsolete rejection never becomes an error.
  expect(screen.getByRole('heading', { name: 'User Settings' })).toBeTruthy();
  expect(screen.queryByText(/stale owner lookup failed/)).toBeNull();
});

it('navigation B: a newer Workspace Settings decision fences a stale owner-lookup failure', async () => {
  const { startLookup } = await mountOwnerFailure();
  const held = startLookup();
  fireEvent.click(screen.getByRole('button', { name: 'Workspace actions for Workspace B' }));
  fireEvent.click(screen.getByRole('menuitem', { name: 'Workspace settings' }));
  await screen.findByRole('heading', { name: 'Workspace Settings — Workspace B' });
  await release(held);
  // Workspace B stays the target; the failed lookup for A cannot retarget it.
  expect(screen.getByRole('heading', { name: 'Workspace Settings — Workspace B' })).toBeTruthy();
  expect(screen.queryByText(/stale owner lookup failed/)).toBeNull();
});

it('navigation C: closing Settings fences a stale owner-lookup failure', async () => {
  const { startLookup } = await mountOwnerFailure();
  fireEvent.click(screen.getByRole('button', { name: 'Settings' }));
  await screen.findByRole('heading', { name: 'User Settings' });
  const held = startLookup();
  fireEvent.click(screen.getByRole('button', { name: 'Close Settings' }));
  expect(screen.queryByRole('heading', { name: 'User Settings' })).toBeNull();
  await release(held);
  expect(screen.queryByRole('dialog')).toBeNull();
  expect(screen.queryByText(/stale owner lookup failed/)).toBeNull();
});

it('navigation D: an authority replacement fences a stale owner-lookup failure', async () => {
  // A real authority replacement retires the old lifetime even though the
  // obsolete catalog request keeps running under it.
  const local = new Server(), alternate = new Server();
  const client = new AppServerClient((url, protocols) => (url === endpoint ? local : alternate).socketFactory(url, protocols));
  client.setAttachmentAdmission(async () => true);
  local.handlers.set('session/configuration', () => ({ type: 'session_configuration', application: failing([{ kind: 'workspace', directory: '/workspace/A' }]) }));
  let hold = false;
  let held = deferred<WorkspaceCatalog>();
  const host = hostWith(async () => { if (hold) { hold = false; return held.promise; } return catalog(); });
  await client.connect(endpoint, TOKEN);
  render(<App client={client} workspaceHost={host} />);
  await screen.findByRole('button', { name: 'Open Session A' });
  fireEvent.click(screen.getByRole('button', { name: 'Open Session A' }));
  await screen.findByText(/Some configuration preparation failed/);
  hold = true; held = deferred<WorkspaceCatalog>();
  fireEvent.click(screen.getByRole('button', { name: 'Open Workspace Settings — /workspace/A' }));
  await act(async () => { await client.connect('wss://remote.example/', TOKEN, 'replace-authority'); });
  await act(async () => { held.reject(new Error('stale owner lookup failed')); await held.promise.catch(() => {}); });
  // No navigation commits under the retired authority and no stale error is published.
  expect(screen.queryByRole('dialog')).toBeNull();
  expect(screen.queryByText(/stale owner lookup failed/)).toBeNull();
  await client.disconnect();
});

it('navigation success: a current lookup still commits the exact owning Workspace', async () => {
  const { startLookup } = await mountOwnerFailure();
  const held = startLookup();
  await release(held, true);
  expect(screen.getByRole('heading', { name: 'Workspace Settings — Workspace A' })).toBeTruthy();
});

it('navigation success: a delayed successful lookup is fenced by a newer navigation', async () => {
  const { startLookup } = await mountOwnerFailure();
  const held = startLookup();
  fireEvent.click(screen.getByRole('button', { name: 'Settings' }));
  await screen.findByRole('heading', { name: 'User Settings' });
  await release(held, true);
  // The late success cannot reopen or retarget the newer Settings decision.
  expect(screen.getByRole('heading', { name: 'User Settings' })).toBeTruthy();
  expect(screen.queryByRole('heading', { name: 'Workspace Settings — Workspace A' })).toBeNull();
});

/** Connection is the Advanced sub-surface of the global client: the Advanced
 * page is selected and the Connection form is what it shows. */
function connectionShown() {
  return screen.getByRole('tab', { name: 'Advanced' }).getAttribute('aria-selected') === 'true'
    && !!screen.queryByRole('region', { name: 'Connection Settings' });
}
const selected = (name: string) => screen.getByRole('tab', { name }).getAttribute('aria-selected');

/** Reach the disconnected recovery surface with an owner lookup still in
 * flight. Leaving the Session view and losing the transport are not Settings
 * navigation decisions, so neither of them fences the lookup — only the
 * "Show details" gesture does. */
async function recoveryWithStaleLookup() {
  const { startLookup } = await mountOwnerFailure();
  const held = startLookup();
  fireEvent.click(screen.getByRole('button', { name: 'View options' }));
  await act(async () => { fireEvent.click(screen.getByRole('menuitem', { name: 'Close all views' })); });
  await act(async () => { await server.client.disconnect(); });
  fireEvent.click(await screen.findByRole('button', { name: 'Show details' }));
  expect(connectionShown()).toBe(true);
  return held;
}

it('navigation E: recovery Show details fences a delayed successful owner lookup', async () => {
  const held = await recoveryWithStaleLookup();
  await release(held, true);
  // Connection Settings remains the selected Settings surface, and the late
  // success can neither retarget it to the owning Workspace nor reopen it.
  expect(connectionShown()).toBe(true);
  fireEvent.click(screen.getByRole('button', { name: 'Back to Advanced' }));
  expect(screen.getByRole('heading', { name: 'User Settings' })).toBeTruthy();
  expect(screen.queryByRole('heading', { name: 'Workspace Settings — Workspace A' })).toBeNull();
});

it('navigation E: recovery Show details fences a stale owner-lookup failure', async () => {
  const held = await recoveryWithStaleLookup();
  await release(held);
  expect(connectionShown()).toBe(true);
  expect(screen.queryByText(/stale owner lookup failed/)).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Back to Advanced' }));
  expect(screen.getByRole('heading', { name: 'User Settings' })).toBeTruthy();
});

// One owner for the displayed Settings page: the navigation machine. A
// top-level decision taken while the dialog stays mounted is what the dialog
// shows, whatever page the user had selected inside it.

it('navigation F: a Connection decision while Settings stays mounted shows Connection', async () => {
  await mountOwnerFailure();
  fireEvent.click(screen.getByRole('button', { name: 'View options' }));
  await act(async () => { fireEvent.click(screen.getByRole('menuitem', { name: 'Close all views' })); });
  await act(async () => { await server.client.disconnect(); });
  fireEvent.click(screen.getByRole('button', { name: 'Settings' }));
  await screen.findByRole('heading', { name: 'User Settings' });
  fireEvent.click(screen.getByRole('tab', { name: 'Tools & Permissions' }));
  expect(selected('Tools & Permissions')).toBe('true');
  // The dialog is still mounted when the recovery surface decides otherwise.
  // The modal hides the page behind it from the accessibility tree, so the
  // decision is taken on the recovery button the dialog covers.
  fireEvent.click(screen.getByRole('button', { name: 'Show details', hidden: true }));
  expect(connectionShown()).toBe(true);
  expect(selected('Tools & Permissions')).toBe('false');
});

it('navigation F: an owning-Workspace decision while Settings stays mounted opens its landing page', async () => {
  const { startLookup } = await mountOwnerFailure();
  fireEvent.click(screen.getByRole('button', { name: 'Settings' }));
  await screen.findByRole('heading', { name: 'User Settings' });
  fireEvent.click(screen.getByRole('tab', { name: 'Tools & Permissions' }));
  expect(selected('Tools & Permissions')).toBe('true');
  const held = startLookup();
  await release(held, true);
  expect(screen.getByRole('heading', { name: 'Workspace Settings — Workspace A' })).toBeTruthy();
  // A Workspace surface is constrained: no General page, and it lands on Models.
  expect(screen.queryByRole('tab', { name: 'General' })).toBeNull();
  expect(selected('Models')).toBe('true');
  expect(selected('Tools & Permissions')).toBe('false');
});

// ── #392 Connection is client-owned: the real App composition ───────────────
//
// `App` always hands Settings a `ConnectionController`. These run the real
// product shell, so a Workspace surface is exercised with exactly the
// controller the product supplies — the presence of that controller must never
// be what decides whether Connection is reachable.

async function mountApp() {
  await server.connect();
  const connection = new ConnectionController(server.client);
  render(<App client={server.client} workspaceHost={hostWith(async () => catalog())} connection={connection} />);
  await screen.findByRole('button', { name: 'Workspace actions for Workspace B' });
}
async function openWorkspaceB() {
  fireEvent.click(screen.getByRole('button', { name: 'Workspace actions for Workspace B', hidden: true }));
  fireEvent.click(screen.getByRole('menuitem', { name: 'Workspace settings' }));
  await screen.findByRole('heading', { name: 'Workspace Settings — Workspace B' });
}

it('N10 real App: User Settings → Advanced exposes Connection', async () => {
  await mountApp();
  fireEvent.click(screen.getByRole('button', { name: 'Settings' }));
  await screen.findByRole('heading', { name: 'User Settings' });
  fireEvent.click(screen.getByRole('tab', { name: 'Advanced' }));
  fireEvent.click(screen.getByRole('button', { name: 'Connection' }));
  expect(connectionShown()).toBe(true);
});

it('N10 real App: Workspace Settings → Advanced exposes no Connection although App supplies a ConnectionController', async () => {
  await mountApp();
  await openWorkspaceB();
  fireEvent.click(screen.getByRole('tab', { name: 'Advanced' }));
  expect(selected('Advanced')).toBe('true');
  expect(screen.queryByRole('button', { name: 'Connection' })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Back to Advanced' })).toBeNull();
  expect(screen.queryByRole('region', { name: 'Connection Settings' })).toBeNull();
});

it('N10 real App: OPEN.CONNECTION from Workspace Settings retargets to User Advanced/Connection', async () => {
  // No Session view is open, so a lost transport shows the recovery surface.
  await mountApp();
  await openWorkspaceB();
  fireEvent.click(screen.getByRole('tab', { name: 'Advanced' }));
  expect(screen.queryByRole('region', { name: 'Connection Settings' })).toBeNull();
  await act(async () => { await server.client.disconnect(); });
  // The recovery surface's explicit Connection decision, taken while the
  // Workspace dialog stays mounted over it.
  fireEvent.click(screen.getByRole('button', { name: 'Show details', hidden: true }));
  expect(screen.getByRole('heading', { name: 'User Settings' })).toBeTruthy();
  expect(screen.queryByRole('heading', { name: 'Workspace Settings — Workspace B' })).toBeNull();
  expect(connectionShown()).toBe(true);
});

it('N11 a Workspace navigation state cannot be forced into Connection, with the product controller supplied', async () => {
  await server.connect();
  const navigation = openSettingsNavigation(workspaceSettingsTarget('wB', 'Workspace B'), 'advanced');
  render(<SettingsSurface client={server.client} target={workspaceSettingsTarget('wB', 'Workspace B')} navigation={navigation}
    connection={new ConnectionController(server.client)} host={hostWith(async () => catalog())} />);
  await screen.findByRole('heading', { name: 'Workspace Settings — Workspace B' });
  act(() => { navigation.send({ type: 'FOCUS', focus: { kind: 'connection' } }); });
  expect(navigation.getSnapshot().context).toMatchObject({ page: 'advanced', focus: {} });
  expect(screen.queryByRole('region', { name: 'Connection Settings' })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Connection' })).toBeNull();
  // The same composition on the User target does reach it, so the absence
  // above is the capability and not a missing fixture.
  act(() => { navigation.send({ type: 'OPEN', target: userSettingsTarget }); });
  act(() => { navigation.send({ type: 'SELECT', page: 'advanced' }); });
  fireEvent.click(await screen.findByRole('button', { name: 'Connection' }));
  expect(connectionShown()).toBe(true);
});

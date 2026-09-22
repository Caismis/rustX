// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it } from 'vitest';
import { App } from '../src/app/App';
import { AppServerClient } from '../src/client/app-server';
import type { ConfigurationApplication, SourceTarget } from '../../protocol/app-server/v18';
import type { ProductHostWorkspaces, WorkspaceCatalog } from '../src/workspaces/host';
import { Server, TOKEN, endpoint } from './fixture';

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
      fireEvent.click(screen.getByRole('button', { name: 'Open Workspace Settings — /workspace/A' }));
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
  expect(screen.getByRole('button', { name: 'Connection' }).getAttribute('aria-current')).toBe('page');
  return held;
}

it('navigation E: recovery Show details fences a delayed successful owner lookup', async () => {
  const held = await recoveryWithStaleLookup();
  await release(held, true);
  // Connection Settings remains the selected Settings surface, and the late
  // success can neither retarget it to the owning Workspace nor reopen it.
  expect(screen.getByRole('button', { name: 'Connection' }).getAttribute('aria-current')).toBe('page');
  fireEvent.click(screen.getByRole('button', { name: 'Overview' }));
  expect(screen.getByRole('heading', { name: 'User Settings' })).toBeTruthy();
  expect(screen.queryByRole('heading', { name: 'Workspace Settings — Workspace A' })).toBeNull();
});

it('navigation E: recovery Show details fences a stale owner-lookup failure', async () => {
  const held = await recoveryWithStaleLookup();
  await release(held);
  expect(screen.getByRole('button', { name: 'Connection' }).getAttribute('aria-current')).toBe('page');
  expect(screen.queryByText(/stale owner lookup failed/)).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Overview' }));
  expect(screen.getByRole('heading', { name: 'User Settings' })).toBeTruthy();
});

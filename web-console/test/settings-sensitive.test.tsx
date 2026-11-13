import { providerAction } from './settings-harness';
import {openResourceModule} from './settings-harness';
import { waitForSettingsRevision } from './settings-harness';
// @vitest-environment jsdom
import { afterEach, expect, it } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { settingsTransactionOwners } from '../src/app/settings/Settings';
import { openResourceRow, openSettingsPage, settingsReady, SettingsSurface } from './settings-harness';
import { workspaceSettingsTarget, userSettingsTarget } from '../src/app/settings/projection';
import { cfg3Client, cfg3Host } from './cfg3-fixture';
afterEach(cleanup);

/** The one sentinel every assertion in this file hunts for. A secret-bearing
 * authored value is the User document's; the Workspace authors none of them. */
const SENTINEL = 'S1-SECRET-SENTINEL';
const retained = (s: ReturnType<typeof cfg3Client>) => JSON.stringify(settingsTransactionOwners(s.client).map(owner => owner.retainedState()));
const writes = (s: ReturnType<typeof cfg3Client>) => s.request.mock.calls.filter(([op]) => op.method === 'configuration/sourceWrite');

/** A native projection in which the User document really authors a literal Tool
 * environment value, a literal Provider credential and a literal MCP `env`
 * entry — projected exactly as the App Server projects them: identity-only. */
function redactedSources(s: ReturnType<typeof cfg3Client>) {
  // `RuntimeLayer.environment` is a list of identities on the wire, so the
  // fixture cannot even express a leaked value here: that is the point.
  s.source.user.authored = {
    ...s.source.user.authored,
    environment: ['SECRET_ENV', 'PLAIN_ENV'],
    providers: { transport: { protocol: 'openai_responses', base_url: 'https://user.invalid', credential: { type: 'literal' } } },
  };
  s.source.resolved = { environment: ['SECRET_ENV', 'PLAIN_ENV'], providers: { transport: { protocol: 'openai_responses', base_url: 'https://user.invalid', credential: { type: 'literal' } } }, models: {} };
  s.source.provenance = { 'environment.SECRET_ENV': { kind: 'user', document: '/bound/rustx.toml', base: '/bound' } };
  s.source.user_mcp.authored = { search: { definition: { type: 'stdio', command: 'search-server' }, retained_env: ['TOKEN'], retained_headers: [] } };
}

/** Values are authored on Advanced, where the native
 * environment identities and the rest of the diagnostics live. */
async function openWorkspaceRuntime(s: ReturnType<typeof cfg3Client>) {
  render(<SettingsSurface client={s.client} target={workspaceSettingsTarget('A', 'Workspace A')} host={cfg3Host(s)} />);
  await settingsReady();
  await openSettingsPage('Advanced');
  await waitForSettingsRevision('workspace-1');
}

it('S1-15 an inherited environment identity is discoverable while its User value is never projected', async () => {
  const s = cfg3Client(); redactedSources(s);
  await openWorkspaceRuntime(s);
  // Identity discovery is a native fact and stays available.
  expect(screen.getByRole('button', { name: 'SECRET_ENV' })).toBeTruthy();
  expect(screen.getByRole('button', { name: 'PLAIN_ENV' })).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'SECRET_ENV' }));
  const form = within(screen.getByRole('form', { name: 'Environment SECRET_ENV' }));
  // The Workspace authors no override, and the effective value is reported as
  // existing-but-never-projected rather than as available, absent or empty.
  expect(screen.getByRole('form', { name: 'Environment SECRET_ENV' }).dataset.authored).toBe('absent');
  // The native-resolved disclosure exists, and carries the redaction fact in
  // place of a value: there is no branch that could render one.
  expect(screen.getByRole('form', { name: 'Environment SECRET_ENV' }).dataset.effective).toBe('redacted');
  expect(form.getByLabelText('Value').getAttribute('type')).toBe('password');
  // Nothing anywhere in the document carries the value.
  expect(document.body.innerHTML).not.toContain(SENTINEL);
  expect(retained(s)).not.toContain(SENTINEL);
});

it('S1-15 Override authors an empty Workspace draft and sends only the newly typed value', async () => {
  const s = cfg3Client(); redactedSources(s);
  await openWorkspaceRuntime(s);
  fireEvent.click(screen.getByRole('button', { name: 'SECRET_ENV' }));
  const form = within(screen.getByRole('form', { name: 'Environment SECRET_ENV' }));
  // Opening authors nothing: Save is unavailable until the user says so.
  expect((form.getByRole('button', { name: 'Save Environment SECRET_ENV' }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(form.getByRole('button', { name: 'Override Environment SECRET_ENV' }));
  // Override starts from the neutral seed, never from the inherited literal.
  const field = form.getByLabelText('Value') as HTMLInputElement;
  expect(field.value).toBe('');
  expect(field.type).toBe('password');
  fireEvent.change(field, { target: { value: 'WORKSPACE-ONLY' } });
  fireEvent.click(form.getByRole('button', { name: 'Save Environment SECRET_ENV' }));
  await waitFor(() => expect(writes(s)).toHaveLength(1));
  expect(writes(s)[0][0]).toEqual({
    method: 'configuration/sourceWrite',
    params: {
      target: { kind: 'workspace', directory: '/workspace/A' }, expected_revision: 'workspace-1',
      mutation: { kind: 'config', mutation: { unit: 'environment', name: 'SECRET_ENV', authored: 'WORKSPACE-ONLY' } },
    },
  });
  expect(JSON.stringify(writes(s))).not.toContain(SENTINEL);
});

it('S1-15 Advanced exposes useful environment names without raw projections or literal values', async () => {
  const s = cfg3Client(); redactedSources(s);
  render(<SettingsSurface client={s.client} target={userSettingsTarget} host={cfg3Host(s)} />);
  await settingsReady();
  await openSettingsPage('Advanced');
  await waitForSettingsRevision('user-1');
  expect(screen.queryByLabelText('Source and application projection')).toBeNull();
  expect(screen.queryByLabelText('Resolved preview projection')).toBeNull();
  expect(screen.getByRole('button', { name: 'SECRET_ENV' })).toBeTruthy();
  expect(document.body.innerHTML).not.toContain(SENTINEL);
  expect(retained(s)).not.toContain(SENTINEL);
});

it('S1-15 a Provider credential is never read back from a shadowed definition', async () => {
  const s = cfg3Client(); redactedSources(s);
  render(<SettingsSurface client={s.client} target={workspaceSettingsTarget('A', 'Workspace A')} host={cfg3Host(s)} />);
  await settingsReady();
  // The inherited identity is reachable and reported as a redacted native fact.
  await openResourceRow('transport');
  const dialog = screen.getByRole('region', { name: 'Settings' });
  const provider = screen.getByRole('form', { name: 'Provider transport' });
  const form = within(provider);
  expect(within(dialog).queryByText(/Native effective Provider transport/)).toBeNull();
  const endpoint = form.getByLabelText('Base URL') as HTMLInputElement;
  expect(endpoint.value).toBe('https://user.invalid');
  expect(endpoint.matches(':disabled')).toBe(true);
  expect((form.getByLabelText('API Key') as HTMLInputElement).value).toBe('');
  const transactionsBefore = retained(s);
  const requestsBefore = s.request.mock.calls.slice();
  const sourceBefore = structuredClone(s.source);
  fireEvent.click(form.getByRole('button', { name: 'Provider actions' }));
  expect(await screen.findByRole('menuitem', { name: 'Customize' })).toBeTruthy();
  expect(screen.queryByRole('menuitem', { name: 'Use an API Key' })).toBeNull();
  fireEvent.keyDown(screen.getByRole('menu'), { key: 'Escape' });
  expect(screen.getByRole('region', { name: 'Settings' })).toBe(dialog);
  expect(screen.getByRole('form', { name: 'Provider transport' })).toBe(provider);
  expect(document.body.innerHTML).not.toContain(SENTINEL);
  expect(retained(s)).not.toContain(SENTINEL);
  expect(retained(s)).toBe(transactionsBefore);
  expect(s.request.mock.calls).toEqual(requestsBefore);
  expect(writes(s)).toHaveLength(0);
  expect(s.source).toEqual(sourceBefore);
});

it('S1-15 an MCP literal environment value is never projected back into its editor', async () => {
  const s = cfg3Client(); redactedSources(s);
  render(<SettingsSurface client={s.client} target={userSettingsTarget} host={cfg3Host(s)} />);
  await settingsReady();
  await openSettingsPage('Extensions');
  await openResourceModule('mcp');
  await openResourceRow('search');
  // Native cleared the literal map and named only the identities it retained.
  expect(screen.queryByRole('group', { name: 'Literal environment' })).toBeNull();
  expect(s.source.user_mcp.authored?.search.retained_env).toEqual(['TOKEN']);
  expect(screen.queryByRole('group',{name:'Retain existing environment keys'})).toBeNull();
  expect(document.body.innerHTML).not.toContain(SENTINEL);
  expect(retained(s)).not.toContain(SENTINEL);
});

it('S2-12 the form library opens no devtools channel, so a typed secret is never broadcast or queued for one', async () => {
  // Act as a devtools bus would: answer the handshake and record every event
  // dispatched on the page. The form library must never attempt either.
  const observed: string[] = [];
  const handshake = () => { observed.push('tanstack-connect'); window.dispatchEvent(new CustomEvent('tanstack-connect-success')); };
  const record = (event: Event) => observed.push(JSON.stringify((event as CustomEvent).detail));
  window.addEventListener('tanstack-connect', handshake);
  window.addEventListener('tanstack-dispatch-event', record);
  try {
    const s = cfg3Client();
    render(<SettingsSurface client={s.client} target={userSettingsTarget} />);
    await settingsReady();
    await openSettingsPage('Models');
    await openResourceRow('transport');
    await providerAction('Use an API Key');
    fireEvent.change(screen.getByLabelText('API Key'), { target: { value: SENTINEL } });
    // The draft legitimately holds the secret in the actor-owned transaction.
    await waitFor(() => expect(retained(s)).toContain(SENTINEL));
    expect(observed).toEqual([]);
  } finally {
    window.removeEventListener('tanstack-connect', handshake);
    window.removeEventListener('tanstack-dispatch-event', record);
  }
});

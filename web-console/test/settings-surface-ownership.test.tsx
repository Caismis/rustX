// @vitest-environment jsdom
import { afterEach, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { Settings } from '../src/app/settings/Settings';
import { SessionConfiguration } from '../src/app/SessionConfiguration';
import { userSettingsTarget, workspaceSettingsTarget } from '../src/app/settings/projection';
import { OutcomeUncertain } from '../src/client/app-server';
import { cfg3Application } from './cfg3-data';
import { cfg3Client, cfg3Host, cfg3Session } from './cfg3-fixture';
import { snapshot } from './fixture';
afterEach(cleanup);

const sourcesReads = (s: ReturnType<typeof cfg3Client>) => s.request.mock.calls.filter(([op]) => op.method === 'configuration/sourcesRead');
const writes = (s: ReturnType<typeof cfg3Client>) => s.request.mock.calls.filter(([op]) => op.method === 'configuration/sourceWrite');
const readTarget = (op: { params: unknown }) => (op.params as { target: { kind: string; directory?: string } }).target;
const writeTarget = (op: { params: unknown }) => (op.params as { target: { kind: string; directory?: string } }).target;

it('S1-01 User Settings opens with zero Sessions and zero Workspaces without hidden runtime allocation', async () => {
  const s = cfg3Client(); s.state.views = {}; s.state.sessions = [];
  render(<Settings client={s.client} target={userSettingsTarget} />);
  await screen.findByText(/Revision: user-1/);
  expect(sourcesReads(s)).toHaveLength(1);
  expect(sourcesReads(s)[0][0]).toEqual({ method: 'configuration/sourcesRead', params: { target: { kind: 'user' } } });
  const names = s.request.mock.calls.map(([op]) => op.method);
  expect(names.some(name => ['session/create', 'session/attach', 'session/effectiveConfiguration', 'turn/start', 'configuration/reconcile'].includes(name))).toBe(false);
  expect(screen.queryByLabelText('Configuration owner')).toBeNull();
});

it('S1-02 Workspace Settings stays bound to its exact target across Session focus and fences a revoked target without rerouting the draft', async () => {
  const s = cfg3Client(); const host = cfg3Host(s); const configure = host.configureWorkspace!;
  let revoked = false; host.configureWorkspace = (...args) => revoked ? Promise.reject(new Error('Workspace unregistered or unauthorized')) : configure(...args);
  const ui = render(<Settings client={s.client} target={workspaceSettingsTarget('A', 'Workspace A')} host={host} />);
  await screen.findByText(/Revision: workspace-1/);
  expect(screen.getByRole('heading', { name: 'Workspace Settings — Workspace A' })).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Tools' }));
  fireEvent.click(screen.getByLabelText('read'));
  // Session focus changes elsewhere cannot retarget this editor.
  s.state.views = {};
  ui.rerender(<Settings client={s.client} target={workspaceSettingsTarget('A', 'Workspace A')} host={host} />);
  await waitFor(() => expect((screen.getByLabelText('read') as HTMLInputElement).checked).toBe(true));
  expect(sourcesReads(s).every(([op]) => readTarget(op).kind === 'workspace' && readTarget(op).directory === '/workspace/A')).toBe(true);
  // Revocation fences the target and preserves the local draft; it is never
  // redirected to User or another Workspace.
  revoked = true;
  fireEvent.click(screen.getByRole('button', { name: 'Read current sources' }));
  await screen.findByRole('alert');
  expect((screen.getByLabelText('read') as HTMLInputElement).checked).toBe(true);
  fireEvent.click(screen.getByRole('button', { name: 'Save Native Tools' }));
  expect(writes(s)).toHaveLength(0);
  expect(s.request.mock.calls.every(([op]) => op.method !== 'configuration/sourceWrite' || writeTarget(op).kind === 'workspace')).toBe(true);
});

it('S1-04 Use global default removes the authored unit through exact CAS and never copies the User value or materializes defaults', async () => {
  const s = cfg3Client(); const host = cfg3Host(s);
  s.source.resolved = { agent: { tools: { builtin: ['read'] } } } as never;
  render(<Settings client={s.client} target={workspaceSettingsTarget('A', 'A')} host={host} />);
  await screen.findByText(/Revision: workspace-1/); fireEvent.click(screen.getByRole('button', { name: 'Tools' }));
  // The inherited native value is displayed, but the draft stays absent: the
  // effective value is never materialized into the authored form.
  expect((screen.getByLabelText('read') as HTMLInputElement).checked).toBe(false);
  await screen.findByText(/Inherited — no Workspace override/);
  fireEvent.click(screen.getByRole('button', { name: 'Remove Native Tools' }));
  await waitFor(() => expect(writes(s)[0][0]).toMatchObject({ params: { target: { kind: 'workspace', directory: '/workspace/A' }, expected_revision: 'workspace-1', mutation: { kind: 'config', mutation: { unit: 'native_tools', authored: null } } } }));
  expect(writes(s)).toHaveLength(1);
});

it('S1-06 a successful read clears only the relevant read error, never a distinct write failure', async () => {
  let failReads = false, failWrites = false;
  const s = cfg3Client(async op => {
    if (op.method === 'configuration/sourcesRead' && failReads) throw new Error('authoritative read unavailable');
    if (op.method === 'configuration/sourceWrite' && failWrites) throw new Error('native write rejected');
  });
  const host = cfg3Host(s);
  render(<Settings client={s.client} target={workspaceSettingsTarget('A', 'A')} host={host} />);
  await screen.findByText(/Revision: workspace-1/); fireEvent.click(screen.getByRole('button', { name: 'Tools' }));
  fireEvent.click(screen.getByLabelText('read'));
  // A distinct write failure.
  failWrites = true;
  fireEvent.click(screen.getByRole('button', { name: 'Save Native Tools' }));
  await screen.findByText(/native write rejected/);
  // A failed read reports its own error.
  failReads = true;
  fireEvent.click(screen.getByRole('button', { name: 'Read current sources' }));
  await screen.findByText(/Source read failed/);
  // A successful read clears the read error only; the write failure remains.
  failReads = false;
  fireEvent.click(screen.getByRole('button', { name: 'Read current sources' }));
  await waitFor(() => expect(screen.queryByText(/Source read failed/)).toBeNull());
  expect(screen.getByText(/native write rejected/)).toBeTruthy();
  expect(screen.getByText(/Revision: workspace-1/)).toBeTruthy();
});

it('S1-07 a committed save followed by a failed reread is saved plus uncertain, never unsaved', async () => {
  let failReads = false;
  const s = cfg3Client(async op => { if (op.method === 'configuration/sourcesRead' && failReads) throw new Error('authoritative read unavailable'); });
  const host = cfg3Host(s);
  render(<Settings client={s.client} target={workspaceSettingsTarget('A', 'A')} host={host} />);
  await screen.findByText(/Revision: workspace-1/); fireEvent.click(screen.getByRole('button', { name: 'Tools' }));
  fireEvent.click(screen.getByLabelText('read'));
  failReads = true;
  fireEvent.click(screen.getByRole('button', { name: 'Save Native Tools' }));
  await screen.findByText(/Source saved\. Native coordination/);
  expect(screen.getByText(/Source read failed/)).toBeTruthy();
  expect(screen.getByText(/Last observation retained; current status uncertain/)).toBeTruthy();
  // Exactly one write: the committed mutation is never replayed or presented as unsaved.
  expect(writes(s)).toHaveLength(1);
});

it('S1-09 a lost write reply rereads authority once and never replays or leaks across targets', async () => {
  let lost = true;
  const s = cfg3Client(async (op, source) => {
    if (op.method === 'configuration/sourceWrite' && lost) { lost = false; source.workspace!.revision = 'committed-1'; throw new OutcomeUncertain(); }
  });
  const host = cfg3Host(s);
  const ui = render(<Settings client={s.client} target={workspaceSettingsTarget('A', 'A')} host={host} />);
  await screen.findByText(/Revision: workspace-1/); fireEvent.click(screen.getByRole('button', { name: 'Tools' }));
  fireEvent.click(screen.getByLabelText('read'));
  fireEvent.click(screen.getByRole('button', { name: 'Save Native Tools' }));
  await screen.findByText(/Save outcome uncertain/);
  await waitFor(() => expect(sourcesReads(s).length).toBeGreaterThanOrEqual(2));
  expect(writes(s)).toHaveLength(1);
  // A separate User Settings lifetime never receives the Workspace draft.
  ui.rerender(<Settings client={s.client} target={userSettingsTarget} host={host} />);
  await screen.findByText(/Revision: user-1/); fireEvent.click(screen.getByRole('button', { name: 'Tools' }));
  expect((screen.getByLabelText('read') as HTMLInputElement).checked).toBe(false);
});

it('S1-10 Session configuration uses native facts, correct owner navigation, and fabricates no candidate', async () => {
  const open = vi.fn();
  const s = cfg3Client(); s.source.application = { ...cfg3Application(), scope: 'source:workspace:/workspace/A', candidate: null, units: { capabilities: { status: 'failed', diagnostic: 'resource failed' } } };
  const ui = render(<SessionConfiguration client={s.client} view={s.state.views[cfg3Session]} openOwningSettings={open} />);
  await screen.findByText(/Some configuration preparation failed/);
  expect(screen.queryByRole('button', { name: 'Adopt configuration' })).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Open owning source Settings' }));
  expect(open).toHaveBeenCalledWith('source:workspace:/workspace/A');
  // A User-scope failure routes to User authoring, never silently elsewhere.
  s.source.application = { ...cfg3Application(), scope: 'source:user', candidate: null, units: { instructions: { status: 'failed', diagnostic: 'bad instructions' } } };
  s.state.views[cfg3Session].snapshot = snapshot('A');
  ui.rerender(<SessionConfiguration client={s.client} view={s.state.views[cfg3Session]} openOwningSettings={open} />);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Open owning source Settings' })).toBeTruthy());
  fireEvent.click(screen.getByRole('button', { name: 'Open owning source Settings' }));
  expect(open).toHaveBeenCalledWith('source:user');
  // Preparing never shows an Adopt action.
  s.source.application = { ...cfg3Application(), candidate: null, units: { capabilities: { status: 'preparing' } } };
  s.state.views[cfg3Session].snapshot = snapshot('A');
  ui.rerender(<SessionConfiguration client={s.client} view={s.state.views[cfg3Session]} openOwningSettings={open} />);
  await screen.findByText(/Preparing configuration/);
  expect(screen.queryByRole('button', { name: 'Adopt configuration' })).toBeNull();
});

it('S1-12 invalid configuration stays repairable and is not presented as empty or default', async () => {
  const s = cfg3Client(); const host = cfg3Host(s);
  s.source.workspace = { path: '/workspace/rustx.toml', revision: 'workspace-1', authored: null, diagnostic: 'invalid rustx.toml' };
  render(<Settings client={s.client} target={workspaceSettingsTarget('A', 'A')} host={host} />);
  await screen.findByText(/invalid rustx\.toml/);
  expect(screen.getByRole('form', { name: 'Repair malformed source' })).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'General' }));
  expect(screen.getAllByText(/Authored source is invalid/).length).toBeGreaterThan(0);
  expect(screen.queryByText(/Inherited — no Workspace override/)).toBeNull();
  expect(screen.queryByText(/Native default — no authored value/)).toBeNull();
  // Repair submits an exact revision-fenced repair mutation.
  const repair = screen.getByRole('form', { name: 'Repair malformed source' });
  fireEvent.change(repair.querySelector('textarea')!, { target: { value: '[agent]\n' } });
  fireEvent.click(repair.querySelector('button[type="submit"]')!);
  await waitFor(() => expect(writes(s)[0][0]).toMatchObject({ params: { expected_revision: 'workspace-1', mutation: { kind: 'repair_config', document: '[agent]\n' } } }));
});

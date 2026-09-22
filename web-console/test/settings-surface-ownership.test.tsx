// @vitest-environment jsdom
import { afterEach, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { Settings } from '../src/app/settings/Settings';
import { SessionConfiguration } from '../src/app/SessionConfiguration';
import { userSettingsTarget, workspaceSettingsTarget } from '../src/app/settings/projection';
import { OutcomeUncertain } from '../src/client/app-server';
import type { ConfigurationApplication, SourceTarget } from '../../protocol/app-server/v18';
import { cfg3Application } from './cfg3-data';
import { cfg3Client, cfg3Host, cfg3Session } from './cfg3-fixture';
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

// Blocking finding 1 — an inherited unit displays the native effective value
// while this browser authors nothing. The six required transitions below are
// each proven independently.
async function inheritedTools(s: ReturnType<typeof cfg3Client>) {
  s.source.resolved = { agent: { tools: { builtin: ['read'] } } } as never;
  s.source.provenance = { 'agent.tools.builtin': { kind: 'user', document: '/bound/rustx.toml', base: '/bound' } };
  render(<Settings client={s.client} target={workspaceSettingsTarget('A', 'A')} host={cfg3Host(s)} />);
  await screen.findByText(/Revision: workspace-1/);
  fireEvent.click(screen.getByRole('button', { name: 'Tools' }));
  return within(screen.getByRole('form', { name: 'Native Tools' }));
}

it('S1-04 an inherited unit displays the native effective value and authors nothing', async () => {
  const s = cfg3Client(); const form = await inheritedTools(s);
  // The authoritative effective value is what the control shows.
  expect((form.getByLabelText('read') as HTMLInputElement).checked).toBe(true);
  expect((form.getByLabelText('write') as HTMLInputElement).checked).toBe(false);
  // Authored presence stays absent and the origin is the native one.
  expect(form.getByText(/Inherited — no Workspace override/).textContent).toContain('Inherited from User');
  expect(form.getByText(/Native effective value available/)).toBeTruthy();
  // Nothing is dirty, so there is no authored intent to submit.
  expect((form.getByRole('button', { name: 'Save Native Tools' }) as HTMLButtonElement).disabled).toBe(true);
});

it('S1-04 opening an inherited unit and saving without an edit writes nothing', async () => {
  const s = cfg3Client(); const form = await inheritedTools(s);
  fireEvent.click(form.getByRole('button', { name: 'Save Native Tools' }));
  fireEvent.submit(screen.getByRole('form', { name: 'Native Tools' }));
  await waitFor(() => expect(sourcesReads(s).length).toBeGreaterThanOrEqual(1));
  // No `configuration/sourceWrite` at all: the inherited ["read"] is never
  // materialized into Workspace authoring, and no empty override is created.
  expect(writes(s)).toHaveLength(0);
});

it('S1-04 an explicit edit authors exactly the edited unit', async () => {
  const s = cfg3Client(); const form = await inheritedTools(s);
  fireEvent.click(form.getByLabelText('write'));
  expect((form.getByRole('button', { name: 'Save Native Tools' }) as HTMLButtonElement).disabled).toBe(false);
  fireEvent.click(form.getByRole('button', { name: 'Save Native Tools' }));
  // The edit starts from the displayed native effective value, so the authored
  // unit is exactly what the user sees plus the change they made.
  await waitFor(() => expect(writes(s)[0][0]).toMatchObject({ params: { target: { kind: 'workspace', directory: '/workspace/A' }, expected_revision: 'workspace-1', mutation: { kind: 'config', mutation: { unit: 'native_tools', authored: ['read', 'write'] } } } }));
  expect(writes(s)).toHaveLength(1);
});

it('S1-04 an explicit Override then an explicit empty selection writes [] and never null', async () => {
  const s = cfg3Client(); const form = await inheritedTools(s);
  fireEvent.click(form.getByRole('button', { name: 'Override Native Tools' }));
  // Override is an explicit authoring action; it starts from what is displayed.
  expect((form.getByLabelText('read') as HTMLInputElement).checked).toBe(true);
  fireEvent.click(form.getByLabelText('read'));
  expect((form.getByLabelText('read') as HTMLInputElement).checked).toBe(false);
  fireEvent.click(form.getByRole('button', { name: 'Save Native Tools' }));
  await waitFor(() => expect(writes(s)[0][0]).toMatchObject({ params: { expected_revision: 'workspace-1', mutation: { kind: 'config', mutation: { unit: 'native_tools', authored: [] } } } }));
  // An explicit empty array is a legal authored value, never a removal.
  expect((writes(s)[0][0].params as { mutation: { mutation: { authored: unknown } } }).mutation.mutation.authored).toEqual([]);
  expect(writes(s)).toHaveLength(1);
});

it('S1-04 Use global default removes the authored unit through exact CAS and never copies the User value', async () => {
  const s = cfg3Client(); const form = await inheritedTools(s);
  fireEvent.click(form.getByRole('button', { name: 'Remove Native Tools' }));
  await waitFor(() => expect(writes(s)[0][0]).toMatchObject({ params: { target: { kind: 'workspace', directory: '/workspace/A' }, expected_revision: 'workspace-1', mutation: { kind: 'config', mutation: { unit: 'native_tools', authored: null } } } }));
  expect(writes(s)).toHaveLength(1);
});

it('S1-04 Discard returns to the inherited presentation without authoring anything', async () => {
  const s = cfg3Client(); const form = await inheritedTools(s);
  fireEvent.click(form.getByLabelText('write'));
  expect((form.getByLabelText('write') as HTMLInputElement).checked).toBe(true);
  fireEvent.click(form.getByRole('button', { name: 'Discard draft' }));
  // Back to the native effective value, not to a client-side empty default.
  expect((form.getByLabelText('read') as HTMLInputElement).checked).toBe(true);
  expect((form.getByLabelText('write') as HTMLInputElement).checked).toBe(false);
  expect(form.getByText(/Inherited — no Workspace override/)).toBeTruthy();
  expect((form.getByRole('button', { name: 'Save Native Tools' }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.submit(screen.getByRole('form', { name: 'Native Tools' }));
  await waitFor(() => expect(sourcesReads(s).length).toBeGreaterThanOrEqual(1));
  expect(writes(s)).toHaveLength(0);
});

it('S1-04 an authored Workspace override is displayed and reported as an override, not as inheritance', async () => {
  const s = cfg3Client();
  s.source.resolved = { agent: { tools: { builtin: ['bash'] } } } as never;
  s.source.provenance = { 'agent.tools.builtin': { kind: 'workspace', document: '/workspace/rustx.toml', base: '/workspace' } };
  s.source.workspace!.authored = { agent: { tools: { builtin: ['bash'] } } };
  render(<Settings client={s.client} target={workspaceSettingsTarget('A', 'A')} host={cfg3Host(s)} />);
  await screen.findByText(/Revision: workspace-1/);
  fireEvent.click(screen.getByRole('button', { name: 'Tools' }));
  const form = within(screen.getByRole('form', { name: 'Native Tools' }));
  expect((form.getByLabelText('bash') as HTMLInputElement).checked).toBe(true);
  expect(form.getByText(/Workspace override — empty selections remain explicit/).textContent).toContain('Workspace override');
  // An existing override needs no Override action, and a no-op Save is still
  // unavailable: authoring is an edit, never a re-write of the same value.
  expect(form.queryByRole('button', { name: 'Override Native Tools' })).toBeNull();
  expect((form.getByRole('button', { name: 'Save Native Tools' }) as HTMLButtonElement).disabled).toBe(true);
});

// Blocking finding 4 — a malformed lower source makes the effective value
// unavailable without making this scope's valid authored state absent or empty.
it('S1-13 an unresolvable configuration never presents the effective value as Unset or a default', async () => {
  const s = cfg3Client();
  s.source.resolved = null;
  s.source.prospective_diagnostic = 'Source cannot be resolved; repair the diagnosed authored document.';
  s.source.user = { path: '/bound/rustx.toml', revision: 'user-1', authored: null, diagnostic: 'invalid rustx.toml; source was not loaded' };
  render(<Settings client={s.client} target={workspaceSettingsTarget('A', 'A')} host={cfg3Host(s)} />);
  await screen.findByText(/Revision: workspace-1/);
  fireEvent.click(screen.getByRole('button', { name: 'Tools' }));
  const form = within(screen.getByRole('form', { name: 'Native Tools' }));
  // Authored presence and effective resolution are reported independently.
  expect(form.getByText(/Inherited — no Workspace override/).textContent).toContain('Native effective value unavailable — resolution failed');
  expect(form.getByText(/Native effective resolution failed/).textContent).toContain('Source cannot be resolved');
  const resolved = screen.getByRole('form', { name: 'Native Tools' }).querySelector('pre')!;
  expect(resolved.textContent).toBe('Native effective value unavailable — resolution failed');
  expect(resolved.textContent).not.toContain('Unset');
  // The valid Workspace source remains authorable through exact CAS.
  fireEvent.click(form.getByLabelText('read'));
  fireEvent.click(form.getByRole('button', { name: 'Save Native Tools' }));
  await waitFor(() => expect(writes(s)[0][0]).toMatchObject({ params: { expected_revision: 'workspace-1', mutation: { kind: 'config', mutation: { unit: 'native_tools', authored: ['read'] } } } }));
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

// Blocking finding 2 — owner navigation uses the native source owners, not the
// application scope key. Every case below uses a real Session application scope.
function failedSession(sources: readonly SourceTarget[]): ConfigurationApplication {
  return { ...cfg3Application(), scope: cfg3Session, sources: [...sources], candidate: null, units: { capabilities: { status: 'failed', diagnostic: 'resource failed' } } };
}

it('S1-10 a Session configuration failure offers one action per native source owner', async () => {
  const open = vi.fn();
  const s = cfg3Client();
  s.source.application = failedSession([{ kind: 'user' }, { kind: 'workspace', directory: '/workspace/A' }]);
  render(<SessionConfiguration client={s.client} view={s.state.views[cfg3Session]} openOwningSettings={open} />);
  await screen.findByText(/Some configuration preparation failed/);
  expect(screen.queryByRole('button', { name: 'Adopt configuration' })).toBeNull();
  // The scope is the Session identity; it is never offered as a source owner.
  expect(s.source.application.scope).toBe(cfg3Session);
  expect(screen.queryByRole('button', { name: /source:/ })).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Open User Settings' }));
  expect(open).toHaveBeenLastCalledWith({ kind: 'user' });
  fireEvent.click(screen.getByRole('button', { name: 'Open Workspace Settings — /workspace/A' }));
  expect(open).toHaveBeenLastCalledWith({ kind: 'workspace', directory: '/workspace/A' });
});

it('S1-10 a User-owned Session application offers only User authoring', async () => {
  const open = vi.fn();
  const s = cfg3Client();
  s.source.application = failedSession([{ kind: 'user' }]);
  render(<SessionConfiguration client={s.client} view={s.state.views[cfg3Session]} openOwningSettings={open} />);
  await screen.findByText(/Some configuration preparation failed/);
  expect(screen.queryByRole('button', { name: /Open Workspace Settings/ })).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Open User Settings' }));
  expect(open).toHaveBeenCalledWith({ kind: 'user' });
});

it('S1-10 preparing and adopted states fabricate no candidate and no owner action', async () => {
  const open = vi.fn();
  const s = cfg3Client();
  s.source.application = { ...cfg3Application(), scope: cfg3Session, candidate: null, units: { capabilities: { status: 'preparing' } } };
  render(<SessionConfiguration client={s.client} view={s.state.views[cfg3Session]} openOwningSettings={open} />);
  await screen.findByText(/Preparing configuration/);
  expect(screen.queryByRole('button', { name: 'Adopt configuration' })).toBeNull();
  expect(screen.queryByRole('button', { name: /^Open / })).toBeNull();
  expect(open).not.toHaveBeenCalled();
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

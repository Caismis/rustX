// @vitest-environment jsdom
import { afterEach, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { settingsTransactionOwners } from '../src/app/settings/Settings';
import { SettingsSurface } from './settings-harness';
import { SessionConfiguration } from '../src/app/SessionConfiguration';
import { userSettingsTarget, workspaceSettingsTarget } from '../src/app/settings/projection';
import { OutcomeUncertain } from '../src/client/app-server';
import type { ConfigurationApplication, SourceSettings, SourceTarget } from '../../protocol/app-server/v18';
import { cfg3Application } from './cfg3-data';
import { cfg3Client, cfg3Host, cfg3Session } from './cfg3-fixture';
afterEach(cleanup);

const sourcesReads = (s: ReturnType<typeof cfg3Client>) => s.request.mock.calls.filter(([op]) => op.method === 'configuration/sourcesRead');
const writes = (s: ReturnType<typeof cfg3Client>) => s.request.mock.calls.filter(([op]) => op.method === 'configuration/sourceWrite');
const readTarget = (op: { params: unknown }) => (op.params as { target: { kind: string; directory?: string } }).target;
const writeTarget = (op: { params: unknown }) => (op.params as { target: { kind: string; directory?: string } }).target;

it('S1-01 User Settings opens with zero Sessions and zero Workspaces without hidden runtime allocation', async () => {
  const s = cfg3Client(); s.state.views = {}; s.state.sessions = [];
  render(<SettingsSurface client={s.client} target={userSettingsTarget} />);
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
  const ui = render(<SettingsSurface client={s.client} target={workspaceSettingsTarget('A', 'Workspace A')} host={host} />);
  await screen.findByText(/Revision: workspace-1/);
  expect(screen.getByRole('heading', { name: 'Workspace Settings — Workspace A' })).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Tools' }));
  fireEvent.click(screen.getByLabelText('read'));
  // Session focus changes elsewhere cannot retarget this editor.
  s.state.views = {};
  ui.rerender(<SettingsSurface client={s.client} target={workspaceSettingsTarget('A', 'Workspace A')} host={host} />);
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
  render(<SettingsSurface client={s.client} target={workspaceSettingsTarget('A', 'A')} host={cfg3Host(s)} />);
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

it('S1-04 an inherited unit offers no Use global default, because there is no Workspace override to remove', async () => {
  const s = cfg3Client(); const form = await inheritedTools(s);
  // "Use global default" means "remove the Workspace-authored unit through
  // exact CAS". With no override authored, that mutation has no meaning and is
  // not offered; Override is the action that exists here.
  expect(form.queryByRole('button', { name: /Use global default/ })).toBeNull();
  expect(form.queryByRole('button', { name: /^Remove Native Tools/ })).toBeNull();
  expect(form.getByRole('button', { name: 'Override Native Tools' })).toBeTruthy();
  expect(writes(s)).toHaveLength(0);
});

it('S1-04 Use global default removes a real Workspace override through exact CAS and returns to the native inherited value', async () => {
  // The Workspace really overrides the unit: User resolves ["read"], this
  // Workspace authors ["bash"].
  const s = cfg3Client(async (op, source) => {
    if (op.method === 'configuration/sourceWrite') {
      // Native commits the removal: the Workspace authors the unit no longer,
      // and the User value becomes the effective one again.
      delete source.workspace!.authored!.agent;
      source.resolved = { agent: { tools: { builtin: ['read'] } } } as never;
      source.provenance = { 'agent.tools.builtin': { kind: 'user', document: '/bound/rustx.toml', base: '/bound' } };
    }
  });
  s.source.resolved = { agent: { tools: { builtin: ['bash'] } } } as never;
  s.source.provenance = { 'agent.tools.builtin': { kind: 'workspace', document: '/workspace/rustx.toml', base: '/workspace' } };
  s.source.workspace!.authored = { agent: { tools: { builtin: ['bash'] } } };
  render(<SettingsSurface client={s.client} target={workspaceSettingsTarget('A', 'A')} host={cfg3Host(s)} />);
  await screen.findByText(/Revision: workspace-1/);
  fireEvent.click(screen.getByRole('button', { name: 'Tools' }));
  const before = within(screen.getByRole('form', { name: 'Native Tools' }));
  // The Workspace override is what is displayed and reported.
  expect((before.getByLabelText('bash') as HTMLInputElement).checked).toBe(true);
  expect(before.getByText(/Workspace override — empty selections remain explicit/).textContent).toContain('Workspace override');
  fireEvent.click(before.getByRole('button', { name: 'Use global default Native Tools' }));
  // The outgoing mutation is the exact revision-fenced removal of this unit.
  await waitFor(() => expect(writes(s)[0][0]).toMatchObject({ params: { target: { kind: 'workspace', directory: '/workspace/A' }, expected_revision: 'workspace-1', mutation: { kind: 'config', mutation: { unit: 'native_tools', authored: null } } } }));
  await screen.findByText(/Revision: saved-2/);
  // The authoritative reread is what decides the resulting presentation: the
  // Workspace authors nothing, and the native inherited value is effective.
  const after = within(screen.getByRole('form', { name: 'Native Tools' }));
  await waitFor(() => expect((after.getByLabelText('read') as HTMLInputElement).checked).toBe(true));
  expect((after.getByLabelText('bash') as HTMLInputElement).checked).toBe(false);
  expect(after.getByText(/Inherited — no Workspace override/).textContent).toContain('Inherited from User');
  // No pending draft, no residual removal action, and exactly one write.
  expect((after.getByRole('button', { name: 'Save Native Tools' }) as HTMLButtonElement).disabled).toBe(true);
  expect(after.queryByRole('button', { name: /Use global default/ })).toBeNull();
  expect(after.queryByRole('button', { name: 'Use reviewed revision' })).toBeNull();
  expect(writes(s)).toHaveLength(1);
});

it.each([
  ['an explicit empty list', { agent: { tools: { builtin: [] } } }, 'Tools', 'Native Tools'],
  ['an explicit false', { agent: { plugins: { todo: { enabled: false } } } }, 'Plugins', 'todo Plugin'],
  ['an explicit empty object', { agent: { plugins: { todo: {} } } }, 'Plugins', 'todo Plugin'],
  ['an explicit empty string', { agent: { description: '' } }, 'General', 'Root description'],
] as const)('S1-04 %s is an authored Workspace override, never an absent one', async (_label, authored, section, unit) => {
  const s = cfg3Client();
  // `[]`, `false`, `{}` and `""` are authored values. Presence is the native
  // projection fact, never a truthiness test, so each is reported as an
  // override and each offers the removal that really applies to it.
  s.source.workspace!.authored = authored as never;
  render(<SettingsSurface client={s.client} target={workspaceSettingsTarget('A', 'A')} host={cfg3Host(s)} />);
  await screen.findByText(/Revision: workspace-1/);
  fireEvent.click(screen.getByRole('button', { name: section }));
  const form = within(screen.getByRole('form', { name: unit }));
  expect(form.getByText(/Workspace override — empty selections remain explicit/)).toBeTruthy();
  expect(form.getByRole('button', { name: `Use global default ${unit}` })).toBeTruthy();
  expect(form.queryByRole('button', { name: `Override ${unit}` })).toBeNull();
  expect(writes(s)).toHaveLength(0);
});

it('S1-04 an invalid Workspace document offers no unit editor and no removal, because native cannot mutate a document it cannot parse', async () => {
  const s = cfg3Client();
  s.source.workspace = { path: '/workspace/rustx.toml', revision: 'workspace-1', authored: null, diagnostic: 'invalid rustx.toml' };
  render(<SettingsSurface client={s.client} target={workspaceSettingsTarget('A', 'A')} host={cfg3Host(s)} />);
  await screen.findByText(/invalid rustx\.toml/);
  fireEvent.click(screen.getByRole('button', { name: 'Tools' }));
  expect(screen.queryByRole('form', { name: 'Native Tools' })).toBeNull();
  expect(screen.queryByRole('button', { name: /Use global default/ })).toBeNull();
  expect(writes(s)).toHaveLength(0);
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
  // Nothing browser-authored is left, so nothing is offered for discarding.
  expect(form.queryByRole('button', { name: 'Discard draft' })).toBeNull();
  fireEvent.submit(screen.getByRole('form', { name: 'Native Tools' }));
  await waitFor(() => expect(sourcesReads(s).length).toBeGreaterThanOrEqual(1));
  expect(writes(s)).toHaveLength(0);
});

it('S1-04 an authored Workspace override is displayed and reported as an override, not as inheritance', async () => {
  const s = cfg3Client();
  s.source.resolved = { agent: { tools: { builtin: ['bash'] } } } as never;
  s.source.provenance = { 'agent.tools.builtin': { kind: 'workspace', document: '/workspace/rustx.toml', base: '/workspace' } };
  s.source.workspace!.authored = { agent: { tools: { builtin: ['bash'] } } };
  render(<SettingsSurface client={s.client} target={workspaceSettingsTarget('A', 'A')} host={cfg3Host(s)} />);
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
  render(<SettingsSurface client={s.client} target={workspaceSettingsTarget('A', 'A')} host={cfg3Host(s)} />);
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
  render(<SettingsSurface client={s.client} target={workspaceSettingsTarget('A', 'A')} host={host} />);
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
  render(<SettingsSurface client={s.client} target={workspaceSettingsTarget('A', 'A')} host={host} />);
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
  const ui = render(<SettingsSurface client={s.client} target={workspaceSettingsTarget('A', 'A')} host={host} />);
  await screen.findByText(/Revision: workspace-1/); fireEvent.click(screen.getByRole('button', { name: 'Tools' }));
  fireEvent.click(screen.getByLabelText('read'));
  fireEvent.click(screen.getByRole('button', { name: 'Save Native Tools' }));
  await screen.findByText(/Save outcome uncertain/);
  await waitFor(() => expect(sourcesReads(s).length).toBeGreaterThanOrEqual(2));
  expect(writes(s)).toHaveLength(1);
  // A separate User Settings lifetime never receives the Workspace draft.
  ui.rerender(<SettingsSurface client={s.client} target={userSettingsTarget} host={host} />);
  await screen.findByText(/Revision: user-1/); fireEvent.click(screen.getByRole('button', { name: 'Tools' }));
  expect((screen.getByLabelText('read') as HTMLInputElement).checked).toBe(false);
});

it.each(['before', 'after'] as const)('S1-09 an uncertain save stays uncertain when the connection generation is replaced %s its outcome arrives', async order => {
  let fail!: () => void;
  const held = new Promise<never>((_, reject) => { fail = () => reject(new OutcomeUncertain()); });
  held.catch(() => {});
  const s = cfg3Client(async op => { if (op.method === 'configuration/sourceWrite') return held; });
  render(<SettingsSurface client={s.client} target={userSettingsTarget} />);
  await screen.findByText(/Revision: user-1/); fireEvent.click(screen.getByRole('button', { name: 'Tools' }));
  fireEvent.click(screen.getByLabelText('read'));
  fireEvent.click(screen.getByRole('button', { name: 'Save Native Tools' }));
  await waitFor(() => expect(writes(s)).toHaveLength(1));
  const outcome = async () => { await act(async () => { fail(); await held.catch(() => {}); }); await screen.findByText(/Save outcome uncertain/); };
  // The App Server connection is replaced: a new generation with its own
  // observation lifetime. The browser never learns whether the write landed.
  const replace = async () => {
    const reads = sourcesReads(s).length;
    act(() => s.publish({ generation: 2 }));
    await waitFor(() => expect(sourcesReads(s).length).toBeGreaterThan(reads));
    await screen.findByText(/Revision: user-1/);
  };
  if (order === 'before') { await replace(); await outcome(); } else { await outcome(); await replace(); }
  // Identical in both delivery orders: the outcome is still uncertain, the
  // draft is intact, and the write was never replayed.
  expect(screen.getByText(/Save outcome uncertain/)).toBeTruthy();
  expect((screen.getByLabelText('read') as HTMLInputElement).checked).toBe(true);
  expect(screen.queryByText(/Source saved/)).toBeNull();
  expect(writes(s)).toHaveLength(1);
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
  render(<SettingsSurface client={s.client} target={workspaceSettingsTarget('A', 'A')} host={host} />);
  await screen.findByText(/invalid rustx\.toml/);
  expect(screen.getByRole('form', { name: 'Repair malformed source' })).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'General' }));
  // No structured editor presents the unparsed document as empty, inherited or
  // defaulted: it names the malformed document and offers only its repair.
  expect(screen.getByText(/Structured editing is unavailable because \/workspace\/rustx\.toml does not parse/)).toBeTruthy();
  expect(screen.queryByText(/Inherited — no Workspace override/)).toBeNull();
  expect(screen.queryByText(/Native default — no authored value/)).toBeNull();
  // Repair submits an exact revision-fenced repair mutation.
  const repair = screen.getByRole('form', { name: 'Repair malformed source' });
  fireEvent.change(repair.querySelector('textarea')!, { target: { value: '[agent]\n' } });
  fireEvent.click(repair.querySelector('button[type="submit"]')!);
  await waitFor(() => expect(writes(s)[0][0]).toMatchObject({ params: { expected_revision: 'workspace-1', mutation: { kind: 'repair_config', document: '[agent]\n' } } }));
});

// Blocking finding — no secret-bearing authored payload may survive a confirmed
// commit merely so a later authoritative projection can settle the transaction.
const SECRET_SENTINEL = 'SECRET_SENTINEL';
/** Everything the Settings transaction owner itself still holds. The assertion
 * is on the owner's retained state, not on the DOM: an editor that unmounted
 * proves nothing about what the store kept. */
const retained = (s: ReturnType<typeof cfg3Client>) => JSON.stringify(settingsTransactionOwners(s.client).map(owner => owner.retainedState()));

it('S1-14 a confirmed Provider literal-secret save drops the submitted payload even when the authoritative reread fails, and settles later without replay', async () => {
  let failReads = false;
  const s = cfg3Client(async (op, source) => {
    if (op.method === 'configuration/sourcesRead' && failReads) throw new Error('authoritative read unavailable');
    if (op.method === 'configuration/sourceWrite') {
      // Native commits the literal credential and reports it redacted.
      source.workspace!.authored = { providers: { secret: { base_url: 'https://native.invalid', credential: { type: 'literal' } } } };
    }
  });
  render(<SettingsSurface client={s.client} target={workspaceSettingsTarget('A', 'A')} host={cfg3Host(s)} />);
  await screen.findByText(/Revision: workspace-1/);
  fireEvent.click(screen.getByRole('button', { name: 'Providers & Models' }));
  fireEvent.change(screen.getByLabelText('New Provider identity'), { target: { value: 'secret' } });
  fireEvent.click(screen.getByRole('button', { name: 'Add Provider' }));
  fireEvent.change(screen.getByLabelText('Endpoint'), { target: { value: 'https://native.invalid' } });
  fireEvent.change(screen.getByLabelText('Credential source'), { target: { value: 'literal' } });
  fireEvent.change(screen.getByLabelText('New literal credential'), { target: { value: SECRET_SENTINEL } });
  // Before submission the sentinel is the live editing draft, which is where an
  // authored secret legitimately lives.
  expect(retained(s)).toContain(SECRET_SENTINEL);
  failReads = true;
  fireEvent.click(screen.getByRole('button', { name: 'Save Provider secret' }));
  // The write commits and native acknowledges it; the post-write authoritative
  // reread fails, so the transaction cannot settle yet.
  await screen.findByText(/Source saved\. Native coordination/);
  await screen.findByText(/Saved, but the authoritative reread failed/);
  expect(writes(s)).toHaveLength(1);
  // Navigating away unmounts the editor; the store survives, and it must no
  // longer hold the submitted authored payload anywhere.
  fireEvent.click(screen.getByRole('button', { name: 'Tools' }));
  expect(retained(s)).not.toContain(SECRET_SENTINEL);
  expect(document.body.innerHTML).not.toContain(SECRET_SENTINEL);
  // The unsettled transaction is still there — it just carries no payload.
  expect(retained(s)).toContain('"committed":"saved-2"');
  // Authority recovers. The acknowledged mutation settles against the projection
  // that now carries its committed revision, with no second write.
  failReads = false;
  fireEvent.click(screen.getByRole('button', { name: 'Read current sources' }));
  await waitFor(() => expect(retained(s)).not.toContain('"committed"'));
  expect(retained(s)).not.toContain(SECRET_SENTINEL);
  expect(writes(s)).toHaveLength(1);
  // The reopened editor reconstructs from the redacted native projection only.
  fireEvent.click(screen.getByRole('button', { name: 'Providers & Models' }));
  fireEvent.click(screen.getByRole('button', { name: 'Edit Provider secret' }));
  expect((screen.getByLabelText('Credential source') as HTMLSelectElement).value).toBe('retain');
  expect(screen.queryByRole('button', { name: 'Use reviewed revision' })).toBeNull();
});

it('S1-14 a confirmed MCP literal-environment save drops the submitted payload on the same terms', async () => {
  let failReads = false;
  const s = cfg3Client(async (op, source) => {
    if (op.method === 'configuration/sourcesRead' && failReads) throw new Error('authoritative read unavailable');
    if (op.method === 'configuration/sourceWrite') source.workspace_mcp!.revision = 'mcp-committed';
  });
  render(<SettingsSurface client={s.client} target={workspaceSettingsTarget('A', 'A')} host={cfg3Host(s)} />);
  await screen.findByText(/Revision: workspace-1/);
  fireEvent.click(screen.getByRole('button', { name: 'MCP' }));
  fireEvent.change(screen.getByLabelText('New MCP identity'), { target: { value: 'search' } });
  fireEvent.click(screen.getByRole('button', { name: 'Add MCP' }));
  fireEvent.change(screen.getByLabelText('MCP command'), { target: { value: 'search-server' } });
  const literals = within(screen.getByRole('group', { name: 'Literal environment' }));
  fireEvent.change(literals.getByLabelText('Literal environment name'), { target: { value: 'TOKEN' } });
  fireEvent.click(literals.getByRole('button', { name: 'Add Literal environment' }));
  fireEvent.change(literals.getByLabelText('TOKEN'), { target: { value: SECRET_SENTINEL } });
  expect(retained(s)).toContain(SECRET_SENTINEL);
  failReads = true;
  fireEvent.click(screen.getByRole('button', { name: 'Save MCP search' }));
  await screen.findByText(/Saved, but the authoritative reread failed/);
  fireEvent.click(screen.getByRole('button', { name: 'Tools' }));
  // The MCP selector settles against the MCP source revision, not the config
  // one, and it carries no literal environment value to do so.
  expect(retained(s)).not.toContain(SECRET_SENTINEL);
  expect(retained(s)).toContain('"committed":"mcp-committed"');
  failReads = false;
  fireEvent.click(screen.getByRole('button', { name: 'Read current sources' }));
  await waitFor(() => expect(retained(s)).not.toContain('"committed"'));
  expect(writes(s)).toHaveLength(1);
});

// Blocking finding — every editable native semantic identity in the native
// effective projection must be discoverable in Workspace Settings, even with no
// Workspace override, without the browser merging two documents.
function inheritedCatalog(s: ReturnType<typeof cfg3Client>) {
  s.source.resolved = {
    providers: { transport: { base_url: 'https://user.invalid', credential: { type: 'literal' } } },
    models: { main: { provider: 'transport', id: 'wire', protocol: 'openai_responses', context_window: '128000', max_output_tokens: 8192, capabilities: { input_modalities: ['text'], output_modalities: ['text'], tool_calls: true, reasoning: false } } },
  } as never;
  s.source.provenance = {
    'providers.transport': { kind: 'user', document: '/bound/rustx.toml', base: '/bound' },
    'models.main': { kind: 'user', document: '/bound/rustx.toml', base: '/bound' },
  };
  return render(<SettingsSurface client={s.client} target={workspaceSettingsTarget('A', 'A')} host={cfg3Host(s)} />);
}

it('S1-15 an inherited User Provider and Model are discoverable in the Workspace catalog with native provenance', async () => {
  const s = cfg3Client(); inheritedCatalog(s);
  await screen.findByText(/Revision: workspace-1/);
  fireEvent.click(screen.getByRole('button', { name: 'Providers & Models' }));
  const catalog = within(screen.getByRole('region', { name: 'Providers & Models' }));
  // Both identities are listed although this Workspace authors neither, and
  // each is reported with the native origin, not a manufactured one.
  expect(catalog.getAllByText('Inherited from User')).toHaveLength(2);
  expect(catalog.getByText(/https:\/\/user\.invalid/)).toBeTruthy();
  // An inherited literal credential stays redacted; the secret is never read
  // back from the shadowed definition.
  expect(catalog.getByText('Literal secret (redacted)')).toBeTruthy();
  // The action names what it really is in this scope.
  expect(catalog.getByRole('button', { name: 'Override Provider transport' })).toBeTruthy();
  expect(catalog.getByRole('button', { name: 'Override Model main' })).toBeTruthy();
  // An identity that is already reachable is not offered as a new one.
  fireEvent.change(catalog.getByLabelText('New Provider identity'), { target: { value: 'transport' } });
  expect((catalog.getByRole('button', { name: 'Add Provider' }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.change(catalog.getByLabelText('New Model identity'), { target: { value: 'main' } });
  expect((catalog.getByRole('button', { name: 'Add Model' }) as HTMLButtonElement).disabled).toBe(true);
});

it('S1-15 viewing an inherited identity authors nothing and writes nothing', async () => {
  const s = cfg3Client(); inheritedCatalog(s);
  await screen.findByText(/Revision: workspace-1/);
  fireEvent.click(screen.getByRole('button', { name: 'Providers & Models' }));
  fireEvent.click(screen.getByRole('button', { name: 'Override Model main' }));
  // The inherited Model is displayed from the native effective projection while
  // this Workspace authors nothing: no draft, no Save, no removal to offer.
  const model = within(screen.getByRole('form', { name: 'Model main' }));
  expect((screen.getByLabelText('Wire model identity') as HTMLInputElement).value).toBe('wire');
  expect(model.getByText(/Inherited — no Workspace override/).textContent).toContain('Inherited from User');
  expect((model.getByRole('button', { name: 'Save Model main' }) as HTMLButtonElement).disabled).toBe(true);
  expect(model.queryByRole('button', { name: /Use global default/ })).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Back to catalog' }));
  fireEvent.click(screen.getByRole('button', { name: 'Override Provider transport' }));
  // An inherited Provider credential is never projected into authoring state:
  // there is no "retain" option, because there is no credential in this scope.
  const provider = within(screen.getByRole('form', { name: 'Provider transport' }));
  expect((screen.getByLabelText('Endpoint') as HTMLInputElement).value).toBe('');
  expect(screen.getByLabelText('Credential source').textContent).not.toContain('Retain');
  // The native effective definition is still reported, redacted.
  expect(screen.getByText(/Native effective Provider transport/).textContent).toContain('Literal secret (redacted)');
  expect((provider.getByRole('button', { name: 'Save Provider transport' }) as HTMLButtonElement).disabled).toBe(true);
  expect(writes(s)).toHaveLength(0);
});

it('S1-15 an explicit Workspace Provider override authors only that unit and never copies the inherited credential', async () => {
  const s = cfg3Client(); inheritedCatalog(s);
  await screen.findByText(/Revision: workspace-1/);
  fireEvent.click(screen.getByRole('button', { name: 'Providers & Models' }));
  fireEvent.click(screen.getByRole('button', { name: 'Override Provider transport' }));
  fireEvent.change(screen.getByLabelText('Endpoint'), { target: { value: 'https://workspace.invalid' } });
  fireEvent.change(screen.getByLabelText('Environment variable'), { target: { value: 'WORKSPACE_KEY' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save Provider transport' }));
  await waitFor(() => expect(writes(s)[0][0]).toMatchObject({ params: { target: { kind: 'workspace', directory: '/workspace/A' }, expected_revision: 'workspace-1', mutation: { kind: 'config', mutation: { unit: 'provider', id: 'transport', authored: { base_url: 'https://workspace.invalid', credential: { kind: 'environment', variable: 'WORKSPACE_KEY' } } } } } }));
  // Exactly one unit is authored, and the authored credential is the one the
  // user entered — never a copy or reconstruction of the shadowed User one.
  expect(writes(s)).toHaveLength(1);
  expect(JSON.stringify(writes(s)[0][0])).not.toContain('retain');
  expect(JSON.stringify(writes(s)[0][0])).not.toContain('user.invalid');
  expect(JSON.stringify(writes(s)[0][0])).not.toContain('models');
});

it('S1-15 an inherited MCP definition and named Agent are discoverable from the native inventory alone', async () => {
  const s = cfg3Client();
  s.source.prospective_resources = {
    definitions: [
      { family: 'mcp', name: 'search', valid: true, location: { scope: 'user', path: '/home/user/rustx/.agents/mcp.toml' } },
      { family: 'mcp', name: 'local', valid: true, location: { scope: 'workspace', path: '/workspace/.agents/mcp.toml' } },
      { family: 'agent', name: 'reviewer', valid: true, location: { scope: 'user', path: '/home/user/rustx/.agents/agents/reviewer.toml' } },
    ],
    resource_diagnostics: [], agents: {}, workflows: {}, sources: {}, skills: [], skill_diagnostics: [],
  } as never;
  s.source.workspace_mcp!.authored = { local: { definition: { type: 'stdio', command: 'local-server' }, retained_env: [], retained_headers: [] } };
  render(<SettingsSurface client={s.client} target={workspaceSettingsTarget('A', 'A')} host={cfg3Host(s)} />);
  await screen.findByText(/Revision: workspace-1/);
  fireEvent.click(screen.getByRole('button', { name: 'MCP' }));
  const mcp = within(screen.getByRole('region', { name: 'MCP definitions' }));
  // A whole-file resource is owned as one identity; native names the winning
  // scope, so the inherited one is reachable without merging two catalogs.
  expect(mcp.getByRole('button', { name: 'Edit MCP local' })).toBeTruthy();
  expect(mcp.getByRole('button', { name: 'Override MCP search' })).toBeTruthy();
  fireEvent.change(mcp.getByLabelText('New MCP identity'), { target: { value: 'search' } });
  expect((mcp.getByRole('button', { name: 'Add MCP' }) as HTMLButtonElement).disabled).toBe(true);
  // Opening the inherited definition authors nothing in this Workspace.
  fireEvent.click(mcp.getByRole('button', { name: 'Override MCP search' }));
  expect((screen.getByRole('button', { name: 'Save MCP search' }) as HTMLButtonElement).disabled).toBe(true);
  expect(screen.queryByRole('button', { name: /Use global default MCP search/ })).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Agents' }));
  const agents = within(screen.getByRole('region', { name: 'Named Agents' }));
  expect(agents.getByRole('button', { name: 'Override Agent reviewer' })).toBeTruthy();
  expect(writes(s)).toHaveLength(0);
});

/** Author one Provider whose credential is a literal secret, and hold the whole
 * Product Host write response after native `sourceWrite` has definitively
 * committed. The barrier is a deferred promise, never a timer. */
async function heldSecretSave(s: ReturnType<typeof cfg3Client>) {
  let release!: () => void;
  const held = new Promise<void>(resolve => { release = resolve; });
  const host = cfg3Host(s);
  const configure = host.configureWorkspace!;
  host.configureWorkspace = async (id, endpoint, operation) => {
    const outcome = await configure(id, endpoint, operation);
    // Native committed; only the Host/browser response is still in flight.
    if (operation.kind === 'write') await held;
    return outcome;
  };
  const ui = render(<SettingsSurface client={s.client} target={workspaceSettingsTarget('A', 'Workspace A')} host={host} />);
  await screen.findByText(/Revision: workspace-1/);
  fireEvent.click(screen.getByRole('button', { name: 'Providers & Models' }));
  fireEvent.change(screen.getByLabelText('New Provider identity'), { target: { value: 'secret' } });
  fireEvent.click(screen.getByRole('button', { name: 'Add Provider' }));
  fireEvent.change(screen.getByLabelText('Endpoint'), { target: { value: 'https://native.invalid' } });
  fireEvent.change(screen.getByLabelText('Credential source'), { target: { value: 'literal' } });
  fireEvent.change(screen.getByLabelText('New literal credential'), { target: { value: SECRET_SENTINEL } });
  // The dirty draft is the only place the authored secret legitimately lives.
  expect(retained(s)).toContain(SECRET_SENTINEL);
  fireEvent.click(screen.getByRole('button', { name: 'Save Provider secret' }));
  await waitFor(() => expect(writes(s)).toHaveLength(1));
  return { ui, host, release: async () => { await act(async () => { release(); await held; }); } };
}
/** Native commits the literal credential and projects it redacted. */
const commitsProvider = async (op: { method: string }, source: SourceSettings) => {
  if (op.method === 'configuration/sourceWrite') source.workspace!.authored = { providers: { secret: { base_url: 'https://native.invalid', credential: { type: 'literal' } } } };
};

it('S1-16 a definitive acknowledgement outlives the whole Settings dialog and retires the submitted secret-bearing transaction', async () => {
  const s = cfg3Client(commitsProvider);
  const { ui, host, release } = await heldSecretSave(s);
  // The whole Settings dialog closes — not merely the editor section — while
  // the definitive acknowledgement is still in flight.
  ui.unmount();
  await release();
  // A retired presentation lifetime cannot reinterpret a definitive commit as a
  // failed submission: the acknowledgement is recorded against the exact
  // transaction that submitted it, and the confirmed secret-bearing draft is
  // gone from everything that store retains.
  await waitFor(() => expect(retained(s)).toContain('"committed":"saved-2"'));
  expect(retained(s)).not.toContain(SECRET_SENTINEL);
  expect(retained(s)).not.toContain('"draft"');
  // Reopening the same Settings target reaches the same durable store, whose
  // acknowledged mutation settles against the authoritative projection.
  render(<SettingsSurface client={s.client} target={workspaceSettingsTarget('A', 'Workspace A')} host={host} />);
  await screen.findByText(/Revision: saved-2/);
  await waitFor(() => expect(retained(s)).not.toContain('"committed"'));
  expect(retained(s)).not.toContain(SECRET_SENTINEL);
  expect(document.body.innerHTML).not.toContain(SECRET_SENTINEL);
  // The committed revision is the base, so closing Settings never manufactures
  // an external conflict or a reviewed-revision gesture.
  fireEvent.click(screen.getByRole('button', { name: 'Providers & Models' }));
  fireEvent.click(screen.getByRole('button', { name: 'Edit Provider secret' }));
  expect((screen.getByLabelText('Credential source') as HTMLSelectElement).value).toBe('retain');
  expect(screen.queryByText(/Source revision changed/)).toBeNull();
  expect(screen.queryByRole('button', { name: 'Use reviewed revision' })).toBeNull();
  // Exactly one write, and reopening replays nothing.
  expect(writes(s)).toHaveLength(1);
});

it('S1-16 an acknowledgement from the retired authority settles its own transaction and never reaches the replacement', async () => {
  const s = cfg3Client(commitsProvider);
  const { ui, host, release } = await heldSecretSave(s);
  const submitting = settingsTransactionOwners(s.client);
  expect(submitting).toHaveLength(1);
  ui.unmount();
  // The App Server authority is replaced while the acknowledgement is in
  // flight, so the reopened Settings owns a different transaction identity.
  s.publish({ authorityRevision: (s.state.authorityRevision ?? 0) + 1 });
  render(<SettingsSurface client={s.client} target={workspaceSettingsTarget('A', 'Workspace A')} host={host} />);
  await screen.findByText(/Revision: /);
  const replacement = settingsTransactionOwners(s.client).filter(owner => !submitting.includes(owner));
  expect(replacement).toHaveLength(1);
  await release();
  // The old authority's acknowledgement settles the old authority's
  // transaction; the replacement's transaction state stays untouched.
  await waitFor(() => expect(JSON.stringify(submitting[0].retainedState())).toContain('"committed"'));
  expect(replacement[0].retainedState()).toEqual([]);
  expect(JSON.stringify(replacement[0].retainedState())).not.toContain(SECRET_SENTINEL);
  expect(JSON.stringify(submitting[0].retainedState())).not.toContain(SECRET_SENTINEL);
  expect(writes(s)).toHaveLength(1);
});

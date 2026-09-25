// @vitest-environment jsdom
import { afterEach, expect, it } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { TextField } from '../src/app/settings/forms/controls';
import { UnitForm } from '../src/app/settings/forms/bridge';
import { userSettingsTarget, workspaceSettingsTarget } from '../src/app/settings/projection';
import { OutcomeUncertain, RpcFailure } from '../src/client/app-server';
import { cfg3Application, cfg3Source } from './cfg3-data';
import {
  confirmAction, openResourceRow, openSettingsPage, renderEditor, sameRevision,
  settingsReady, SettingsSurface,
} from './settings-harness';
import { cfg3Client, cfg3Host } from './cfg3-fixture';
afterEach(cleanup);

/** The native source path and revision are diagnostics: they live on Advanced,
 * not on the ordinary product pages. A test that asserts on them navigates
 * there exactly as a user would, and comes back to the page it was editing —
 * which also exercises the draft surviving that navigation. */
async function revisionOnAdvanced(pattern: RegExp, back?: string) {
  await openSettingsPage('Advanced');
  await screen.findByText(pattern);
  if (back) await openSettingsPage(back);
}

it('C09 Workspace A/B drafts survive navigation and Session focus without retargeting', async () => {
 const s = cfg3Client(); const host = cfg3Host(s);
 const ui = render(<SettingsSurface client={s.client} target={workspaceSettingsTarget('A', 'A')} host={host}/>);
 await settingsReady(); await openSettingsPage('Tools & Permissions');
 fireEvent.click(screen.getByLabelText('read'));
 // A different Workspace Settings instance is a different target and must not
 // receive Workspace A's draft.
 ui.rerender(<SettingsSurface client={s.client} target={workspaceSettingsTarget('B', 'B')} host={host}/>);
 await openSettingsPage('Tools & Permissions');
 await waitFor(() => expect((screen.getByLabelText('read') as HTMLInputElement).checked).toBe(false));
 fireEvent.click(screen.getByLabelText('write'));
 // Returning to A restores A's own draft and excludes B's.
 ui.rerender(<SettingsSurface client={s.client} target={workspaceSettingsTarget('A', 'A')} host={host}/>);
 await openSettingsPage('Tools & Permissions');
 await waitFor(() => expect((screen.getByLabelText('read') as HTMLInputElement).checked).toBe(true));
 expect((screen.getByLabelText('write') as HTMLInputElement).checked).toBe(false);
 s.state.views = {}; ui.rerender(<SettingsSurface client={s.client} target={workspaceSettingsTarget('A', 'A')} host={host}/>);
 await openSettingsPage('Tools & Permissions');
 fireEvent.click(screen.getByRole('button', { name: 'Save Native Tools' }));
 await waitFor(() => expect(s.request.mock.calls.find(([op]) => op.method === 'configuration/sourceWrite')?.[0]).toMatchObject({ params: { target: { kind: 'workspace', directory: '/workspace/A' }, expected_revision: 'workspace-1', mutation: { mutation: { authored: ['read'] } } } }));
});

it('C09 external source revision notification preserves dirty draft and original CAS', async () => {
 const s = cfg3Client(); render(<SettingsSurface client={s.client} target={userSettingsTarget}/>);
 await openSettingsPage('Tools & Permissions');
 fireEvent.click(screen.getByLabelText('read')); s.source.user.revision = 'external';
 // The native publication reaches the Settings actor at the client publication
 // itself; no presentation render is involved.
 act(() => s.publish({ configuration: { 'source:user': { ...cfg3Application(), scope: 'source:user', version: '9' } } }));
 await revisionOnAdvanced(/Revision: external/, 'Tools & Permissions');
 expect((screen.getByLabelText('read') as HTMLInputElement).checked).toBe(true);
 fireEvent.click(screen.getByRole('button', { name: 'Save Native Tools' }));
 await waitFor(() => expect(s.request.mock.calls.find(([op]) => op.method === 'configuration/sourceWrite')?.[0]).toMatchObject({ params: { expected_revision: 'user-1' } }));
});

it('C10 Workspace revocation disables mutation and preserves local draft', async () => {
 const s = cfg3Client(); const host = cfg3Host(s); const configure = host.configureWorkspace!;
 let revoked = false; host.configureWorkspace = (...args) => revoked ? Promise.reject(new Error('Workspace revoked')) : configure(...args);
 render(<SettingsSurface client={s.client} target={workspaceSettingsTarget('A', 'A')} host={host}/>);
 await settingsReady(); await openSettingsPage('Tools & Permissions');
 fireEvent.click(screen.getByLabelText('read')); revoked = true;
 fireEvent.click(screen.getByRole('button', { name: 'Reload configuration' }));
 await screen.findByRole('alert');
 expect((screen.getByLabelText('read') as HTMLInputElement).checked).toBe(true);
 fireEvent.click(screen.getByRole('button', { name: 'Save Native Tools' }));
 expect(s.request.mock.calls.filter(([op]) => op.method === 'configuration/sourceWrite')).toHaveLength(0);
});

it.each(['target', 'connection'] as const)('C10 late response after %s replacement cannot overwrite new authority', async invalidation => {
 let release!: (result: import('../../protocol/app-server/v22').MethodResult) => void;
 const pending = new Promise<import('../../protocol/app-server/v22').MethodResult>(resolve => { release = resolve; });
 let reads = 0;
 const s = cfg3Client(async op => { if (op.method === 'configuration/sourcesRead' && ++reads === 1) return pending; });
 const host = cfg3Host(s); const ui = render(<SettingsSurface client={s.client} target={workspaceSettingsTarget('A', 'A')} host={host}/>);
 await waitFor(() => expect(reads).toBe(1)); const stale = structuredClone(s.source); stale.workspace!.revision = 'stale';
 s.source.workspace!.revision = 'fresh';
 if (invalidation === 'connection') act(() => s.publish({ authorityRevision: 2 }));
 ui.rerender(<SettingsSurface client={s.client} target={workspaceSettingsTarget(invalidation === 'target' ? 'B' : 'A', invalidation === 'target' ? 'B' : 'A')} host={host}/>);
 await openSettingsPage('Advanced');
 await screen.findByText(/Revision: fresh/);
 await act(async () => { release({ type: 'source_settings', projection: stale }); await pending; });
 expect(screen.queryByText(/Revision: stale/)).toBeNull(); expect(screen.getByText(/Revision: fresh/)).toBeTruthy();
});

async function open(subject: ReturnType<typeof cfg3Client>, page: string, scope: 'User' | 'Workspace' = 'Workspace') {
  render(<SettingsSurface client={subject.client} target={scope === 'User' ? userSettingsTarget : workspaceSettingsTarget('A', 'A')} host={subject.host ??= cfg3Host(subject)} />);
  await settingsReady();
  await openSettingsPage(page);
}

// S2-01: the six primary product pages, their landing page, and the removal of
// every obsolete configuration-unit route.
const obsoletePages = ['Overview', 'Providers & Models', 'Default model', 'Tool Policies', 'Tools', 'Skill access',
  'Plugins', 'Agents & Workflows', 'Agents', 'MCP', 'Managed Python', 'Skills', 'Workflows',
  'Appearance', 'Connection', 'Server & source diagnostics'];

it('S2-01 global Settings has exactly six product pages and opens at General', async () => {
  const subject = cfg3Client();
  render(<SettingsSurface client={subject.client} target={userSettingsTarget} />);
  expect(screen.getAllByRole('tab').map(tab => tab.textContent)).toEqual([
    'General', 'Models', 'Agent', 'Tools & Permissions', 'Extensions', 'Advanced',
  ]);
  expect(screen.getByRole('tab', { name: 'General', selected: true })).toBeTruthy();
  for (const gone of obsoletePages) expect(screen.queryByRole('tab', { name: gone })).toBeNull();
});

it('S2-01 Workspace Settings is a constrained page set that lands on Models', async () => {
  const subject = cfg3Client();
  render(<SettingsSurface client={subject.client} target={workspaceSettingsTarget('A', 'A')} host={cfg3Host(subject)} />);
  // General holds client-owned preferences that no native source authors, so a
  // Workspace has no General page at all rather than an empty one.
  expect(screen.getAllByRole('tab').map(tab => tab.textContent)).toEqual([
    'Models', 'Agent', 'Tools & Permissions', 'Extensions', 'Advanced',
  ]);
  expect(screen.getByRole('tab', { name: 'Models', selected: true })).toBeTruthy();
  expect(screen.queryByRole('tab', { name: 'General' })).toBeNull();
  await settingsReady();
  expect(screen.queryByLabelText('Theme')).toBeNull();
});

it('S2-01 every primary page is reachable and renders its own product surface', async () => {
  const subject = cfg3Client();
  render(<SettingsSurface client={subject.client} target={userSettingsTarget} />);
  await settingsReady();
  for (const [page, region] of [['Models', 'Models'], ['Agent', 'Agent'], ['Tools & Permissions', 'Tools & Permissions'],
    ['Extensions', 'Extensions'], ['Advanced', 'Advanced']] as const) {
    await openSettingsPage(page);
    expect(screen.getByRole('region', { name: region })).toBeTruthy();
  }
  await openSettingsPage('General');
  expect(screen.getByRole('region', { name: 'General' })).toBeTruthy();
});

it('User Settings works without any Session and has no adopted-state or Effective editor', async () => {
 const subject = cfg3Client(); subject.state.views = {}; render(<SettingsSurface client={subject.client} target={userSettingsTarget}/>);
 await openSettingsPage('Advanced');
 await screen.findByText(/Revision: user-1/);
 expect(subject.request.mock.calls.every(([op]) => op.method === 'configuration/sourcesRead')).toBe(true);
 expect(subject.request.mock.calls[0][0]).toEqual({ method: 'configuration/sourcesRead', params: { target: { kind: 'user' } } });
});

it('User Settings has no ordinary Configuration owner selector', async () => {
 const subject = cfg3Client(); render(<SettingsSurface client={subject.client} target={userSettingsTarget}/>);
 await openSettingsPage('Advanced');
 await screen.findByText(/Revision: user-1/);
 expect(screen.queryByLabelText('Configuration owner')).toBeNull();
 expect(screen.getByRole('heading', { name: 'User Settings' })).toBeTruthy();
});

it('Workspace Settings is bound to one exact target and never retargets on Session focus', async () => {
 const subject = cfg3Client(); subject.state.views = {}; const ui = render(<SettingsSurface client={subject.client} target={workspaceSettingsTarget('A', 'Workspace A')} host={cfg3Host(subject)}/>);
 await settingsReady();
 expect(screen.getByRole('heading', { name: 'Workspace Settings — Workspace A' })).toBeTruthy();
 expect(screen.queryByLabelText('Configuration owner')).toBeNull();
 // A Session focus change elsewhere cannot change this instance's target.
 subject.state.views = {}; ui.rerender(<SettingsSurface client={subject.client} target={workspaceSettingsTarget('A', 'Workspace A')} host={cfg3Host(subject)}/>);
 expect(subject.request.mock.calls.every(([op]) => op.method !== 'session/attach' && op.method !== 'session/create')).toBe(true);
});

it('saves a whole Workspace Provider with explicit credentials without copying User members', async () => {
  const subject = cfg3Client(); await open(subject, 'Models');
  fireEvent.change(screen.getByLabelText('New Provider identity'), { target: { value: 'transport' } });
  fireEvent.click(screen.getByRole('button', { name: 'Add Provider' }));
  expect((screen.getByLabelText('Endpoint') as HTMLInputElement).value).toBe('');
  fireEvent.change(screen.getByLabelText('Endpoint'), { target: { value: 'https://workspace.invalid' } });
  fireEvent.change(screen.getByLabelText('Environment variable'), { target: { value: 'WORKSPACE_KEY' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save Provider transport' }));
  await screen.findByText(/Provider transport saved/);
  expect(subject.request.mock.calls.find(([operation]) => operation.method === 'configuration/sourceWrite')?.[0]).toEqual({ method: 'configuration/sourceWrite', params: { target: { kind: 'workspace', directory: '/workspace/A' }, expected_revision: 'workspace-1', mutation: { kind: 'config', mutation: { unit: 'provider', id: 'transport', authored: { base_url: 'https://workspace.invalid', credential: { kind: 'environment', variable: 'WORKSPACE_KEY' } } } } } });
  expect(subject.request.mock.calls.filter(([operation]) => operation.method === 'session/adoptConfiguration')).toHaveLength(0);
  expect(screen.queryByRole('button', { name: /Adopt/ })).toBeNull();
});

it('preserves stale drafts and exact revision until a separate explicit review gesture', async () => {
  const subject = cfg3Client(async (operation, source) => {
    if (operation.method === 'configuration/sourceWrite') { source.workspace!.revision = 'external-edit'; throw new RpcFailure({ code: -32000, message: 'Conflict', data: { kind: 'source_conflict', scope: 'workspace', expected: operation.params.expected_revision, actual: 'external-edit' } }); }
  });
  await open(subject, 'Tools & Permissions');
  fireEvent.click(screen.getByLabelText('read')); fireEvent.click(screen.getByRole('button', { name: 'Save Native Tools' }));
  await screen.findByRole('alert'); await screen.findByRole('button', { name: 'Use reviewed revision' });
  expect((screen.getByLabelText('read') as HTMLInputElement).checked).toBe(true);
  expect(subject.request.mock.calls.filter(([operation]) => operation.method === 'configuration/sourceWrite')).toHaveLength(1);
  fireEvent.click(screen.getByRole('button', { name: 'Save Native Tools' }));
  await waitFor(() => expect(subject.request.mock.calls.filter(([operation]) => operation.method === 'configuration/sourceWrite')).toHaveLength(2));
  const writes = subject.request.mock.calls.filter(([operation]) => operation.method === 'configuration/sourceWrite');
  expect(writes[1][0]).toMatchObject({ params: { expected_revision: 'workspace-1' } });
});

it('repairs uncertain writes by rereading and never replays the mutation', async () => {
  const subject = cfg3Client(async operation => { if (operation.method === 'configuration/sourceWrite') throw new OutcomeUncertain(); });
  await open(subject, 'Tools & Permissions'); fireEvent.click(screen.getByLabelText('read')); fireEvent.click(screen.getByRole('button', { name: 'Save Native Tools' }));
  // The target reports the unknown outcome and the unit reports its own; both
  // say the write is never replayed, and neither claims it was saved.
  await waitFor(() => expect(screen.getAllByRole('alert').map(node => node.textContent).join(' ')).toMatch(/never replayed/));
  await waitFor(() => expect(subject.request.mock.calls.filter(([operation]) => operation.method === 'configuration/sourcesRead')).toHaveLength(2));
  expect(subject.request.mock.calls.filter(([operation]) => operation.method === 'configuration/sourceWrite')).toHaveLength(1);
  expect((screen.getByLabelText('read') as HTMLInputElement).checked).toBe(true);
});

it('edits an independent named-Agent whole resource with inherited model and extensions off', async () => {
  const subject = cfg3Client(); await open(subject, 'Extensions');
  fireEvent.click(screen.getByRole('tab', { name: 'Agents' }));
  fireEvent.change(await screen.findByLabelText('New Agent identity'), { target: { value: 'researcher' } });
  fireEvent.click(screen.getByRole('button', { name: 'Add Agent' }));
  expect(screen.getByText(/invoking Attempt's already-frozen effective model/)).toBeTruthy();
  expect((screen.getByLabelText('todo') as HTMLInputElement).checked).toBe(false);
  fireEvent.change(screen.getByLabelText('Description'), { target: { value: 'Research a topic' } });
  fireEvent.change(screen.getByLabelText('Instructions'), { target: { value: 'Read and report findings.' } });
  fireEvent.click(screen.getByLabelText('read')); fireEvent.click(screen.getByLabelText('todo'));
  fireEvent.click(screen.getByRole('button', { name: 'Save Agent researcher' }));
  await waitFor(() => expect(subject.request.mock.calls.find(([operation]) => operation.method === 'configuration/sourceWrite')?.[0]).toMatchObject({ params: { expected_revision: 'missing', mutation: { kind: 'agent', name: 'researcher', authored: { description: 'Research a topic', tools: { builtin: ['read'] }, plugins: { todo: { enabled: true } } } } } }));
});

it('keeps shadowed User resources visible using native shadowing facts', async () => {
  const subject = cfg3Client();
  subject.source.prospective_resources = { ...subject.effective.resources, definitions: [{ family: 'skill', name: 'review', valid: false, location: { scope: 'workspace', path: '/workspace/.agents/skills/review/SKILL.md', shadowed: '/home/user/rustx/.agents/skills/review/SKILL.md' } }] };
  render(<SettingsSurface client={subject.client} target={userSettingsTarget} />);
  await settingsReady();
  await openSettingsPage('Extensions');
  fireEvent.click(screen.getByRole('tab', { name: 'Skills' }));
  // The invalid winning definition is shown as the winner it is; the shadowed
  // User definition is named, never used to stand in for it.
  expect(await screen.findByText(/Shadowed by the Workspace definition/)).toBeTruthy();
  expect(screen.getByText(/Shadows \/home\/user\/rustx\/.agents\/skills\/review\/SKILL.md/)).toBeTruthy();
  expect(screen.getByText('Invalid definition')).toBeTruthy();
  expect(screen.queryByText('Visible to the root Agent')).toBeNull();
});

it('retains a Workspace draft and its original CAS revision across page navigation and a separate User Settings instance', async () => {
  const subject = cfg3Client(); const host = cfg3Host(subject);
  const workspace = render(<SettingsSurface client={subject.client} target={workspaceSettingsTarget('A', 'A')} host={host} />);
  await settingsReady(); await openSettingsPage('Tools & Permissions');
  fireEvent.click(screen.getByLabelText('read'));
  // A separate User Settings instance cannot receive the Workspace draft.
  workspace.rerender(<SettingsSurface client={subject.client} target={userSettingsTarget} host={host} />);
  await openSettingsPage('Tools & Permissions');
  expect((screen.getByLabelText('read') as HTMLInputElement).checked).toBe(false);
  // Page navigation inside Workspace Settings preserves the dirty draft.
  workspace.rerender(<SettingsSurface client={subject.client} target={workspaceSettingsTarget('A', 'A')} host={host} />);
  await settingsReady();
  await openSettingsPage('Agent');
  subject.source.workspace!.revision = 'external';
  fireEvent.click(screen.getByRole('button', { name: 'Reload configuration' }));
  await openSettingsPage('Advanced');
  await screen.findByText(/Revision: external/);
  await openSettingsPage('Tools & Permissions');
  expect((screen.getByLabelText('read') as HTMLInputElement).checked).toBe(true);
  fireEvent.click(screen.getByRole('button', { name: 'Save Native Tools' }));
  await waitFor(() => expect(subject.request.mock.calls.find(([op]) => op.method === 'configuration/sourceWrite')?.[0]).toMatchObject({ params: { expected_revision: 'workspace-1' } }));
});

it('replaces against the newer revision only after an explicit review gesture', async () => {
  let first = true;
  const subject = cfg3Client(async (op, source) => {
    if (op.method === 'configuration/sourceWrite' && first) { first = false; source.workspace!.revision = 'reviewed'; throw new RpcFailure({ code: -32000, message: 'Conflict', data: { kind: 'source_conflict', scope: 'workspace', expected: 'workspace-1', actual: 'reviewed' } }); }
  });
  await open(subject, 'Tools & Permissions'); fireEvent.click(screen.getByLabelText('read'));
  fireEvent.click(screen.getByRole('button', { name: 'Save Native Tools' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Use reviewed revision' }));
  fireEvent.click(screen.getByRole('button', { name: 'Save Native Tools' }));
  await screen.findByText(/Native Tools saved/);
  expect(subject.request.mock.calls.filter(([op]) => op.method === 'configuration/sourceWrite')[1][0]).toMatchObject({ params: { expected_revision: 'reviewed', mutation: { mutation: { authored: ['read'] } } } });
});

it.each(['empty', 'omit'] as const)('preserves native %s Tool selection as a distinct semantic-unit operation', async mode => {
  const subject = cfg3Client();
  // Removal replaces an existing Workspace override; an explicit empty
  // selection authors one where none exists. They are two different authored
  // intents, so each starts from the authored state it actually applies to.
  if (mode === 'omit') subject.source.workspace!.authored = { agent: { tools: { builtin: ['bash'] } } };
  await open(subject, 'Tools & Permissions');
  // The empty one has to be authored first: rendering an inherited unit never
  // produces it.
  if (mode === 'empty') {
    fireEvent.click(screen.getByRole('button', { name: 'Override Native Tools' }));
    fireEvent.click(screen.getByRole('button', { name: 'Save Native Tools' }));
  } else {
    await confirmAction('Use global default Native Tools');
  }
  await waitFor(() => expect(subject.request.mock.calls.find(([op]) => op.method === 'configuration/sourceWrite')?.[0]).toMatchObject({ params: { mutation: { mutation: { unit: 'native_tools', authored: mode === 'empty' ? [] : null } } } }));
});

it('process state comes from native classification, outside Session adoption', async () => {
 const subject = cfg3Client(); subject.source.application = cfg3Application(); subject.source.application.units.process_bindings = { status: 'process_restart' };
 await open(subject, 'Advanced', 'User');
 expect(screen.getByText(/Restart required/)).toBeTruthy();
 expect(screen.queryByRole('button', { name: /Adopt/ })).toBeNull();
});

it('does not equate invalid resource existence with readiness or Root authority', async () => {
  const subject = cfg3Client();
  subject.source.prospective_resources = structuredClone(subject.effective.resources);
  subject.source.prospective_resources.definitions = [{ name: 'unselected', family: 'managed_python', valid: false, location: { scope: 'workspace', path: '/workspace/.agents/python/unselected' } }];
  subject.source.prospective_resources.sources = { 'python:unselected': { status: 'unavailable' } };
  await open(subject, 'Extensions');
  fireEvent.click(screen.getByRole('tab', { name: 'Python' }));
  // Four independent native facts, four independent renderings.
  expect(await screen.findByText('Invalid definition')).toBeTruthy();
  expect(screen.getByText('Preparation unavailable')).toBeTruthy();
  // Native published no root inspection here, so root availability is reported
  // as unobserved. Unobserved is never rendered as "allowed" or as "not
  // allowed", and preparation is never inferred from validity.
  expect(screen.getByText('Root selection not observed')).toBeTruthy();
  expect(screen.queryByText('Allowed for the root Agent')).toBeNull();
  expect(screen.queryByText('Prepared')).toBeNull();
});

it('keeps contributor default intent unspecified when enabling the closed Agent Status extension', async () => {
  const subject = cfg3Client(); await open(subject, 'Extensions');
  fireEvent.click(screen.getByRole('tab', { name: 'Native' }));
  fireEvent.click(await screen.findByRole('switch', { name: 'Enable Agent Status' }));
  fireEvent.click(screen.getByRole('button', { name: 'Save Agent Status extension' }));
  await waitFor(() => expect(subject.request.mock.calls.find(([op]) => op.method === 'configuration/sourceWrite')?.[0]).toMatchObject({ params: { mutation: { mutation: { unit: 'agent_status', authored: { enabled: true } } } } }));
  const write = subject.request.mock.calls.find(([op]) => op.method === 'configuration/sourceWrite')![0];
  expect(JSON.stringify(write)).not.toMatch(/"time"|"background"|npm|cordis/);
});

it('reconnect rereads native sources without replaying a dirty draft', async () => {
  const subject = cfg3Client(); render(<SettingsSurface client={subject.client} target={workspaceSettingsTarget('A', 'A')} host={subject.host ??= cfg3Host(subject)} />);
  await settingsReady(); await openSettingsPage('Tools & Permissions');
  fireEvent.click(screen.getByLabelText('read'));
  act(() => subject.publish({ generation: 2 }));
  await waitFor(() => expect(subject.request.mock.calls.filter(([op]) => op.method === 'configuration/sourcesRead')).toHaveLength(2));
  expect(subject.request.mock.calls.filter(([op]) => op.method === 'configuration/sourceWrite')).toHaveLength(0);
  expect((screen.getByLabelText('read') as HTMLInputElement).checked).toBe(true);
});

it('an older authoritative read cannot replace a newer read', async () => {
  let release: (value: import('../../protocol/app-server/v22').MethodResult) => void = () => {};
  let count = 0;
  const subject = cfg3Client(async op => { if (op.method === 'configuration/sourcesRead' && ++count === 2) return new Promise(resolve => { release = resolve; }); });
  render(<SettingsSurface client={subject.client} target={workspaceSettingsTarget('A', 'A')} host={subject.host ??= cfg3Host(subject)} />);
  await openSettingsPage('Advanced');
  await screen.findByText(/Revision: workspace-1/);
  const stale = structuredClone(subject.source);
  fireEvent.click(screen.getByRole('button', { name: 'Reload configuration' }));
  await waitFor(() => expect(count).toBe(2));
  subject.source.workspace!.revision = 'newest';
  fireEvent.click(screen.getByRole('button', { name: 'Reload configuration' }));
  await screen.findByText(/Revision: newest/);
  release({ type: 'source_settings', projection: stale });
  await waitFor(() => expect(screen.getByText(/Revision: newest/)).toBeTruthy());
});

it('removes literal credentials from a successful Provider draft using the redacted native acknowledgement', async () => {
  const subject = cfg3Client(async (op, source) => {
    if (op.method === 'configuration/sourceWrite' && op.params.mutation.kind === 'config' && op.params.mutation.mutation.unit === 'provider') {
      source.workspace!.authored = { providers: { secret: { base_url: 'https://native.invalid', credential: { type: 'literal' } } } };
    }
  });
  await open(subject, 'Models');
  fireEvent.change(screen.getByLabelText('New Provider identity'), { target: { value: 'secret' } }); fireEvent.click(screen.getByRole('button', { name: 'Add Provider' }));
  fireEvent.change(screen.getByLabelText('Endpoint'), { target: { value: 'https://native.invalid' } });
  fireEvent.click(screen.getByRole('button', { name: /Credential source/ }));
  fireEvent.click(await screen.findByRole('option', { name: 'Enter a literal secret' }));
  fireEvent.change(await screen.findByLabelText('New literal credential'), { target: { value: 'SECRET_SENTINEL' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save Provider secret' }));
  await screen.findByText(/Provider secret saved/);
  await waitFor(() => expect(screen.queryByLabelText('New literal credential')).toBeNull());
  expect(document.body.innerHTML).not.toContain('SECRET_SENTINEL');
  fireEvent.click(screen.getByRole('button', { name: '← Models' }));
  await openResourceRow('secret');
  expect((await screen.findByRole('button', { name: /Credential source/ })).textContent).toContain('Keep the credential this scope already authored');
});

it('retains a Model draft when native validation rejects its semantic unit', async () => {
 const subject = cfg3Client(async op => { if (op.method === 'configuration/sourceWrite') throw new RpcFailure({ code: -32000, message: 'Native validation failed', data: { kind: 'configuration_adoption', rejection: { status: 'failed', diagnostic: 'Invalid provider reference' } } }); });
 await open(subject, 'Models');
 fireEvent.click(screen.getByRole('button', { name: /^All Models/ }));
 fireEvent.change(await screen.findByLabelText('New Model identity'), { target: { value: 'draft-model' } });
 fireEvent.click(screen.getByRole('button', { name: 'Add Model' }));
 fireEvent.change(await screen.findByLabelText('Wire model identity'), { target: { value: 'wire' } });
 fireEvent.change(screen.getByLabelText('Provider identity'), { target: { value: 'missing' } });
 fireEvent.click(screen.getByRole('button', { name: 'Save Model draft-model' }));
 expect((await screen.findByRole('alert')).textContent).toContain('Invalid provider reference');
 expect((screen.getByLabelText('Provider identity') as HTMLInputElement).value).toBe('missing');
 expect(subject.request.mock.calls.filter(([op]) => op.method === 'session/adoptConfiguration')).toHaveLength(0);
});

it('preserves the original revision when removing an otherwise clean unit conflicts', async () => {
  const subject = cfg3Client(async (op, source) => {
    if (op.method === 'configuration/sourceWrite') {
      source.workspace!.revision = 'external-removal-conflict';
      throw new RpcFailure({ code: -32000, message: 'Conflict', data: { kind: 'source_conflict', scope: 'workspace', expected: op.params.expected_revision, actual: source.workspace!.revision } });
    }
  });
  subject.source.workspace!.authored = { agent: { tools: { builtin: ['bash'] } } };
  await open(subject, 'Tools & Permissions');
  await confirmAction('Use global default Native Tools');
  await screen.findByRole('button', { name: 'Use reviewed revision' });
  await confirmAction('Use global default Native Tools');
  await waitFor(() => expect(subject.request.mock.calls.filter(([op]) => op.method === 'configuration/sourceWrite')).toHaveLength(2));
  expect(subject.request.mock.calls.filter(([op]) => op.method === 'configuration/sourceWrite')[1][0]).toMatchObject({ params: { expected_revision: 'workspace-1', mutation: { mutation: { authored: null } } } });
});

it('preserves a clean removal\'s frozen CAS base across an editor remount and advances it only through explicit review', async () => {
  const subject = cfg3Client(async (op, source) => {
    if (op.method === 'configuration/sourceWrite') {
      source.workspace!.revision = 'external-removal-conflict';
      throw new RpcFailure({ code: -32000, message: 'Conflict', data: { kind: 'source_conflict', scope: 'workspace', expected: op.params.expected_revision, actual: source.workspace!.revision } });
    }
  });
  subject.source.workspace!.authored = { agent: { tools: { builtin: ['bash'] } } };
  await open(subject, 'Tools & Permissions');
  await confirmAction('Use global default Native Tools');
  await screen.findByRole('button', { name: 'Use reviewed revision' });
  // Leaving and re-entering the page remounts the editor subtree. The frozen
  // base is durable transaction state, not component-local state.
  await openSettingsPage('Agent');
  await openSettingsPage('Tools & Permissions');
  await screen.findByRole('button', { name: 'Use global default Native Tools' });
  expect(screen.getByRole('button', { name: 'Use reviewed revision' })).toBeTruthy();
  await confirmAction('Use global default Native Tools');
  await waitFor(() => expect(subject.request.mock.calls.filter(([op]) => op.method === 'configuration/sourceWrite')).toHaveLength(2));
  expect(subject.request.mock.calls.filter(([op]) => op.method === 'configuration/sourceWrite')[1][0]).toMatchObject({ params: { expected_revision: 'workspace-1', mutation: { mutation: { authored: null } } } });
  // Only the explicit reviewed-revision gesture advances the operation to R2.
  fireEvent.click(screen.getByRole('button', { name: 'Use reviewed revision' }));
  await confirmAction('Use global default Native Tools');
  await waitFor(() => expect(subject.request.mock.calls.filter(([op]) => op.method === 'configuration/sourceWrite')).toHaveLength(3));
  expect(subject.request.mock.calls.filter(([op]) => op.method === 'configuration/sourceWrite')[2][0]).toMatchObject({ params: { expected_revision: 'external-removal-conflict', mutation: { mutation: { authored: null } } } });
});

it.each([
  ['effect', 'refresh'], ['refresh', 'refresh'], ['refresh', 'write'],
] as const)('fences an obsolete %s read rejection after a newer %s', async (readKind, successor) => {
  let rejectRead!: (error: Error) => void;
  const pending = new Promise<import('../../protocol/app-server/v22').MethodResult>((_, reject) => { rejectRead = reject; });
  let reads = 0;
  const subject = cfg3Client(async op => {
    if (op.method === 'configuration/sourcesRead' && ++reads === 2) return pending;
  });
  render(<SettingsSurface client={subject.client} target={workspaceSettingsTarget('A', 'A')} host={subject.host ??= cfg3Host(subject)} />);
  await settingsReady();
  await openSettingsPage('Tools & Permissions');
  if (readKind === 'effect') act(() => subject.publish({ generation: 2 }));
  else fireEvent.click(screen.getByRole('button', { name: 'Reload configuration' }));
  await waitFor(() => expect(reads).toBe(2));
  if (successor === 'refresh') {
    subject.source.workspace!.revision = 'new-authority';
    fireEvent.click(screen.getByRole('button', { name: 'Reload configuration' }));
    await openSettingsPage('Advanced');
  } else {
    fireEvent.click(screen.getByLabelText('read'));
    fireEvent.click(screen.getByRole('button', { name: 'Save Native Tools' }));
    await screen.findByText(/Native Tools saved/);
    await openSettingsPage('Advanced');
  }
  const revision = successor === 'refresh' ? 'new-authority' : 'saved-2';
  await screen.findByText(new RegExp(`Revision: ${revision}`));
  await act(async () => { rejectRead(new Error('obsolete read failed')); await pending.catch(() => {}); });
  expect(screen.getByText(new RegExp(`Revision: ${revision}`))).toBeTruthy();
  expect(screen.queryByRole('alert')).toBeNull();
  expect(document.body.textContent).not.toContain('obsolete read failed');
});

it.each(['before acknowledgement', 'after acknowledgement', 'after the next edit'] as const)('T12/T16 source projection %s preserves subsequent drafts', async order => {
  // The acknowledgement is held explicitly; nothing here depends on timing.
  let acknowledge!: (outcome: { acknowledgement: import('../../protocol/app-server/v22').SourceSettings }) => void;
  const held = new Promise<{ acknowledgement: import('../../protocol/app-server/v22').SourceSettings }>(resolve => { acknowledge = resolve; });
  const source = cfg3Source();
  const form = (authored: { command: string }, revision: string) => <UnitForm<{ command: string }>
    title="MCP acknowledgement" authored={authored} blank={{ command: '' }} revision={revision}
    mutation={value => ({ kind: 'mcp', id: 'fixture', authored: value ? { definition: { type: 'stdio', command: value.command } } : null })}>
    {(value, change) => <TextField label="Acknowledged command" value={value.command} change={command => change({ command })} />}
  </UnitForm>;
  const { rerender } = await renderEditor(form({ command: 'original' }, 'r1'), { source, write: () => held });
  fireEvent.change(screen.getByLabelText('Acknowledged command'), { target: { value: 'submitted-literal' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save MCP acknowledgement' }));
  // Hold source publication and the acknowledgement independently.
  if (order === 'before acknowledgement') rerender(form({ command: 'native-redacted' }, 'r2'));
  await act(async () => { acknowledge({ acknowledgement: sameRevision(source, 'r2') }); await held; });
  if (order === 'after acknowledgement') rerender(form({ command: 'native-redacted' }, 'r2'));
  if (order !== 'after the next edit') expect((screen.getByLabelText('Acknowledged command') as HTMLInputElement).value).toBe('native-redacted');
  fireEvent.change(screen.getByLabelText('Acknowledged command'), { target: { value: 'preserved-draft' } });
  // A later native application notification rereads the same source, then a
  // conflict reread advances its revision. Neither owns this subsequent edit.
  rerender(form({ command: 'native-redacted' }, 'r2'));
  rerender(form({ command: 'external' }, 'r3'));
  expect((screen.getByLabelText('Acknowledged command') as HTMLInputElement).value).toBe('preserved-draft');
  expect(screen.getByText(/Draft base revision:/).textContent).toContain('r2');
  expect(screen.getByRole('button', { name: 'Use reviewed revision' })).toBeTruthy();
});

it('T17 a post-commit read that observes the exact pre-save revision asks for review', async () => {
  // Native commits r2, then an external writer restores the pre-save bytes:
  // every authoritative read, including the post-commit one, answers r1.
  const source = sameRevision(cfg3Source(), 'r1');
  const committed = sameRevision(source, 'r2');
  await renderEditor(<UnitForm<{ command: string }>
    title="MCP rollback" authored={{ command: 'original' }} blank={{ command: '' }} revision="r1"
    mutation={value => ({ kind: 'mcp', id: 'fixture', authored: value ? { definition: { type: 'stdio', command: value.command } } : null })}>
    {(value, change) => <TextField label="Rollback command" value={value.command} change={command => change({ command })} />}
  </UnitForm>, { source, write: async () => ({ acknowledgement: committed }) });
  fireEvent.change(screen.getByLabelText('Rollback command'), { target: { value: 'saved' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save MCP rollback' }));
  // The commit is definitive; the source no longer carries it.
  await screen.findByRole('button', { name: 'Use reviewed revision' });
  expect(screen.getByText(/Source revision changed/)).toBeTruthy();
  expect(screen.getByText(/Draft base revision:/).textContent).toContain('r2');
  expect(screen.getByText(/Draft base revision:/).textContent).toContain('Current revision: r1');
});

it('retires a confirmed Provider save, including its literal credential, after the editor unmounts', async () => {
  let release!: (result: import('../../protocol/app-server/v22').MethodResult) => void;
  const heldWrite = new Promise<import('../../protocol/app-server/v22').MethodResult>(resolve => { release = resolve; });
  const subject = cfg3Client(async operation => { if (operation.method === 'configuration/sourceWrite') return heldWrite; });
  const host = cfg3Host(subject); subject.host = host;
  render(<SettingsSurface client={subject.client} target={workspaceSettingsTarget('A', 'A')} host={host} />);
  await settingsReady(); await openSettingsPage('Models');
  fireEvent.change(screen.getByLabelText('New Provider identity'), { target: { value: 'secret' } });
  fireEvent.click(screen.getByRole('button', { name: 'Add Provider' }));
  fireEvent.change(screen.getByLabelText('Endpoint'), { target: { value: 'https://native.invalid' } });
  fireEvent.click(screen.getByRole('button', { name: /Credential source/ }));
  fireEvent.click(await screen.findByRole('option', { name: 'Enter a literal secret' }));
  fireEvent.change(await screen.findByLabelText('New literal credential'), { target: { value: 'SECRET_SENTINEL' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save Provider secret' }));
  await waitFor(() => expect(subject.request.mock.calls.filter(([op]) => op.method === 'configuration/sourceWrite')).toHaveLength(1));
  // The editor unmounts while the native write is still in flight.
  await openSettingsPage('Agent');
  expect(screen.queryByLabelText('New literal credential')).toBeNull();
  // Native commit and its authoritative reread both succeed after unmount.
  subject.source.workspace!.revision = 'saved-2';
  subject.source.workspace!.authored = { providers: { secret: { base_url: 'https://native.invalid', credential: { type: 'literal' } } } };
  release({ type: 'source_settings', projection: structuredClone(subject.source) });
  await openSettingsPage('Advanced');
  await screen.findByText(/Revision: saved-2/);
  // Returning to Models restores the exact detail this page was left on —
  // focus is per-page navigation state with one owner — and the editor
  // reconstructs from the redacted native projection, not from the draft.
  await openSettingsPage('Models');
  expect(await screen.findByRole('heading', { name: 'Provider secret' })).toBeTruthy();
  expect((await screen.findByRole('button', { name: /Credential source/ })).textContent).toContain('Keep the credential this scope already authored');
  expect(document.body.innerHTML).not.toContain('SECRET_SENTINEL');
  expect(screen.queryByRole('button', { name: 'Use reviewed revision' })).toBeNull();
  expect(subject.request.mock.calls.filter(([op]) => op.method === 'configuration/sourceWrite')).toHaveLength(1);
});

it('a late acknowledgement advances the CAS base without erasing a newer draft submitted after it', async () => {
  let release!: (result: import('../../protocol/app-server/v22').MethodResult) => void;
  const heldWrite = new Promise<import('../../protocol/app-server/v22').MethodResult>(resolve => { release = resolve; });
  const subject = cfg3Client(async operation => { if (operation.method === 'configuration/sourceWrite') return heldWrite; });
  const host = cfg3Host(subject); subject.host = host;
  render(<SettingsSurface client={subject.client} target={workspaceSettingsTarget('A', 'A')} host={host} />);
  await settingsReady(); await openSettingsPage('Tools & Permissions');
  fireEvent.click(screen.getByLabelText('read'));
  fireEvent.click(screen.getByRole('button', { name: 'Save Native Tools' }));
  await waitFor(() => expect(subject.request.mock.calls.filter(([op]) => op.method === 'configuration/sourceWrite')).toHaveLength(1));
  // A newer editing intent exists before the first mutation is acknowledged.
  fireEvent.click(screen.getByLabelText('write'));
  subject.source.workspace!.revision = 'saved-2';
  release({ type: 'source_settings', projection: structuredClone(subject.source) });
  const nativeTools = () => within(screen.getByRole('form', { name: 'Native Tools' }));
  await waitFor(() => expect(nativeTools().getByText(/Draft base revision:/).textContent).toContain('saved-2'));
  // The newer draft survives the old acknowledgement; only the submitted
  // mutation is retired, and the newer intent's base advanced to the commit.
  expect((screen.getByLabelText('read') as HTMLInputElement).checked).toBe(true);
  expect((screen.getByLabelText('write') as HTMLInputElement).checked).toBe(true);
  expect(screen.queryByRole('button', { name: 'Use reviewed revision' })).toBeNull();
});

it('keeps a confirmed Workspace save when the post-write authoritative reread fails', async () => {
  let failReads = false;
  const subject = cfg3Client(async operation => { if (operation.method === 'configuration/sourcesRead' && failReads) throw new Error('reread unavailable'); });
  const host = cfg3Host(subject); subject.host = host;
  render(<SettingsSurface client={subject.client} target={workspaceSettingsTarget('A', 'A')} host={host} />);
  await settingsReady(); await openSettingsPage('Tools & Permissions');
  fireEvent.click(screen.getByLabelText('read'));
  failReads = true;
  fireEvent.click(screen.getByRole('button', { name: 'Save Native Tools' }));
  // The commit is reported as saved, and the failed reread is a separate fact.
  await screen.findByText(/Native Tools saved/);
  await screen.findByText(/Saved, but the authoritative reread failed/);
  expect(subject.request.mock.calls.filter(([op]) => op.method === 'configuration/sourceWrite')).toHaveLength(1);
  // A failed reread never turns the confirmed write back into an unsaved draft:
  // the submitted intent was retired with its confirmed commit, so there is no
  // pending Save and no review conflict against a stale base.
  expect((screen.getByRole('button', { name: 'Save Native Tools' }) as HTMLButtonElement).disabled).toBe(true);
  expect(screen.queryByRole('button', { name: 'Use reviewed revision' })).toBeNull();
});

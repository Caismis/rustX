// @vitest-environment jsdom
import { afterEach, expect, it } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { TextField, UnitForm } from '../src/app/settings/controls';
import { userSettingsTarget, workspaceSettingsTarget } from '../src/app/settings/projection';
import { OutcomeUncertain, RpcFailure } from '../src/client/app-server';
import { cfg3Application, cfg3Source } from './cfg3-data';
import { renderEditor, sameRevision, SettingsSurface } from './settings-harness';
import { cfg3Client, cfg3Host } from './cfg3-fixture';
afterEach(cleanup);

it('C09 Workspace A/B drafts survive navigation and Session focus without retargeting', async () => {
 const s = cfg3Client(); const host = cfg3Host(s);
 const ui = render(<SettingsSurface client={s.client} target={workspaceSettingsTarget('A', 'A')} host={host}/>);
 await screen.findByText(/Revision: workspace-1/); fireEvent.click(screen.getByRole('button', { name: 'Tools' }));
 fireEvent.click(screen.getByLabelText('read'));
 // A different Workspace Settings instance is a different target and must not
 // receive Workspace A's draft.
 ui.rerender(<SettingsSurface client={s.client} target={workspaceSettingsTarget('B', 'B')} host={host}/>);
 await waitFor(() => expect((screen.getByLabelText('read') as HTMLInputElement).checked).toBe(false));
 fireEvent.click(screen.getByLabelText('write'));
 // Returning to A restores A's own draft and excludes B's.
 ui.rerender(<SettingsSurface client={s.client} target={workspaceSettingsTarget('A', 'A')} host={host}/>);
 await waitFor(() => expect((screen.getByLabelText('read') as HTMLInputElement).checked).toBe(true));
 expect((screen.getByLabelText('write') as HTMLInputElement).checked).toBe(false);
 s.state.views = {}; ui.rerender(<SettingsSurface client={s.client} target={workspaceSettingsTarget('A', 'A')} host={host}/>);
 fireEvent.click(screen.getByRole('button', { name: 'Save Native Tools' }));
 await waitFor(() => expect(s.request.mock.calls.find(([op]) => op.method === 'configuration/sourceWrite')?.[0]).toMatchObject({ params: { target: { kind: 'workspace', directory: '/workspace/A' }, expected_revision: 'workspace-1', mutation: { mutation: { authored: ['read'] } } } }));
});

it('C09 external source revision notification preserves dirty draft and original CAS', async () => {
 const s = cfg3Client(); const ui = render(<SettingsSurface client={s.client} target={userSettingsTarget}/>);
 await screen.findByText(/Revision: user-1/); fireEvent.click(screen.getByRole('button', { name: 'Tools' }));
 fireEvent.click(screen.getByLabelText('read')); s.source.user.revision = 'external';
 s.client.getSnapshot = () => state;
 const state = { ...s.state, configuration: { 'source:user': { ...cfg3Application(), scope: 'source:user', version: '9' } } };
 ui.rerender(<SettingsSurface client={s.client} target={userSettingsTarget}/>);
 await screen.findByText(/Revision: external/);
 expect((screen.getByLabelText('read') as HTMLInputElement).checked).toBe(true);
 fireEvent.click(screen.getByRole('button', { name: 'Save Native Tools' }));
 await waitFor(() => expect(s.request.mock.calls.find(([op]) => op.method === 'configuration/sourceWrite')?.[0]).toMatchObject({ params: { expected_revision: 'user-1' } }));
});

it('C10 Workspace revocation disables mutation and preserves local draft', async () => {
 const s = cfg3Client(); const host = cfg3Host(s); const configure = host.configureWorkspace!;
 let revoked = false; host.configureWorkspace = (...args) => revoked ? Promise.reject(new Error('Workspace revoked')) : configure(...args);
 render(<SettingsSurface client={s.client} target={workspaceSettingsTarget('A', 'A')} host={host}/>);
 await screen.findByText(/Revision: workspace-1/); fireEvent.click(screen.getByRole('button', { name: 'Tools' }));
 fireEvent.click(screen.getByLabelText('read')); revoked = true;
 fireEvent.click(screen.getByRole('button', { name: 'Read current sources' })); await screen.findByRole('alert');
 expect((screen.getByLabelText('read') as HTMLInputElement).checked).toBe(true);
 fireEvent.click(screen.getByRole('button', { name: 'Save Native Tools' }));
 expect(s.request.mock.calls.filter(([op]) => op.method === 'configuration/sourceWrite')).toHaveLength(0);
});

it.each(['target', 'connection'] as const)('C10 late response after %s replacement cannot overwrite new authority', async invalidation => {
 let release!: (result: import('../../protocol/app-server/v18').MethodResult) => void;
 const pending = new Promise<import('../../protocol/app-server/v18').MethodResult>(resolve => { release = resolve; });
 let reads = 0;
 const s = cfg3Client(async op => { if (op.method === 'configuration/sourcesRead' && ++reads === 1) return pending; });
 const host = cfg3Host(s); const ui = render(<SettingsSurface client={s.client} target={workspaceSettingsTarget('A', 'A')} host={host}/>);
 await waitFor(() => expect(reads).toBe(1)); const stale = structuredClone(s.source); stale.workspace!.revision = 'stale';
 s.source.workspace!.revision = 'fresh';
 if (invalidation === 'connection') act(() => s.publish({ authorityRevision: 2 }));
 ui.rerender(<SettingsSurface client={s.client} target={workspaceSettingsTarget(invalidation === 'target' ? 'B' : 'A', invalidation === 'target' ? 'B' : 'A')} host={host}/>);
 await screen.findByText(/Revision: fresh/);
 await act(async () => { release({ type: 'source_settings', projection: stale }); await pending; });
 expect(screen.queryByText(/Revision: stale/)).toBeNull(); expect(screen.getByText(/Revision: fresh/)).toBeTruthy();
});
async function open(subject: ReturnType<typeof cfg3Client>, section: string, scope: 'User' | 'Workspace' = 'Workspace') {
  render(<SettingsSurface client={subject.client} target={scope === 'User' ? userSettingsTarget : workspaceSettingsTarget('A', 'A')} host={subject.host ??= cfg3Host(subject)} />);
  await screen.findByText(new RegExp(`Revision: ${scope === 'User' ? 'user-1' : 'workspace-1'}`));
  fireEvent.click(screen.getByRole('button', { name: section }));
}
it('User Settings works without any Session and has no adopted-state or Effective editor', async () => {
 const subject = cfg3Client(); subject.state.views = {}; render(<SettingsSurface client={subject.client} target={userSettingsTarget}/>);
 await screen.findByText(/Revision: user-1/);
 expect(screen.queryByRole('tab')).toBeNull();
 expect(subject.request.mock.calls.every(([op]) => op.method === 'configuration/sourcesRead')).toBe(true);
 expect(subject.request.mock.calls[0][0]).toEqual({ method: 'configuration/sourcesRead', params: { target: { kind: 'user' } } });
});
it('User Settings has no ordinary Configuration owner selector', async () => {
 const subject = cfg3Client(); render(<SettingsSurface client={subject.client} target={userSettingsTarget}/>);
 await screen.findByText(/Revision: user-1/);
 expect(screen.queryByLabelText('Configuration owner')).toBeNull();
 expect(screen.getByRole('heading', { name: 'User Settings' })).toBeTruthy();
});
it('Workspace Settings is bound to one exact target and never retargets on Session focus', async () => {
 const subject = cfg3Client(); subject.state.views = {}; const ui = render(<SettingsSurface client={subject.client} target={workspaceSettingsTarget('A', 'Workspace A')} host={cfg3Host(subject)}/>);
 await screen.findByText(/Revision: workspace-1/);
 expect(screen.getByRole('heading', { name: 'Workspace Settings — Workspace A' })).toBeTruthy();
 expect(screen.queryByLabelText('Configuration owner')).toBeNull();
 // A Session focus change elsewhere cannot change this instance's target.
 subject.state.views = {}; ui.rerender(<SettingsSurface client={subject.client} target={workspaceSettingsTarget('A', 'Workspace A')} host={cfg3Host(subject)}/>);
 expect(subject.request.mock.calls.every(([op]) => op.method !== 'session/attach' && op.method !== 'session/create')).toBe(true);
});
it('saves a whole Workspace Provider with explicit credentials without copying User members', async () => {
  const subject = cfg3Client(); await open(subject, 'Providers & Models');
  fireEvent.change(screen.getByLabelText('New Provider identity'), { target: { value: 'transport' } });
  fireEvent.click(screen.getByRole('button', { name: 'Add Provider' }));
  expect((screen.getByLabelText('Endpoint') as HTMLInputElement).value).toBe('');
  fireEvent.change(screen.getByLabelText('Endpoint'), { target: { value: 'https://workspace.invalid' } });
  fireEvent.change(screen.getByLabelText('Environment variable'), { target: { value: 'WORKSPACE_KEY' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save Provider transport' }));
  await screen.findByText(/Source saved. Native coordination/);
  expect(subject.request.mock.calls.find(([operation]) => operation.method === 'configuration/sourceWrite')?.[0]).toEqual({ method: 'configuration/sourceWrite', params: { target: { kind: 'workspace', directory: '/workspace/A' }, expected_revision: 'workspace-1', mutation: { kind: 'config', mutation: { unit: 'provider', id: 'transport', authored: { base_url: 'https://workspace.invalid', credential: { kind: 'environment', variable: 'WORKSPACE_KEY' } } } } } });
  expect(subject.request.mock.calls.filter(([operation]) => operation.method === 'session/adoptConfiguration')).toHaveLength(0);
  expect(screen.queryByRole('button', { name: /Adopt/ })).toBeNull();
});
it('preserves stale drafts and exact revision until a separate explicit review gesture', async () => {
  const subject = cfg3Client(async (operation, source) => {
    if (operation.method === 'configuration/sourceWrite') { source.workspace!.revision = 'external-edit'; throw new RpcFailure({ code: -32000, message: 'Conflict', data: { kind: 'source_conflict', scope: 'workspace', expected: operation.params.expected_revision, actual: 'external-edit' } }); }
  });
  await open(subject, 'Tools');
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
  await open(subject, 'Tools'); fireEvent.click(screen.getByLabelText('read')); fireEvent.click(screen.getByRole('button', { name: 'Save Native Tools' }));
  await screen.findByRole('alert');
  await waitFor(() => expect(subject.request.mock.calls.filter(([operation]) => operation.method === 'configuration/sourcesRead')).toHaveLength(2));
  expect(subject.request.mock.calls.filter(([operation]) => operation.method === 'configuration/sourceWrite')).toHaveLength(1);
  expect((screen.getByLabelText('read') as HTMLInputElement).checked).toBe(true);
});
it('edits an independent named-Agent whole resource with inherited model and Plugins off', async () => {
  const subject = cfg3Client(); await open(subject, 'Agents');
  fireEvent.change(screen.getByLabelText('New Agent identity'), { target: { value: 'researcher' } }); fireEvent.click(screen.getByRole('button', { name: 'Add Agent' }));
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
  await screen.findByText(/Revision: user-1/);
  fireEvent.click(screen.getByRole('button', { name: 'Skills' }));
  expect(screen.getByText(/Shadowed by Workspace/).textContent).toContain('/home/user/rustx/.agents/skills/review/SKILL.md');
  expect(screen.queryByText(/Root visible/)).toBeNull();
});

it('retains a Workspace draft and its original CAS revision across section navigation and a separate User Settings instance', async () => {
  const subject = cfg3Client(); const host = cfg3Host(subject);
  const workspace = render(<SettingsSurface client={subject.client} target={workspaceSettingsTarget('A', 'A')} host={host} />);
  await screen.findByText(/Revision: workspace-1/); fireEvent.click(screen.getByRole('button', { name: 'Tools' }));
  fireEvent.click(screen.getByLabelText('read'));
  // A separate User Settings instance cannot receive the Workspace draft.
  workspace.rerender(<SettingsSurface client={subject.client} target={userSettingsTarget} host={host} />);
  await screen.findByText(/Revision: user-1/); expect((screen.getByLabelText('read') as HTMLInputElement).checked).toBe(false);
  // Section navigation inside Workspace Settings preserves the dirty draft.
  workspace.rerender(<SettingsSurface client={subject.client} target={workspaceSettingsTarget('A', 'A')} host={host} />);
  await screen.findByText(/Revision: workspace-1/);
  fireEvent.click(screen.getByRole('button', { name: 'General' }));
  subject.source.workspace!.revision = 'external';
  fireEvent.click(screen.getByRole('button', { name: 'Read current sources' }));
  await screen.findByText(/Revision: external/);
  fireEvent.click(screen.getByRole('button', { name: 'Tools' }));
  expect((screen.getByLabelText('read') as HTMLInputElement).checked).toBe(true);
  fireEvent.click(screen.getByRole('button', { name: 'Save Native Tools' }));
  await waitFor(() => expect(subject.request.mock.calls.find(([op]) => op.method === 'configuration/sourceWrite')?.[0]).toMatchObject({ params: { expected_revision: 'workspace-1' } }));
});

it('replaces against the newer revision only after an explicit review gesture', async () => {
  let first = true;
  const subject = cfg3Client(async (op, source) => {
    if (op.method === 'configuration/sourceWrite' && first) { first = false; source.workspace!.revision = 'reviewed'; throw new RpcFailure({ code: -32000, message: 'Conflict', data: { kind: 'source_conflict', scope: 'workspace', expected: 'workspace-1', actual: 'reviewed' } }); }
  });
  await open(subject, 'Tools'); fireEvent.click(screen.getByLabelText('read'));
  fireEvent.click(screen.getByRole('button', { name: 'Save Native Tools' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Use reviewed revision' }));
  fireEvent.click(screen.getByRole('button', { name: 'Save Native Tools' }));
  await screen.findByText(/Source saved. Native coordination/);
  expect(subject.request.mock.calls.filter(([op]) => op.method === 'configuration/sourceWrite')[1][0]).toMatchObject({ params: { expected_revision: 'reviewed', mutation: { mutation: { authored: ['read'] } } } });
});

it.each(['empty', 'omit'] as const)('preserves native %s Tool selection as a distinct semantic-unit operation', async mode => {
  const subject = cfg3Client();
  // Removal replaces an existing Workspace override; an explicit empty
  // selection authors one where none exists. They are two different authored
  // intents, so each starts from the authored state it actually applies to.
  if (mode === 'omit') subject.source.workspace!.authored = { agent: { tools: { builtin: ['bash'] } } };
  await open(subject, 'Tools');
  // The empty one has to be authored first: rendering an inherited unit never
  // produces it.
  if (mode === 'empty') fireEvent.click(screen.getByRole('button', { name: 'Override Native Tools' }));
  fireEvent.click(screen.getByRole('button', { name: `${mode === 'empty' ? 'Save' : 'Use global default'} Native Tools` }));
  await waitFor(() => expect(subject.request.mock.calls.find(([op]) => op.method === 'configuration/sourceWrite')?.[0]).toMatchObject({ params: { mutation: { mutation: { unit: 'native_tools', authored: mode === 'empty' ? [] : null } } } }));
});


it('process state comes from native classification, outside Session adoption', async () => {
 const subject = cfg3Client(); subject.source.application = cfg3Application(); subject.source.application.units.process_bindings = { status: 'process_restart' };
 await open(subject, 'Server & source diagnostics', 'User');
 expect(screen.getByText(/Restart required/)).toBeTruthy();
 expect(screen.queryByRole('button', { name: /Adopt/ })).toBeNull();
});

it('does not equate invalid resource existence with readiness or Root authority', async () => {
  const subject = cfg3Client();
  subject.source.prospective_resources = structuredClone(subject.effective.resources);
  subject.source.prospective_resources.definitions = [{ name: 'unselected', family: 'managed_python', valid: false, location: { scope: 'workspace', path: '/workspace/.agents/python/unselected' } }];
  subject.source.prospective_resources.sources = { 'python:unselected': { status: 'unavailable' } };
  render(<SettingsSurface client={subject.client} target={workspaceSettingsTarget('A', 'A')} host={subject.host ??= cfg3Host(subject)} />); await screen.findByText(/Revision: workspace-1/);
  fireEvent.click(screen.getByRole('button', { name: 'Managed Python' }));
  expect(screen.getByText('Invalid definition')).toBeTruthy(); expect(screen.getByText('Defined only')).toBeTruthy();
  expect(screen.getAllByText('unavailable').length).toBeGreaterThan(0);
  expect(screen.queryByText('Root selected')).toBeNull();
});

it('keeps contributor default intent unspecified when enabling the closed Agent Status Plugin', async () => {
  const subject = cfg3Client(); await open(subject, 'Plugins');
  expect((screen.getByLabelText('Time contributor') as HTMLSelectElement).value).toBe('');
  fireEvent.click(screen.getByRole('switch', { name: 'Enable Agent Status Plugin' }));
  fireEvent.click(screen.getByRole('button', { name: 'Save Agent Status Plugin' }));
  await waitFor(() => expect(subject.request.mock.calls.find(([op]) => op.method === 'configuration/sourceWrite')?.[0]).toMatchObject({ params: { mutation: { mutation: { unit: 'agent_status', authored: { enabled: true } } } } }));
  const write = subject.request.mock.calls.find(([op]) => op.method === 'configuration/sourceWrite')![0];
  expect(JSON.stringify(write)).not.toMatch(/"time"|"background"|npm|cordis/);
});

it('reconnect rereads native sources without replaying a dirty draft', async () => {
  const subject = cfg3Client(); const ui = render(<SettingsSurface client={subject.client} target={workspaceSettingsTarget('A', 'A')} host={subject.host ??= cfg3Host(subject)} />);
  await screen.findByText(/Revision: workspace-1/); fireEvent.click(screen.getByRole('button', { name: 'Tools' }));
  fireEvent.click(screen.getByLabelText('read'));
  const snapshot = { ...subject.state, generation: 2 };
  subject.client.getSnapshot = () => snapshot;
  ui.rerender(<SettingsSurface client={subject.client} target={workspaceSettingsTarget('A', 'A')} host={subject.host ??= cfg3Host(subject)} />);
  await waitFor(() => expect(subject.request.mock.calls.filter(([op]) => op.method === 'configuration/sourcesRead')).toHaveLength(2));
  expect(subject.request.mock.calls.filter(([op]) => op.method === 'configuration/sourceWrite')).toHaveLength(0);
  expect((screen.getByLabelText('read') as HTMLInputElement).checked).toBe(true);
});

it('an older authoritative read cannot replace a newer read', async () => {
  let release: (value: import('../../protocol/app-server/v18').MethodResult) => void = () => {};
  let count = 0;
  const subject = cfg3Client(async op => { if (op.method === 'configuration/sourcesRead' && ++count === 2) return new Promise(resolve => { release = resolve; }); });
  render(<SettingsSurface client={subject.client} target={workspaceSettingsTarget('A', 'A')} host={subject.host ??= cfg3Host(subject)} />); await screen.findByText(/Revision: workspace-1/);
  const stale = structuredClone(subject.source);
  fireEvent.click(screen.getByRole('button', { name: 'Read current sources' }));
  await waitFor(() => expect(count).toBe(2));
  subject.source.workspace!.revision = 'newest';
  fireEvent.click(screen.getByRole('button', { name: 'Read current sources' }));
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
  await open(subject, 'Providers & Models');
  fireEvent.change(screen.getByLabelText('New Provider identity'), { target: { value: 'secret' } }); fireEvent.click(screen.getByRole('button', { name: 'Add Provider' }));
  fireEvent.change(screen.getByLabelText('Endpoint'), { target: { value: 'https://native.invalid' } }); fireEvent.change(screen.getByLabelText('Credential source'), { target: { value: 'literal' } });
  fireEvent.change(screen.getByLabelText('New literal credential'), { target: { value: 'SECRET_SENTINEL' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save Provider secret' }));
  await screen.findByText(/Source saved. Native coordination/);
  await waitFor(() => expect(screen.queryByLabelText('New literal credential')).toBeNull());
  expect(document.body.innerHTML).not.toContain('SECRET_SENTINEL');
  fireEvent.click(screen.getByRole('button', { name: 'Back to catalog' })); fireEvent.click(screen.getByRole('button', { name: 'Edit Provider secret' }));
  expect((screen.getByLabelText('Credential source') as HTMLSelectElement).value).toBe('retain');
});

it('retains a Model draft when native validation rejects its semantic unit', async () => {
 const subject = cfg3Client(async op => { if (op.method === 'configuration/sourceWrite') throw new RpcFailure({ code: -32000, message: 'Native validation failed', data: { kind: 'configuration_adoption', rejection: { status: 'failed', diagnostic: 'Invalid provider reference' } } }); });
 await open(subject, 'Providers & Models'); fireEvent.change(screen.getByLabelText('New Model identity'), { target: { value: 'draft-model' } }); fireEvent.click(screen.getByRole('button', { name: 'Add Model' }));
 fireEvent.change(screen.getByLabelText('Wire model identity'), { target: { value: 'wire' } }); fireEvent.change(screen.getByLabelText('Provider identity'), { target: { value: 'missing' } });
 fireEvent.click(screen.getByRole('button', { name: 'Save Model draft-model' }));
 expect((await screen.findByRole('alert')).textContent).toContain('Invalid provider reference'); expect((screen.getByLabelText('Provider identity') as HTMLInputElement).value).toBe('missing');
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
  await open(subject, 'Tools');
  fireEvent.click(screen.getByRole('button', { name: 'Use global default Native Tools' }));
  await screen.findByRole('button', { name: 'Use reviewed revision' });
  fireEvent.click(screen.getByRole('button', { name: 'Use global default Native Tools' }));
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
  await open(subject, 'Tools');
  fireEvent.click(screen.getByRole('button', { name: 'Use global default Native Tools' }));
  await screen.findByRole('button', { name: 'Use reviewed revision' });
  // Leaving and re-entering the section remounts the editor subtree. The frozen
  // base is durable editor state, not component-local state.
  fireEvent.click(screen.getByRole('button', { name: 'General' }));
  fireEvent.click(screen.getByRole('button', { name: 'Tools' }));
  await screen.findByRole('button', { name: 'Use global default Native Tools' });
  expect(screen.getByRole('button', { name: 'Use reviewed revision' })).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Use global default Native Tools' }));
  await waitFor(() => expect(subject.request.mock.calls.filter(([op]) => op.method === 'configuration/sourceWrite')).toHaveLength(2));
  expect(subject.request.mock.calls.filter(([op]) => op.method === 'configuration/sourceWrite')[1][0]).toMatchObject({ params: { expected_revision: 'workspace-1', mutation: { mutation: { authored: null } } } });
  // Only the explicit reviewed-revision gesture advances the operation to R2.
  fireEvent.click(screen.getByRole('button', { name: 'Use reviewed revision' }));
  fireEvent.click(screen.getByRole('button', { name: 'Use global default Native Tools' }));
  await waitFor(() => expect(subject.request.mock.calls.filter(([op]) => op.method === 'configuration/sourceWrite')).toHaveLength(3));
  expect(subject.request.mock.calls.filter(([op]) => op.method === 'configuration/sourceWrite')[2][0]).toMatchObject({ params: { expected_revision: 'external-removal-conflict', mutation: { mutation: { authored: null } } } });
});
it.each([
  ['effect', 'refresh'], ['refresh', 'refresh'], ['refresh', 'write'],
] as const)('fences an obsolete %s read rejection after a newer %s', async (readKind, successor) => {
  let rejectRead!: (error: Error) => void;
  const pending = new Promise<import('../../protocol/app-server/v18').MethodResult>((_, reject) => { rejectRead = reject; });
  let reads = 0;
  const subject = cfg3Client(async op => {
    if (op.method === 'configuration/sourcesRead' && ++reads === 2) return pending;
  });
  const ui = render(<SettingsSurface client={subject.client} target={workspaceSettingsTarget('A', 'A')} host={subject.host ??= cfg3Host(subject)} />);
  await screen.findByText(/Revision: workspace-1/);
  fireEvent.click(screen.getByRole('button', { name: 'Tools' }));
  if (readKind === 'effect') {
    const snapshot = { ...subject.state, generation: 2 };
    subject.client.getSnapshot = () => snapshot;
    ui.rerender(<SettingsSurface client={subject.client} target={workspaceSettingsTarget('A', 'A')} host={subject.host ??= cfg3Host(subject)} />);
  } else fireEvent.click(screen.getByRole('button', { name: 'Read current sources' }));
  await waitFor(() => expect(reads).toBe(2));
  if (successor === 'refresh') {
    subject.source.workspace!.revision = 'new-authority';
    fireEvent.click(screen.getByRole('button', { name: 'Read current sources' }));
  } else {
    fireEvent.click(screen.getByLabelText('read'));
    fireEvent.click(screen.getByRole('button', { name: 'Save Native Tools' }));
    await screen.findByText(/Source saved. Native coordination/);
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
  let acknowledge!: (outcome: { acknowledgement: import('../../protocol/app-server/v18').SourceSettings }) => void;
  const held = new Promise<{ acknowledgement: import('../../protocol/app-server/v18').SourceSettings }>(resolve => { acknowledge = resolve; });
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
  let release!: (result: import('../../protocol/app-server/v18').MethodResult) => void;
  const heldWrite = new Promise<import('../../protocol/app-server/v18').MethodResult>(resolve => { release = resolve; });
  const subject = cfg3Client(async operation => { if (operation.method === 'configuration/sourceWrite') return heldWrite; });
  const host = cfg3Host(subject); subject.host = host;
  render(<SettingsSurface client={subject.client} target={workspaceSettingsTarget('A', 'A')} host={host} />);
  await screen.findByText(/Revision: workspace-1/);
  fireEvent.click(screen.getByRole('button', { name: 'Providers & Models' }));
  fireEvent.change(screen.getByLabelText('New Provider identity'), { target: { value: 'secret' } }); fireEvent.click(screen.getByRole('button', { name: 'Add Provider' }));
  fireEvent.change(screen.getByLabelText('Endpoint'), { target: { value: 'https://native.invalid' } });
  fireEvent.change(screen.getByLabelText('Credential source'), { target: { value: 'literal' } });
  fireEvent.change(screen.getByLabelText('New literal credential'), { target: { value: 'SECRET_SENTINEL' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save Provider secret' }));
  await waitFor(() => expect(subject.request.mock.calls.filter(([op]) => op.method === 'configuration/sourceWrite')).toHaveLength(1));
  // The editor unmounts while the native write is still in flight.
  fireEvent.click(screen.getByRole('button', { name: 'General' }));
  expect(screen.queryByLabelText('New literal credential')).toBeNull();
  // Native commit and its authoritative reread both succeed after unmount.
  subject.source.workspace!.revision = 'saved-2';
  subject.source.workspace!.authored = { providers: { secret: { base_url: 'https://native.invalid', credential: { type: 'literal' } } } };
  release({ type: 'source_settings', projection: structuredClone(subject.source) });
  await screen.findByText(/Revision: saved-2/);
  // Reopening reconstructs from the redacted native projection, not the draft.
  fireEvent.click(screen.getByRole('button', { name: 'Providers & Models' }));
  fireEvent.click(screen.getByRole('button', { name: 'Edit Provider secret' }));
  expect((screen.getByLabelText('Credential source') as HTMLSelectElement).value).toBe('retain');
  expect(document.body.innerHTML).not.toContain('SECRET_SENTINEL');
  expect(screen.queryByRole('button', { name: 'Use reviewed revision' })).toBeNull();
  expect(subject.request.mock.calls.filter(([op]) => op.method === 'configuration/sourceWrite')).toHaveLength(1);
});

it('a late acknowledgement advances the CAS base without erasing a newer draft submitted after it', async () => {
  let release!: (result: import('../../protocol/app-server/v18').MethodResult) => void;
  const heldWrite = new Promise<import('../../protocol/app-server/v18').MethodResult>(resolve => { release = resolve; });
  const subject = cfg3Client(async operation => { if (operation.method === 'configuration/sourceWrite') return heldWrite; });
  const host = cfg3Host(subject); subject.host = host;
  render(<SettingsSurface client={subject.client} target={workspaceSettingsTarget('A', 'A')} host={host} />);
  await screen.findByText(/Revision: workspace-1/);
  fireEvent.click(screen.getByRole('button', { name: 'Tools' }));
  fireEvent.click(screen.getByLabelText('read'));
  fireEvent.click(screen.getByRole('button', { name: 'Save Native Tools' }));
  await waitFor(() => expect(subject.request.mock.calls.filter(([op]) => op.method === 'configuration/sourceWrite')).toHaveLength(1));
  // A newer editing intent exists before the first mutation is acknowledged.
  fireEvent.click(screen.getByLabelText('write'));
  subject.source.workspace!.revision = 'saved-2';
  release({ type: 'source_settings', projection: structuredClone(subject.source) });
  await screen.findByText(/Revision: saved-2/);
  // The newer draft survives the old acknowledgement; only the submitted
  // mutation is retired, and the newer intent's base advanced to the commit.
  expect((screen.getByLabelText('read') as HTMLInputElement).checked).toBe(true);
  expect((screen.getByLabelText('write') as HTMLInputElement).checked).toBe(true);
  expect(screen.getByText(/Draft base revision:/).textContent).toContain('saved-2');
  expect(screen.queryByRole('button', { name: 'Use reviewed revision' })).toBeNull();
});

it('keeps a confirmed Workspace save when the post-write authoritative reread fails', async () => {
  let failReads = false;
  const subject = cfg3Client(async operation => { if (operation.method === 'configuration/sourcesRead' && failReads) throw new Error('reread unavailable'); });
  const host = cfg3Host(subject); subject.host = host;
  render(<SettingsSurface client={subject.client} target={workspaceSettingsTarget('A', 'A')} host={host} />);
  await screen.findByText(/Revision: workspace-1/);
  fireEvent.click(screen.getByRole('button', { name: 'Tools' }));
  fireEvent.click(screen.getByLabelText('read'));
  failReads = true;
  fireEvent.click(screen.getByRole('button', { name: 'Save Native Tools' }));
  // The commit is reported as saved, and the failed reread is a separate fact.
  await screen.findByText(/Source saved. Native coordination/);
  await screen.findByText(/Saved, but the authoritative reread failed/);
  expect(subject.request.mock.calls.filter(([op]) => op.method === 'configuration/sourceWrite')).toHaveLength(1);
  // A failed reread never turns the confirmed write back into an unsaved draft:
  // the submitted intent was retired with its confirmed commit, so there is no
  // pending Save and no review conflict against a stale base.
  expect((screen.getByRole('button', { name: 'Save Native Tools' }) as HTMLButtonElement).disabled).toBe(true);
  expect(screen.queryByRole('button', { name: 'Use reviewed revision' })).toBeNull();
});

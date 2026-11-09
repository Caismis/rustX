import { providerAction } from './settings-harness';
import { waitForSettingsSave } from './settings-harness';
import {openResourceModule} from './settings-harness';
import { settingsContext } from './settings-harness';
import { observedRevision } from './settings-harness';
import { waitForSettingsRevision } from './settings-harness';
import { sourceReceipt } from './fixture';
import { boundedSettings } from "./fixture";
// @vitest-environment jsdom
import { afterEach, expect, it } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { TextField } from '../src/app/settings/forms/controls';
import { UnitForm } from '../src/app/settings/forms/bridge';
import { userSettingsTarget, workspaceSettingsTarget } from '../src/app/settings/projection';
import { OutcomeUncertain, RpcFailure } from '../src/client/errors';
import { cfg3Application, cfg3Source } from './cfg3-data';
import {
  chooseOption, confirmAction, openResourceRow, openSettingsPage, renderEditor, sameRevision,
  settingsReady, SettingsSurface, newSubagent,
} from './settings-harness';
import { cfg3Client, cfg3Host } from './cfg3-fixture';
import { composerPreferences, COMPOSER_PREFERENCE_KEY } from '../src/app/composer/preferences';
afterEach(cleanup);

/** The native source path and revision are diagnostics: they live on Advanced,
 * not on the ordinary product pages. A test that asserts on them navigates
 * there exactly as a user would, and comes back to the page it was editing —
 * which also exercises the draft surviving that navigation. */
async function revisionOnAdvanced(pattern: RegExp, back?: string) {
  await openSettingsPage('Advanced');
  await waitForSettingsRevision(pattern.source.replace(/^Revision: /, ''));
  if (back) await openSettingsPage(back);
}

it('C09 Workspace A/B drafts survive navigation and Session focus without retargeting', async () => {
 const s = cfg3Client(); const host = cfg3Host(s);
 // Query the semantic unit being edited: scanning every label on this large
 // page repeatedly makes the contract depend on unrelated editors' DOM size.
 const nativeTools = () => within(screen.getByRole('form', { name: 'Built-in tools' }));
 const ui = render(<SettingsSurface client={s.client} target={workspaceSettingsTarget('A', 'A')} host={host}/>);
 await settingsReady(); await openSettingsPage('Tools & Permissions');
 fireEvent.click(nativeTools().getByLabelText('read'));
 // A different Workspace Settings instance is a different target and must not
 // receive Workspace A's draft.
 ui.rerender(<SettingsSurface client={s.client} target={workspaceSettingsTarget('B', 'B')} host={host}/>);
 await openSettingsPage('Tools & Permissions');
 await waitFor(() => expect((nativeTools().getByLabelText('read') as HTMLInputElement).checked).toBe(false));
 fireEvent.click(nativeTools().getByLabelText('write'));
 // Returning to A restores A's own draft and excludes B's.
 ui.rerender(<SettingsSurface client={s.client} target={workspaceSettingsTarget('A', 'A')} host={host}/>);
 await openSettingsPage('Tools & Permissions');
 await waitFor(() => expect((nativeTools().getByLabelText('read') as HTMLInputElement).checked).toBe(true));
 expect((nativeTools().getByLabelText('write') as HTMLInputElement).checked).toBe(false);
 s.state.views = {}; ui.rerender(<SettingsSurface client={s.client} target={workspaceSettingsTarget('A', 'A')} host={host}/>);
 await openSettingsPage('Tools & Permissions');
 fireEvent.click(nativeTools().getByRole('button', { name: 'Save Built-in tools' }));
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
 fireEvent.click(screen.getByRole('button', { name: 'Save Built-in tools' }));
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
 fireEvent.click(screen.getByRole('button', { name: 'Save Built-in tools' }));
 expect(s.request.mock.calls.filter(([op]) => op.method === 'configuration/sourceWrite')).toHaveLength(0);
});

it.each(['target', 'connection'] as const)('C10 late response after %s replacement cannot overwrite new authority', async invalidation => {
 let release!: (result: import('../../protocol/app-server/v77').MethodResult) => void;
 const pending = new Promise<import('../../protocol/app-server/v77').MethodResult>(resolve => { release = resolve; });
 let reads = 0;
 const s = cfg3Client(async op => { if (op.method === 'configuration/sourcesRead' && ++reads === 1) return pending; });
 const host = cfg3Host(s); const ui = render(<SettingsSurface client={s.client} target={workspaceSettingsTarget('A', 'A')} host={host}/>);
 await waitFor(() => expect(reads).toBe(1)); const stale = structuredClone(s.source); stale.workspace!.revision = 'stale';
 s.source.workspace!.revision = 'fresh';
 if (invalidation === 'connection') act(() => s.publish({ authorityRevision: 2 }));
 ui.rerender(<SettingsSurface client={s.client} target={workspaceSettingsTarget(invalidation === 'target' ? 'B' : 'A', invalidation === 'target' ? 'B' : 'A')} host={host}/>);
 await openSettingsPage('Advanced');
 await waitForSettingsRevision('fresh');
 await act(async () => { release({ type: 'source_settings', projection:boundedSettings(stale) }); await pending; });
 expect(observedRevision('stale')).toBeNull(); expect(observedRevision('fresh')).toBeTruthy();
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

it('S2-01 global Settings has six product pages and opens at General', async () => {
  const subject = cfg3Client();
  render(<SettingsSurface client={subject.client} target={userSettingsTarget} />);
  expect(screen.getAllByRole('tab').map(tab => tab.textContent)).toEqual([
    'General', 'Models', 'Agent', 'Tools & Permissions', 'Extensions', 'Advanced',
  ]);
  expect(screen.getByRole('tab', { name: 'General', selected: true })).toBeTruthy();
  for (const gone of obsoletePages) expect(screen.queryByRole('tab', { name: gone })).toBeNull();
});

it('General owns the persisted busy Enter selector', async () => {
  act(() => composerPreferences().setBusyEnter('queue'));
  const subject = cfg3Client();
  render(<SettingsSurface client={subject.client} target={userSettingsTarget} />);
  const general = screen.getByRole('region', { name: 'General' });
  fireEvent.click(within(general).getByRole('button', { name: /Send behavior while busy/ }));
  fireEvent.click(await screen.findByRole('option', { name: 'Immediately' }));
  expect(composerPreferences().getSnapshot()).toBe('steer');
  expect(localStorage.getItem(COMPOSER_PREFERENCE_KEY)).toBe('steer');
  act(() => composerPreferences().setBusyEnter('queue'));
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
  for (const [page, region] of [['Models', 'Model Settings'], ['Agent', 'Agent'], ['Tools & Permissions', 'Tools & Permissions'],
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
 await waitForSettingsRevision('user-1');
 expect(subject.request.mock.calls.every(([op]) => op.method === 'configuration/sourcesRead')).toBe(true);
 expect(subject.request.mock.calls[0][0]).toEqual({ method: 'configuration/sourcesRead', params: { target: { kind: 'user' } } });
});

it('User Settings has no ordinary Configuration owner selector', async () => {
 const subject = cfg3Client(); render(<SettingsSurface client={subject.client} target={userSettingsTarget}/>);
 await openSettingsPage('Advanced');
 await waitForSettingsRevision('user-1');
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
 expect(subject.request.mock.calls.every(([op]) => op.method !== 'thread/resume' && op.method !== 'thread/start')).toBe(true);
});

it('saves a whole Workspace Provider with explicit credentials without copying User members', async () => {
  const subject = cfg3Client(async (operation, source) => {
    if (operation.method !== 'configuration/sourceWrite' || operation.params.mutation.kind !== 'config') return;
    const mutation = operation.params.mutation.mutation;
    if (mutation.unit === 'provider' && mutation.authored?.credential.kind === 'environment') {
      source.workspace!.authored = { providers: { [mutation.id]: { protocol: mutation.authored.protocol, base_url: mutation.authored.base_url, credential: { type: 'environment', variable: mutation.authored.credential.variable } } } };
    }
  });
  subject.source.resolved = { providers: subject.source.user.authored!.providers };
  await open(subject, 'Models');
  await openResourceRow('transport');
  await providerAction('Customize');
  expect((screen.getByLabelText('Base URL') as HTMLInputElement).value).toBe('https://user.invalid');
  fireEvent.change(screen.getByLabelText('Base URL'), { target: { value: 'https://workspace.invalid' } });
  await providerAction('Use an environment variable');
  fireEvent.change(screen.getByLabelText('Environment variable'), { target: { value: 'WORKSPACE_KEY' } });
  await chooseOption('API format', 'OpenAI Responses (/v1/responses)');
  fireEvent.keyDown(screen.getByLabelText('Environment variable'), { key: 'Enter' });
  await waitForSettingsSave();
  expect(subject.request.mock.calls.find(([operation]) => operation.method === 'configuration/sourceWrite')?.[0]).toMatchObject({ method: 'configuration/sourceWrite', params: { target: { kind: 'workspace', directory: '/workspace/A' }, expected_revision: 'workspace-1', mutation: { kind: 'config', mutation: { unit: 'provider', id: 'transport', authored: { protocol: 'openai_responses', base_url: 'https://workspace.invalid', credential: { kind: 'environment', variable: 'WORKSPACE_KEY' } } } } } });
  expect(screen.queryByRole('button', { name: /Adopt/ })).toBeNull();
});

it('preserves stale drafts and exact revision until a separate explicit review gesture', async () => {
  const subject = cfg3Client(async (operation, source) => {
    if (operation.method === 'configuration/sourceWrite') { source.workspace!.revision = 'external-edit'; throw new RpcFailure({ code: -32000, message: 'Conflict', data: { kind: 'source_conflict', scope: 'workspace', expected: operation.params.expected_revision, actual: 'external-edit' } }); }
  });
  await open(subject, 'Tools & Permissions');
  fireEvent.click(screen.getByLabelText('read')); fireEvent.click(screen.getByRole('button', { name: 'Save Built-in tools' }));
  await screen.findByRole('alert'); await screen.findByRole('button', { name: 'Use latest setting' });
  expect((screen.getByLabelText('read') as HTMLInputElement).checked).toBe(true);
  expect(subject.request.mock.calls.filter(([operation]) => operation.method === 'configuration/sourceWrite')).toHaveLength(1);
  fireEvent.click(screen.getByRole('button', { name: 'Save Built-in tools' }));
  await waitFor(() => expect(subject.request.mock.calls.filter(([operation]) => operation.method === 'configuration/sourceWrite')).toHaveLength(2));
  const writes = subject.request.mock.calls.filter(([operation]) => operation.method === 'configuration/sourceWrite');
  expect(writes[1][0]).toMatchObject({ params: { expected_revision: 'workspace-1' } });
});

it('repairs uncertain writes by rereading and never replays the mutation', async () => {
  const subject = cfg3Client(async operation => { if (operation.method === 'configuration/sourceWrite') throw new OutcomeUncertain(); });
  await open(subject, 'Tools & Permissions'); fireEvent.click(screen.getByLabelText('read')); fireEvent.click(screen.getByRole('button', { name: 'Save Built-in tools' }));
  // The target reports the unknown outcome and the unit reports its own; both
  // say the write is never replayed, and neither claims it was saved.
  await waitFor(() => expect(screen.getAllByRole('alert').map(node => node.textContent).join(' ')).toMatch(/result is unknown/));
  await waitFor(() => expect(subject.request.mock.calls.filter(([operation]) => operation.method === 'configuration/sourcesRead')).toHaveLength(2));
  expect(subject.request.mock.calls.filter(([operation]) => operation.method === 'configuration/sourceWrite')).toHaveLength(1);
  expect((screen.getByLabelText('read') as HTMLInputElement).checked).toBe(true);
});

it('edits an independent named-Agent whole resource with inherited model and extensions off', async () => {
  const subject = cfg3Client(); await open(subject, 'Extensions');
  await openResourceModule('subagents');
  await newSubagent('researcher');
  expect(screen.getByRole('button', { name: 'Inherit default Model' })).toBeTruthy();
  expect((screen.getByLabelText('todo') as HTMLInputElement).checked).toBe(false);
  fireEvent.change(screen.getByLabelText('Description'), { target: { value: 'Research a topic' } });
  fireEvent.change(screen.getByLabelText('System prompt'), { target: { value: 'Read and report findings.' } });
  fireEvent.click(screen.getByLabelText('read'));
  fireEvent.click(screen.getByRole('button', { name: 'Clear tools' }));
  expect((screen.getByLabelText('read') as HTMLInputElement).checked).toBe(false);
  fireEvent.click(screen.getByLabelText('read')); fireEvent.click(screen.getByLabelText('read_image')); fireEvent.click(screen.getByLabelText('todo'));
  fireEvent.click(screen.getByRole('button', { name: 'Save New subagent' }));
  await waitFor(() => expect(subject.request.mock.calls.find(([operation]) => operation.method === 'configuration/sourceWrite')?.[0]).toMatchObject({ params: { expected_revision: 'missing', mutation: { kind: 'agent', name: 'researcher', authored: { description: 'Research a topic', tools: { builtin: ['read', 'read_image'] }, plugins: { todo: { enabled: true } } } } } }));
});

it('an invalid skill shows the native read error without an editor or an invented healthy definition',async()=>{
 const subject=cfg3Client(async op=>{if(op.method==='skills/read')throw new Error('Invalid SKILL.md');});
 subject.source.prospective_resources={...subject.effective.resources,definitions:[{family:'skill',name:'review',valid:false,location:{scope:'user',path:'/home/user/rustx/.agents/skills/review/SKILL.md'}}]};
 render(<SettingsSurface client={subject.client} target={userSettingsTarget} host={cfg3Host(subject)}/>);
 await openResourceModule('skills');
 expect(screen.queryByRole('switch',{name:'Allow review for the agent'})).toBeNull();
 await openResourceRow('review');
 expect((await screen.findByRole('alert')).textContent).toContain('Invalid SKILL.md');
 expect(screen.queryByRole('textbox')).toBeNull();
 expect(screen.queryByRole('button',{name:/^(Save|Edit|Delete)/})).toBeNull();
 expect(subject.request.mock.calls.some(([op])=>op.method==='configuration/sourceWrite')).toBe(false);
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
  await waitForSettingsRevision('external');
  await openSettingsPage('Tools & Permissions');
  expect((screen.getByLabelText('read') as HTMLInputElement).checked).toBe(true);
  fireEvent.click(screen.getByRole('button', { name: 'Save Built-in tools' }));
  await waitFor(() => expect(subject.request.mock.calls.find(([op]) => op.method === 'configuration/sourceWrite')?.[0]).toMatchObject({ params: { expected_revision: 'workspace-1' } }));
});

it('replaces against the newer revision only after an explicit review gesture', async () => {
  let first = true;
  const subject = cfg3Client(async (op, source) => {
    if (op.method === 'configuration/sourceWrite' && first) { first = false; source.workspace!.revision = 'reviewed'; throw new RpcFailure({ code: -32000, message: 'Conflict', data: { kind: 'source_conflict', scope: 'workspace', expected: 'workspace-1', actual: 'reviewed' } }); }
  });
  await open(subject, 'Tools & Permissions'); fireEvent.click(screen.getByLabelText('read'));
  fireEvent.click(screen.getByRole('button', { name: 'Save Built-in tools' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Use latest setting' }));
  fireEvent.click(screen.getByRole('button', { name: 'Save Built-in tools' }));
  await waitForSettingsSave();
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
    fireEvent.click(screen.getByRole('button', { name: 'Override Built-in tools' }));
    fireEvent.click(screen.getByRole('button', { name: 'Save Built-in tools' }));
  } else {
    await confirmAction('Use global default Built-in tools');
  }
  await waitFor(() => expect(subject.request.mock.calls.find(([op]) => op.method === 'configuration/sourceWrite')?.[0]).toMatchObject({ params: { mutation: { mutation: { unit: 'native_tools', authored: mode === 'empty' ? [] : null } } } }));
});

it('process state comes from native classification, outside the Session commit', async () => {
 const subject = cfg3Client(); subject.source.application = cfg3Application(); subject.source.application.units.process_bindings = { status: 'process_restart' };
 await open(subject, 'Advanced', 'User');
 expect(settingsContext().observation?.application?.units.process_bindings?.status).toBe('process_restart');
 expect(screen.queryByText(/Restart required/)).toBeNull();
 expect(screen.queryByRole('button', { name: /Adopt/ })).toBeNull();
});

it('does not equate invalid resource existence with readiness or Root authority', async () => {
  const subject = cfg3Client();
  subject.source.prospective_resources = structuredClone(subject.effective.resources);
  subject.source.prospective_resources.definitions = [{ name: 'unselected', family: 'managed_python', valid: false, location: { scope: 'workspace', path: '/workspace/.agents/python/unselected' } }];
  subject.source.prospective_resources.sources = { 'python:unselected': { status: 'unavailable' } };
  await open(subject, 'Extensions');
  fireEvent.click(screen.getByRole('tab', { name: 'Python' }));
  await openResourceRow('unselected');
  // Four independent native facts, four independent renderings.
  expect(screen.queryByText('Valid definition')).toBeNull();
  expect(screen.getByText('This resource is unavailable. Check its configuration and dependencies.')).toBeTruthy();
  // Native published no root inspection here, so root availability is reported
  // as unobserved. Unobserved is never rendered as "allowed" or as "not
  // allowed", and preparation is never inferred from validity.
  expect(screen.queryByText('Root selection not observed')).toBeNull();
  expect(screen.queryByText('Allowed for the root Agent')).toBeNull();
  expect(screen.queryByText('Prepared')).toBeNull();
});

it('keeps contributor default intent unspecified when enabling the closed Agent Status extension', async () => {
  const subject = cfg3Client(); await open(subject, 'Extensions');
  fireEvent.click(screen.getByRole('tab', { name: 'Built-in' }));
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
  let release: (value: import('../../protocol/app-server/v77').MethodResult) => void = () => {};
  let count = 0;
  const subject = cfg3Client(async op => { if (op.method === 'configuration/sourcesRead' && ++count === 2) return new Promise(resolve => { release = resolve; }); });
  render(<SettingsSurface client={subject.client} target={workspaceSettingsTarget('A', 'A')} host={subject.host ??= cfg3Host(subject)} />);
  await openSettingsPage('Advanced');
  await waitForSettingsRevision('workspace-1');
  const stale = structuredClone(subject.source);
  fireEvent.click(screen.getByRole('button', { name: 'Reload configuration' }));
  await waitFor(() => expect(count).toBe(2));
  subject.source.workspace!.revision = 'newest';
  fireEvent.click(screen.getByRole('button', { name: 'Reload configuration' }));
  await waitForSettingsRevision('newest');
  release({ type: 'source_settings', projection:boundedSettings(stale) });
  await waitFor(() => expect(observedRevision('newest')).toBeTruthy());
});

it('removes literal credentials from a successful Provider draft using the redacted native acknowledgement', async () => {
  const subject = cfg3Client(async (op, source) => {
    if (op.method === 'configuration/sourceWrite' && op.params.mutation.kind === 'config' && op.params.mutation.mutation.unit === 'provider') {
      source.workspace!.authored = { providers: { secret: { protocol: 'openai_responses', base_url: 'https://native.invalid', credential: { type: 'literal' } } } };
    }
  });
  await open(subject, 'Models');
  fireEvent.click(screen.getByRole('button', { name: '+ Add Provider' }));
  fireEvent.change(screen.getByLabelText('New Provider identity'), { target: { value: 'secret' } }); fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
  fireEvent.change(screen.getByLabelText('Base URL'), { target: { value: 'https://native.invalid' } });
  fireEvent.change(await screen.findByLabelText('API Key'), { target: { value: 'SECRET_SENTINEL' } });
  fireEvent.keyDown(screen.getByLabelText('API Key'), { key: 'Enter' });
  await waitForSettingsSave();
  await waitFor(() => expect((screen.getByLabelText('API Key') as HTMLInputElement).value).toBe(''));
  expect(document.body.innerHTML).not.toContain('SECRET_SENTINEL');
  await openResourceRow('secret');
  expect((screen.getByLabelText('API Key') as HTMLInputElement).placeholder).toContain('API Key saved');
});

it('round-trips the single native JSON Schema claim through whole Workspace Model replacement', async () => {
  const subject = cfg3Client(async (op, source) => {
    if (op.method !== 'configuration/sourceWrite' || op.params.mutation.kind !== 'config') return;
    const mutation = op.params.mutation.mutation;
    if (mutation.unit === 'model' && mutation.authored) {
      source.workspace!.authored = { models: { [mutation.id]: structuredClone(mutation.authored) } };
    }
  });
  const inherited = subject.effective.document.models!.main;
  inherited.capabilities.json_schema_output = true;
  subject.source.user.authored!.models = { main: structuredClone(inherited) };
  subject.source.resolved = structuredClone(subject.effective.document);
  subject.source.resolved.providers = subject.source.user.authored!.providers;
  await open(subject, 'Models');
  await openResourceRow('main');
  const nativeSchema = () => screen.getByRole('switch', { name: 'Structured output' }) as HTMLInputElement;
  expect(screen.getAllByRole('switch', { name: 'Structured output' })).toHaveLength(1);
  expect(nativeSchema().checked).toBe(true);
  expect((screen.getByRole('button', { name: 'Save Model main' }) as HTMLButtonElement).disabled).toBe(true);
  for (const enabled of [false, true]) {
    fireEvent.click(nativeSchema());
    fireEvent.click(screen.getByRole('button', { name: 'Save Model main' }));
    await waitForSettingsSave();
    fireEvent.click(within(screen.getByRole('dialog', { name: 'Model main' })).getByRole('button', { name: 'Close' }));
      const reads = subject.request.mock.calls.filter(([op]) => op.method === 'configuration/sourcesRead').length;
    fireEvent.click(screen.getByRole('button', { name: 'Reload configuration' }));
    await waitFor(() => expect(subject.request.mock.calls.filter(([op]) => op.method === 'configuration/sourcesRead')).toHaveLength(reads + 1));
    await openResourceRow('main');
    await waitFor(() => expect(nativeSchema().checked).toBe(enabled));
    const lastWrite = subject.request.mock.calls.flatMap(([op]) => op.method === 'configuration/sourceWrite' ? [op] : []).at(-1);
    expect(lastWrite).toMatchObject({ params: { target: { kind: 'workspace', directory: '/workspace/A' }, mutation: { kind: 'config', mutation: { unit: 'model', id: 'main', authored: { ...inherited, capabilities: { ...inherited.capabilities, json_schema_output: enabled } } } } } });
    expect(subject.source.user.authored!.models!.main.capabilities.json_schema_output).toBe(true);
  }
});

it('retains a Model draft when native validation rejects its semantic unit', async () => {
 const subject = cfg3Client(async op => { if (op.method === 'configuration/sourceWrite') throw new RpcFailure({ code: -32602, message: 'Invalid provider reference', data: { kind: 'invalid_params' } }); });
 subject.source.resolved = { providers: subject.source.user.authored!.providers };
 await open(subject, 'Models');
 await openResourceRow('transport');
 fireEvent.click(screen.getByRole('button', { name: /^(?:\+ )?Add Model$/ }));
 fireEvent.change(await screen.findByLabelText('Model ID'), { target: { value: 'wire' } });
 fireEvent.click(screen.getByRole('button', { name: 'Advanced configuration' }));
 fireEvent.change(screen.getByLabelText('Model ID'), { target: { value: 'rejected-model' } });
 fireEvent.click(screen.getByRole('button', { name: 'Save Add Model' }));
 expect((await screen.findByRole('alert')).textContent).toContain('Invalid provider reference');
 expect((screen.getByLabelText('Model ID') as HTMLInputElement).value).toBe('rejected-model');
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
  await confirmAction('Use global default Built-in tools');
  await screen.findByRole('button', { name: 'Use latest setting' });
  await confirmAction('Use global default Built-in tools');
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
  // The transaction belongs to this semantic unit. Scoping queries avoids
  // recomputing every unrelated editor's accessible names on each retry.
  const nativeTools = () => within(screen.getByRole('form', { name: 'Built-in tools' }));
  await confirmAction('Use global default Built-in tools', nativeTools());
  await nativeTools().findByRole('button', { name: 'Use latest setting' });
  // Leaving and re-entering the page remounts the editor subtree. The frozen
  // base is durable transaction state, not component-local state.
  await openSettingsPage('Agent');
  await openSettingsPage('Tools & Permissions');
  await nativeTools().findByRole('button', { name: 'Use global default Built-in tools' });
  expect(nativeTools().getByRole('button', { name: 'Use latest setting' })).toBeTruthy();
  await confirmAction('Use global default Built-in tools', nativeTools());
  await waitFor(() => expect(subject.request.mock.calls.filter(([op]) => op.method === 'configuration/sourceWrite')).toHaveLength(2));
  expect(subject.request.mock.calls.filter(([op]) => op.method === 'configuration/sourceWrite')[1][0]).toMatchObject({ params: { expected_revision: 'workspace-1', mutation: { mutation: { authored: null } } } });
  // Submission alone does not settle its conflict and authoritative reread.
  // Wait for the unit to admit another removal before reviewing that revision.
  await waitFor(() => expect((nativeTools().getByRole('button', { name: 'Use global default Built-in tools' }) as HTMLButtonElement).disabled).toBe(false));
  // Only the explicit reviewed-revision gesture advances the operation to R2.
  fireEvent.click(nativeTools().getByRole('button', { name: 'Use latest setting' }));
  await confirmAction('Use global default Built-in tools', nativeTools());
  await waitFor(() => expect(subject.request.mock.calls.filter(([op]) => op.method === 'configuration/sourceWrite')).toHaveLength(3));
  expect(subject.request.mock.calls.filter(([op]) => op.method === 'configuration/sourceWrite')[2][0]).toMatchObject({ params: { expected_revision: 'external-removal-conflict', mutation: { mutation: { authored: null } } } });
});

it.each([
  ['effect', 'refresh'], ['refresh', 'refresh'], ['refresh', 'write'],
] as const)('fences an obsolete %s read rejection after a newer %s', async (readKind, successor) => {
  let rejectRead!: (error: Error) => void;
  const pending = new Promise<import('../../protocol/app-server/v77').MethodResult>((_, reject) => { rejectRead = reject; });
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
    fireEvent.click(screen.getByRole('button', { name: 'Save Built-in tools' }));
    await waitForSettingsSave();
    await openSettingsPage('Advanced');
  }
  const revision = successor === 'refresh' ? 'new-authority' : 'saved-2';
  await waitForSettingsRevision(`${revision}`);
  await act(async () => { rejectRead(new Error('obsolete read failed')); await pending.catch(() => {}); });
  expect(observedRevision(`${revision}`)).toBeTruthy();
  expect(screen.queryByRole('alert')).toBeNull();
  expect(document.body.textContent).not.toContain('obsolete read failed');
});

it.each(['before acknowledgement', 'after acknowledgement', 'after the next edit'] as const)('T12/T16 source projection %s preserves subsequent drafts', async order => {
  // The acknowledgement is held explicitly; nothing here depends on timing.
  let acknowledge!: (outcome: { acknowledgement: {revision:string} }) => void;
  const held = new Promise<{ acknowledgement: {revision:string} }>(resolve => { acknowledge = resolve; });
  const source = cfg3Source();
  const form = (authored: { command: string }, revision: string) => <UnitForm<{ command: string }>
    title="MCP acknowledgement" authored={authored} blank={{ command: '' }} revision={revision}
    mutation={value => ({ kind: 'mcp', id: 'fixture', authored: value ? { definition: { type: 'stdio', command: value.command } } : null })}>
    {(value, change) => <TextField label="Acknowledged command" value={value.command} change={command => change({ command })} />}
  </UnitForm>;
  const { rerender, actor } = await renderEditor(form({ command: 'original' }, 'r1'), { source, write: () => held });
  fireEvent.change(screen.getByLabelText('Acknowledged command'), { target: { value: 'submitted-literal' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save MCP acknowledgement' }));
  // Hold source publication and the acknowledgement independently.
  if (order === 'before acknowledgement') rerender(form({ command: 'native-redacted' }, 'r2'));
  await act(async () => { acknowledge({ acknowledgement: sourceReceipt(sameRevision(source, 'r2')) }); await held; });
  if (order === 'after acknowledgement') rerender(form({ command: 'native-redacted' }, 'r2'));
  if (order !== 'after the next edit') expect((screen.getByLabelText('Acknowledged command') as HTMLInputElement).value).toBe('native-redacted');
  fireEvent.change(screen.getByLabelText('Acknowledged command'), { target: { value: 'preserved-draft' } });
  // A later native application notification rereads the same source, then a
  // conflict reread advances its revision. Neither owns this subsequent edit.
  rerender(form({ command: 'native-redacted' }, 'r2'));
  rerender(form({ command: 'external' }, 'r3'));
  expect((screen.getByLabelText('Acknowledged command') as HTMLInputElement).value).toBe('preserved-draft');
  expect(Object.values(actor.getSnapshot().context.units).some(unit => unit.getSnapshot().context.base === 'r2')).toBe(true);
  expect(screen.getByRole('button', { name: 'Use latest setting' })).toBeTruthy();
});

it('T17 a post-commit read that observes the exact pre-save revision asks for review', async () => {
  // Native commits r2, then an external writer restores the pre-save bytes:
  // every authoritative read, including the post-commit one, answers r1.
  const source = sameRevision(cfg3Source(), 'r1');
  const committed = sameRevision(source, 'r2');
  const { actor } = await renderEditor(<UnitForm<{ command: string }>
    title="MCP rollback" authored={{ command: 'original' }} blank={{ command: '' }} revision="r1"
    mutation={value => ({ kind: 'mcp', id: 'fixture', authored: value ? { definition: { type: 'stdio', command: value.command } } : null })}>
    {(value, change) => <TextField label="Rollback command" value={value.command} change={command => change({ command })} />}
  </UnitForm>, { source, write: async () => ({ acknowledgement: sourceReceipt(committed) }) });
  fireEvent.change(screen.getByLabelText('Rollback command'), { target: { value: 'saved' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save MCP rollback' }));
  // The commit is definitive; the source no longer carries it.
  await screen.findByRole('button', { name: 'Use latest setting' });
  expect(screen.getByText(/This setting changed elsewhere/)).toBeTruthy();
  expect(Object.values(actor.getSnapshot().context.units).some(unit => unit.getSnapshot().context.base === 'r2')).toBe(true);
  expect(actor.getSnapshot().context.observation?.user.revision).toBe('r1');
});

it('retires a confirmed Provider save, including its literal credential, after the editor unmounts', async () => {
  let release!: (result: import('../../protocol/app-server/v77').MethodResult) => void;
  const heldWrite = new Promise<import('../../protocol/app-server/v77').MethodResult>(resolve => { release = resolve; });
  const subject = cfg3Client(async operation => { if (operation.method === 'configuration/sourceWrite') return heldWrite; });
  const host = cfg3Host(subject); subject.host = host;
  render(<SettingsSurface client={subject.client} target={workspaceSettingsTarget('A', 'A')} host={host} />);
  await settingsReady(); await openSettingsPage('Models');
  fireEvent.click(screen.getByRole('button', { name: '+ Add Provider' }));
  fireEvent.change(screen.getByLabelText('New Provider identity'), { target: { value: 'secret' } });
  fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
  fireEvent.change(screen.getByLabelText('Base URL'), { target: { value: 'https://native.invalid' } });
  fireEvent.change(await screen.findByLabelText('API Key'), { target: { value: 'SECRET_SENTINEL' } });
  fireEvent.keyDown(screen.getByLabelText('API Key'), { key: 'Enter' });
  await waitFor(() => expect(subject.request.mock.calls.filter(([op]) => op.method === 'configuration/sourceWrite')).toHaveLength(1));
  // The editor unmounts while the native write is still in flight.
  await openSettingsPage('Agent');
  expect(screen.queryByLabelText('API Key')).toBeNull();
  // Native commit and its authoritative reread both succeed after unmount.
  subject.source.workspace!.revision = 'saved-2';
  subject.source.workspace!.authored = { providers: { secret: { protocol: 'openai_responses', base_url: 'https://native.invalid', credential: { type: 'literal' } } } };
  release({type:'sources_written',revision:'saved-2',configuration_revision:'1'});
  await openSettingsPage('Advanced');
  await waitForSettingsRevision('saved-2');
  // Returning to Models restores the exact detail this page was left on —
  // focus is per-page navigation state with one owner — and the editor
  // reconstructs from the redacted native projection, not from the draft.
  await openSettingsPage('Models');
  expect(await screen.findByRole('heading', { name: 'Provider secret' })).toBeTruthy();
  expect((screen.getByLabelText('API Key') as HTMLInputElement).placeholder).toContain('API Key saved');
  expect(document.body.innerHTML).not.toContain('SECRET_SENTINEL');
  expect(screen.queryByRole('button', { name: 'Use latest setting' })).toBeNull();
  expect(subject.request.mock.calls.filter(([op]) => op.method === 'configuration/sourceWrite')).toHaveLength(1);
});

it('a late acknowledgement advances the CAS base without erasing a newer draft submitted after it', async () => {
  let release!: (result: import('../../protocol/app-server/v77').MethodResult) => void;
  const heldWrite = new Promise<import('../../protocol/app-server/v77').MethodResult>(resolve => { release = resolve; });
  const subject = cfg3Client(async operation => { if (operation.method === 'configuration/sourceWrite') return heldWrite; });
  const host = cfg3Host(subject); subject.host = host;
  render(<SettingsSurface client={subject.client} target={workspaceSettingsTarget('A', 'A')} host={host} />);
  await settingsReady(); await openSettingsPage('Tools & Permissions');
  fireEvent.click(screen.getByLabelText('read'));
  fireEvent.click(screen.getByRole('button', { name: 'Save Built-in tools' }));
  await waitFor(() => expect(subject.request.mock.calls.filter(([op]) => op.method === 'configuration/sourceWrite')).toHaveLength(1));
  // A newer editing intent exists before the first mutation is acknowledged.
  fireEvent.click(screen.getByLabelText('write'));
  subject.source.workspace!.revision = 'saved-2';
  release({type:'sources_written',revision:'saved-2',configuration_revision:'1'});
  await waitFor(() => expect(Object.values(settingsContext().units).some(unit => unit.getSnapshot().context.base === 'saved-2')).toBe(true));
  // The newer draft survives the old acknowledgement; only the submitted
  // mutation is retired, and the newer intent's base advanced to the commit.
  expect((screen.getByLabelText('read') as HTMLInputElement).checked).toBe(true);
  expect((screen.getByLabelText('write') as HTMLInputElement).checked).toBe(true);
  expect(screen.queryByRole('button', { name: 'Use latest setting' })).toBeNull();
});

it('keeps a confirmed Workspace save when the post-write authoritative reread fails', async () => {
  let failReads = false;
  const subject = cfg3Client(async operation => { if (operation.method === 'configuration/sourcesRead' && failReads) throw new Error('reread unavailable'); });
  const host = cfg3Host(subject); subject.host = host;
  render(<SettingsSurface client={subject.client} target={workspaceSettingsTarget('A', 'A')} host={host} />);
  await settingsReady(); await openSettingsPage('Tools & Permissions');
  fireEvent.click(screen.getByLabelText('read'));
  failReads = true;
  fireEvent.click(screen.getByRole('button', { name: 'Save Built-in tools' }));
  // The commit is reported as saved, and the failed reread is a separate fact.
  await waitForSettingsSave();
  await screen.findByText(/Saved, but the authoritative reread failed/);
  expect(subject.request.mock.calls.filter(([op]) => op.method === 'configuration/sourceWrite')).toHaveLength(1);
  // A failed reread never turns the confirmed write back into an unsaved draft:
  // the submitted intent was retired with its confirmed commit, so there is no
  // pending Save and no review conflict against a stale base.
  expect((screen.getByRole('button', { name: 'Save Built-in tools' }) as HTMLButtonElement).disabled).toBe(true);
  expect(screen.queryByRole('button', { name: 'Use latest setting' })).toBeNull();
});

it('authors read_image through the native selection, rereads it, and excludes fixed policy editing', async () => {
  const subject = cfg3Client(async (operation, source) => {
    if (operation.method === 'configuration/sourceWrite' && operation.params.mutation.kind === 'config') {
      const mutation = operation.params.mutation.mutation;
      if (mutation.unit === 'native_tools') source.workspace!.authored = { agent: { tools: { builtin: mutation.authored } } };
    }
  });
  subject.source.workspace!.authored = { agent: { tools: { builtin: [] } } };
  await open(subject, 'Tools & Permissions');
  const selected = () => (screen.getByLabelText('read_image') as HTMLInputElement).checked;
  expect(selected()).toBe(false);
  expect(screen.queryByRole('form', { name: 'read_image policy' })).toBeNull();
  fireEvent.click(screen.getByLabelText('read_image'));
  fireEvent.click(screen.getByRole('button', { name: 'Save Built-in tools' }));
  await waitFor(() => expect(subject.request.mock.calls.find(([op]) => op.method === 'configuration/sourceWrite')?.[0]).toMatchObject({ params: { mutation: { kind: 'config', mutation: { unit: 'native_tools', authored: ['read_image'] } } } }));
  await waitFor(() => expect(subject.source.workspace!.revision).toBe('saved-2'));
  fireEvent.click(screen.getByRole('button', { name: 'Reload configuration' }));
  await waitFor(() => expect(selected()).toBe(true));
  fireEvent.click(screen.getByLabelText('read_image'));
  fireEvent.click(screen.getByRole('button', { name: 'Save Built-in tools' }));
  await waitFor(() => expect(subject.request.mock.calls.filter(([op]) => op.method === 'configuration/sourceWrite').at(-1)?.[0]).toMatchObject({ params: { mutation: { mutation: { unit: 'native_tools', authored: [] } } } }));
  fireEvent.click(screen.getByRole('button', { name: 'Reload configuration' }));
  await waitFor(() => expect(selected()).toBe(false));
});

// @vitest-environment jsdom
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';
import { afterEach, expect, it } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import type { CapabilityInspection1, Model, Request1 } from '../../protocol/app-server/v18';
import { settingsTransactionOwners } from '../src/app/settings/Settings';
import { userSettingsTarget, workspaceSettingsTarget } from '../src/app/settings/projection';
import { OutcomeUncertain } from '../src/client/app-server';
import {
  chooseOption, confirmAction, openResourceRow, openSettingsPage, settingsReady, SettingsSurface,
} from './settings-harness';
import { cfg3Client, cfg3Host } from './cfg3-fixture';
afterEach(cleanup);

// #392 product workflows. Every ordering below is established by an explicit
// deferred promise or by the fixture's synchronous native store; nothing waits
// on a timer.

type Subject = ReturnType<typeof cfg3Client>;
type Write = Extract<Request1, { method: 'configuration/sourceWrite' }>;
const writes = (s: Subject) => s.request.mock.calls.filter(([op]) => op.method === 'configuration/sourceWrite').map(([op]) => op as Write);
const methods = (s: Subject) => s.request.mock.calls.map(([op]) => op.method);
const retained = (s: Subject) => JSON.stringify(settingsTransactionOwners(s.client).map(owner => owner.retainedState()));
const forms = () => screen.queryAllByRole('form').map(form => form.getAttribute('aria-label'));

function deferred<T>() {
  let resolve!: (value: T) => void, reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

const fullModel = (): Model => ({
  provider: 'transport', id: 'wire', protocol: 'openai_responses', context_window: '128000', max_output_tokens: 8192,
  capabilities: { input_modalities: ['text'], output_modalities: ['text'], tool_calls: true, reasoning: true },
  reasoning: { default_profile: 'deep', profiles: { deep: { enabled: true, request_params: { effort: 'high' } } } },
  compat: { responses_storage: 'stateless' },
});

async function user(s: Subject, page?: string) {
  render(<SettingsSurface client={s.client} target={userSettingsTarget} />);
  await settingsReady();
  if (page) await openSettingsPage(page);
}
async function workspace(s: Subject, page?: string) {
  render(<SettingsSurface client={s.client} target={workspaceSettingsTarget('A', 'A')} host={cfg3Host(s)} />);
  await settingsReady();
  if (page) await openSettingsPage(page);
}

// ── S2-02 Provider → Provider detail → Model detail ─────────────────────────

it('S2-02 an edit reached through Provider → Model drill-down replaces the complete typed Model', async () => {
  const s = cfg3Client();
  s.source.user.authored!.models = { main: fullModel() };
  await user(s, 'Models');
  await openResourceRow('transport');
  expect(screen.getByRole('heading', { name: 'Provider transport' })).toBeTruthy();
  const served = within(screen.getByRole('grid', { name: 'Models of Provider transport' }));
  fireEvent.click(served.getByRole('row', { name: 'main' }));
  expect(screen.getByRole('button', { name: '← Provider transport' })).toBeTruthy();
  fireEvent.change(screen.getByLabelText('Context window'), { target: { value: '256000' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save Model main' }));
  await waitFor(() => expect(writes(s)).toHaveLength(1));
  // The one field the user changed, and every other typed field unchanged —
  // reasoning profiles and compatibility included.
  expect(writes(s)[0].params.mutation).toEqual({ kind: 'config', mutation: { unit: 'model', id: 'main', authored: { ...fullModel(), context_window: '256000' } } });
});

it('S2-02 a Model added from its Provider names that Provider and is authored as one complete unit', async () => {
  const s = cfg3Client();
  await user(s, 'Models');
  await openResourceRow('transport');
  fireEvent.change(screen.getByLabelText('New Model identity'), { target: { value: 'second' } });
  fireEvent.click(screen.getByRole('button', { name: 'Add Model to transport' }));
  expect((screen.getByLabelText('Provider identity') as HTMLInputElement).value).toBe('transport');
  fireEvent.change(screen.getByLabelText('Wire model identity'), { target: { value: 'wire-2' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save Model second' }));
  await waitFor(() => expect(writes(s)).toHaveLength(1));
  expect(writes(s)[0].params).toMatchObject({ expected_revision: 'user-1', mutation: { kind: 'config', mutation: {
    unit: 'model', id: 'second', authored: { provider: 'transport', id: 'wire-2', protocol: 'openai_responses', context_window: '128000', max_output_tokens: 8192 },
  } } });
});

it('S2-02 deleting a Model removes exactly that authored unit after confirmation', async () => {
  const s = cfg3Client();
  s.source.user.authored!.models = { main: fullModel() };
  await user(s, 'Models');
  fireEvent.click(screen.getByRole('button', { name: /^All Models/ }));
  await openResourceRow('main');
  await confirmAction('Remove Model main');
  await waitFor(() => expect(writes(s)).toHaveLength(1));
  expect(writes(s)[0].params.mutation).toEqual({ kind: 'config', mutation: { unit: 'model', id: 'main', authored: null } });
});

// ── S2-03 Default model and one primary editor per setting ──────────────────

it('S2-03 saving the default model writes the root_model source unit and sends no Session mutation', async () => {
  const s = cfg3Client();
  s.source.user.authored!.models = { main: fullModel() };
  await user(s, 'Models');
  const before = methods(s).length;
  const form = screen.getByRole('form', { name: 'Default model' });
  await chooseOption('Model', 'main', within(form));
  fireEvent.click(within(form).getByRole('button', { name: 'Save Default model' }));
  await waitFor(() => expect(writes(s)).toHaveLength(1));
  expect(writes(s)[0].params).toMatchObject({ target: { kind: 'user' }, mutation: { kind: 'config', mutation: { unit: 'root_model', authored: { model: 'main' } } } });
  await screen.findByText(/Default model saved/);
  // Only source reads and the one source write: no Session model selection,
  // adoption or effective-configuration request is ever issued.
  expect(methods(s).slice(before).every(method => method === 'configuration/sourceWrite' || method === 'configuration/sourcesRead')).toBe(true);
  expect(methods(s).some(method => method.startsWith('session/'))).toBe(false);
});

it('S2-03 each setting has exactly one primary editor across the six pages', async () => {
  const s = cfg3Client();
  await user(s);
  const seen = new Map<string, string>();
  for (const page of ['Models', 'Agent', 'Tools & Permissions', 'Extensions', 'Advanced']) {
    await openSettingsPage(page);
    if (page === 'Extensions') fireEvent.click(screen.getByRole('tab', { name: 'Native' }));
    for (const form of forms()) {
      expect(seen.get(form!), `${form} is edited on both ${seen.get(form!)} and ${page}`).toBeUndefined();
      seen.set(form!, page);
    }
  }
  expect(seen.get('Default model')).toBe('Models');
  expect(seen.get('Root instructions')).toBe('Agent');
  expect(seen.get('Native Tools')).toBe('Tools & Permissions');
  expect(seen.get('Todo extension')).toBe('Extensions');
  expect(seen.get('Context policy')).toBe('Advanced');
});

// ── S2-04 Agent guidance and Tool permissions follow tasks ──────────────────

it('S2-04 Agent holds identity and guidance; Tools & Permissions holds what the Agent may do', async () => {
  const s = cfg3Client();
  await user(s, 'Agent');
  expect(forms()).toEqual(['Root identity', 'Root description', 'Root instructions', 'Project guidance']);
  await openSettingsPage('Tools & Permissions');
  expect(forms()).toEqual(expect.arrayContaining(['Approval mode', 'Native Tools', 'Skill visibility', 'Agent allowlist', 'Workflow allowlist']));
  // Per-Tool policies are one task deeper, not scattered primary tabs.
  expect(screen.getByRole('button', { name: 'Advanced Tool policies' }).getAttribute('aria-expanded')).toBe('false');
  // The old native-unit tabs do not exist as navigation.
  for (const obsolete of ['Tool Policies', 'Skill access', 'Agents & Workflows', 'Plugins', 'Default model']) {
    expect(screen.queryByRole('tab', { name: obsolete })).toBeNull();
  }
});

// ── S2-05 Extensions keep native facts distinct ─────────────────────────────

function inventory(): CapabilityInspection1 {
  return {
    definitions: [
      { family: 'mcp', name: 'search', valid: true, location: { scope: 'user', path: '/home/user/rustx/.agents/mcp.toml' } },
      { family: 'mcp', name: 'broken', valid: false, location: { scope: 'user', path: '/home/user/rustx/.agents/mcp.toml' } },
      { family: 'skill', name: 'docs', valid: true, location: { scope: 'user', path: '/home/user/rustx/.agents/skills/docs' } },
      { family: 'workflow', name: 'nightly', valid: true, location: { scope: 'user', path: '/home/user/rustx/.agents/workflows/nightly' } },
      { family: 'managed_python', name: 'analysis', valid: true, location: { scope: 'user', path: '/home/user/rustx/.agents/python/analysis' } },
    ],
    resource_diagnostics: [{ identity: 'broken', file: '/home/user/rustx/.agents/mcp.toml', reason: 'missing command' }],
    main: {
      identity: 'main', tools: [], skills: [{ name: 'docs' }], agents: [], workflows: [], plugins: [], diagnostics: [],
      tool_selection: [{ source_id: 'search', origin: 'all' }],
    },
    agents: {}, workflows: { nightly: { status: 'disabled', diagnostics: [] } },
    sources: { search: { status: 'ready' }, broken: { status: 'unprepared' }, 'python:analysis': { status: 'unavailable' } },
    skills: [], skill_diagnostics: [],
  } as never;
}

it('S2-05 every extension row reports kind, scope, validity, preparation and root selection as separate facts', async () => {
  const s = cfg3Client();
  s.source.prospective_resources = inventory();
  await user(s, 'Extensions');
  const facts = (name: string) => within(screen.getByRole('row', { name })).getAllByText(/./).map(node => node.textContent);
  expect(facts('search')).toEqual(expect.arrayContaining(['MCP', 'User', 'Valid definition', 'Prepared', 'Allowed for the root Agent']));
  // Invalid, unprepared and unselected are three facts, not one "broken" state.
  expect(facts('broken')).toEqual(expect.arrayContaining(['MCP', 'Invalid definition', 'Not prepared', 'Not allowed for the root Agent', 'missing command']));
  expect(facts('docs')).toEqual(expect.arrayContaining(['Skill', 'Valid definition', 'Visible to the root Agent']));
  // A valid Workflow can still be refused admission, and is still not selected.
  expect(facts('nightly')).toEqual(expect.arrayContaining(['Workflow', 'Valid definition', 'Not admitted', 'Not allowed for the root Agent']));
  expect(facts('analysis')).toEqual(expect.arrayContaining(['Managed Python', 'Valid definition', 'Preparation unavailable']));
  // Opening the page probed nothing.
  expect(methods(s).every(method => method === 'configuration/sourcesRead')).toBe(true);
});

// ── S2-06 Definition and selection stay independent mutations ───────────────

async function mcpDetail(s: Subject) {
  s.source.user_mcp!.authored = { search: { definition: { type: 'stdio', command: 'search-server', args: [] }, retained_env: [], retained_headers: [] } };
  s.source.prospective_resources = inventory();
  await user(s, 'Extensions');
  fireEvent.click(screen.getByRole('tab', { name: 'MCP' }));
  await openResourceRow('search');
  fireEvent.change(screen.getByLabelText('MCP command'), { target: { value: 'search-server-2' } });
  const selection = screen.getByRole('form', { name: 'Source search' });
  await chooseOption('Selection', 'All', within(selection));
}

it('S2-06 an uncertain definition save never submits the root selection on its behalf', async () => {
  const held = deferred<never>();
  held.promise.catch(() => {});
  const s = cfg3Client(async op => { if (op.method === 'configuration/sourceWrite' && op.params.mutation.kind === 'mcp') return held.promise; });
  await mcpDetail(s);
  fireEvent.click(screen.getByRole('button', { name: 'Save MCP search' }));
  await waitFor(() => expect(writes(s)).toHaveLength(1));
  await act(async () => { held.reject(new OutcomeUncertain()); await held.promise.catch(() => {}); });
  await screen.findByText(/Save outcome uncertain/);
  // The selection is still only a draft of its own: no automatic follow-up
  // write, no aggregate success and no replay of the uncertain definition.
  const selection = within(screen.getByRole('form', { name: 'Source search' }));
  await waitFor(() => expect((selection.getByRole('button', { name: 'Save Source search' }) as HTMLButtonElement).disabled).toBe(false));
  expect(writes(s)).toHaveLength(1);
  expect(screen.queryByText(/Source search saved/)).toBeNull();
  // Granting access is its own explicit mutation with its own outcome.
  fireEvent.click(selection.getByRole('button', { name: 'Save Source search' }));
  await waitFor(() => expect(writes(s)).toHaveLength(2));
  expect(writes(s)[1].params.mutation).toEqual({ kind: 'config', mutation: { unit: 'source_tools', id: 'search', authored: 'all' } });
  expect(writes(s).filter(op => op.params.mutation.kind === 'mcp')).toHaveLength(1);
});

it('S2-06 a rejected definition and a saved selection are reported as two outcomes, never one', async () => {
  const s = cfg3Client(async op => {
    if (op.method === 'configuration/sourceWrite' && op.params.mutation.kind === 'mcp') throw new Error('native rejected the MCP definition');
  });
  await mcpDetail(s);
  fireEvent.click(screen.getByRole('button', { name: 'Save MCP search' }));
  await waitFor(() => expect(writes(s)).toHaveLength(1));
  await screen.findByText(/native rejected the MCP definition/);
  // The rejected definition keeps its draft.
  expect((screen.getByLabelText('MCP command') as HTMLInputElement).value).toBe('search-server-2');
  fireEvent.click(screen.getByRole('button', { name: 'Save Source search' }));
  await screen.findByText(/Source search saved/);
  // The selection's success is not the definition's: the definition is still
  // unsaved, still holds its draft and can still be submitted on its own.
  expect(screen.queryByText(/MCP search saved/)).toBeNull();
  expect((screen.getByLabelText('MCP command') as HTMLInputElement).value).toBe('search-server-2');
  expect((screen.getByRole('button', { name: 'Save MCP search' }) as HTMLButtonElement).disabled).toBe(false);
  expect(writes(s).map(op => op.params.mutation.kind)).toEqual(['mcp', 'config']);
});

// ── S2-07 Workspace constrained surface ─────────────────────────────────────

it('S2-07 Workspace Settings offers only Workspace overrides, with inheritance removal instead of deletion', async () => {
  const s = cfg3Client();
  s.source.workspace!.authored = { providers: { transport: { base_url: 'https://workspace.invalid', credential: { type: 'environment', variable: 'WS_KEY' } } } };
  await workspace(s);
  expect(screen.getAllByRole('tab', { selected: false }).concat(screen.getAllByRole('tab', { selected: true }))
    .filter(tab => tab.closest('[aria-label="Settings pages"]')).map(tab => tab.textContent).sort())
    .toEqual(['Advanced', 'Agent', 'Extensions', 'Models', 'Tools & Permissions']);
  await openResourceRow('transport');
  // The Workspace override is removed to inherit again; it is never "deleted".
  expect(screen.queryByRole('button', { name: 'Remove Provider transport' })).toBeNull();
  const trigger = screen.getByRole('button', { name: 'Use global default Provider transport' });
  expect(trigger.closest('[data-removal]')!.getAttribute('data-removal')).toBe('override-removal');
  fireEvent.click(trigger);
  expect(within(await screen.findByRole('alertdialog')).getByText('Use the global default for Provider transport?')).toBeTruthy();
  fireEvent.click(within(screen.getByRole('alertdialog')).getByRole('button', { name: 'Use global default Provider transport' }));
  await waitFor(() => expect(writes(s)).toHaveLength(1));
  expect(writes(s)[0].params).toMatchObject({ target: { kind: 'workspace', directory: '/workspace/A' }, mutation: { kind: 'config', mutation: { unit: 'provider', id: 'transport', authored: null } } });
  // User-only process policy and the client-owned Connection are not here.
  await openSettingsPage('Advanced');
  expect(screen.queryByRole('form', { name: 'App Server policy' })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Connection' })).toBeNull();
  expect(methods(s).some(method => method.startsWith('session/'))).toBe(false);
});

// ── S2-08 No fictitious authoring ───────────────────────────────────────────

it.each([
  ['Workflows', 'nightly', 'Workflow allowlist'],
  ['Skills', 'docs', 'Skill visibility'],
  ['Python', 'analysis', 'Source python:analysis'],
] as const)('S2-08 %s offer inventory and root selection but no definition mutation', async (filter, name, selectionForm) => {
  const s = cfg3Client();
  s.source.prospective_resources = inventory();
  await user(s, 'Extensions');
  fireEvent.click(screen.getByRole('tab', { name: filter }));
  expect(screen.queryByRole('button', { name: /^Add / })).toBeNull();
  expect(screen.getByText(/This protocol has no operation that authors a .* definition/)).toBeTruthy();
  await openResourceRow(name);
  // The only form is the supported root selection; no Edit, Save or Delete is
  // offered for the definition itself.
  expect(forms()).toEqual([selectionForm]);
  const selection = screen.getByRole('form', { name: selectionForm });
  const definitionActions = screen.queryAllByRole('button', { name: /^(Edit|Remove|Delete|Save|Author|Override) / })
    .filter(button => !selection.contains(button));
  expect(definitionActions).toEqual([]);
  expect(writes(s)).toHaveLength(0);
});

it('S2-08 the supported Workflow selection of an unauthorable resource saves through its real unit', async () => {
  const s = cfg3Client();
  s.source.prospective_resources = inventory();
  await user(s, 'Extensions');
  fireEvent.click(screen.getByRole('tab', { name: 'Workflows' }));
  await openResourceRow('nightly');
  fireEvent.click(screen.getByLabelText('Include nightly'));
  fireEvent.click(screen.getByRole('button', { name: 'Save Workflow allowlist' }));
  await waitFor(() => expect(writes(s)).toHaveLength(1));
  expect(writes(s)[0].params.mutation).toEqual({ kind: 'config', mutation: { unit: 'workflows', authored: ['nightly'] } });
});

// ── S2-09 Drafts survive navigation, revisions and native errors ────────────

it('S2-09 a dirty Provider draft survives list/detail, page, filter and revision changes and a native rejection', async () => {
  let reject = false;
  const s = cfg3Client(async op => { if (op.method === 'configuration/sourceWrite' && reject) throw new Error('native rejected the Provider'); });
  await user(s, 'Models');
  await openResourceRow('transport');
  const endpoint = () => screen.getByLabelText('Endpoint') as HTMLInputElement;
  fireEvent.change(endpoint(), { target: { value: 'https://draft.invalid' } });
  // The draft lives in the actor-owned transaction, not in the form library.
  expect(retained(s)).toContain('https://draft.invalid');
  // Detail → list → detail.
  fireEvent.click(screen.getByRole('button', { name: '← Models' }));
  await openResourceRow('transport');
  expect(endpoint().value).toBe('https://draft.invalid');
  // Another page, a filter change there, and back: Models restores its focus.
  await openSettingsPage('Extensions');
  fireEvent.click(screen.getByRole('tab', { name: 'MCP' }));
  await openSettingsPage('Models');
  expect(screen.getByRole('heading', { name: 'Provider transport' })).toBeTruthy();
  expect(endpoint().value).toBe('https://draft.invalid');
  // An external revision change: the draft and its original CAS base survive,
  // and replacing against the newer revision is an explicit review gesture.
  s.source.user.revision = 'user-external';
  fireEvent.click(screen.getByRole('button', { name: 'Reload configuration' }));
  await screen.findByRole('button', { name: 'Use reviewed revision' });
  expect(endpoint().value).toBe('https://draft.invalid');
  fireEvent.click(screen.getByRole('button', { name: 'Use reviewed revision' }));
  // A native rejection keeps the draft too.
  reject = true;
  fireEvent.click(screen.getByRole('button', { name: 'Save Provider transport' }));
  await screen.findByText(/native rejected the Provider/);
  expect(endpoint().value).toBe('https://draft.invalid');
  expect(writes(s)).toHaveLength(1);
  expect(writes(s)[0].params.expected_revision).toBe('user-external');
});

// ── S2-10 Deletion confirmation ─────────────────────────────────────────────

it('S2-10 cancelling a deletion writes nothing and returns focus; confirming performs exactly one removal', async () => {
  const s = cfg3Client();
  await user(s, 'Models');
  await openResourceRow('transport');
  const trigger = screen.getByRole('button', { name: 'Remove Provider transport' });
  expect(trigger.closest('[data-removal]')!.getAttribute('data-removal')).toBe('authored-removal');
  // A real press focuses its target first; the dialog restores focus to it.
  trigger.focus();
  fireEvent.click(trigger);
  const dialog = await screen.findByRole('alertdialog');
  // The dialog states the actual removal: this User definition, nothing else.
  expect(within(dialog).getByText('Remove Provider transport from User configuration?')).toBeTruthy();
  expect(within(dialog).getByText(/Models that name this Provider identity are not changed/)).toBeTruthy();
  fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));
  await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull());
  expect(writes(s)).toHaveLength(0);
  await waitFor(() => expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Remove Provider transport' })));
  await confirmAction('Remove Provider transport');
  await waitFor(() => expect(writes(s)).toHaveLength(1));
  expect(writes(s)[0].params.mutation).toEqual({ kind: 'config', mutation: { unit: 'provider', id: 'transport', authored: null } });
});

// ── S2-11 Diagnostics stay on Advanced ──────────────────────────────────────

it('S2-11 source paths, revisions and raw projections are on Advanced only', async () => {
  const s = cfg3Client();
  await user(s);
  const text = () => screen.getByRole('region', { name: 'Settings' }).textContent!;
  for (const page of ['General', 'Models', 'Agent', 'Tools & Permissions', 'Extensions']) {
    await openSettingsPage(page);
    expect(text(), page).not.toContain('/bound/rustx.toml');
    expect(text(), page).not.toMatch(/Revision: user-1/);
    expect(screen.queryByLabelText('Source and application projection'), page).toBeNull();
    // Protocol vocabulary is not needed for ordinary tasks.
    expect(text(), page).not.toMatch(/\b(root_model|native_tools|source_tools|expected_revision|sourceWrite)\b/);
    // The per-unit CAS base is a collapsed disclosure, never page content.
    for (const disclosure of screen.queryAllByRole('button', { name: 'Source revision & replacement' })) {
      expect(disclosure.getAttribute('aria-expanded')).toBe('false');
    }
  }
  await openSettingsPage('Advanced');
  expect(screen.getByText(/\/bound\/rustx\.toml · Revision: user-1/)).toBeTruthy();
  expect(screen.getByLabelText('Source and application projection')).toBeTruthy();
});

// ── S2-12 Bounded architecture ──────────────────────────────────────────────

const root = join(__dirname, '..');
function sources(dir: string): string[] {
  return readdirSync(dir).flatMap(name => {
    const path = join(dir, name);
    return statSync(path).isDirectory() ? sources(path) : /\.(ts|tsx)$/.test(name) ? [path] : [];
  });
}
const importers = (module: string) => sources(join(root, 'src'))
  .filter(path => new RegExp(`from '${module}'`).test(readFileSync(path, 'utf8')))
  .map(path => relative(root, path)).sort();

it('S2-12 no global state framework, router, second design system or form devtools is a dependency', () => {
  const pkg = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8')) as { dependencies: Record<string, string>; devDependencies: Record<string, string> };
  const names = Object.keys({ ...pkg.dependencies, ...pkg.devDependencies });
  const forbidden = /^(zustand|redux|@reduxjs\/.*|mobx.*|@tanstack\/react-query|@tanstack\/.*devtools.*|@radix-ui\/.*|react-router.*|@tanstack\/react-router|@mui\/.*|antd|zod|valibot|@react-spectrum\/.*|@adobe\/react-spectrum)$/;
  expect(names.filter(name => forbidden.test(name))).toEqual([]);
  expect(pkg.dependencies['react-aria-components']).toBeDefined();
  expect(pkg.dependencies['@tanstack/react-form']).toBeDefined();
});

it('S2-12 React Aria and TanStack Form stay bounded to Settings interaction and form mechanics', () => {
  expect(importers('react-aria-components')).toEqual([
    'src/app/settings/primitives/aria.tsx',
    'src/presentation/settings/SettingsRoot.tsx',
  ]);
  expect(importers('@tanstack/react-form')).toEqual(['src/app/settings/forms/bridge.tsx']);
  // One Settings modal root.
  const modalRoots = sources(join(root, 'src')).filter(path => readFileSync(path, 'utf8').includes('<Modal className={css.panel}>'));
  expect(modalRoots.map(path => relative(root, path))).toEqual(['src/presentation/settings/SettingsRoot.tsx']);
  // Settings keeps no browser storage and no URL state.
  for (const path of sources(join(root, 'src/app/settings'))) {
    expect(readFileSync(path, 'utf8'), relative(root, path)).not.toMatch(/localStorage|sessionStorage|history\.pushState|location\.hash|console\.log/);
  }
});

it('S2-12 no compatibility alias for a removed route survives', () => {
  const settings = sources(join(root, 'src/app/settings')).map(path => readFileSync(path, 'utf8')).join('\n');
  expect(settings).not.toMatch(/'overview'|SettingsSection\b|CatalogEditor|RootEditor|RuntimeEditor|ResourceInventory|Integrations\b/);
});

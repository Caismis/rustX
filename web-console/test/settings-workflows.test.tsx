import { providerAction } from './settings-harness';
import { waitForSettingsSave } from './settings-harness';
import {openResourceModule} from './settings-harness';
import { sourceReceipt } from './fixture';
// @vitest-environment jsdom
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';
import { afterEach, expect, it } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import type { RuntimeClientSnapshot, Model, Request1, SourceSettings } from '../../protocol/app-server/v77';
type CapabilityInspection = NonNullable<NonNullable<RuntimeClientSnapshot['resources']>['inspection']>;
import { settingsTransactionOwners } from '../src/app/settings/Settings';
import { userSettingsTarget, workspaceSettingsTarget } from '../src/app/settings/projection';
import { OutcomeUncertain } from '../src/client/errors';
import {
  chooseOption, confirmAction, openMcpContractEditor, openResourceRow, openSettingsPage, settingsReady, SettingsSurface,
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
  provider: 'transport', id: 'wire', context_window: '128000', max_output_tokens: 8192,
  capabilities: { input_modalities: ['text'], output_modalities: ['text'], json_schema_output: false },
  default_profile: 'deep', profiles: { deep: { request_params: { effort: 'high', unset: null } } },
  compat: { responses_storage: 'stateless' },
});

async function user(s: Subject, page?: string) {
  render(<SettingsSurface client={s.client} target={userSettingsTarget} host={cfg3Host(s)} />);
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
  expect(screen.getByRole('button', { name: 'Close' })).toBeTruthy();
  fireEvent.change(screen.getByLabelText('Context window'), { target: { value: '256000' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save Model main' }));
  await waitFor(() => expect(writes(s)).toHaveLength(1));
  // The one field the user changed, and every other typed field unchanged —
  // Model Profiles and compatibility included.
  expect(writes(s)[0].params.mutation).toEqual({ kind: 'config', mutation: { unit: 'model', id: 'main', authored: { ...fullModel(), context_window: '256000' } } });
});

it('S2-02 a Model added from its Provider names that Provider and is authored as one complete unit', async () => {
  const s = cfg3Client();
  await user(s, 'Models');
  await openResourceRow('transport');
  fireEvent.click(screen.getByRole('button', { name: /^(?:\+ )?Add Model$/ }));
  fireEvent.click(screen.getByRole('button', { name: 'Advanced configuration' }));
  expect(screen.queryByLabelText('Provider identity')).toBeNull();
  fireEvent.change(screen.getByLabelText('Model ID'), { target: { value: 'wire-2' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save Add Model' }));
  await waitFor(() => expect(writes(s)).toHaveLength(1));
  expect(writes(s)[0].params).toMatchObject({ expected_revision: 'user-1', mutation: { kind: 'config', mutation: {
    unit: 'model', id: 'transport/wire-2', authored: { provider: 'transport', id: 'wire-2', context_window: '128000', max_output_tokens: 8192 },
  } } });
});

it('S2-02 deleting a Model removes exactly that authored unit after confirmation', async () => {
  const s = cfg3Client();
  s.source.user.authored!.models = { main: fullModel() };
  await user(s, 'Models');
  await openResourceRow('main');
  const editor = within(screen.getByRole('dialog', { name: 'Model main' }));
  expect(editor.queryByRole('button', { name: 'Remove Model main' })).toBeNull();
  fireEvent.click(editor.getByRole('button', { name: 'Close' }));
  const row = within(screen.getByRole('row', { name: 'main' }));
  fireEvent.click(row.getByRole('button', { name: 'Remove Model main' }));
  fireEvent.click(within(await screen.findByRole('alertdialog')).getByRole('button', { name: 'Cancel' }));
  expect(writes(s)).toHaveLength(0);
  await confirmAction('Remove Model main', row);
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
  await waitForSettingsSave();
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
    if (page === 'Extensions') fireEvent.click(screen.getByRole('tab', { name: 'Built-in' }));
    for (const form of forms()) {
      expect(seen.get(form!), `${form} is edited on both ${seen.get(form!)} and ${page}`).toBeUndefined();
      seen.set(form!, page);
    }
  }
  expect(seen.get('Default model')).toBe('Models');
  expect(seen.get('Instructions')).toBe('Agent');
  expect(seen.get('Built-in tools')).toBe('Tools & Permissions');
  expect(seen.get('Todo extension')).toBe('Extensions');
  expect(seen.get('Context')).toBe('Advanced');
});

// ── S2-04 Agent guidance and Tool permissions follow tasks ──────────────────

it('S2-04 Agent holds identity and guidance; Tools & Permissions holds what the Agent may do', async () => {
  const s = cfg3Client();
  await user(s, 'Agent');
  expect(forms()).toEqual(['Agent name', 'Description', 'Instructions', 'Project guidance']);
  await openSettingsPage('Tools & Permissions');
  expect(forms()).toEqual(['Approval mode', 'Built-in tools']);
  // Per-Tool policies are one task deeper, not scattered primary tabs.
  expect(screen.getByRole('button', { name: 'Advanced Tool policies' }).getAttribute('aria-expanded')).toBe('false');
  // The old native-unit tabs do not exist as navigation.
  for (const obsolete of ['Tool Policies', 'Skill access', 'Agents & Workflows', 'Plugins', 'Default model']) {
    expect(screen.queryByRole('tab', { name: obsolete })).toBeNull();
  }
});

// ── S2-05 Extensions keep native facts distinct ─────────────────────────────

function inventory(): CapabilityInspection {
  return {
    definitions: [
      { family: 'mcp', name: 'search', valid: true, location: { scope: 'user', path: '/home/user/rustx/.agents/mcp.toml' } },
      { family: 'mcp', name: 'broken', valid: false, location: { scope: 'user', path: '/home/user/rustx/.agents/mcp.toml' } },
      { family: 'skill', name: 'docs', valid: true, location: { scope: 'user', path: '/home/user/rustx/.agents/skills/docs' } },
      { family: 'workflow', name: 'nightly', valid: true, location: { scope: 'user', path: '/home/user/rustx/.agents/workflows/nightly' } },
      { family: 'managed_python', name: 'analysis', valid: true, location: { scope: 'user', path: '/home/user/rustx/.agents/python/analysis' } },
    ],
    resource_diagnostics: [{ subject: { kind: 'resource', family: 'mcp', name: 'broken' }, file: '/home/user/rustx/.agents/mcp.toml', field: 'mcp_servers.broken', reason: 'missing command' }],
    main: {
      identity: 'main', tools: [], skills: [{ name: 'docs' }], agents: [], workflows: [], plugins: [], diagnostics: [],
      tool_selection: [{ source_id: 'search', origin: 'all' }],
    },
    agents: {}, workflows: { nightly: { status: 'disabled', diagnostics: [] } },
    sources: { search: { status: 'ready' }, broken: { status: 'unprepared' }, 'python:analysis': { status: 'unavailable' } },
    skills: [], skill_diagnostics: [],
  } as never;
}

it('S2-05 resource details show actual errors without healthy inspection metadata', async () => {
  const s = cfg3Client();
  s.source.prospective_resources = inventory();
  await user(s, 'Extensions');
  const facts = async (name: string) => {
    if (['search','broken'].includes(name) || name.endsWith('-mcp')) await openResourceModule('mcp');
    else if(name==='docs') await openResourceModule('skills');
    else {await openSettingsPage('Extensions');fireEvent.click(screen.getByRole('tab',{name:'Python'}));}
    await openResourceRow(name);
    const values = screen.getAllByText(/./).map(node => node.textContent);
    fireEvent.click(screen.getByRole('button', {name:name==='docs'?'Close':/^(← (Extensions|MCP servers)|Back to list)$/}));
    return values;
  };
  expect(await facts('search')).toContain('Edit MCP server');
  expect(await facts('search')).not.toContain('Valid definition');
  // Invalid, unprepared and unselected are three facts, not one "broken" state.
  expect(await facts('broken')).toEqual(expect.arrayContaining(['Edit MCP server', 'missing command']));
  // `search` is defined in the same document, and the diagnostic is not its.
  expect(await facts('search')).not.toContain('missing command');
  expect(await facts('docs')).toEqual(expect.arrayContaining(['docs']));
  expect(await facts('analysis')).toEqual(expect.arrayContaining(['analysis', 'This resource is unavailable. Check its configuration and dependencies.']));
  // Disabled MCP entries only read status or release an existing settings connection.
  expect(methods(s).every(method => ['configuration/sourcesRead','skills/list','skills/read'].includes(method))).toBe(true);
});

/** Two MCP identities authored in one `mcp.toml`, one valid and one not. */
function siblingInventory(): CapabilityInspection {
  const mcp = '/home/user/rustx/.agents/mcp.toml';
  return {
    definitions: [
      { family: 'mcp', name: 'search', valid: true, location: { scope: 'user', path: mcp } },
      { family: 'mcp', name: 'broken', valid: false, location: { scope: 'user', path: mcp } },
    ],
    resource_diagnostics: [
      // `broken`'s own diagnostic. Its file is the document `search` is also
      // defined in, and its field names `search` — neither is an attribution.
      { subject: { kind: 'resource', family: 'mcp', name: 'broken' }, file: mcp, field: 'search', reason: 'broken: missing command' },
      // A failure of the document as a whole, which no identity owns.
      { subject: { kind: 'collection', family: 'mcp' }, file: mcp, field: 'mcp_servers', reason: 'MCP catalog exceeds 128 definitions' },
    ],
    main: null, agents: {}, workflows: {}, sources: {}, skills: [], skill_diagnostics: [],
  } as never;
}

it('S2-05 a resource diagnostic stays on the identity native attributes it to, never on a same-file sibling', async () => {
  const s = cfg3Client();
  s.source.prospective_resources = siblingInventory();
  await user(s, 'Extensions');
  await openResourceModule('mcp');
  const row = (name: string) => within(screen.getByRole('listitem', { name }));
  expect(row('broken').getByText('broken: missing command')).toBeTruthy();
  expect(row('search').queryByText('broken: missing command')).toBeNull();
  // Collection diagnostics are omitted; the actual server error stays on its identity.
  expect(screen.queryByRole('region', { name: 'Source diagnostics' })).toBeNull();
  expect(screen.queryByText(/MCP catalog exceeds/)).toBeNull();
  // The detail of each identity agrees with its row.
  await openResourceRow('search');
  const search = within(screen.getByRole('region', { name: 'MCP search' }));
  expect(search.queryByText(/missing command|MCP catalog exceeds/)).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: /^(← (Extensions|MCP servers)|Back to list)$/ }));
  await openResourceRow('broken');
  const broken = within(screen.getByRole('region', { name: 'MCP broken' }));
  expect(broken.getByText('broken: missing command')).toBeTruthy();
  expect(broken.queryByText(/MCP catalog exceeds/)).toBeNull();
});

/** Every native preparation and admission status, next to one identity of each
 * observed family native published no observation for. */
function observationInventory(): CapabilityInspection {
  const at = (path: string) => ({ scope: 'user' as const, path: `/home/user/rustx/.agents/${path}` });
  return {
    definitions: [
      { family: 'mcp', name: 'ready-mcp', valid: true, location: at('mcp.toml') },
      { family: 'mcp', name: 'idle-mcp', valid: true, location: at('mcp.toml') },
      { family: 'mcp', name: 'down-mcp', valid: true, location: at('mcp.toml') },
      { family: 'mcp', name: 'unseen-mcp', valid: true, location: at('mcp.toml') },
      { family: 'managed_python', name: 'analysis', valid: true, location: at('tools/analysis') },
      { family: 'managed_python', name: 'unseen-python', valid: true, location: at('tools/unseen-python') },
      { family: 'workflow', name: 'admitted', valid: true, location: at('workflows/admitted.yaml') },
      { family: 'workflow', name: 'refused', valid: true, location: at('workflows/refused.yaml') },
      { family: 'workflow', name: 'unseen-workflow', valid: true, location: at('workflows/unseen-workflow.yaml') },
    ],
    resource_diagnostics: [], main: null, agents: {}, skills: [], skill_diagnostics: [],
    sources: { 'ready-mcp': { status: 'ready' }, 'idle-mcp': { status: 'unprepared' }, 'down-mcp': { status: 'unavailable' }, 'python:analysis': { status: 'ready' } },
    workflows: { admitted: { status: 'enabled' }, refused: { status: 'disabled', diagnostics: [] } },
  } as never;
}

it('S2-05 an identity native published no preparation or admission for is unobserved, never a negative status', async () => {
  const s = cfg3Client();
  s.source.prospective_resources = observationInventory();
  await user(s, 'Extensions');
  const facts = async (name: string) => {
    if (['search','broken'].includes(name) || name.endsWith('-mcp')) await openResourceModule('mcp');
    else if(name==='docs') await openResourceModule('skills');
    else {await openSettingsPage('Extensions');fireEvent.click(screen.getByRole('tab',{name:'Python'}));}
    await openResourceRow(name);
    const values = screen.getAllByText(/./).map(node => node.textContent);
    fireEvent.click(screen.getByRole('button', {name:name==='docs'?'Close':/^(← (Extensions|MCP servers)|Back to list)$/}));
    return values;
  };
  const negative = ['Not prepared', 'Preparation unavailable', 'Not admitted', 'Prepared', 'Admitted'];
  // Native's own statuses, exactly.
  expect(await facts('ready-mcp')).not.toContain('Prepared');
  expect(await facts('idle-mcp')).not.toContain('Not prepared');
  expect(await facts('down-mcp')).not.toContain('Preparation unavailable');
  expect(await facts('analysis')).not.toContain('Prepared');
  // No observation: unknown, and no status native did not publish.
  for (const [name, label] of [['unseen-mcp', 'Preparation not observed'], ['unseen-python', 'Preparation not observed']]) {
    const observed = await facts(name);
    expect(observed).not.toContain(label);
    for (const status of negative) expect(observed).not.toContain(status);
  }

});

it('S2-05 collection and Skill discovery diagnostics stay out of resource pages', async () => {
  const s = cfg3Client();
  const resources = inventory();
  resources.resource_diagnostics.push(
    { subject: { kind: 'collection', family: 'agent' }, field: 'agents', reason: 'Agent source collection diagnostic' },
    { subject: { kind: 'collection', family: 'managed_python' }, field: 'tools', reason: 'Python source collection diagnostic' },
  );
  resources.skill_diagnostics = [
    { kind: 'source_root_missing', source: 'workspace', root: '/workspace/.agents/skills' },
  ];
  s.source.prospective_resources = resources;
  await user(s, 'Extensions');
  fireEvent.click(screen.getByRole('tab', { name: 'Python' }));
  expect(screen.getByRole('listitem', { name: 'analysis' })).toBeTruthy();
  expect(screen.queryByText(/Python source collection diagnostic/)).toBeNull();
  await openResourceModule('subagents');
  expect(screen.getByRole('region', { name: 'Subagents' })).toBeTruthy();
  expect(screen.queryByText(/Agent source collection diagnostic/)).toBeNull();
  await openResourceModule('skills');
  expect(screen.getByRole('listitem', { name: 'docs' })).toBeTruthy();
  expect(screen.queryByRole('region', { name: 'Source diagnostics' })).toBeNull();
  expect(screen.queryByRole('button', { name: /Skill discovery diagnostics/ })).toBeNull();
});

// ── S2-06 Definition and selection stay independent mutations ───────────────

async function mcpDetail(s: Subject) {
  s.source.user_mcp!.authored = { search: { definition: { type: 'stdio', command: 'search-server', args: [] }, retained_env: [], retained_headers: [] } };
  s.source.prospective_resources = inventory();
  await user(s, 'Extensions');
  await openResourceModule('mcp');
  await openResourceRow('search');
  fireEvent.change(screen.getByLabelText('Command'), { target: { value: 'search-server-2' } });
  const selection = screen.getByRole('form', { name: 'Tools: search' });
  await chooseOption('Selection', 'All', within(selection));
}

it('S2-06 an uncertain definition save never submits the root selection on its behalf', async () => {
  const held = deferred<never>();
  held.promise.catch(() => {});
  const s = cfg3Client(async op => { if (op.method === 'configuration/sourceWrite' && op.params.mutation.kind === 'mcp') return held.promise; });
  await mcpDetail(s);
  fireEvent.click(screen.getByRole('button', { name: 'Save' }));
  await waitFor(() => expect(writes(s)).toHaveLength(1));
  await act(async () => { held.reject(new OutcomeUncertain()); await held.promise.catch(() => {}); });
  await screen.findByText(/The save result is unknown/);
  // The selection is still only a draft of its own: no automatic follow-up
  // write, no aggregate success and no replay of the uncertain definition.
  const selection = within(screen.getByRole('form', { name: 'Tools: search' }));
  await waitFor(() => expect((selection.getByRole('button', { name: 'Save Tools: search' }) as HTMLButtonElement).disabled).toBe(false));
  expect(writes(s)).toHaveLength(1);
  expect(screen.queryByText(/Tools: search saved/)).toBeNull();
  // Granting access is its own explicit mutation with its own outcome.
  fireEvent.click(selection.getByRole('button', { name: 'Save Tools: search' }));
  await waitFor(() => expect(writes(s)).toHaveLength(2));
  expect(writes(s)[1].params.mutation).toEqual({ kind: 'config', mutation: { unit: 'source_tools', id: 'search', authored: 'all' } });
  expect(writes(s).filter(op => op.params.mutation.kind === 'mcp')).toHaveLength(1);
});

it('S2-06 a rejected definition and a saved selection are reported as two outcomes, never one', async () => {
  const s = cfg3Client(async op => {
    if (op.method === 'configuration/sourceWrite' && op.params.mutation.kind === 'mcp') throw new Error('native rejected the MCP definition');
  });
  await mcpDetail(s);
  fireEvent.click(screen.getByRole('button', { name: 'Save' }));
  await waitFor(() => expect(writes(s)).toHaveLength(1));
  await screen.findByText(/native rejected the MCP definition/);
  // The rejected definition keeps its draft.
  expect((screen.getByLabelText('Command') as HTMLInputElement).value).toBe('search-server-2');
  fireEvent.click(screen.getByRole('button', { name: 'Save Tools: search' }));
  await waitForSettingsSave();
  // The selection's success is not the definition's: the definition is still
  // unsaved, still holds its draft and can still be submitted on its own.
  expect(screen.queryByText(/MCP search saved/)).toBeNull();
  expect((screen.getByLabelText('Command') as HTMLInputElement).value).toBe('search-server-2');
  expect((screen.getByRole('button', { name: 'Save' }) as HTMLButtonElement).disabled).toBe(false);
  expect(writes(s).map(op => op.params.mutation.kind)).toEqual(['mcp', 'config']);
});

// ── S2-07 Workspace constrained surface ─────────────────────────────────────

it('S2-07 Workspace Settings offers only Workspace overrides, with inheritance removal instead of deletion', async () => {
  const s = cfg3Client();
  s.source.workspace!.authored = { providers: { transport: { protocol: 'openai_responses', base_url: 'https://workspace.invalid', credential: { type: 'environment', variable: 'WS_KEY' } } } };
  await workspace(s);
  expect(screen.getAllByRole('tab', { selected: false }).concat(screen.getAllByRole('tab', { selected: true }))
    .filter(tab => tab.closest('[aria-label="Settings pages"]')).map(tab => tab.textContent).sort())
    .toEqual(['Advanced', 'Agent', 'Extensions', 'Models', 'Tools & Permissions']);
  await openResourceRow('transport');
  // The Workspace override is removed to inherit again; it is never "deleted".
  expect(screen.queryByRole('button', { name: 'Remove Provider transport' })).toBeNull();
  await providerAction('Use global default');
  const restore = await screen.findByRole('dialog', { name: 'Use the global default for Provider transport?' });
  expect(screen.queryByRole('alertdialog')).toBeNull();
  fireEvent.click(within(restore).getByRole('button', { name: 'Use global default Provider transport' }));
  await waitFor(() => expect(writes(s)).toHaveLength(1));
  expect(writes(s)[0].params).toMatchObject({ target: { kind: 'workspace', directory: '/workspace/A' }, mutation: { kind: 'config', mutation: { unit: 'provider', id: 'transport', authored: null } } });
  // User-only process policy and the client-owned Connection are not here.
  await openSettingsPage('Advanced');
  expect(screen.queryByRole('form', { name: 'App Server policy' })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Connection' })).toBeNull();
  expect(methods(s).some(method => method.startsWith('session/'))).toBe(false);
});

// ── S2-07 Viewing an inherited resource is not Workspace authoring ──────────

/** A literal the User MCP definition holds. Native never projects it; this
 * fixture puts it on the wire anyway, so the browser's own refusal to copy an
 * inherited secret is what the assertions below observe. */
const INHERITED_SECRET = 'inherited-user-literal-value';

/** A Workspace that inherits one User MCP definition and one User named Agent,
 * over a native store that commits each Workspace definition mutation exactly
 * as native projects it. */
function inheritingWorkspace() {
  const s = cfg3Client(async (op, source) => {
    if (op.method !== 'configuration/sourceWrite' || op.params.target.kind !== 'workspace') return;
    const mutation = op.params.mutation;
    if (mutation.kind === 'mcp') {
      const authored = { ...source.workspace_mcp!.authored };
      if (mutation.authored) {
        const { env, headers, ...definition } = mutation.authored.definition;
        authored[mutation.id] = { definition, retained_env: Object.keys(env ?? {}), retained_headers: Object.keys(headers ?? {}) };
      } else delete authored[mutation.id];
      source.workspace_mcp = { ...source.workspace_mcp!, revision: `${source.workspace_mcp!.revision}+`, authored };
    }
    if (mutation.kind === 'agent') {
      source.agents = source.agents.filter(agent => !(agent.scope === 'workspace' && agent.name === mutation.name));
      if (mutation.authored) source.agents.push({ scope: 'workspace', name: mutation.name, source: { path: `/workspace/.agents/agents/${mutation.name}.toml`, revision: 'agent-ws-1', authored: mutation.authored } });
    }
  });
  s.source.user_mcp!.authored = { search: {
    definition: { type: 'stdio', command: 'search-server', args: ['--index'], sensitive_env: { API_TOKEN: '$SEARCH_TOKEN' }, env: { LITERAL_TOKEN: INHERITED_SECRET } },
    retained_env: ['LITERAL_TOKEN'], retained_headers: [],
  } } as never;
  s.source.agents = [{ scope: 'user', name: 'reviewer', source: { path: '/home/user/rustx/.agents/agents/reviewer.toml', revision: 'agent-user-1', authored: { description: 'User reviewer', instructions: 'Review carefully.' } } }];
  s.source.prospective_resources = {
    definitions: [
      { family: 'mcp', name: 'search', valid: true, location: { scope: 'user', path: '/home/user/rustx/.agents/mcp.toml' } },
      { family: 'agent', name: 'reviewer', valid: true, location: { scope: 'user', path: '/home/user/rustx/.agents/agents/reviewer.toml' } },
    ],
    resource_diagnostics: [], agents: {}, workflows: {}, sources: {}, skills: [], skill_diagnostics: [],
  } as never;
  return s;
}
const definition = (title: string) => screen.getByRole('form', { name: title });
const definitionState = (title: string) => definition(title).getAttribute('data-definition');
const field = (label: string) => screen.getByLabelText(label) as HTMLInputElement;
const disabled = (element: HTMLElement) => element.matches(':disabled');

it('S2-07 an inherited MCP definition is inspected read-only; only Override begins a Workspace draft, and Save writes it once', async () => {
  const s = inheritingWorkspace();
  await workspace(s, 'Extensions');
  await openResourceModule('mcp');
  await openResourceRow('search');
  // Inspecting: the inherited safe facts are shown, nothing is writable, no
  // Workspace draft exists and nothing is written.
  expect(definitionState('Edit MCP server')).toBe('inherited');
  expect(within(definition('Edit MCP server')).getByText('Using global settings.')).toBeTruthy();
  expect(field('Command').value).toBe('search-server');
  expect(disabled(field('Command'))).toBe(true);
  expect((screen.getByRole('button', { name: 'Save' }) as HTMLButtonElement).disabled).toBe(true);
  expect(screen.queryByRole('button', { name: /Use global default MCP search|Remove MCP search/ })).toBeNull();
  // Even an input that reaches the field anyway authors nothing.
  fireEvent.change(field('Command'), { target: { value: 'sneaked-edit' } });
  await waitFor(() => expect(field('Command').value).toBe('search-server'));
  expect(retained(s)).not.toContain('search-server');
  expect(retained(s)).not.toContain('sneaked-edit');
  // The withheld literal is named, never shown or retained.
  expect(within(definition('Edit MCP server')).getByText(/Re-enter these credentials.*LITERAL_TOKEN/)).toBeTruthy();
  expect(document.body.innerHTML).not.toContain(INHERITED_SECRET);
  expect(writes(s)).toHaveLength(0);

  // The explicit transition: a Workspace draft seeded from the inherited
  // definition, still unwritten, carrying no inherited secret.
  fireEvent.click(screen.getByRole('button', { name: 'Override' }));
  expect(definitionState('Edit MCP server')).toBe('overriding');
  expect(disabled(field('Command'))).toBe(false);
  expect(retained(s)).toContain('search-server');
  expect(retained(s)).not.toContain(INHERITED_SECRET);
  expect(retained(s)).not.toContain('LITERAL_TOKEN');
  expect(writes(s)).toHaveLength(0);

  // Edit, leave the page and come back: the transaction owner kept the draft.
  fireEvent.change(field('Command'), { target: { value: 'search-workspace' } });
  await openSettingsPage('Models');
  await openResourceModule('mcp');
  expect(field('Command').value).toBe('search-workspace');
  expect(definitionState('Edit MCP server')).toBe('overriding');
  // Root availability is a second, independent draft.
  await chooseOption('Selection', 'All', within(screen.getByRole('form', { name: 'Tools: search' })));

  fireEvent.click(screen.getByRole('button', { name: 'Save' }));
  await waitFor(() => expect(writes(s)).toHaveLength(1));
  expect(writes(s)[0].params).toEqual({
    target: { kind: 'workspace', directory: '/workspace/A' }, expected_revision: 'mcp-2',
    mutation: { kind: 'mcp', id: 'search', authored: {
      definition: { type: 'stdio', command: 'search-workspace', args: ['--index'], sensitive_env: { API_TOKEN: '$SEARCH_TOKEN' } },
      retained_env: [], retained_headers: [],
    } },
  });
  expect(JSON.stringify(writes(s))).not.toContain(INHERITED_SECRET);
  await screen.findByRole('listitem',{name:'search'});
  await openResourceRow('search');
  expect(definitionState('Edit MCP server')).toBe('authored');
  // The definition save did not submit the root selection on its behalf.
  expect(writes(s)).toHaveLength(1);
  await waitFor(() => expect((screen.getByRole('button', { name: 'Save Tools: search' }) as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(screen.getByRole('button', { name: 'Save Tools: search' }));
  await waitFor(() => expect(writes(s)).toHaveLength(2));
  expect(writes(s)[1].params.mutation).toEqual({ kind: 'config', mutation: { unit: 'source_tools', id: 'search', authored: 'all' } });

  // Removing the Workspace override is inheritance, not deletion: exactly one
  // Workspace removal, and the User definition is inspected again.
  await waitFor(() => expect((screen.getByRole('button', { name: 'Use global default MCP search' }) as HTMLButtonElement).disabled).toBe(false));
  expect(screen.getByRole('button', { name: 'Use global default MCP search' }).closest('[data-removal]')!.getAttribute('data-removal')).toBe('override-removal');
  await confirmAction('Use global default MCP search');
  await waitFor(() => expect(writes(s)).toHaveLength(3));
  expect(writes(s)[2].params).toMatchObject({ target: { kind: 'workspace', directory: '/workspace/A' }, mutation: { kind: 'mcp', id: 'search', authored: null } });
  await screen.findByRole('listitem',{name:'search'});
  await openResourceRow('search');
  expect(definitionState('Edit MCP server')).toBe('inherited');
  expect(s.source.user_mcp!.authored!.search).toBeTruthy();
  expect(field('Command').value).toBe('search-server');
  expect(writes(s).filter(op => op.params.target.kind === 'user')).toHaveLength(0);
});

it('S2-07 discarding a Workspace MCP override writes nothing and returns to inspection', async () => {
  const s = inheritingWorkspace();
  await workspace(s, 'Extensions');
  await openResourceModule('mcp');
  await openResourceRow('search');
  fireEvent.click(screen.getByRole('button', { name: 'Override' }));
  fireEvent.change(field('Command'), { target: { value: 'abandoned' } });
  fireEvent.click(screen.getByRole('button', { name: 'Discard' }));
  await waitFor(() => expect(definitionState('Edit MCP server')).toBe('inherited'));
  expect(field('Command').value).toBe('search-server');
  expect(disabled(field('Command'))).toBe(true);
  expect(retained(s)).not.toContain('abandoned');
  expect(writes(s)).toHaveLength(0);
});

it('S2-07 an inherited named Agent is inspected read-only; Override, Save and Use global default are each one explicit Workspace mutation', async () => {
  const s = inheritingWorkspace();
  await workspace(s, 'Extensions');
  await openResourceModule('subagents');
  await openResourceRow('reviewer');
  expect(definitionState('Agent reviewer')).toBe('inherited');
  expect(field('Description').value).toBe('User reviewer');
  expect(disabled(field('Description'))).toBe(true);
  fireEvent.change(field('Description'), { target: { value: 'sneaked-edit' } });
  await waitFor(() => expect(field('Description').value).toBe('User reviewer'));
  expect(retained(s)).not.toContain('User reviewer');
  expect(writes(s)).toHaveLength(0);

  fireEvent.click(screen.getByRole('button', { name: 'Override Agent reviewer in this Workspace' }));
  expect(definitionState('Agent reviewer')).toBe('overriding');
  expect(retained(s)).toContain('User reviewer');
  expect(writes(s)).toHaveLength(0);
  fireEvent.change(field('Description'), { target: { value: 'Workspace reviewer' } });
  fireEvent.click(screen.getByRole('button', { name: /^(← (Extensions|MCP servers)|Back to list)$/ }));
  await openResourceRow('reviewer');
  expect(field('Description').value).toBe('Workspace reviewer');

  fireEvent.click(screen.getByRole('button', { name: 'Save Agent reviewer' }));
  await waitFor(() => expect(writes(s)).toHaveLength(1));
  expect(writes(s)[0].params).toEqual({
    target: { kind: 'workspace', directory: '/workspace/A' }, expected_revision: 'missing',
    mutation: { kind: 'agent', name: 'reviewer', authored: { description: 'Workspace reviewer', instructions: 'Review carefully.' } },
  });
  await waitFor(() => expect(definitionState('Agent reviewer')).toBe('authored'));
  // The root delegation allowlist is a separate unit that nothing wrote.
  expect(writes(s).some(op => op.params.mutation.kind === 'config')).toBe(false);

  await waitFor(() => expect((screen.getByRole('button', { name: 'Use global default Agent reviewer' }) as HTMLButtonElement).disabled).toBe(false));
  await confirmAction('Use global default Agent reviewer');
  await waitFor(() => expect(writes(s)).toHaveLength(2));
  expect(writes(s)[1].params).toMatchObject({ target: { kind: 'workspace', directory: '/workspace/A' }, mutation: { kind: 'agent', name: 'reviewer', authored: null } });
  await waitFor(() => expect(definitionState('Agent reviewer')).toBe('inherited'));
  expect(s.source.agents.filter(agent => agent.scope === 'user').map(agent => agent.name)).toEqual(['reviewer']);
});

it('S2-07 a new Workspace resource and a Workspace-only definition are worded as creation and deletion, never inheritance', async () => {
  const s = inheritingWorkspace();
  s.source.workspace_mcp!.authored = { local: { definition: { type: 'stdio', command: 'local-server', args: [] }, retained_env: [], retained_headers: [] } };
  await workspace(s, 'Extensions');
  await openResourceModule('mcp');
  fireEvent.click(screen.getAllByRole('button', { name: /^(＋ )?New$/ })[0]);
  fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'fresh' } });
  openMcpContractEditor();
  // Creation is its own explicit gesture: writable at once, no override action.
  expect(definitionState('New MCP server')).toBe('new');
  expect(disabled(field('Command'))).toBe(false);
  expect(screen.queryByRole('button', { name: /^Override MCP fresh/ })).toBeNull();
  expect(writes(s)).toHaveLength(0);
  fireEvent.click(screen.getByRole('button', { name: /^(← (Extensions|MCP servers)|Back to list)$/ }));
  await openResourceRow('local');
  // A Workspace definition that shadows nothing is removed, not "inherited again".
  expect(definitionState('Edit MCP server')).toBe('authored');
  expect(screen.queryByRole('button', { name: 'Use global default MCP local' })).toBeNull();
  expect(screen.getByRole('button', { name: 'Remove MCP local' }).closest('[data-removal]')!.getAttribute('data-removal')).toBe('authored-removal');
});

// ── S2-07 A definition that does not parse is still authored ────────────────

const USER_REVIEWER = '/home/user/rustx/.agents/agents/reviewer.toml';
const WORKSPACE_REVIEWER = '/workspace/.agents/agents/reviewer.toml';
const INVALID_AGENT = 'invalid named Agent definition';

/** Named-Agent files in both scopes over a native store that projects them
 * exactly as native does: a file that does not parse keeps its path and its
 * real revision and carries a diagnostic instead of an authored value. The
 * resource inventory is resolved from the files the way native resolves them —
 * the Workspace file wins, parsed or not, and shadows the same-name User file. */
function agentFiles(files: { user?: 'valid' | 'malformed'; workspace?: 'valid' | 'malformed' }) {
  type Scope = 'user' | 'workspace';
  const path = (scope: Scope) => scope === 'user' ? USER_REVIEWER : WORKSPACE_REVIEWER;
  let commits = 0;
  const resolve = (source: SourceSettings) => {
    const file = (scope: Scope) => source.agents.find(agent => agent.scope === scope && agent.name === 'reviewer');
    const user = file('user'), workspace = file('workspace'), winner = workspace ?? user;
    source.prospective_resources = {
      definitions: winner ? [{ family: 'agent', name: 'reviewer', valid: !!winner.source.authored, location: {
        scope: winner.scope, path: winner.source.path, ...(workspace && user ? { shadowed: user.source.path } : {}),
      } }] : [],
      resource_diagnostics: winner && !winner.source.authored
        ? [{ subject: { kind: 'resource', family: 'agent', name: 'reviewer' }, file: winner.source.path, field: 'agents.reviewer', reason: 'reviewer: expected a table' }] : [],
      agents: {}, workflows: {}, sources: {}, skills: [], skill_diagnostics: [],
    } as never;
  };
  const s = cfg3Client(async (op, source) => {
    if (op.method !== 'configuration/sourceWrite' || op.params.mutation.kind !== 'agent') return;
    const { target: { kind: scope }, mutation } = op.params;
    source.agents = source.agents.filter(agent => !(agent.scope === scope && agent.name === mutation.name));
    if (mutation.authored) source.agents.push({ scope, name: mutation.name, source: { path: path(scope), revision: `agent-${scope}-${++commits}`, authored: mutation.authored } });
    resolve(s.source);
  });
  s.source.agents = (['user', 'workspace'] as const).flatMap(scope => {
    const state = files[scope];
    if (!state) return [];
    return [{ scope, name: 'reviewer', source: state === 'valid'
      ? { path: path(scope), revision: `agent-${scope}-valid`, authored: { description: `${scope === 'user' ? 'User' : 'Workspace'} reviewer`, instructions: 'Review carefully.' } }
      : { path: path(scope), revision: `agent-${scope}-bad`, diagnostic: INVALID_AGENT } }];
  });
  resolve(s.source);
  return s;
}
async function openReviewer(s: Subject, scope: 'user' | 'workspace') {
  await (scope === 'user' ? user : workspace)(s, 'Extensions');
  await openResourceModule('subagents');
  await openResourceRow('reviewer');
}
const authoredValue = (title: string) => definition(title).getAttribute('data-authored-value');
const enabled = (name: string) => !(screen.getByRole('button', { name }) as HTMLButtonElement).disabled;

it('S2-07 a User named-Agent file that does not parse is an existing invalid User definition, replaced on its real revision', async () => {
  const s = agentFiles({ user: 'malformed' });
  await openReviewer(s, 'user');
  // Present in the source, without a parsed value: authored, never new.
  expect(definitionState('Agent reviewer')).toBe('authored');
  expect(authoredValue('Agent reviewer')).toBe('unparsed');
  expect(screen.queryByText(/New User definition/)).toBeNull();
  expect(within(definition('Agent reviewer')).getByText(/This configuration could not be read/)).toBeTruthy();
  expect(screen.getAllByText(INVALID_AGENT).length).toBeGreaterThan(0);
  // Its removal is the real removal of the User file, available at once.
  expect(screen.getByRole('button', { name: 'Remove Agent reviewer' }).closest('[data-removal]')!.getAttribute('data-removal')).toBe('authored-removal');
  expect(enabled('Remove Agent reviewer')).toBe(true);
  // Opening it authored nothing.
  expect(field('Description').value).toBe('');
  expect(disabled(field('Description'))).toBe(false);
  expect(enabled('Save Agent reviewer')).toBe(false);
  expect(retained(s)).not.toContain('agent-user-bad');
  expect(writes(s)).toHaveLength(0);

  // Replacement is an ordinary unit edit, pinned on the malformed file's own
  // revision, never on the absent-identity token.
  fireEvent.change(field('Description'), { target: { value: 'Repaired reviewer' } });
  expect(retained(s)).toContain('"base":"agent-user-bad"');
  fireEvent.click(screen.getByRole('button', { name: 'Save Agent reviewer' }));
  await waitFor(() => expect(writes(s)).toHaveLength(1));
  expect(writes(s)[0].params).toEqual({
    target: { kind: 'user' }, expected_revision: 'agent-user-bad',
    mutation: { kind: 'agent', name: 'reviewer', authored: { description: 'Repaired reviewer' } },
  });
  // The authoritative reread settles the same unit with the parsed value.
  await waitFor(() => expect(authoredValue('Agent reviewer')).toBe('parsed'));
  expect(definitionState('Agent reviewer')).toBe('authored');
  expect(field('Description').value).toBe('Repaired reviewer');
  expect(writes(s)).toHaveLength(1);
});

it('S2-07 removing a malformed User named Agent deletes exactly that file through its real revision', async () => {
  const s = agentFiles({ user: 'malformed' });
  await openReviewer(s, 'user');
  await confirmAction('Remove Agent reviewer');
  await waitFor(() => expect(writes(s)).toHaveLength(1));
  expect(writes(s)[0].params).toEqual({
    target: { kind: 'user' }, expected_revision: 'agent-user-bad',
    mutation: { kind: 'agent', name: 'reviewer', authored: null },
  });
  // After the authoritative reread the identity is absent in this source.
  await waitFor(() => expect(definitionState('Agent reviewer')).toBe('new'));
  expect(s.source.agents).toEqual([]);
  expect(writes(s)).toHaveLength(1);
});

it('S2-07 a malformed Workspace named Agent is the Workspace definition shadowing the User one, replaced on its own revision', async () => {
  const s = agentFiles({ user: 'valid', workspace: 'malformed' });
  await openReviewer(s, 'workspace');
  // The Workspace file is authored and invalid, and it shadows the User file.
  expect(definitionState('Agent reviewer')).toBe('authored');
  expect(authoredValue('Agent reviewer')).toBe('unparsed');
  expect(within(definition('Agent reviewer')).getByText(/This configuration could not be read/)).toBeTruthy();
  expect(screen.getAllByText(INVALID_AGENT).length).toBeGreaterThan(0);
  expect(screen.queryByText(/Inherited from User/)).toBeNull();
  // Nothing is locked behind an Override transition, and nothing is seeded
  // from the User definition: the Workspace one already exists.
  expect(screen.queryByRole('button', { name: /^Override Agent reviewer/ })).toBeNull();
  expect(disabled(field('Description'))).toBe(false);
  expect(field('Description').value).toBe('');
  expect(retained(s)).not.toContain('User reviewer');
  expect(writes(s)).toHaveLength(0);
  // Removing it restores the User definition, so it is worded as inheritance.
  expect(screen.queryByRole('button', { name: 'Remove Agent reviewer' })).toBeNull();
  expect(screen.getByRole('button', { name: 'Use global default Agent reviewer' }).closest('[data-removal]')!.getAttribute('data-removal')).toBe('override-removal');

  // The replacement draft is the transaction actor's: it survives leaving the
  // detail and the page, and navigation writes nothing.
  fireEvent.change(field('Description'), { target: { value: 'Workspace reviewer' } });
  expect(retained(s)).toContain('"base":"agent-workspace-bad"');
  await openSettingsPage('Models');
  await openResourceModule('subagents');
  expect(field('Description').value).toBe('Workspace reviewer');
  expect(writes(s)).toHaveLength(0);

  fireEvent.click(screen.getByRole('button', { name: 'Save Agent reviewer' }));
  await waitFor(() => expect(writes(s)).toHaveLength(1));
  expect(writes(s)[0].params).toEqual({
    target: { kind: 'workspace', directory: '/workspace/A' }, expected_revision: 'agent-workspace-bad',
    mutation: { kind: 'agent', name: 'reviewer', authored: { description: 'Workspace reviewer' } },
  });
  await waitFor(() => expect(authoredValue('Agent reviewer')).toBe('parsed'));
  expect(screen.queryByText('Invalid definition')).toBeNull();
  expect(writes(s).filter(op => op.params.target.kind === 'user')).toHaveLength(0);
  expect(writes(s)).toHaveLength(1);
});

it('S2-07 using the global default in place of a malformed Workspace named Agent removes it on its revision and the User one takes effect', async () => {
  const s = agentFiles({ user: 'valid', workspace: 'malformed' });
  await openReviewer(s, 'workspace');
  await confirmAction('Use global default Agent reviewer');
  await waitFor(() => expect(writes(s)).toHaveLength(1));
  expect(writes(s)[0].params).toEqual({
    target: { kind: 'workspace', directory: '/workspace/A' }, expected_revision: 'agent-workspace-bad',
    mutation: { kind: 'agent', name: 'reviewer', authored: null },
  });
  // The authoritative reread shows the User definition in effect again,
  // inspected read-only, with no Workspace draft created by the transition.
  await waitFor(() => expect(definitionState('Agent reviewer')).toBe('inherited'));
  expect(screen.getByText('Using global settings.')).toBeTruthy();
  expect(field('Description').value).toBe('User reviewer');
  expect(disabled(field('Description'))).toBe(true);
  expect(s.source.agents.map(agent => agent.scope)).toEqual(['user']);
  expect(writes(s)).toHaveLength(1);
});

it('S2-07 a malformed Workspace-only named Agent is authored by the Workspace and removed as a real deletion', async () => {
  const s = agentFiles({ workspace: 'malformed' });
  await openReviewer(s, 'workspace');
  expect(definitionState('Agent reviewer')).toBe('authored');
  expect(authoredValue('Agent reviewer')).toBe('unparsed');
  expect(within(definition('Agent reviewer')).getByText(/This configuration could not be read/)).toBeTruthy();
  expect(screen.queryByRole('button', { name: 'Use global default Agent reviewer' })).toBeNull();
  expect(screen.getByRole('button', { name: 'Remove Agent reviewer' }).closest('[data-removal]')!.getAttribute('data-removal')).toBe('authored-removal');
  await confirmAction('Remove Agent reviewer');
  await waitFor(() => expect(writes(s)).toHaveLength(1));
  expect(writes(s)[0].params).toEqual({
    target: { kind: 'workspace', directory: '/workspace/A' }, expected_revision: 'agent-workspace-bad',
    mutation: { kind: 'agent', name: 'reviewer', authored: null },
  });
  await waitFor(() => expect(definitionState('Agent reviewer')).toBe('new'));
  expect(s.source.agents).toEqual([]);
});

// ── S2-08 No fictitious authoring ───────────────────────────────────────────

it('S2-08 Skills offer read-only instructions and no definition mutation',async()=>{
  const s=cfg3Client();s.source.prospective_resources=inventory();
  await user(s);await openResourceModule('skills');await openResourceRow('docs');
  expect(screen.getByRole('dialog',{name:'docs'})).toBeTruthy();
  expect(forms()).toEqual([]);
  expect(screen.queryByRole('textbox')).toBeNull();
  expect(screen.queryByRole('button',{name:/^(Edit|Remove|Delete|Save)/})).toBeNull();
  expect(writes(s)).toHaveLength(0);
});
it('S2-08 Python offers inventory and root selection but no definition mutation',async()=>{
  const s=cfg3Client();s.source.prospective_resources=inventory();await user(s,'Extensions');
  fireEvent.click(screen.getByRole('tab',{name:'Python'}));await openResourceRow('analysis');
  expect(forms()).toEqual(['Tools: python:analysis']);
  const selection=screen.getByRole('form',{name:'Tools: python:analysis'});
  expect(screen.queryAllByRole('button',{name:/^(Edit|Remove|Delete|Save|Author|Override) /}).filter(button=>!selection.contains(button))).toEqual([]);
  expect(writes(s)).toHaveLength(0);
});
it('S2-08 Workflows are no longer listed in Settings',async()=>{
  const s=cfg3Client();s.source.prospective_resources=inventory();await user(s,'Extensions');
  expect(screen.queryByRole('tab',{name:'Workflows'})).toBeNull();
  expect(screen.queryByText('nightly')).toBeNull();expect(writes(s)).toHaveLength(0);
});

// ── S2-09 Drafts survive navigation, revisions and native errors ────────────

it('S2-09 an incomplete Provider draft survives navigation and needs review after an external edit', async () => {
  let reject = false;
  const s = cfg3Client(async op => { if (op.method === 'configuration/sourceWrite' && reject) throw new Error('native rejected the Provider'); });
  await user(s, 'Models');
  await openResourceRow('transport');
  const endpoint = () => screen.getByLabelText('Base URL') as HTMLInputElement;
  fireEvent.change(endpoint(), { target: { value: 'incomplete' } });
  // The draft lives in the actor-owned transaction, not in the form library.
  expect(retained(s)).toContain('incomplete');
  // Detail → list → detail.
  await openResourceRow('transport');
  expect(endpoint().value).toBe('incomplete');
  // Another page, a filter change there, and back: Models restores its focus.
  await openSettingsPage('Extensions');
  await openResourceModule('mcp');
  await openSettingsPage('Models');
  expect(screen.getByRole('heading', { name: 'Provider transport' })).toBeTruthy();
  expect(endpoint().value).toBe('incomplete');
  // An external revision change: the draft and its original CAS base survive,
  // and replacing against the newer revision is an explicit review gesture.
  s.source.user.revision = 'user-external';
  fireEvent.click(screen.getByRole('button', { name: 'Reload configuration' }));
  const review = await screen.findByRole('button', { name: 'Apply my changes' });
  expect(endpoint().value).toBe('incomplete');
  fireEvent.change(endpoint(), { target: { value: 'https://draft.invalid' } });
  reject = true;
  fireEvent.click(review);
  await screen.findByText(/native rejected the Provider/);
  expect(endpoint().value).toBe('https://draft.invalid');
  expect(writes(s)).toHaveLength(1);
  expect(writes(s)[0].params.expected_revision).toBe('user-external');
});

it('S2-09 a Provider autosave waits for the committed Model reread and uses its revision', async () => {
  // Native applies each write and acknowledges it at a new revision; the read
  // after a commit is held until the test releases it.
  let revision = 1;
  let reread: ReturnType<typeof deferred<void>> | undefined;
  const s = cfg3Client(async (op, source) => {
    if (op.method === 'configuration/sourceWrite') {
      const { mutation } = op.params.mutation as Extract<Write['params']['mutation'], { kind: 'config' }>;
      if (mutation.unit === 'model') source.user.authored!.models![mutation.id] = mutation.authored!;
      if (mutation.unit === 'provider') source.user.authored!.providers![mutation.id].base_url = mutation.authored!.base_url;
      source.user.revision = `user-${++revision}`;
      reread = deferred<void>();
      return {type:'sources_written',revision:sourceReceipt(structuredClone(source)).revision,configuration_revision:null};
    }
    if (op.method === 'configuration/sourcesRead' && reread) await reread.promise;
  });
  s.source.user.authored!.models = { main: fullModel() };
  s.source.user.revision = 'user-1';
  await user(s, 'Models');
  await openResourceRow('transport');
  fireEvent.click(within(screen.getByRole('grid', { name: 'Models of Provider transport' })).getByRole('row', { name: 'main' }));
  const wire = () => screen.getByLabelText('Model ID') as HTMLInputElement;
  const contextWindow = () => screen.getByLabelText('Context window') as HTMLInputElement;
  const save = (name: string) => screen.getByRole('button', { name: `Save ${name}` }) as HTMLButtonElement;
  // D1 changes one field of the whole Model and commits at user-2.
  fireEvent.change(wire(), { target: { value: 'wire-new' } });
  fireEvent.click(save('Model main'));
  await waitFor(() => expect(writes(s)).toHaveLength(1));
  expect(writes(s)[0].params.expected_revision).toBe('user-1');
  expect(writes(s)[0].params.mutation).toEqual({ kind: 'config', mutation: { unit: 'model', id: 'main', authored: { ...fullModel(), id: 'wire-new' } } });
  await waitFor(() => expect(save('Model main').getAttribute('aria-busy')).toBe('true'));
  // The post-commit read is outstanding: the presentation still holds the
  // pre-commit Model, and it is no base for a new draft of the same unit.
  expect(wire().value).toBe('wire');
  expect(wire().closest('fieldset')!.disabled).toBe(true);
  expect(contextWindow().closest('fieldset')!.disabled).toBe(true);
  fireEvent.change(contextWindow(), { target: { value: '256000' } });
  expect(contextWindow().value).toBe('128000');
  expect(retained(s)).not.toContain('256000');
  expect(save('Model main').disabled).toBe(true);
  expect(writes(s)).toHaveLength(1);
  // Another unit stays editable in the same window, but cannot submit.
  fireEvent.click(screen.getByRole('button', { name: 'Close' }));
  fireEvent.change(screen.getByLabelText('Base URL'), { target: { value: 'https://draft.invalid' } });
  fireEvent.keyDown(screen.getByLabelText('Base URL'), { key: 'Enter' });
  expect(retained(s)).toContain('https://draft.invalid');
  expect(writes(s)).toHaveLength(1);
  // The authoritative observation of the committed Model releases both.
  await act(async () => reread!.resolve());
  await waitFor(() => expect(writes(s)).toHaveLength(2));
  expect(writes(s)[1].params).toMatchObject({ expected_revision: 'user-2', mutation: {
    kind: 'config', mutation: { unit: 'provider', id: 'transport', authored: { base_url: 'https://draft.invalid' } },
  } });
  await act(async () => reread!.resolve());
  await waitForSettingsSave();
  expect(screen.queryByRole('button', { name: 'Apply my changes' })).toBeNull();
  fireEvent.click(within(screen.getByRole('grid', { name: 'Models of Provider transport' })).getByRole('row', { name: 'main' }));
  await waitFor(() => expect(wire().value).toBe('wire-new'));
  expect(contextWindow().closest('fieldset')!.disabled).toBe(false);
  // The next edit is a fresh draft of the observed Model, fenced on its
  // revision, and the complete replacement keeps what D1 committed.
  fireEvent.change(contextWindow(), { target: { value: '256000' } });
  fireEvent.click(save('Model main'));
  await waitFor(() => expect(writes(s)).toHaveLength(3));
  expect(writes(s)[2].params.expected_revision).toBe('user-3');
  expect(writes(s)[2].params.mutation).toEqual({ kind: 'config', mutation: {
    unit: 'model', id: 'main', authored: { ...fullModel(), id: 'wire-new', context_window: '256000' },
  } });
  await act(async () => reread!.resolve());
  await waitForSettingsSave();
  fireEvent.click(screen.getByRole('button', { name: 'Close' }));
  expect((screen.getByLabelText('Base URL') as HTMLInputElement).value).toBe('https://draft.invalid');
  expect(screen.queryByRole('button', { name: 'Apply my changes' })).toBeNull();
  expect(screen.queryByText('Provider transport saved.')).toBeNull();
});

// ── S2-10 Deletion confirmation ─────────────────────────────────────────────

it('S2-10 cancelling a deletion writes nothing and returns focus; confirming performs exactly one removal', async () => {
  const s = cfg3Client();
  await user(s, 'Models');
  await openResourceRow('transport');
  const trigger = screen.getByRole('button', { name: 'Provider actions' });
  trigger.focus();
  await providerAction('Delete');
  const dialog = await screen.findByRole('alertdialog', { name: 'Remove Provider transport from User configuration?' });
  fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));
  await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull());
  expect(writes(s)).toHaveLength(0);
  await waitFor(() => expect(document.activeElement).toBe(trigger));
  await providerAction('Delete');
  fireEvent.click(within(await screen.findByRole('alertdialog')).getByRole('button', { name: 'Remove Provider transport' }));
  await waitFor(() => expect(writes(s)).toHaveLength(1));
  expect(writes(s)[0].params.mutation).toEqual({ kind: 'config', mutation: { unit: 'provider', id: 'transport', authored: null } });
});

// ── S2-11 Diagnostics stay on Advanced ──────────────────────────────────────

it('S2-11 source revisions, process budgets and raw projections are absent from all settings pages', async () => {
  const s = cfg3Client();
  await user(s);
  const text = () => screen.getByRole('region', { name: 'Settings' }).textContent!;
  for (const page of ['General', 'Models', 'Agent', 'Tools & Permissions', 'Extensions', 'Advanced']) {
    await openSettingsPage(page);
    expect(text(), page).not.toContain('/bound/rustx.toml');
    expect(text(), page).not.toMatch(/Revision: user-1/);
    expect(screen.queryByLabelText('Source and application projection'), page).toBeNull();
    // Protocol vocabulary is not needed for ordinary tasks.
    expect(text(), page).not.toMatch(/\b(root_model|native_tools|source_tools|expected_revision|sourceWrite)\b/);
    // The per-unit CAS base is a collapsed disclosure, never page content.
    for (const disclosure of screen.queryAllByRole('button', { name: 'Configuration details' })) {
      expect(disclosure.getAttribute('aria-expanded')).toBe('false');
    }
  }
  await openSettingsPage('Advanced');
  expect(screen.queryByText(/Revision: user-1/)).toBeNull();
  expect(screen.queryByLabelText('Source and application projection')).toBeNull();
  expect(screen.queryByRole('form', { name: 'App Server policy' })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Configuration details' })).toBeNull();
});

// ── S2-12 Bounded architecture ──────────────────────────────────────────────

const root = join(__dirname, '..');
function sources(dir: string): string[] {
  return readdirSync(dir).flatMap(name => {
    const path = join(dir, name);
    return statSync(path).isDirectory() ? sources(path) : /\.(ts|tsx)$/.test(name) ? [path] : [];
  });
}
// Source files do not change during this suite. Read once, especially on bind mounts.
const sourceImports = sources(join(root, 'src')).map(path => ({ path, text: readFileSync(path, 'utf8') }));
const importers = (module: string) => sourceImports
  .filter(source => new RegExp(`from '${module}'`).test(source.text))
  .map(source => relative(root, source.path)).sort();

it('S2-12 no global state framework, router, second design system or form devtools is a dependency', () => {
  const pkg = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8')) as { dependencies: Record<string, string>; devDependencies: Record<string, string> };
  const names = Object.keys({ ...pkg.dependencies, ...pkg.devDependencies });
  const forbidden = /^(zustand|redux|@reduxjs\/.*|mobx.*|@tanstack\/react-query|@tanstack\/.*devtools.*|@radix-ui\/.*|react-router.*|@tanstack\/react-router|@mui\/.*|antd|zod|valibot|@react-spectrum\/.*|@adobe\/react-spectrum)$/;
  expect(names.filter(name => forbidden.test(name))).toEqual([]);
  expect(pkg.dependencies['react-aria-components']).toBeDefined();
  expect(pkg.dependencies['@tanstack/react-form']).toBeDefined();
});

it('S2-12 React Aria and TanStack Form stay bounded to their declared interaction owners', () => {
  expect(importers('react-aria-components')).toEqual([
    'src/app/resources/ResourceSurface.tsx',
    'src/app/scheduler/TimingEditor.tsx',
    'src/app/settings/models/ModelsPage.tsx',
    'src/app/settings/models/ProviderIcon.tsx',
    'src/app/settings/primitives/aria.tsx',
    'src/presentation/settings/SettingsRoot.tsx',
  ]);
  expect(importers('@base-ui/react/dialog')).toEqual(['src/presentation/primitives/DialogSurface.tsx']);
  expect(importers('@base-ui/react/alert-dialog')).toEqual(['src/presentation/primitives/DialogSurface.tsx']);
  expect(importers('@tanstack/react-form')).toEqual(['src/app/settings/forms/bridge.tsx']);
  // One Settings page root; transient operation dialogs have separate owners.
  const pageRoots = sources(join(root, 'src')).filter(path => readFileSync(path, 'utf8').includes('data-settings-page'));
  expect(pageRoots.map(path => relative(root, path))).toEqual(['src/presentation/settings/SettingsRoot.tsx']);
  // Settings keeps no browser storage and no URL state.
  for (const path of sources(join(root, 'src/app/settings'))) {
    expect(readFileSync(path, 'utf8'), relative(root, path)).not.toMatch(/localStorage|sessionStorage|history\.pushState|location\.hash|console\.log/);
  }
});

it('S2-12 no compatibility alias for a removed route survives', () => {
  const settings = sources(join(root, 'src/app/settings')).map(path => readFileSync(path, 'utf8')).join('\n');
  expect(settings).not.toMatch(/'overview'|SettingsSection\b|CatalogEditor|RootEditor|RuntimeEditor|ResourceInventory|Integrations\b/);
});

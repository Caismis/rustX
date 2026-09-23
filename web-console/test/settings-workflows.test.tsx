// @vitest-environment jsdom
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';
import { afterEach, expect, it } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import type { CapabilityInspection1, Model, Request1 } from '../../protocol/app-server/v19';
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

it('S2-05 every extension row reports kind, scope, validity, preparation and root selection as separate facts', async () => {
  const s = cfg3Client();
  s.source.prospective_resources = inventory();
  await user(s, 'Extensions');
  const facts = (name: string) => within(screen.getByRole('row', { name })).getAllByText(/./).map(node => node.textContent);
  expect(facts('search')).toEqual(expect.arrayContaining(['MCP', 'User', 'Valid definition', 'Prepared', 'Allowed for the root Agent']));
  // Invalid, unprepared and unselected are three facts, not one "broken" state.
  expect(facts('broken')).toEqual(expect.arrayContaining(['MCP', 'Invalid definition', 'Not prepared', 'Not allowed for the root Agent', 'missing command']));
  // `search` is defined in the same document, and the diagnostic is not its.
  expect(facts('search')).not.toContain('missing command');
  expect(facts('docs')).toEqual(expect.arrayContaining(['Skill', 'Valid definition', 'Visible to the root Agent']));
  // A valid Workflow can still be refused admission, and is still not selected.
  expect(facts('nightly')).toEqual(expect.arrayContaining(['Workflow', 'Valid definition', 'Not admitted', 'Not allowed for the root Agent']));
  expect(facts('analysis')).toEqual(expect.arrayContaining(['Managed Python', 'Valid definition', 'Preparation unavailable']));
  // Opening the page probed nothing.
  expect(methods(s).every(method => method === 'configuration/sourcesRead')).toBe(true);
});

/** Two MCP identities authored in one `mcp.toml`, one valid and one not. */
function siblingInventory(): CapabilityInspection1 {
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
  fireEvent.click(screen.getByRole('tab', { name: 'MCP' }));
  const row = (name: string) => within(screen.getByRole('row', { name }));
  expect(row('broken').getByText('broken: missing command')).toBeTruthy();
  expect(row('search').queryByText('broken: missing command')).toBeNull();
  // The document's own diagnostic is the MCP family's, listed once, and on
  // neither of the identities the document holds.
  expect(row('broken').queryByText(/MCP catalog exceeds/)).toBeNull();
  expect(row('search').queryByText(/MCP catalog exceeds/)).toBeNull();
  const sources = within(screen.getByRole('region', { name: 'Source diagnostics' }));
  expect(sources.getAllByText(/MCP catalog exceeds 128 definitions/)).toHaveLength(1);
  expect(sources.queryByText(/missing command/)).toBeNull();
  // The detail of each identity agrees with its row.
  await openResourceRow('search');
  const search = within(screen.getByRole('region', { name: 'MCP search' }));
  expect(search.queryByText(/missing command|MCP catalog exceeds/)).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: '← Extensions' }));
  fireEvent.click(await screen.findByRole('tab', { name: 'MCP' }));
  await openResourceRow('broken');
  const broken = within(screen.getByRole('region', { name: 'MCP broken' }));
  expect(broken.getByText('broken: missing command')).toBeTruthy();
  expect(broken.queryByText(/MCP catalog exceeds/)).toBeNull();
});

/** Every native preparation and admission status, next to one identity of each
 * observed family native published no observation for. */
function observationInventory(): CapabilityInspection1 {
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
  const facts = (name: string) => within(screen.getByRole('row', { name })).getAllByText(/./).map(node => node.textContent);
  const negative = ['Not prepared', 'Preparation unavailable', 'Not admitted', 'Prepared', 'Admitted'];
  // Native's own statuses, exactly.
  expect(facts('ready-mcp')).toContain('Prepared');
  expect(facts('idle-mcp')).toContain('Not prepared');
  expect(facts('down-mcp')).toContain('Preparation unavailable');
  expect(facts('analysis')).toContain('Prepared');
  expect(facts('admitted')).toContain('Admitted');
  expect(facts('refused')).toContain('Not admitted');
  // No observation: unknown, and no status native did not publish.
  for (const [name, label] of [['unseen-mcp', 'Preparation not observed'], ['unseen-python', 'Preparation not observed'], ['unseen-workflow', 'Admission not observed']]) {
    expect(facts(name)).toContain(label);
    for (const status of negative) expect(facts(name)).not.toContain(status);
  }
  // The detail names the same fact under the same title.
  fireEvent.click(screen.getByRole('tab', { name: 'Workflows' }));
  await openResourceRow('unseen-workflow');
  const detail = within(screen.getByRole('region', { name: 'Workflow unseen-workflow' }));
  expect(detail.getAllByText('Admission not observed').length).toBeGreaterThan(0);
  expect(detail.queryByText('Not admitted')).toBeNull();
});

it('S2-05 Skill discovery diagnostics are the Skills family\'s, never shown on another Skill\'s detail', async () => {
  const s = cfg3Client();
  s.source.prospective_resources = inventory();
  // A package that failed validation. It is not `docs`, and nothing in the
  // diagnostic names a Skill identity at all.
  s.source.prospective_resources!.skill_diagnostics = [
    { kind: 'package_invalid', source: 'user', package: '/home/user/rustx/.agents/skills/broken', cause: { cause: 'io', path: '/home/user/rustx/.agents/skills/broken/SKILL.md' } },
  ] as never;
  await user(s, 'Extensions');
  fireEvent.click(screen.getByRole('tab', { name: 'Skills' }));
  const sources = within(screen.getByRole('region', { name: 'Source diagnostics' }));
  expect(sources.getByRole('button', { name: 'Skill discovery diagnostics (1)' })).toBeTruthy();
  await openResourceRow('docs');
  expect(screen.queryByRole('region', { name: 'Source diagnostics' })).toBeNull();
  expect(screen.queryByRole('button', { name: /Skill (package|discovery) diagnostics/ })).toBeNull();
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
  fireEvent.click(screen.getByRole('tab', { name: 'MCP' }));
  await openResourceRow('search');
  // Inspecting: the inherited safe facts are shown, nothing is writable, no
  // Workspace draft exists and nothing is written.
  expect(definitionState('MCP search')).toBe('inherited');
  expect(within(definition('MCP search')).getByText(/Inherited from User \(\/home\/user\/rustx\/\.agents\/mcp\.toml\)/)).toBeTruthy();
  expect(field('MCP command').value).toBe('search-server');
  expect(disabled(field('MCP command'))).toBe(true);
  expect((screen.getByRole('button', { name: 'Save MCP search' }) as HTMLButtonElement).disabled).toBe(true);
  expect(screen.queryByRole('button', { name: /Use global default MCP search|Remove MCP search/ })).toBeNull();
  // Even an input that reaches the field anyway authors nothing.
  fireEvent.change(field('MCP command'), { target: { value: 'sneaked-edit' } });
  await waitFor(() => expect(field('MCP command').value).toBe('search-server'));
  expect(retained(s)).not.toContain('search-server');
  expect(retained(s)).not.toContain('sneaked-edit');
  // The withheld literal is named, never shown or retained.
  expect(within(definition('MCP search')).getByText(/literal values for LITERAL_TOKEN/)).toBeTruthy();
  expect(document.body.innerHTML).not.toContain(INHERITED_SECRET);
  expect(writes(s)).toHaveLength(0);

  // The explicit transition: a Workspace draft seeded from the inherited
  // definition, still unwritten, carrying no inherited secret.
  fireEvent.click(screen.getByRole('button', { name: 'Override MCP search in this Workspace' }));
  expect(definitionState('MCP search')).toBe('overriding');
  expect(disabled(field('MCP command'))).toBe(false);
  expect(retained(s)).toContain('search-server');
  expect(retained(s)).not.toContain(INHERITED_SECRET);
  expect(retained(s)).not.toContain('LITERAL_TOKEN');
  expect(writes(s)).toHaveLength(0);

  // Edit, leave the page and come back: the transaction owner kept the draft.
  fireEvent.change(field('MCP command'), { target: { value: 'search-workspace' } });
  await openSettingsPage('Models');
  await openSettingsPage('Extensions');
  expect(field('MCP command').value).toBe('search-workspace');
  expect(definitionState('MCP search')).toBe('overriding');
  // Root availability is a second, independent draft.
  await chooseOption('Selection', 'All', within(screen.getByRole('form', { name: 'Source search' })));

  fireEvent.click(screen.getByRole('button', { name: 'Save MCP search' }));
  await waitFor(() => expect(writes(s)).toHaveLength(1));
  expect(writes(s)[0].params).toEqual({
    target: { kind: 'workspace', directory: '/workspace/A' }, expected_revision: 'mcp-2',
    mutation: { kind: 'mcp', id: 'search', authored: {
      definition: { type: 'stdio', command: 'search-workspace', args: ['--index'], sensitive_env: { API_TOKEN: '$SEARCH_TOKEN' } },
      retained_env: [], retained_headers: [],
    } },
  });
  expect(JSON.stringify(writes(s))).not.toContain(INHERITED_SECRET);
  await waitFor(() => expect(definitionState('MCP search')).toBe('authored'));
  // The definition save did not submit the root selection on its behalf.
  expect(writes(s)).toHaveLength(1);
  await waitFor(() => expect((screen.getByRole('button', { name: 'Save Source search' }) as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(screen.getByRole('button', { name: 'Save Source search' }));
  await waitFor(() => expect(writes(s)).toHaveLength(2));
  expect(writes(s)[1].params.mutation).toEqual({ kind: 'config', mutation: { unit: 'source_tools', id: 'search', authored: 'all' } });

  // Removing the Workspace override is inheritance, not deletion: exactly one
  // Workspace removal, and the User definition is inspected again.
  await waitFor(() => expect((screen.getByRole('button', { name: 'Use global default MCP search' }) as HTMLButtonElement).disabled).toBe(false));
  expect(screen.getByRole('button', { name: 'Use global default MCP search' }).closest('[data-removal]')!.getAttribute('data-removal')).toBe('override-removal');
  await confirmAction('Use global default MCP search');
  await waitFor(() => expect(writes(s)).toHaveLength(3));
  expect(writes(s)[2].params).toMatchObject({ target: { kind: 'workspace', directory: '/workspace/A' }, mutation: { kind: 'mcp', id: 'search', authored: null } });
  await waitFor(() => expect(definitionState('MCP search')).toBe('inherited'));
  expect(s.source.user_mcp!.authored!.search).toBeTruthy();
  expect(field('MCP command').value).toBe('search-server');
  expect(writes(s).filter(op => op.params.target.kind === 'user')).toHaveLength(0);
});

it('S2-07 discarding a Workspace MCP override writes nothing and returns to inspection', async () => {
  const s = inheritingWorkspace();
  await workspace(s, 'Extensions');
  fireEvent.click(screen.getByRole('tab', { name: 'MCP' }));
  await openResourceRow('search');
  fireEvent.click(screen.getByRole('button', { name: 'Override MCP search in this Workspace' }));
  fireEvent.change(field('MCP command'), { target: { value: 'abandoned' } });
  fireEvent.click(screen.getByRole('button', { name: 'Discard draft' }));
  await waitFor(() => expect(definitionState('MCP search')).toBe('inherited'));
  expect(field('MCP command').value).toBe('search-server');
  expect(disabled(field('MCP command'))).toBe(true);
  expect(retained(s)).not.toContain('abandoned');
  expect(writes(s)).toHaveLength(0);
});

it('S2-07 an inherited named Agent is inspected read-only; Override, Save and Use global default are each one explicit Workspace mutation', async () => {
  const s = inheritingWorkspace();
  await workspace(s, 'Extensions');
  fireEvent.click(screen.getByRole('tab', { name: 'Agents' }));
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
  fireEvent.click(screen.getByRole('button', { name: '← Extensions' }));
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
  fireEvent.click(screen.getByRole('tab', { name: 'MCP' }));
  fireEvent.change(screen.getByLabelText('New MCP identity'), { target: { value: 'fresh' } });
  fireEvent.click(screen.getByRole('button', { name: 'Add MCP' }));
  // Creation is its own explicit gesture: writable at once, no override action.
  expect(definitionState('MCP fresh')).toBe('new');
  expect(disabled(field('MCP command'))).toBe(false);
  expect(screen.queryByRole('button', { name: /^Override MCP fresh/ })).toBeNull();
  expect(writes(s)).toHaveLength(0);
  fireEvent.click(screen.getByRole('button', { name: '← Extensions' }));
  await openResourceRow('local');
  // A Workspace definition that shadows nothing is removed, not "inherited again".
  expect(definitionState('MCP local')).toBe('authored');
  expect(screen.queryByRole('button', { name: 'Use global default MCP local' })).toBeNull();
  expect(screen.getByRole('button', { name: 'Remove MCP local' }).closest('[data-removal]')!.getAttribute('data-removal')).toBe('authored-removal');
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

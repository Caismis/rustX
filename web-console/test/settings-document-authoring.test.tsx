// @vitest-environment jsdom
import { afterEach, expect, it } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import type { Model, Request1, SourceSettings } from '../../protocol/app-server/v25';
import { userSettingsTarget, workspaceSettingsTarget } from '../src/app/settings/projection';
import { findOnAdvanced, openResourceRow, SettingsSurface } from './settings-harness';
import { cfg3Client, cfg3Host } from './cfg3-fixture';
afterEach(cleanup);

/** Every product page whose editors mutate `rustx.toml` semantic units. */
const configPages = ['Models', 'Agent', 'Tools & Permissions', 'Advanced'] as const;
const writes = (s: ReturnType<typeof cfg3Client>) => s.request.mock.calls.filter(([op]) => op.method === 'configuration/sourceWrite').map(([op]) => op as Extract<Request1, { method: 'configuration/sourceWrite' }>);
const sourcesReads = (s: ReturnType<typeof cfg3Client>) => s.request.mock.calls.filter(([op]) => op.method === 'configuration/sourcesRead');
const open = (page: string) => fireEvent.click(screen.getByRole('tab', { name: page }));
const filter = (family: string) => fireEvent.click(screen.getByRole('tab', { name: family }));
const forms = () => screen.queryAllByRole('form').map(form => form.getAttribute('aria-label'));
const MALFORMED = 'invalid rustx.toml; source was not loaded';

function malformed(source: SourceSettings, scope: 'user' | 'workspace') {
  const path = scope === 'user' ? '/bound/rustx.toml' : '/workspace/rustx.toml';
  source[scope] = { path, revision: `${scope}-1`, authored: null, diagnostic: MALFORMED };
  source.resolved = null;
  source.prospective_diagnostic = 'Source cannot be resolved; repair the diagnosed authored document.';
}

/** A native store that repairs the malformed document exactly when it receives
 * `repair_config`, as native does: the next authoritative read parses it. The
 * fixture's native write then commits the repaired document as `saved-2`. */
function repairingClient(scope: 'user' | 'workspace') {
  const s = cfg3Client(async (op, source) => {
    if (op.method === 'configuration/sourceWrite' && op.params.mutation.kind === 'repair_config') {
      source[scope] = { path: source[scope]!.path, revision: source[scope]!.revision, authored: {}, diagnostic: null };
      source.resolved = {};
      source.prospective_diagnostic = null;
    }
  });
  malformed(s.source, scope);
  return s;
}

async function assertRepairIsTheOnlyConfigMutation(s: ReturnType<typeof cfg3Client>, revision: string) {
  // Every config-backed page: the malformed document is named, and no
  // structured editor, add action or catalog row is offered for it.
  for (const page of configPages) {
    open(page);
    expect(screen.getByText(/Structured editing is unavailable because .*rustx\.toml does not parse/)).toBeTruthy();
    expect(forms()).toEqual(['Repair malformed source']);
    expect(screen.queryByRole('button', { name: /^(Add|Edit|Override|Author|Use global default|Remove) |^Save (?!Repair malformed source$)/ })).toBeNull();
  }
  // Extensions: the native extensions and root availability are units of the
  // same document, and they are closed on the same terms.
  open('Extensions');
  filter('Native');
  expect(screen.getByText(/Structured editing of this source's rustx\.toml is unavailable/)).toBeTruthy();
  expect(forms()).toEqual([]);
  open('Advanced');
  expect(writes(s)).toHaveLength(0);
  // The repair form is enabled and fenced on the exact current revision.
  const repair = screen.getByRole('form', { name: 'Repair malformed source' });
  expect((repair.querySelector('textarea') as HTMLTextAreaElement).disabled).toBe(false);
  fireEvent.change(repair.querySelector('textarea')!, { target: { value: '[agent]\n' } });
  fireEvent.click(within(repair).getByRole('button', { name: 'Save Repair malformed source' }));
  await waitFor(() => expect(writes(s)).toHaveLength(1));
  expect(writes(s)[0].params).toMatchObject({ expected_revision: revision, mutation: { kind: 'repair_config', document: '[agent]\n' } });
}

it.each(['user', 'workspace'] as const)('DA-02 a malformed %s rustx.toml exposes repair_config as its only mutation, and structured editing returns after the authoritative reread', async scope => {
  const s = repairingClient(scope);
  render(scope === 'user'
    ? <SettingsSurface client={s.client} target={userSettingsTarget} />
    : <SettingsSurface client={s.client} target={workspaceSettingsTarget('A', 'A')} host={cfg3Host(s)} />);
  // Diagnostics stay on Advanced even while the document does not parse.
  await findOnAdvanced(new RegExp(`Revision: ${scope}-1`));
  open('Advanced');
  expect(screen.getAllByRole('alert').map(alert => alert.textContent)).toContain(MALFORMED);
  if (scope === 'workspace') expect(forms()).toEqual(['Repair malformed source']);
  await assertRepairIsTheOnlyConfigMutation(s, `${scope}-1`);
  // The committed repair is observed by an authoritative read, and only that
  // observation makes structured editing available again.
  await screen.findByText(/Revision: saved-2/);
  expect(screen.queryByRole('form', { name: 'Repair malformed source' })).toBeNull();
  open('Tools & Permissions');
  const tools = within(screen.getByRole('form', { name: 'Native Tools' }));
  fireEvent.click(tools.getByLabelText('read'));
  fireEvent.click(tools.getByRole('button', { name: 'Save Native Tools' }));
  await waitFor(() => expect(writes(s)).toHaveLength(2));
  expect(writes(s)[1].params).toMatchObject({ expected_revision: 'saved-2', mutation: { kind: 'config', mutation: { unit: 'native_tools', authored: ['read'] } } });
  expect(sourcesReads(s).length).toBeGreaterThanOrEqual(2);
});

it('DA-03 a malformed rustx.toml leaves the independent MCP and named Agent documents editable through their own CAS', async () => {
  const s = cfg3Client();
  malformed(s.source, 'user');
  render(<SettingsSurface client={s.client} target={userSettingsTarget} />);
  await findOnAdvanced(/Revision: user-1/);
  // MCP: its own valid document, its own revision.
  open('Extensions');
  filter('MCP');
  expect(screen.queryByRole('form', { name: 'Repair malformed source' })).toBeNull();
  fireEvent.change(screen.getByLabelText('New MCP identity'), { target: { value: 'probe' } });
  fireEvent.click(screen.getByRole('button', { name: 'Add MCP' }));
  const mcp = within(screen.getByRole('form', { name: 'MCP probe' }));
  fireEvent.change(mcp.getByLabelText('MCP command'), { target: { value: 'probe-server' } });
  fireEvent.click(mcp.getByRole('button', { name: 'Save MCP probe' }));
  await waitFor(() => expect(writes(s)).toHaveLength(1));
  expect(writes(s)[0].params).toMatchObject({ expected_revision: 'mcp-1', mutation: { kind: 'mcp', id: 'probe' } });
  // Named Agent: a whole resource document of its own.
  fireEvent.click(screen.getByRole('button', { name: '← Extensions' }));
  filter('Agents');
  expect(screen.queryByRole('form', { name: 'Repair malformed source' })).toBeNull();
  fireEvent.change(screen.getByLabelText('New Agent identity'), { target: { value: 'helper' } });
  fireEvent.click(screen.getByRole('button', { name: 'Add Agent' }));
  const agent = within(screen.getByRole('form', { name: 'Agent helper' }));
  fireEvent.change(agent.getByLabelText('Description'), { target: { value: 'independent' } });
  fireEvent.click(agent.getByRole('button', { name: 'Save Agent helper' }));
  await waitFor(() => expect(writes(s)).toHaveLength(2));
  expect(writes(s)[1].params).toMatchObject({ expected_revision: 'missing', mutation: { kind: 'agent', name: 'helper', authored: { description: 'independent' } } });
  // No structured rustx.toml mutation was ever submitted.
  expect(writes(s).some(op => op.params.mutation.kind === 'config')).toBe(false);
});

it('DA-04 a malformed MCP document admits no MCP mutation and leaves rustx.toml editing intact', async () => {
  const s = cfg3Client();
  s.source.user_mcp = { path: '/home/user/rustx/.agents/mcp.toml', revision: 'mcp-1', authored: null, diagnostic: 'invalid MCP document' };
  render(<SettingsSurface client={s.client} target={userSettingsTarget} />);
  await findOnAdvanced(/Revision: user-1/);
  open('Extensions');
  filter('MCP');
  expect(screen.getByText('invalid MCP document')).toBeTruthy();
  expect(screen.getByText(/MCP editing is unavailable because this document does not parse/)).toBeTruthy();
  expect(screen.queryByLabelText('New MCP identity')).toBeNull();
  expect(forms()).toEqual([]);
  open('Tools & Permissions');
  const tools = within(screen.getByRole('form', { name: 'Native Tools' }));
  fireEvent.click(tools.getByLabelText('read'));
  fireEvent.click(tools.getByRole('button', { name: 'Save Native Tools' }));
  await waitFor(() => expect(writes(s)).toHaveLength(1));
  expect(writes(s)[0].params).toMatchObject({ expected_revision: 'user-1', mutation: { kind: 'config' } });
});

// ── Model identity discovery ────────────────────────────────────────────────

const model = (id: string): Model => ({ provider: 'transport', id, protocol: 'openai_responses', context_window: '128000', max_output_tokens: 8192, capabilities: { input_modalities: ['text'], output_modalities: ['text'], tool_calls: true, reasoning: false } });
/** The option labels of one React Aria Select, read by opening it. */
function options(scope: HTMLElement, label: RegExp) {
  fireEvent.click(within(scope).getByRole('button', { name: label }));
  const listed = within(screen.getByRole('listbox')).getAllByRole('option').map(option => option.textContent!);
  fireEvent.keyDown(screen.getByRole('listbox'), { key: 'Escape' });
  return listed;
}
const allModels = () => {
  fireEvent.click(screen.getByRole('button', { name: /^All Models/ }));
  return within(screen.getByRole('grid', { name: 'All Models' }));
};

/** The Model catalog, the default-model selector and a named Agent's
 * explicit-model selector, in that order, for one rendered Settings surface. */
async function reachableModels() {
  open('Models');
  const defaultToggle = screen.queryByRole('button', { name: 'Default model for new Sessions', expanded: false });
  if (defaultToggle) fireEvent.click(defaultToggle);
  const catalog = allModels().queryAllByRole('row').map(row => row.getAttribute('aria-label')!);
  const root = options(screen.getByRole('form', { name: 'Default model' }), /Model$/);
  open('Extensions');
  filter('Agents');
  fireEvent.change(screen.getByLabelText('New Agent identity'), { target: { value: 'helper' } });
  fireEvent.click(screen.getByRole('button', { name: 'Add Agent' }));
  const agent = screen.getByRole('form', { name: 'Agent helper' });
  fireEvent.click(within(agent).getByLabelText('Explicit child model'));
  const named = options(agent, /Model$/);
  fireEvent.click(screen.getByRole('button', { name: '← Extensions' }));
  const selectable = (list: string[]) => list.filter(option => option !== 'Select model');
  return { catalog, root: selectable(root), named: selectable(named) };
}

it('ID-03 Workspace-authored models stay reachable in every selector when a malformed User document leaves resolution unavailable', async () => {
  const s = cfg3Client();
  s.source.user = { path: '/bound/rustx.toml', revision: 'user-1', authored: null, diagnostic: MALFORMED };
  s.source.workspace = { path: '/workspace/rustx.toml', revision: 'workspace-1', authored: { models: { 'workspace-model': model('wire-w') } } };
  s.source.resolved = null;
  s.source.prospective_diagnostic = 'Source cannot be resolved; repair the diagnosed authored document.';
  render(<SettingsSurface client={s.client} target={workspaceSettingsTarget('A', 'A')} host={cfg3Host(s)} />);
  await findOnAdvanced(/Revision: workspace-1/);
  // The three surfaces agree, and list exactly what this Workspace authors:
  // nothing is invented from the malformed User document.
  expect(await reachableModels()).toEqual({ catalog: ['workspace-model'], root: ['workspace-model'], named: ['workspace-model'] });
  // No effective value is fabricated for the identity.
  open('Models');
  const defaultToggle = screen.queryByRole('button', { name: 'Default model for new Sessions', expanded: false });
  if (defaultToggle) fireEvent.click(defaultToggle);
  const rootForm = within(screen.getByRole('form', { name: 'Default model' }));
  expect(rootForm.getByText(/Inherited — no Workspace override/).getAttribute('data-effective')).toBe('invalid');
  expect(rootForm.queryByText(/Native effective value available/)).toBeNull();
  allModels();
  await openResourceRow('workspace-model');
  expect(within(screen.getByRole('form', { name: 'Model workspace-model' })).getByText(/Workspace override/)).toBeTruthy();
  // Nothing was copied into Workspace authoring.
  expect(writes(s)).toHaveLength(0);
});

it('ID-04 a resolved Workspace reaches inherited effective models in every selector; User reaches exactly its own', async () => {
  const s = cfg3Client();
  s.source.user.authored = { models: { 'user-model': model('wire-u') } };
  s.source.workspace = { path: '/workspace/rustx.toml', revision: 'workspace-1', authored: { models: { 'workspace-model': model('wire-w') } } };
  s.source.resolved = { models: { 'user-model': model('wire-u'), 'workspace-model': model('wire-w') } } as never;
  const view = render(<SettingsSurface client={s.client} target={workspaceSettingsTarget('A', 'A')} host={cfg3Host(s)} />);
  await findOnAdvanced(/Revision: workspace-1/);
  expect(await reachableModels()).toEqual({
    catalog: ['user-model', 'workspace-model'], root: ['user-model', 'workspace-model'], named: ['user-model', 'workspace-model'],
  });
  view.unmount();
  render(<SettingsSurface client={s.client} target={userSettingsTarget} />);
  await findOnAdvanced(/Revision: user-1/);
  expect(await reachableModels()).toEqual({ catalog: ['user-model'], root: ['user-model'], named: ['user-model'] });
  expect(writes(s)).toHaveLength(0);
});

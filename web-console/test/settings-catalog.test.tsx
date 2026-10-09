// @vitest-environment jsdom
import { afterEach, expect, it } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { userSettingsTarget } from '../src/app/settings/projection';
import { cfg3Client, cfg3Host } from './cfg3-fixture';
import { openResourceRow, openSettingsPage, settingsReady, SettingsSurface } from './settings-harness';
afterEach(cleanup);

const writes = (s: ReturnType<typeof cfg3Client>) => s.request.mock.calls.flatMap(([op]) => op.method === 'configuration/sourceWrite' ? [op.params] : []);
async function catalog(s: ReturnType<typeof cfg3Client>) {
  s.source.user_mcp.authored = { search: { definition: { type: 'stdio', command: 'search-server' }, retained_env: [], retained_headers: [] } };
  render(<SettingsSurface client={s.client} target={userSettingsTarget} host={cfg3Host(s)}/>);
  await settingsReady();
  await openSettingsPage('Extensions');
  fireEvent.click(screen.getByRole('tab', { name: 'MCP servers' }));
}

it('catalog search filters by command and quick availability submits only the current scope selection', async () => {
  const s = cfg3Client();
  await catalog(s);
  fireEvent.change(screen.getByRole('searchbox'), { target: { value: 'no-match' } });
  expect(screen.queryByRole('listitem', { name: 'search' })).toBeNull();
  fireEvent.change(screen.getByRole('searchbox'), { target: { value: 'search-server' } });
  fireEvent.click(within(screen.getByRole('listitem', { name: 'search' })).getByRole('switch'));
  await waitFor(() => expect(writes(s)).toHaveLength(1));
  expect(writes(s)[0]).toMatchObject({ target: { kind: 'user' }, expected_revision: 'user-1', mutation: { kind: 'config', mutation: { unit: 'source_tools', id: 'search', authored: 'all' } } });
});

it('scope menu preserves the resource category and keeps definition drafts bound to their original owner', async () => {
  const s = cfg3Client();
  await catalog(s);
  await openResourceRow('search');
  fireEvent.change(screen.getByLabelText('MCP command'), { target: { value: 'unsaved-server' } });
  fireEvent.click(screen.getByRole('button', { name: /^← (Extensions|MCP servers)$/ }));
  fireEvent.click(screen.getByRole('button', { name: 'Configuration scope' }));
  fireEvent.click(await screen.findByRole('menuitem', { name: 'A' }));
  await screen.findByRole('heading', { name: 'Workspace Settings — A' });
  expect(screen.getByRole('tab', { name: 'MCP servers', selected: true })).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Configuration scope' }));
  fireEvent.click(await screen.findByRole('menuitem', { name: 'User (global)' }));
  await screen.findByRole('heading', { name: 'User Settings' });
  await openResourceRow('search');
  expect((screen.getByLabelText('MCP command') as HTMLInputElement).value).toBe('unsaved-server');
  expect(writes(s)).toHaveLength(0);
});

it('partial MCP grants stay in the detailed selection editor', async () => {
  const s = cfg3Client();
  s.source.user.authored!.agent = { tools: { sources: { search: ['lookup'] } } };
  await catalog(s);
  const card = within(screen.getByRole('listitem', { name: 'search' }));
  expect(card.queryByRole('switch')).toBeNull();
  fireEvent.click(card.getByRole('button', { name: 'Manage selection' }));
  expect(screen.getByRole('form', { name: 'Source search' })).toBeTruthy();
  expect(writes(s)).toHaveLength(0);
});

it.each(['skill', 'agent'] as const)('%s quick selection preserves other names and writes the workspace override', async family => {
  const s = cfg3Client();
  const unit = family === 'skill' ? 'skills' : 'agents';
  s.source.workspace!.authored!.agent = { [unit]: ['existing'] };
  s.source.prospective_resources = { ...s.effective.resources, definitions: [{ family, name: 'helper', valid: true, location: { scope: 'workspace', path: '/workspace/.agents/helper' } }] };
  render(<SettingsSurface client={s.client} target={{ kind: 'workspace', id: 'A', displayName: 'A' }} host={cfg3Host(s)}/>);
  await settingsReady();
  await openSettingsPage('Extensions');
  fireEvent.click(screen.getByRole('tab', { name: family === 'skill' ? 'Skills' : 'Subagents' }));
  fireEvent.click(within(screen.getByRole('listitem', { name: 'helper' })).getByRole('switch'));
  await waitFor(() => expect(writes(s)).toHaveLength(1));
  expect(writes(s)[0]).toMatchObject({ target: { kind: 'workspace', directory: '/workspace/A' }, expected_revision: 'workspace-1', mutation: { kind: 'config', mutation: { unit, authored: ['existing', 'helper'] } } });
});

it('a rejected quick write exposes its retained draft through detail rather than retrying', async () => {
  const s = cfg3Client(async op => { if (op.method === 'configuration/sourceWrite') throw new Error('catalog rejected'); });
  await catalog(s);
  fireEvent.click(within(screen.getByRole('listitem', { name: 'search' })).getByRole('switch'));
  fireEvent.click(await screen.findByRole('button', { name: 'Manage selection' }));
  await screen.findByText(/catalog rejected/);
  expect(writes(s)).toHaveLength(1);
});

it('MCP observation stays unknown and navigation and refresh only read configuration', async () => {
  const s = cfg3Client();
  s.source.user.authored!.agent = {tools:{sources:{search:'all'}}};
  await catalog(s);
  await screen.findByRole('img', {name:/MCP server status is not yet available/});
  await openSettingsPage('MCP servers');
  fireEvent.click(screen.getByRole('button', {name:'Refresh'}));
  await settingsReady();
  await openSettingsPage('General');
  await openSettingsPage('MCP servers');
  expect([...new Set(s.request.mock.calls.map(([op]) => op.method))]).toEqual(['configuration/sourcesRead']);
  expect(writes(s)).toHaveLength(0);
});

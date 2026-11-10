import {openResourceModule} from './settings-harness';
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
  await openResourceModule('mcp');
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
  fireEvent.change(screen.getByLabelText('Command'), { target: { value: 'unsaved-server' } });
  fireEvent.click(screen.getByRole('button', { name: /^← (Extensions|MCP servers)$/ }));
  fireEvent.click(screen.getByRole('tab',{name:'Workspace'}));
  fireEvent.click(screen.getByRole('button',{name:'Choose Workspace'}));
  fireEvent.click(await screen.findByRole('menuitem',{name:'A'}));
  await screen.findByRole('group',{name:'Installed'});
  fireEvent.click(screen.getByRole('tab',{name:'Global'}));
  await screen.findByRole('listitem',{name:'search'});
  await openResourceRow('search');
  expect((screen.getByLabelText('Command') as HTMLInputElement).value).toBe('unsaved-server');
  expect(writes(s)).toHaveLength(0);
});

it('disabling a partially granted MCP source explicitly revokes that source', async () => {
  const s = cfg3Client();
  s.source.user.authored!.agent = { tools: { sources: { search: ['lookup'] } } };
  await catalog(s);
  const card = within(screen.getByRole('listitem', { name: 'search' }));
  expect(card.queryByRole('button', { name: 'Manage selection' })).toBeNull();
  const toggle = card.getByRole('switch') as HTMLButtonElement;
  expect(toggle.disabled).toBe(false);
  fireEvent.click(toggle);
  await waitFor(()=>expect(writes(s)).toHaveLength(1));
  expect(writes(s)[0]).toMatchObject({mutation:{kind:'config',mutation:{unit:'source_tools',id:'search',authored:[]}}});
});

it.each(['skill', 'agent'] as const)('%s quick selection preserves other names and writes the workspace override', async family => {
  const s = cfg3Client();
  const unit = family === 'skill' ? 'skills' : 'agents';
  s.source.workspace!.authored!.agent = { [unit]: ['existing'] };
  s.source.prospective_resources = { ...s.effective.resources, definitions: [{ family, name: 'helper', valid: true, location: { scope: 'workspace', path: '/workspace/.agents/helper' } }] };
  render(<SettingsSurface client={s.client} target={{ kind: 'workspace', id: 'A', displayName: 'A' }} host={cfg3Host(s)}/>);
  await settingsReady();
  await openSettingsPage('Extensions');
  await openResourceModule(family==='skill'?'skills':'subagents');
  fireEvent.click(within(screen.getByRole('listitem', { name: 'helper' })).getByRole('switch'));
  await waitFor(() => expect(writes(s)).toHaveLength(1));
  expect(writes(s)[0]).toMatchObject({ target: { kind: 'workspace', directory: '/workspace/A' }, expected_revision: 'workspace-1', mutation: { kind: 'config', mutation: { unit, authored: ['existing', 'helper'] } } });
});

it('a rejected MCP toggle exposes the failure and cannot blindly retry', async () => {
  const s = cfg3Client(async op => { if (op.method === 'configuration/sourceWrite') throw new Error('catalog rejected'); });
  await catalog(s);
  fireEvent.click(within(screen.getByRole('listitem', { name: 'search' })).getByRole('switch'));
  await screen.findByText(/catalog rejected/);
  expect((within(screen.getByRole('listitem', {name:'search'})).getByRole('switch') as HTMLButtonElement).disabled).toBe(true);
  expect(screen.queryByRole('button', {name:'Manage selection'})).toBeNull();
  expect(writes(s)).toHaveLength(1);
});

it('MCP navigation, search and refresh stay passive; Test connection executes one definition', async () => {
  const s = cfg3Client();
  await catalog(s);
  expect(s.request.mock.calls.some(([op]) => op.method === 'mcp/probe')).toBe(false);
  await openResourceModule('mcp');
  fireEvent.change(screen.getByRole('searchbox'), {target:{value:'search'}});
  fireEvent.click(screen.getByRole('button', {name:'Refresh'}));
  await settingsReady();
  await openSettingsPage('General');
  await openResourceModule('mcp');
  expect(s.request.mock.calls.some(([op]) => op.method === 'mcp/probe')).toBe(false);
  fireEvent.click(screen.getByRole('button', {name:'Test connection search'}));
  await screen.findByRole('img', {name:/Connection test succeeded/});
  expect(s.request.mock.calls.filter(([op]) => op.method === 'mcp/probe')).toHaveLength(1);
  expect(writes(s)).toHaveLength(0);
});

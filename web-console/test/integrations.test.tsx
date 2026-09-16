// @vitest-environment jsdom
import { afterEach, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import type { SourceSettings, SourceMutation } from '../../protocol/app-server/v5';
import { Integrations } from '../src/app/settings/Integrations';
afterEach(cleanup);
const entry = { enabled: false, transport: 'stdio' as const, command: 'native-command', args: [], cwd: null, url: null, retained_env: ['TOKEN'], retained_headers: [], sensitive_env: { KEY: '$HOST_KEY' }, sensitive_headers: {} };
const source = (): SourceSettings => ({ catalog: { valid: true, document: '/models', revision: 'c1', models: { models: [] }, providers: {} }, user: { document: '/settings', revision: 'u1', active: true, authored: null }, workspace: { document: '/workspace/rustx.toml', revision: 'w1', active: true, authored: null }, resolution_available: true, provenance: {}, integrations: { mcp_tool_policies: {}, mcp_valid: true, mcp: [{ id: 'native', user: entry, workspace: { ...entry, command: 'project', sensitive_env: {}, retained_env: [] }, winning: { kind: 'project', document: '/workspace/rustx.toml', base: '/workspace' }, activation: null }], user: {}, workspace: {}, prospective: {}, provenance: {}, inventory: null, agents: [] } });
it('edits the exact shadowed User identity, retains draft across refresh, and explicitly retries', async () => {
  const save = vi.fn<(_: 'user' | 'workspace', mutation: SourceMutation, revision?: string) => Promise<boolean>>().mockResolvedValueOnce(false).mockResolvedValue(true);
  const value = source(); const view = render(<Integrations source={value} busy={false} save={save} />);
  expect(screen.getByText('Winning source: Workspace')).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Edit User · native' }));
  fireEvent.change(screen.getByLabelText('Command'), { target: { value: 'draft-command' } });
  fireEvent.change(screen.getByLabelText('Transport'), { target: { value: '' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save MCP' })); await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
  const fresh = source(); fresh.user.revision = 'u2'; fresh.integrations.mcp[0].user!.command = 'concurrent';
  view.rerender(<Integrations source={fresh} busy={false} save={save} />);
  expect((screen.getByLabelText('Command') as HTMLInputElement).value).toBe('draft-command');
  expect((screen.getByLabelText('Integration scope') as HTMLSelectElement).disabled).toBe(true);
  expect(save.mock.calls[0]).toMatchObject(['user', { kind: 'mcp', scope: 'user', id: 'native', authored: { command: 'draft-command', transport: null } }, 'u1']);
  fireEvent.click(screen.getByRole('button', { name: 'Save MCP' })); await waitFor(() => expect(save).toHaveBeenCalledTimes(2));
  expect(save.mock.calls[1][2]).toBe('u2');
  await waitFor(() => expect(screen.queryByLabelText('Command')).toBeNull());
});
it('untrusted Workspace stays read-only and secret references never create password storage', () => {
  const value = source(); value.workspace.active = false;
  render(<Integrations source={value} busy={false} save={vi.fn()} />);
  fireEvent.change(screen.getByLabelText('Integration scope'), { target: { value: 'workspace' } });
  expect((screen.getByRole('button', { name: 'Add Workspace MCP server' }) as HTMLButtonElement).disabled).toBe(true);
  expect(screen.getByText(/Host cwd authorization/)).toBeTruthy();
  expect(document.querySelector('input[type=password]')).toBeNull(); expect(localStorage.length).toBe(0); expect(sessionStorage.length).toBe(0);
});
it('deletes only the selected authored entry and sends extension member mutations', async () => {
  const save = vi.fn().mockResolvedValue(true);
  render(<Integrations source={source()} busy={false} save={save} />);
  fireEvent.click(screen.getByRole('button', { name: 'Edit User · native' }));
  fireEvent.click(screen.getByRole('button', { name: 'Delete authored MCP entry' }));
  await waitFor(() => expect(save).toHaveBeenCalledWith('user', { kind: 'mcp', scope: 'user', id: 'native', authored: null }, 'u1'));
  fireEvent.click(screen.getByLabelText('goal'));
  expect(save).toHaveBeenLastCalledWith('user', { kind: 'integration', scope: 'user', control: { kind: 'extension', identity: 'goal', enabled: true } });
});

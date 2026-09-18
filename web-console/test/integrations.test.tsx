// @vitest-environment jsdom
import { afterEach, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { Integrations } from '../src/app/settings/Integrations';
import type { SaveSource } from '../src/app/settings/controls';
import { cfg3Source } from './cfg3-fixture';
afterEach(cleanup);
it('edits MCP definition independently from Root selection and retains exact source revision', async () => {
  const source = cfg3Source();
  source.workspace_mcp.authored = { search: { definition: { type: 'stdio', command: 'server' }, retained_env: ['TOKEN'], retained_headers: [] } };
  const save = vi.fn<SaveSource>().mockResolvedValue(undefined);
  const view = render(<Integrations source={source} scope="workspace" save={save} />);
  fireEvent.click(screen.getByRole('button', { name: 'Edit MCP search' }));
  fireEvent.change(screen.getByLabelText('MCP command'), { target: { value: 'edited-server' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save MCP search' }));
  await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
  expect(save.mock.calls[0]).toEqual([{ kind: 'mcp', scope: 'workspace', id: 'search', authored: { definition: { type: 'stdio', command: 'edited-server' }, retained_env: ['TOKEN'], retained_headers: [] } }, 'mcp-2']);
  const fresh = structuredClone(source); fresh.workspace_mcp.revision = 'external';
  view.rerender(<Integrations source={fresh} scope="workspace" save={save} />);
  expect((screen.getByLabelText('MCP command') as HTMLInputElement).value).toBe('edited-server');
  fireEvent.click(screen.getByRole('button', { name: 'Save MCP search' }));
  await waitFor(() => expect(save).toHaveBeenCalledTimes(2)); expect(save.mock.calls[1][1]).toBe('mcp-2');
  expect(screen.queryByLabelText(/enabled/i)).toBeNull();
});
it('creates a Workspace definition without borrowing the User transport or credentials', () => {
  const source = cfg3Source(); source.user_mcp.authored = { remote: { definition: { type: 'http', url: 'https://user.invalid' }, retained_headers: ['Authorization'], retained_env: [] } };
  render(<Integrations source={source} scope="workspace" save={vi.fn<SaveSource>()} />);
  fireEvent.change(screen.getByLabelText('New MCP identity'), { target: { value: 'remote' } }); fireEvent.click(screen.getByRole('button', { name: 'Add MCP' }));
  expect((screen.getByLabelText('MCP command') as HTMLInputElement).value).toBe('');
  expect(screen.getByRole('group', { name: 'Retain existing header keys' }).querySelectorAll('input')).toHaveLength(0);
  expect(screen.queryByText(/trusted/i)).toBeNull();
});

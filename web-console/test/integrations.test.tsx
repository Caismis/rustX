// @vitest-environment jsdom
import { afterEach, expect, it } from 'vitest';
import { cleanup, fireEvent, screen, waitFor } from '@testing-library/react';
import { mcpTransport } from '../src/bindings/mcp';
import { Integrations } from '../src/app/settings/Integrations';
import { renderEditor } from './settings-harness';
import { cfg3Source } from './cfg3-data';
afterEach(cleanup);
it('edits MCP definition independently from Root selection and retains exact source revision', async () => {
  const source = cfg3Source();
  source.workspace_mcp!.authored = { search: { definition: { type: 'stdio', command: 'server' }, retained_env: ['TOKEN'], retained_headers: [] } };
  const { writes: save, rerender } = await renderEditor(<Integrations source={source} scope="workspace" />, { source, context: source });
  fireEvent.click(screen.getByRole('button', { name: 'Edit MCP search' }));
  fireEvent.change(screen.getByLabelText('MCP command'), { target: { value: 'edited-server' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save MCP search' }));
  await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
  expect(save.mock.calls[0]).toEqual([{ kind: 'mcp', id: 'search', authored: { definition: { type: 'stdio', command: 'edited-server' }, retained_env: ['TOKEN'], retained_headers: [] } }, 'mcp-2']);
  const fresh = structuredClone(source); fresh.workspace_mcp!.revision = 'external';
  rerender(<Integrations source={fresh} scope="workspace" />);
  expect((screen.getByLabelText('MCP command') as HTMLInputElement).value).toBe('edited-server');
  fireEvent.click(screen.getByRole('button', { name: 'Save MCP search' }));
  await waitFor(() => expect(save).toHaveBeenCalledTimes(2)); expect(save.mock.calls[1][1]).toBe('mcp-2');
  expect(screen.queryByLabelText(/enabled/i)).toBeNull();
});
it('creates a Workspace definition without borrowing the User transport or credentials', async () => {
  const source = cfg3Source(); source.user_mcp.authored = { remote: { definition: { type: 'http', url: 'https://user.invalid' }, retained_headers: ['Authorization'], retained_env: [] } };
  await renderEditor(<Integrations source={source} scope="workspace" />, { source, context: source });
  fireEvent.change(screen.getByLabelText('New MCP identity'), { target: { value: 'remote' } }); fireEvent.click(screen.getByRole('button', { name: 'Add MCP' }));
  expect((screen.getByLabelText('MCP command') as HTMLInputElement).value).toBe('');
  expect(screen.getByRole('group', { name: 'Retain existing header keys' }).querySelectorAll('input')).toHaveLength(0);
  expect(screen.queryByText(/trusted/i)).toBeNull();
});
it.each(['http', 'stdio'] as const)('projects implicit %s without rewriting transport or retained credentials', async transport => {
  const source = cfg3Source();
  const authored = transport === 'http'
    ? { definition: { url: 'https://remote.invalid/mcp', sensitive_headers: { 'X-Key': '$KEY' } }, retained_headers: ['Authorization'], retained_env: [] }
    : { definition: { command: 'server', sensitive_env: { KEY: '$KEY' } }, retained_headers: [], retained_env: ['TOKEN'] };
  source.workspace_mcp!.authored = { inferred: authored };
  const initial = structuredClone(authored);
  const { writes: save } = await renderEditor(<Integrations source={source} scope="workspace" />, { source, context: source });
  expect(screen.getByRole('article').textContent).toContain(transport);
  fireEvent.click(screen.getByRole('button', { name: 'Edit MCP inferred' }));
  expect((screen.getByLabelText('Transport') as HTMLSelectElement).value).toBe(transport);
  expect(source.workspace_mcp!.authored.inferred).toEqual(initial);
  expect(save).not.toHaveBeenCalled();
  if (transport === 'http') {
    expect(screen.queryByLabelText('MCP command')).toBeNull();
    fireEvent.change(screen.getByLabelText('MCP URL'), { target: { value: 'https://remote.invalid/edited' } });
  } else {
    expect(screen.queryByLabelText('MCP URL')).toBeNull();
    fireEvent.change(screen.getByLabelText('Working directory'), { target: { value: 'tools' } });
  }
  fireEvent.click(screen.getByRole('button', { name: 'Save MCP inferred' }));
  await waitFor(() => expect(save).toHaveBeenCalledWith({ kind: 'mcp', id: 'inferred', authored: { ...initial, definition: { ...initial.definition, ...(transport === 'http' ? { url: 'https://remote.invalid/edited' } : { cwd: 'tools' }) } } }, 'mcp-2'));
  const nextTransport = transport === 'http' ? 'stdio' : 'http';
  fireEvent.change(screen.getByLabelText('Transport'), { target: { value: nextTransport } });
  fireEvent.change(screen.getByLabelText(nextTransport === 'http' ? 'MCP URL' : 'MCP command'), { target: { value: nextTransport === 'http' ? 'https://new.invalid' : 'new-server' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save MCP inferred' }));
  await waitFor(() => expect(save).toHaveBeenLastCalledWith({ kind: 'mcp', id: 'inferred', authored: { definition: nextTransport === 'http' ? { type: 'http', url: 'https://new.invalid' } : { type: 'stdio', command: 'new-server', args: [] }, retained_env: [], retained_headers: [] } }, 'mcp-2'));
});

it('honors explicit MCP transport and keeps the new-definition default local', () => {
  expect(mcpTransport(Object.freeze({ type: 'stdio', url: 'https://invalid-hybrid.test' }))).toBe('stdio');
  expect(mcpTransport(Object.freeze({ type: 'http', command: 'invalid-hybrid' }))).toBe('http');
  expect(mcpTransport(Object.freeze({}))).toBe('stdio');
});

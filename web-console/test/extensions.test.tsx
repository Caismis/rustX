// @vitest-environment jsdom
import { afterEach, expect, it } from 'vitest';
import { cleanup, fireEvent, screen, waitFor } from '@testing-library/react';
import { mcpTransport } from '../src/bindings/mcp';
import { ExtensionDetail } from '../src/app/settings/extensions/ExtensionDetail';
import { chooseOption, renderEditor } from './settings-harness';
import { cfg3Source } from './cfg3-data';
import type { SourceScope, SourceSettings } from '../../protocol/app-server/v19';
afterEach(cleanup);

const noop = () => {};
/** One MCP resource's detail: its definition, and — as a separate native
 * mutation with its own Save — its availability to the root Agent. */
const mcpDetail = (source: SourceSettings, scope: SourceScope, name: string) =>
  <ExtensionDetail source={source} scope={scope} revision="cfg-1" models={[]} family="mcp" name={name} onFocus={noop} />;

it('edits an MCP definition independently from root selection and retains exact source revision', async () => {
  const source = cfg3Source();
  source.workspace_mcp!.authored = { search: { definition: { type: 'stdio', command: 'server' }, retained_env: ['TOKEN'], retained_headers: [] } };
  const { writes: save, rerender } = await renderEditor(mcpDetail(source, 'workspace', 'search'), { source, context: source });
  fireEvent.change(screen.getByLabelText('MCP command'), { target: { value: 'edited-server' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save MCP search' }));
  await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
  expect(save.mock.calls[0]).toEqual([{ kind: 'mcp', id: 'search', authored: { definition: { type: 'stdio', command: 'edited-server' }, retained_env: ['TOKEN'], retained_headers: [] } }, 'mcp-2']);
  // An external revision change never resets the dirty actor-owned draft, and
  // the next write is still fenced on the revision the draft pinned.
  const fresh = structuredClone(source); fresh.workspace_mcp!.revision = 'external';
  rerender(mcpDetail(fresh, 'workspace', 'search'));
  expect((screen.getByLabelText('MCP command') as HTMLInputElement).value).toBe('edited-server');
  fireEvent.click(screen.getByRole('button', { name: 'Save MCP search' }));
  await waitFor(() => expect(save).toHaveBeenCalledTimes(2));
  expect(save.mock.calls[1][1]).toBe('mcp-2');
  // Saving the definition never touched the root Agent's selection: that is a
  // separate native unit with a separate Save of its own.
  expect(save.mock.calls.every(([mutation]) => mutation.kind === 'mcp')).toBe(true);
  expect(screen.getByRole('form', { name: 'Source search' })).toBeTruthy();
});

it('creates a Workspace definition without borrowing the User transport or credentials', async () => {
  const source = cfg3Source();
  source.user_mcp.authored = { remote: { definition: { type: 'http', url: 'https://user.invalid' }, retained_headers: ['Authorization'], retained_env: [] } };
  await renderEditor(mcpDetail(source, 'workspace', 'remote'), { source, context: source });
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
  const { writes: save } = await renderEditor(mcpDetail(source, 'workspace', 'inferred'), { source, context: source });
  // Opening an authored definition authors nothing and rewrites nothing.
  expect(screen.getByRole('button', { name: (name: string) => name.endsWith('Transport') }).textContent)
    .toContain(transport === 'http' ? 'HTTP' : 'stdio');
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
  // Choosing the other transport replaces the whole definition, exactly as
  // native owns it, and drops the retained secret keys of the old one.
  const next = transport === 'http' ? 'stdio' : 'http';
  await chooseOption('Transport', next === 'http' ? 'HTTP' : 'stdio');
  fireEvent.change(screen.getByLabelText(next === 'http' ? 'MCP URL' : 'MCP command'), { target: { value: next === 'http' ? 'https://new.invalid' : 'new-server' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save MCP inferred' }));
  await waitFor(() => expect(save).toHaveBeenLastCalledWith({ kind: 'mcp', id: 'inferred', authored: { definition: next === 'http' ? { type: 'http', url: 'https://new.invalid' } : { type: 'stdio', command: 'new-server', args: [] }, retained_env: [], retained_headers: [] } }, 'mcp-2'));
});

it('honors explicit MCP transport and keeps the new-definition default local', () => {
  expect(mcpTransport(Object.freeze({ type: 'stdio', url: 'https://invalid-hybrid.test' }))).toBe('stdio');
  expect(mcpTransport(Object.freeze({ type: 'http', command: 'invalid-hybrid' }))).toBe('http');
  expect(mcpTransport(Object.freeze({}))).toBe('stdio');
});

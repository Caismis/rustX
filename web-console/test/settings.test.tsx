// @vitest-environment jsdom
import { afterEach, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import type { MethodResult, Request1 } from '../../protocol/app-server/v5';
import { AppServerClient, OutcomeUncertain, RpcFailure } from '../src/client/app-server';
import { Settings } from '../src/app/settings/Settings';
afterEach(cleanup);
type Read = Extract<MethodResult, { type: 'source_settings' }>;
const fixture = (): Read => ({ type: 'source_settings', session_revision: '7', session_selection: { model: 'native/a' }, projection: {
  integrations: { mcp_tool_policies: {}, mcp: [], mcp_valid: true, user: {}, workspace: {}, prospective: {}, provenance: {}, inventory: null, agents: [] },
  catalog: { valid: true, document: '/bound/models.toml', revision: 'catalog-1', models: { models: ['native/a', 'native/b'].map(model => ({ model, protocol: 'openai_chat_completions' as const, contextWindow: 128000, maxOutputTokens: 4096, declaredCapabilities: { inputModalities: ['text' as const], outputModalities: ['text' as const], toolCalls: true, reasoning: false }, effectiveCapabilities: { inputModalities: ['text' as const], outputModalities: ['text' as const], toolCalls: true, reasoning: false }, credentialSource: { type: 'environment' as const, variable: 'FIXTURE_KEY' } })) }, providers: {} },
  user: { document: '/bound/settings.toml', revision: 'user-1', active: true, authored: { model: 'native/a' } },
  workspace: { document: '/workspace/rustx.toml', revision: 'workspace-1', active: false, authored: null },
  // Deliberately differs from authored inputs: rendering must use this projection.
  effective: { model: 'native/server-resolved' }, provenance: { 'agent.model.model': { kind: 'project', document: '/workspace/rustx.toml', base: '/workspace' } }, resolution_available: true,
} });
function client(handler: (operation: Request1) => Promise<MethodResult>) {
  const request = vi.fn(handler);
  return { client: { request, getSnapshot: () => ({ views: {} }) } as unknown as AppServerClient, request };
}
it('renders backend effective model/provenance, native catalog choices and untrusted scope', async () => {
  const subject = client(async () => fixture());
  render(<Settings client={subject.client} sessionId="A" />);
  expect(screen.getByRole('status').textContent).toContain('Loading');
  await screen.findByText('native/server-resolved');
  expect(screen.getByText('Workspace · /workspace/rustx.toml')).toBeTruthy();
  expect(screen.getByLabelText('User model').querySelectorAll('option')).toHaveLength(3);
  expect((screen.getByRole('button', { name: 'Save Workspace' }).closest('fieldset') as HTMLFieldSetElement).disabled).toBe(true);
  expect(screen.getByText('No Providers configured. Add one to begin.')).toBeTruthy();
  expect(subject.request).toHaveBeenCalledTimes(1);
});
it('sends the exact scope revision and adopts fresh authoritative state after save', async () => {
  let reads = 0;
  const subject = client(async operation => {
    if (operation.method === 'settings/sourcesRead') { const result = fixture(); if (reads++) { result.projection.effective = { model: 'native/new-effective' }; result.projection.user.revision = 'user-2'; } return result; }
    return fixture();
  });
  render(<Settings client={subject.client} sessionId="A" />); await screen.findByText('native/server-resolved');
  fireEvent.change(screen.getByLabelText('User model'), { target: { value: 'native/b' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save User' }));
  await screen.findByText('native/new-effective');
  expect(subject.request.mock.calls[1][0]).toMatchObject({ method: 'settings/sourcesWrite', params: { expected_revision: 'user-1', mutation: { kind: 'user_model', authored: { model: 'native/b' } } } });
  expect(screen.getByRole('status').textContent).toContain('Source committed');
});
it('surfaces typed conflict, preserves draft, rereads and only retries on explicit Save', async () => {
  let reads = 0, writes = 0;
  const subject = client(async operation => {
    if (operation.method === 'settings/sourcesRead') { const result = fixture(); if (reads++) { result.projection.user.revision = 'user-2'; result.projection.catalog.revision = 'catalog-current'; } return result; }
    if (++writes === 1) throw new RpcFailure({ code: -32000, message: 'Operation rejected', data: { kind: 'source_conflict', scope: 'user', expected: 'user-1', actual: 'user-2' } });
    return fixture();
  });
  render(<Settings client={subject.client} sessionId="A" />); await screen.findByText('native/server-resolved');
  fireEvent.change(screen.getByLabelText('User model'), { target: { value: 'native/b' } }); fireEvent.click(screen.getByRole('button', { name: 'Save User' }));
  await screen.findByRole('alert'); await waitFor(() => expect(subject.request).toHaveBeenCalledTimes(3));
  expect(screen.getByRole('alert').textContent).toContain('Conflict');
  expect(screen.getByText(/Catalog revision: catalog-current/)).toBeTruthy();
  expect((screen.getByLabelText('User model') as HTMLSelectElement).value).toBe('native/b'); expect(writes).toBe(1);
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Save User' })));
  expect(subject.request.mock.calls[3][0]).toMatchObject({ params: { expected_revision: 'user-2' } });
});
it('resets Session with null and its own revision; uncertain writes are never replayed', async () => {
  const subject = client(async operation => { if (operation.method === 'settings/selectModel') throw new OutcomeUncertain(); return fixture(); });
  render(<Settings client={subject.client} sessionId="A" />); await screen.findByText('native/server-resolved');
  fireEvent.click(screen.getByRole('button', { name: 'Reset Session' })); fireEvent.click(screen.getByRole('button', { name: 'Save Session' }));
  await screen.findByRole('alert');
  expect(subject.request.mock.calls[1][0]).toEqual({ method: 'settings/selectModel', params: { session_id: 'A', expected_revision: '7', selection: null } });
  expect(screen.getByRole('alert').textContent).toContain('uncertain'); expect(subject.request.mock.calls.filter(([request]) => request.method === 'settings/selectModel')).toHaveLength(1);
});
it('offers retry for load failure and exposes native unconfigured state', async () => {
  let calls = 0;
  const subject = client(async () => { if (!calls++) throw new Error('offline'); const value = fixture(); value.projection.effective = null; value.projection.resolution_available = false; return value; });
  render(<Settings client={subject.client} sessionId="A" />); await screen.findByRole('alert');
  fireEvent.click(screen.getByRole('button', { name: 'Retry loading settings' })); await screen.findByText('Unconfigured or invalid sources');
});
it('keeps catalog authoring User scoped and sends structured credential references', async () => {
  const subject = client(async () => fixture());
  render(<Settings client={subject.client} sessionId="A" />); await screen.findByText('native/server-resolved');
  fireEvent.change(screen.getByLabelText('New Provider identity'), { target: { value: 'added' } });
  fireEvent.click(screen.getByRole('button', { name: 'Add Provider' }));
  fireEvent.change(screen.getByLabelText('Endpoint · added'), { target: { value: 'https://example.invalid/v1' } });
  fireEvent.change(screen.getByLabelText('Credential environment reference · added'), { target: { value: 'ADDED_KEY' } });
  fireEvent.click(screen.getByRole('button', { name: 'Add model to added' }));
  fireEvent.change(screen.getByLabelText('Model identity'), { target: { value: 'custom' } });
  fireEvent.change(screen.getByLabelText('Context window'), { target: { value: '128000' } });
  fireEvent.change(screen.getByLabelText('Maximum output tokens'), { target: { value: '4096' } });
  expect(document.querySelector('input[type=password]')).toBeNull();
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Save User catalog' })));
  expect(subject.request.mock.calls[1][0]).toMatchObject({ method: 'settings/sourcesWrite', params: { expected_revision: 'catalog-1', mutation: { kind: 'catalog', providers: { added: { base_url: 'https://example.invalid/v1', credential: { type: 'environment', variable: 'ADDED_KEY' }, models: [{ id: 'custom', context_window: '128000', max_output_tokens: 4096 }] } } } } });
});

it('round trips partial source policy without materializing model or unrelated defaults', async () => {
  const value = fixture();
  value.projection.user.authored = { request_params: { temperature: 0.3 } };
  const subject = client(async () => value);
  render(<Settings client={subject.client} sessionId="A" />); await screen.findByText('native/server-resolved');
  expect((screen.getByLabelText('User model') as HTMLSelectElement).value).toBe('');
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Save User' })));
  expect(subject.request.mock.calls[1][0]).toEqual({ method: 'settings/sourcesWrite', params: { session_id: 'A', expected_revision: 'user-1', mutation: { kind: 'user_model', authored: { request_params: { temperature: 0.3 } } } } });
  fireEvent.change(screen.getByLabelText('User reasoning profile'), { target: { value: 'catalog_default' } });
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Save User' })));
  expect(subject.request.mock.calls[3][0]).toMatchObject({ params: { mutation: { authored: { request_params: { temperature: 0.3 }, reasoning_profile: { mode: 'catalog_default' } } } } });
});

it('preserves Workspace omissions and removes only explicit fields or the entire selected layer on reset', async () => {
  const value = fixture(); value.projection.workspace.active = true;
  value.projection.workspace.authored = { max_output_tokens: { mode: 'catalog_default' }, summary_model: { mode: 'session' } };
  const subject = client(async () => value);
  render(<Settings client={subject.client} sessionId="A" />); await screen.findByText('native/server-resolved');
  fireEvent.change(screen.getByLabelText('Workspace output policy'), { target: { value: '' } });
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Save Workspace' })));
  expect(subject.request.mock.calls[1][0]).toEqual({ method: 'settings/sourcesWrite', params: { session_id: 'A', expected_revision: 'workspace-1', mutation: { kind: 'workspace_model', authored: { summary_model: { mode: 'session' } } } } });
  fireEvent.click(screen.getByRole('button', { name: 'Reset Workspace' }));
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Save Workspace' })));
  expect(subject.request.mock.calls[3][0]).toMatchObject({ params: { mutation: { kind: 'workspace_model', authored: null } } });
});

it('shows same-model prospective, current, and frozen request differences from native projections', async () => {
  const value = fixture();
  const invocation = (reasoningProfile: string, maxOutputTokens: number, temperature: number) => ({ model: 'native/a', protocol: 'openai_chat_completions' as const, contextWindow: 128000, modelMaxOutputTokens: 4096, maxOutputTokens, reasoningProfile, reasoningEnabled: true, requestParams: { temperature }, declaredCapabilities: { inputModalities: ['text' as const], outputModalities: ['text' as const], toolCalls: true, reasoning: true }, capabilities: { inputModalities: ['text' as const], outputModalities: ['text' as const], toolCalls: true, reasoning: true } });
  value.projection.effective = { model: 'native/a', reasoningProfile: 'new', maxOutputTokens: 1000 };
  value.projection.effective_request = invocation('new', 1000, 0.8);
  const subject = client(async () => value);
  subject.client.getSnapshot = () => ({ views: { A: { attachment: 'attached', snapshot: { capabilities: { revision: '0' }, model: { configured: { model: 'native/a', reasoningProfile: 'current' }, effective: invocation('current', 2000, 0.5), summary: { mode: 'session' } }, attempt: { model: { primary: invocation('old', 3000, 0.2), summary: { mode: 'session' } } } } } } }) as unknown as ReturnType<AppServerClient['getSnapshot']>;
  render(<Settings client={subject.client} sessionId="A" />);
  const prospective = await screen.findByRole('region', { name: 'Prospective effective request' });
  const current = screen.getByRole('region', { name: 'Runtime effective request' });
  const frozen = screen.getByRole('region', { name: 'Frozen request' });
  expect(prospective.textContent).toContain('native/a'); expect(current.textContent).toContain('native/a'); expect(frozen.textContent).toContain('native/a');
  expect(prospective.textContent).toContain('new'); expect(prospective.textContent).toContain('1000'); expect(prospective.textContent).toContain('0.8');
  expect(current.textContent).toContain('current'); expect(current.textContent).toContain('2000'); expect(current.textContent).toContain('0.5');
  expect(frozen.textContent).toContain('old'); expect(frozen.textContent).toContain('3000'); expect(frozen.textContent).toContain('0.2');
});

it('repairs a lost MCP save response by exact-source reread without replay', async () => {
  const value = fixture();
  const subject = client(async operation => {
    if (operation.method === 'settings/read') return { type: 'settings', revision: '7', settings: { cwd: '/workspace', no_automatic_skills: false, no_builtin_tools: false, no_direct_tools: false } };
    if (operation.method === 'settings/sourcesWrite' && operation.params.mutation.kind === 'mcp') {
      const mutation = operation.params.mutation;
      value.projection.integrations.mcp = [{ id: mutation.id, policy_state: 'absent', user: mutation.authored, workspace: null, winning: { kind: 'user', document: '/bound/settings.toml', base: '/bound' }, activation: 'disabled' }];
      value.projection.user.revision = 'committed';
      throw new OutcomeUncertain();
    }
    return value;
  });
  render(<Settings client={subject.client} sessionId="A" />); await screen.findByText('native/server-resolved');
  fireEvent.click(screen.getByRole('button', { name: 'Integrations' }));
  fireEvent.click(screen.getByRole('button', { name: 'Add User MCP server' }));
  fireEvent.change(screen.getByLabelText('Server identity'), { target: { value: 'exact' } });
  fireEvent.change(screen.getByLabelText('Command'), { target: { value: 'inert' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save MCP' }));
  await screen.findByText(/Authoritative reread matches/);
  expect(subject.request.mock.calls.filter(([r]) => r.method === 'settings/sourcesWrite')).toHaveLength(1);
  expect(screen.queryByLabelText('Command')).toBeNull();
});

it('isolates MCP form validation from hidden model drafts and explicitly discards integration drafts', async () => {
  const subject = client(async operation => operation.method === 'settings/read' ? { type: 'settings', revision: '7', settings: { cwd: '/workspace', no_automatic_skills: false, no_builtin_tools: false, no_direct_tools: false } } : fixture());
  render(<Settings client={subject.client} sessionId="A" />); await screen.findByText('native/server-resolved');
  fireEvent.change(screen.getByLabelText('New Provider identity'), { target: { value: 'unsaved' } });
  fireEvent.click(screen.getByRole('button', { name: 'Add Provider' }));
  fireEvent.click(screen.getByRole('button', { name: 'Integrations' }));
  fireEvent.click(screen.getByRole('button', { name: 'Add User MCP server' }));
  fireEvent.change(screen.getByLabelText('Server identity'), { target: { value: 'independent' } });
  fireEvent.change(screen.getByLabelText('Command'), { target: { value: 'inert' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save MCP' }));
  await waitFor(() => expect(subject.request.mock.calls.some(([r]) => r.method === 'settings/sourcesWrite' && r.params.mutation.kind === 'mcp')).toBe(true));
  await waitFor(() => expect(screen.queryByLabelText('Command')).toBeNull());
  fireEvent.click(screen.getByRole('button', { name: 'Add User MCP server' }));
  fireEvent.change(screen.getByLabelText('Command'), { target: { value: 'discard-me' } });
  fireEvent.click(screen.getByRole('button', { name: 'Reload / discard draft' }));
  await waitFor(() => expect(screen.queryByLabelText('Command')).toBeNull());
});

it.each([false, true])('repairs uncertain User policy post-state (reset=%s) with exactly one write', async reset => {
  const value = fixture();
  value.projection.integrations.mcp_tool_policies.exact = { approval: 'always', execution: 'foreground_only', concurrency: 'sequential' };
  value.projection.integrations.mcp = [{ id: 'exact', user: null, workspace: reset ? null : { enabled: false, transport: 'stdio', command: 'inert', args: [], cwd: null, url: null, retained_env: [], retained_headers: [], sensitive_env: {}, sensitive_headers: {} }, winning: reset ? null : { kind: 'project', document: '/workspace/rustx.toml', base: '/workspace' }, activation: reset ? null : 'disabled', policy_state: reset ? 'dangling' : 'valid' }];
  value.projection.workspace.active = !reset;
  value.projection.integrations.mcp_valid = !reset;
  const subject = client(async operation => {
    if (operation.method === 'settings/read') return { type: 'settings', revision: '7', settings: { cwd: '/workspace', no_automatic_skills: false, no_builtin_tools: false, no_direct_tools: false } };
    if (operation.method === 'settings/sourcesWrite' && operation.params.mutation.kind === 'mcp_policy') {
      const mutation = operation.params.mutation;
      if (mutation.authored) value.projection.integrations.mcp_tool_policies[mutation.id] = mutation.authored;
      else { delete value.projection.integrations.mcp_tool_policies[mutation.id]; value.projection.integrations.mcp = []; value.projection.integrations.mcp_valid = true; }
      value.projection.user.revision = 'observed-new-revision';
      throw new OutcomeUncertain();
    }
    return structuredClone(value);
  });
  render(<Settings client={subject.client} sessionId="A" />); await screen.findByText('native/server-resolved');
  fireEvent.click(screen.getByRole('button', { name: 'Integrations' }));
  fireEvent.change(screen.getByLabelText('Approval · exact'), { target: { value: 'never' } });
  fireEvent.click(screen.getByRole('button', { name: reset ? 'Reset User Tool policy' : 'Save User Tool policy' }));
  await screen.findByText(/Authoritative reread matches the requested source state/);
  const writes = subject.request.mock.calls.filter(([r]) => r.method === 'settings/sourcesWrite');
  expect(writes).toHaveLength(1);
  expect(writes[0][0]).toMatchObject({ params: { expected_revision: 'user-1', mutation: { kind: 'mcp_policy', id: 'exact', authored: reset ? null : { approval: 'never', execution: 'foreground_only', concurrency: 'sequential' } } } });
  expect(screen.queryByText(/Save outcome uncertain/)).toBeNull();
  expect(screen.queryByText(/Draft revision:/)).toBeNull();
});

it('keeps policy CAS pinned across another Settings mutation and refreshes only for explicit retry', async () => {
  const value = fixture();
  value.projection.integrations.mcp = [{ id: 'exact', policy_state: 'absent', user: { enabled: false, transport: 'stdio', command: 'inert', args: [], cwd: null, url: null, retained_env: [], retained_headers: [], sensitive_env: {}, sensitive_headers: {} }, workspace: null, winning: { kind: 'user', document: '/bound/settings.toml', base: '/bound' }, activation: 'disabled' }];
  const subject = client(async operation => {
    if (operation.method === 'settings/read') return { type: 'settings', revision: '7', settings: { cwd: '/workspace', no_automatic_skills: false, no_builtin_tools: false, no_direct_tools: false } };
    if (operation.method === 'settings/sourcesWrite') {
      if (operation.params.expected_revision !== value.projection.user.revision) throw new RpcFailure({ code: -32000, message: 'Operation rejected', data: { kind: 'source_conflict', scope: 'user', expected: operation.params.expected_revision, actual: value.projection.user.revision } });
      value.projection.user.revision = 'user-2';
      if (operation.params.mutation.kind === 'mcp_policy') value.projection.integrations.mcp_tool_policies.exact = operation.params.mutation.authored!;
    }
    return structuredClone(value);
  });
  render(<Settings client={subject.client} sessionId="A" />); await screen.findByText('native/server-resolved');
  fireEvent.click(screen.getByRole('button', { name: 'Integrations' }));
  fireEvent.change(screen.getByLabelText('Approval · exact'), { target: { value: 'always' } });
  fireEvent.click(screen.getByLabelText('goal'));
  await screen.findByText(/Source committed/);
  expect(screen.getByText(/User revision: user-2 · Draft revision: user-1/)).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Save User Tool policy' }));
  await screen.findByRole('button', { name: 'Retry User Tool policy' });
  expect(screen.getByRole('alert').textContent).toContain('Conflict');
  expect((screen.getByLabelText('Approval · exact') as HTMLSelectElement).value).toBe('always');
  fireEvent.click(screen.getByRole('button', { name: 'Retry User Tool policy' }));
  await screen.findByText(/Source committed/);
  const writes = subject.request.mock.calls.flatMap(([r]) => r.method === 'settings/sourcesWrite' && r.params.mutation.kind === 'mcp_policy' ? [r.params] : []);
  expect(writes.map(r => r.expected_revision)).toEqual(['user-1', 'user-2']);
  expect(writes[0].mutation).toEqual(writes[1].mutation);
});

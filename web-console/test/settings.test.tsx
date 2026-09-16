// @vitest-environment jsdom
import { afterEach, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import type { MethodResult, Request1 } from '../../protocol/app-server/v5';
import { AppServerClient, OutcomeUncertain, RpcFailure } from '../src/client/app-server';
import { Settings } from '../src/app/settings/Settings';
afterEach(cleanup);
type Read = Extract<MethodResult, { type: 'source_settings' }>;
const fixture = (): Read => ({ type: 'source_settings', session_revision: '7', session_selection: { model: 'native/a' }, projection: {
  catalog: { document: '/bound/models.toml', revision: 'catalog-1', models: { models: ['native/a', 'native/b'].map(model => ({ model, protocol: 'openai_chat_completions' as const, contextWindow: 128000, maxOutputTokens: 4096, declaredCapabilities: { inputModalities: ['text' as const], outputModalities: ['text' as const], toolCalls: true, reasoning: false }, effectiveCapabilities: { inputModalities: ['text' as const], outputModalities: ['text' as const], toolCalls: true, reasoning: false }, credentialSource: { type: 'environment' as const, variable: 'FIXTURE_KEY' } })) }, providers: {} },
  user: { document: '/bound/settings.toml', revision: 'user-1', active: true, selection: { model: 'native/a' } },
  workspace: { document: '/workspace/rustx.toml', revision: 'workspace-1', active: false, selection: null },
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
  expect(subject.request.mock.calls[1][0]).toMatchObject({ method: 'settings/sourcesWrite', params: { expected_revision: 'user-1', mutation: { kind: 'user_model', selection: { model: 'native/b' } } } });
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

import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { NewConversation } from '../src/app/new-conversation/NewConversation';
import { NavigationEpoch } from '../src/app/commands/native';
import { RpcFailure } from '../src/client/app-server';
import { WorkspaceHostError, type ProductHostWorkspaces } from '../src/workspaces/host';
import type { CatalogModelView, SourceSettings } from '../../protocol/app-server/v22';
import { cfg3Source } from './cfg3-data';
import { Server } from './fixture';
let server: Server;
afterEach(() => { cleanup(); server?.client.disconnect(); });
const capabilities = { inputModalities: ['text' as const], outputModalities: ['text' as const], toolCalls: true, reasoning: true };
function nativeModel(model: string, profiles: string[] = [], defaultReasoningProfile?: string): CatalogModelView {
  return { model, protocol: 'openai_responses', contextWindow: 128000, maxOutputTokens: 8192, declaredCapabilities: capabilities, effectiveCapabilities: capabilities,
    reasoningProfiles: profiles.map(id => ({ id, enabled: true })), ...(defaultReasoningProfile ? { defaultReasoningProfile } : {}), credentialSource: { type: 'environment', variable: 'KEY' } };
}
const authored = { provider: 'transport', id: 'wire', protocol: 'openai_responses' as const, context_window: '128000', max_output_tokens: 8192, capabilities: { input_modalities: ['text' as const], output_modalities: ['text' as const], tool_calls: true, reasoning: false } };
/** A Workspace source read whose configuration documents name a model the
 * native Session catalog does not publish, and whose catalog carries native
 * reasoning vocabulary the documents never mention. */
function workspaceSource(session_models: SourceSettings['session_models'] = { kind: 'available', catalog: { models: [nativeModel('fixture/native', ['low', 'high'], 'high'), nativeModel('fixture/second')] } }): SourceSettings {
  return { ...cfg3Source(), target: { kind: 'workspace', directory: '/workspace' }, prospective_approval_mode: 'policy',
    resolved: { models: { 'fixture/configuration-only': authored, 'fixture/native': authored } }, session_models };
}
async function mount({ current = () => true, source = workspaceSource(), configureWorkspace, ready = true }: { current?: () => boolean; source?: SourceSettings; configureWorkspace?: (id: string) => Promise<{ kind: 'read'; projection: SourceSettings }>; ready?: boolean } = {}) {
  server = new Server(); await server.connect();
  const host = { ...server.workspaceHost,
    listWorkspaces: vi.fn(server.workspaceHost.listWorkspaces),
    resolveWorkspace: vi.fn(async () => ({ cwd: '/workspace' })),
    configureWorkspace: vi.fn<NonNullable<ProductHostWorkspaces['configureWorkspace']>>(configureWorkspace ?? (async () => ({ kind: 'read' as const, projection: source }))),
  };
  const opened = vi.fn();
  await act(async () => { render(<NewConversation client={server.client} host={host} initialWorkspace="workspace-a" current={current} opened={opened}/>); });
  fireEvent.change(screen.getByRole('textbox', { name: 'Message' }), { target: { value: 'Preserved draft' } });
  if (ready) await waitFor(() => expect((screen.getByRole('button', { name: 'Send' }) as HTMLButtonElement).disabled).toBe(false));
  return { host, opened };
}
const methods = () => server.requests.map(r => r.request.method);
const modelChoices = () => screen.queryAllByRole('menuitem').map(item => item.textContent ?? '').filter(label => label.startsWith('fixture/'));
async function openModelMenu() {
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Model and reasoning' })));
  fireEvent.click(screen.getByRole('menuitem', { name: 'Model' }));
}
function gate<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(done => { resolve = done; }); return { promise, resolve }; }
it.each(['resolve', 'create'] as const)('known %s rejection keeps the actual composer editable and retries only on a new gesture', async phase => {
  const { host, opened } = await mount();
  if (phase === 'resolve') host.resolveWorkspace.mockRejectedValueOnce(new WorkspaceHostError('Workspace revoked'));
  else server.handlers.set('session/create', () => { throw new RpcFailure({ code: -32000, message: 'Create rejected' }); });
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Send' })));
  expect(screen.getByRole('alert').textContent).toContain('No Session was created');
  const input = screen.getByRole('textbox', { name: 'Message' }) as HTMLTextAreaElement;
  expect(input.disabled).toBe(false); expect(input.value).toBe('Preserved draft'); expect(opened).not.toHaveBeenCalled();
  const calls = () => server.requests.filter(r => r.request.method === 'session/create');
  expect(calls()).toHaveLength(phase === 'resolve' ? 0 : 1);
  server.held.add('session/create');
  fireEvent.change(input, { target: { value: 'Corrected draft' } });
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Send' })));
  expect(calls()).toHaveLength(phase === 'resolve' ? 1 : 2);
});
it('uncertain creation preserves the draft, refuses another Send and requires native inspection', async () => {
  const { host, opened } = await mount();
  const previousResolutions = host.resolveWorkspace.mock.calls.length;
  host.resolveWorkspace.mockRejectedValueOnce(new WorkspaceHostError('Unknown outcome', undefined, true));
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Send' })));
  expect(screen.getByRole('alert').textContent).toContain('Inspect native Sessions');
  expect((screen.getByRole('button', { name: 'Send' }) as HTMLButtonElement).disabled).toBe(true);
  expect((screen.getByRole('textbox', { name: 'Message' }) as HTMLTextAreaElement).value).toBe('Preserved draft');
  fireEvent.click(screen.getByRole('button', { name: 'Send' }));
  expect(host.resolveWorkspace).toHaveBeenCalledTimes(previousResolutions + 1);
  expect(opened).not.toHaveBeenCalled();
});

it('confirmed create survives a transport replacement: it opens the exact committed Session without continuing native effects', async () => {
  const { opened } = await mount();
  server.held.add('session/create');
  server.handlers.set('session/create', () => ({ type: 'session_transition', session: { id: 'native-committed', active_node: 'node-native', active_conversation_id: 'conversation-native', node_count: 1, created_at: '0', updated_at: '0' } }));
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Send' })));
  const request = await server.waitFor('session/create', 1);
  // Deliver the acknowledgement first; the transport replacement then occurs
  // before XState starts its next, separately fenced continuation.
  await act(async () => { server.reply(request); await Promise.resolve(); server.socket.close(); });
  await waitFor(() => expect(opened).toHaveBeenCalledWith('native-committed', expect.stringContaining('Authority changed')));
  expect(server.requests.filter(r => ['session/attach', 'session/setModel', 'session/upload', 'turn/start'].includes(r.request.method))).toHaveLength(0);
  expect(server.requests.filter(r => r.request.method === 'session/create')).toHaveLength(1);
  await act(async () => server.connect());
  expect(server.requests.filter(r => r.request.method === 'session/create')).toHaveLength(1);
  expect(opened).toHaveBeenCalledTimes(1);
});

it('a replaced New Conversation navigation cannot open a Session committed by its older route', async () => {
  const navigation = new NavigationEpoch(); const { opened } = await mount({ current: navigation.capture() });
  server.held.add('session/create');
  server.handlers.set('session/create', () => ({ type: 'session_transition', session: { id: 'native-committed', active_node: 'node-native', active_conversation_id: 'conversation-native', node_count: 1, created_at: '0', updated_at: '0' } }));
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Send' })));
  const request = await server.waitFor('session/create', 1);
  await act(async () => { server.reply(request); navigation.invalidate(); });
  expect(screen.getByRole('alert').textContent).toContain('Session native-committed was created');
  expect(server.requests.filter(r => ['session/attach', 'session/setModel', 'turn/start'].includes(r.request.method))).toHaveLength(0);
  expect(opened).not.toHaveBeenCalled(); // obsolete navigation cannot take over a newer route
  expect((screen.getByRole('button', { name: 'Send' }) as HTMLButtonElement).disabled).toBe(true);
});

it('offers exactly the native Session catalog, in native order, never a configuration-only model', async () => {
  await mount();
  await openModelMenu();
  expect(modelChoices()).toEqual(['fixture/native', 'fixture/second']);
  expect(screen.queryByRole('menuitem', { name: 'fixture/configuration-only' })).toBeNull();
  await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'fixture/native' })));
  // The native default reasoning profile is shown as published, not invented.
  expect(screen.getByRole('button', { name: 'Model and reasoning' }).textContent).toBe('fixture/nativehigh');
  fireEvent.click(screen.getByRole('menuitem', { name: 'Reasoning profile' }));
  expect(['low', 'high'].map(name => !!screen.queryByRole('menuitem', { name }))).toEqual([true, true]);
  expect(screen.queryByRole('menuitem', { name: 'fixture/configuration-only' })).toBeNull();
});
it('a model choice is draft Session intent: nothing is written, created or started until Send, and the first turn waits for its native observation', async () => {
  const { host } = await mount();
  await openModelMenu();
  await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'fixture/native' })));
  fireEvent.click(screen.getByRole('menuitem', { name: 'Reasoning profile' }));
  await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'low' })));
  expect(host.configureWorkspace.mock.calls.every(([, , operation]) => operation.kind === 'read')).toBe(true);
  expect(methods().filter(method => ['session/create', 'session/setModel', 'configuration/sourceWrite', 'turn/start'].includes(method))).toEqual([]);
  server.handlers.set('session/create', () => ({ type: 'session_transition', session: { id: 'A', active_node: 'node-A', active_conversation_id: 'conversation-A', node_count: 1, created_at: '0', updated_at: '0' } }));
  server.held.add('session/setModel');
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Send' })));
  const request = await server.waitFor('session/setModel', 1);
  expect(request.params).toEqual({ target: server.target('A'), config: { model: 'fixture/native', reasoningProfile: 'low' } });
  expect(methods()).not.toContain('turn/start');
  expect(host.configureWorkspace.mock.calls.every(([, , operation]) => operation.kind === 'read')).toBe(true);
});
it.each([
  ['read failure', async () => { throw new Error('Workspace read failed'); }, 'Workspace read failed'],
  ['native unavailability', async () => ({ kind: 'read' as const, projection: workspaceSource({ kind: 'unavailable', diagnostic: 'unknown catalog model local/missing' }) }), 'unknown catalog model local/missing'],
] as const)('catalog %s offers no fabricated choice and creates no Session', async (_, configureWorkspace, message) => {
  await mount({ configureWorkspace, ready: false });
  await waitFor(() => expect(screen.getAllByRole('alert').some(alert => alert.textContent?.includes(message))).toBe(true));
  expect((screen.getByRole('button', { name: 'Model and reasoning' }) as HTMLButtonElement).disabled).toBe(true);
  expect(modelChoices()).toEqual([]);
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Send' })));
  expect(methods()).not.toContain('session/create');
});
it('an obsolete Workspace catalog read cannot replace the current Workspace catalog', async () => {
  const obsolete = gate<{ kind: 'read'; projection: SourceSettings }>();
  const current = workspaceSource({ kind: 'available', catalog: { models: [nativeModel('fixture/current-workspace')] } });
  await mount({ configureWorkspace: async id => id === 'workspace-a' ? obsolete.promise : { kind: 'read', projection: current }, ready: false });
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Choose Workspace' })));
  await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'Workspace B' })));
  await openModelMenu();
  expect(modelChoices()).toEqual(['fixture/current-workspace']);
  await act(async () => obsolete.resolve({ kind: 'read', projection: workspaceSource({ kind: 'available', catalog: { models: [nativeModel('fixture/obsolete-workspace')] } }) }));
  expect(modelChoices()).toEqual(['fixture/current-workspace']);
  expect(screen.queryByRole('menuitem', { name: 'fixture/obsolete-workspace' })).toBeNull();
});
it('a draft model the native catalog stops publishing blocks Send instead of reaching Session creation', async () => {
  let projection = workspaceSource();
  const { host } = await mount({ configureWorkspace: async () => ({ kind: 'read', projection }) });
  await openModelMenu();
  await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'fixture/second' })));
  const reads = host.configureWorkspace.mock.calls.length;
  projection = workspaceSource({ kind: 'available', catalog: { models: [nativeModel('fixture/native', ['low', 'high'], 'high')] } });
  await act(async () => { server.socket.deliver({ jsonrpc: '2.0', method: 'configuration/changed', params: { application: {
    scope: 'source:workspace:/workspace', sources: [{ kind: 'workspace', directory: '/workspace' }], version: '2',
    desired: { input_revision: 'input-2', attempt: '2' }, units: {}, candidate: null, eligibility: { status: 'unavailable' } } } }); });
  await waitFor(() => expect(host.configureWorkspace.mock.calls.length).toBeGreaterThan(reads));
  await waitFor(() => expect((screen.getByRole('button', { name: 'Send' }) as HTMLButtonElement).disabled).toBe(true));
  expect(screen.getByText(/not in this Workspace's native model catalog/)).toBeTruthy();
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Send' })));
  expect(methods()).not.toContain('session/create');
});
it('a transport drop keeps the draft mounted with no Product Host traffic, and the next Send binds the reconnected generation', async () => {
  const { host } = await mount();
  const lists = host.listWorkspaces.mock.calls.length, reads = host.configureWorkspace.mock.calls.length;
  await act(async () => server.socket.close());
  expect((screen.getByRole('button', { name: 'Send' }) as HTMLButtonElement).disabled).toBe(true);
  expect((screen.getByRole('textbox', { name: 'Message' }) as HTMLTextAreaElement).value).toBe('Preserved draft');
  expect(host.listWorkspaces).toHaveBeenCalledTimes(lists); expect(host.configureWorkspace).toHaveBeenCalledTimes(reads);
  await act(async () => server.connect());
  await waitFor(() => expect((screen.getByRole('button', { name: 'Send' }) as HTMLButtonElement).disabled).toBe(false));
  expect(host.listWorkspaces).toHaveBeenCalledTimes(lists);
  expect((screen.getByRole('textbox', { name: 'Message' }) as HTMLTextAreaElement).value).toBe('Preserved draft');
  server.held.add('session/create');
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Send' })));
  expect((await server.waitFor('session/create', 1)).params).toEqual({ settings: { cwd: '/workspace' } });
});

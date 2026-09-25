import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { App } from '../src/app/App';
import { AgentComposer } from '../src/app/agent/AgentComposer';
import { AppFrame } from '../src/presentation/layout/AppFrame';
import { modelPreferences, NewSessionModelPreference } from '../src/app/model-preference';
import { inputTrigger } from '../src/app/composer/input-trigger';
import { cfg3Source } from './cfg3-data';
import { Server, snapshot, endpoint } from './fixture';
import type { CatalogModelView, RuntimeClientSnapshot, SessionModelConfig, SourceSettings } from '../../protocol/app-server/v22';

// These spies execute the actual functions, including their hooks. Calls count
// render invocations, not merely DOM mutation or wrapper/parent renders.
vi.mock('../src/app/agent/AgentComposer', async original => {
  const module = await original<typeof import('../src/app/agent/AgentComposer')>();
  return { ...module, AgentComposer: vi.fn(module.AgentComposer) };
});
vi.mock('../src/presentation/layout/AppFrame', async original => {
  const module = await original<typeof import('../src/presentation/layout/AppFrame')>();
  return { ...module, AppFrame: vi.fn(module.AppFrame) };
});
let server: Server;
beforeEach(() => { localStorage.clear(); server = new Server(); });
afterEach(() => { cleanup(); server.client.disconnect(); vi.useRealTimers(); });
const input = () => screen.getByRole('textbox', { name: 'Message' }) as HTMLTextAreaElement;
function live(text = 'first token'): RuntimeClientSnapshot {
  return { ...snapshot(), transcript: { entries: [], statistics: { turns: '1', steps: '2', completed_responses: '0', model_requests: '1', requests_with_usage: '0', latest_turn: { attempt_id: 'exact-attempt', started_at: '2026-09-25T00:00:00Z' } } },
    attempt: { attempt_id: 'exact-attempt', phase: { type: 'running' }, turn: 2, in_flight: { message_id: 'exact-message', blocks: [{ type: 'text', block_index: 0, text }] } } };
}
it('streaming publications update the transcript but do not render AppFrame or composer; local clock ticks stay local', async () => {
  server.snapshots.set('A', live());
  await server.attached('A');
  localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint, openViews: ['A'] }));
  await act(async () => { render(<App client={server.client} workspaceHost={server.workspaceHost}/>); });
  const message = input(), seat = message.closest('[data-composer-seat]'), header = screen.getByLabelText('Session title').closest('header');
  fireEvent.change(message, { target: { value: 'untouched draft' } }); message.focus(); message.setSelectionRange(4, 7);
  const frameCalls = vi.mocked(AppFrame).mock.calls.length, composerCalls = vi.mocked(AgentComposer).mock.calls.length;
  for (let index = 0; index < 5; index++) await act(async () => server.update('A', live('token ' + index)));
  expect(screen.getByText('token 4')).toBeTruthy();
  expect(input()).toBe(message); expect(message.closest('[data-composer-seat]')).toBe(seat);
  expect(screen.getByLabelText('Session title').closest('header')).toBe(header);
  expect(vi.mocked(AppFrame).mock.calls.length).toBe(frameCalls);
  expect(vi.mocked(AgentComposer).mock.calls.length).toBe(composerCalls);
  expect(document.activeElement).toBe(message); expect([message.selectionStart, message.selectionEnd]).toEqual([4, 7]);
  expect(message.value).toBe('untouched draft'); expect(header?.textContent).not.toContain('/workspace');
  // Explicit clock synchronization, not a sleep. Start a new exact native
  // attempt so the timer is installed under the fake clock.
  vi.useFakeTimers(); vi.setSystemTime(new Date('2026-09-25T00:00:00Z'));
  const next = live(); next.attempt!.attempt_id = 'clock-attempt'; next.transcript.statistics!.latest_turn!.attempt_id = 'clock-attempt';
  await act(async () => server.update('A', next));
  const before = [vi.mocked(AppFrame).mock.calls.length, vi.mocked(AgentComposer).mock.calls.length];
  await act(async () => vi.advanceTimersByTime(5000));
  expect(screen.getByRole('button', { name: 'Deep diving for 5s' })).toBeTruthy();
  expect([vi.mocked(AppFrame).mock.calls.length, vi.mocked(AgentComposer).mock.calls.length]).toEqual(before);
  expect(screen.queryByText('Working…')).toBeNull();
});

const model = (id: string): CatalogModelView => ({ model: id, protocol: 'openai_responses', contextWindow: 8192, maxOutputTokens: 1024, credentialSource: { type: 'environment', variable: 'KEY' }, declaredCapabilities: { inputModalities: ['text'], outputModalities: ['text'], reasoning: false, toolCalls: true }, effectiveCapabilities: { inputModalities: ['text'], outputModalities: ['text'], reasoning: false, toolCalls: true }, reasoningProfiles: [] });
function host() {
  const source: SourceSettings = { ...cfg3Source(), target: { kind: 'workspace', directory: '/workspace/A' }, prospective_approval_mode: 'policy', session_models: { kind: 'available', default_model: { model: 'fixture/root' }, catalog: { models: [model('fixture/root'), model('fixture/chosen')] } } };
  return { ...server.workspaceHost, configureWorkspace: async () => ({ kind: 'read' as const, projection: source }), resolveWorkspace: async () => ({ cwd: '/workspace/A' }), classifyLocations: async (paths: string[]) => paths.map(() => ({ authorized: true, workspaceId: 'workspace-a' })) };
}
function nativeModel() {
  server.handlers.set('session/create', () => ({ type: 'session_transition', session: { id: 'A', active_node: 'node-A', active_conversation_id: 'conversation-A', node_count: 1, created_at: '0', updated_at: '0' } }));
  server.handlers.set('session/setModel', request => {
    if (request.method !== 'session/setModel') throw Error('wrong request');
    const selection = request.params.config;
    const value = { configured: selection, effective: { model: selection.model, reasoningProfile: selection.reasoningProfile } } as NonNullable<RuntimeClientSnapshot['model']>;
    server.snapshots.set('A', { ...snapshot(), model: value });
    return { type: 'model', model: value };
  });
}
it('hero and committed Session retain the exact composer card/input; internal phases add no layout rows', async () => {
  await server.connect(); nativeModel();
  modelPreferences().select(endpoint, { model: 'fixture/chosen' });
  server.held.add('session/create'); server.held.add('session/attach'); server.held.add('turn/start');
  await act(async () => { render(<App client={server.client} workspaceHost={host()}/>); });
  fireEvent.click(screen.getByRole('button', { name: 'Choose Workspace' }));
  await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'Workspace A' })));
  const message = input(), card = message.closest('[data-composer-card]'), seat = message.closest('[data-composer-seat]');
  fireEvent.change(message, { target: { value: 'first task' } }); message.focus(); message.setSelectionRange(4, 4);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Send' })).toHaveProperty('disabled', false));
  fireEvent.keyDown(message, { key: 'Enter', isComposing: true });
  expect(server.requests.some(row => row.request.method === 'session/create')).toBe(false);
  fireEvent.keyDown(message, { key: 'Enter' });
  const created = await server.waitFor('session/create', 1);
  expect(input()).toBe(message); expect(document.activeElement).toBe(message);
  expect(screen.queryByText(/creating…|attaching…|creating_session|attaching_session/i)).toBeNull();
  await act(async () => server.reply(created));
  const attached = await server.waitFor('session/attach', 1);
  expect(input()).toBe(message); expect(message.closest('[data-composer-card]')).toBe(card);
  await act(async () => server.reply(attached));
  const sent = await server.waitFor('turn/start', 1);
  expect(input()).toBe(message); expect(message.selectionStart).toBe(4);
  await act(async () => server.reply(sent));
  expect(screen.getByLabelText('Session title')).toBeTruthy();
  expect(input()).toBe(message); expect(message.closest('[data-composer-seat]')).toBe(seat);
  expect(document.activeElement).toBe(message); expect(message.value).toBe('');
  expect(server.client.getSnapshot().views.A.snapshot?.model?.configured.model).toBe('fixture/chosen');
});

it.each([false, true])('one launcher grammar preserves unrelated draft and caret (active=%s)', async active => {
  if (active) { await server.attached('A'); localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint, openViews: ['A'] })); }
  else await server.connect();
  await act(async () => { render(<App client={server.client} workspaceHost={host()}/>); });
  const message = input(); fireEvent.change(message, { target: { value: 'unrelated prose' } }); message.focus(); message.setSelectionRange(3, 6);
  fireEvent.mouseDown(screen.getByRole('button', { name: 'Commands' })); fireEvent.click(screen.getByRole('button', { name: 'Commands' }));
  const launcher = screen.getByRole('listbox');
  expect(message.value).toBe('unrelated prose'); expect(document.activeElement).toBe(message); expect([message.selectionStart, message.selectionEnd]).toEqual([3, 6]);
  const choices = launcher.textContent;
  fireEvent.keyDown(message, { key: 'Escape' }); expect(screen.queryByRole('listbox')).toBeNull();
  fireEvent.change(message, { target: { value: '/' } });
  expect(screen.getByRole('listbox').textContent).toBe(choices);
  expect(inputTrigger(undefined, { type: 'toggle' })).toEqual({ source: 'launcher', query: '', highlight: 0 });
  expect(inputTrigger(undefined, { type: 'track', draft: '/' })).toEqual({ source: 'typed', query: '', highlight: 0 });
});

it('the durable preference is authority-scoped data, never an existing Session model or authored configuration', () => {
  const preference = new NewSessionModelPreference(localStorage);
  const existing: SessionModelConfig = { model: 'session-owned', reasoningProfile: 'high' };
  preference.select(endpoint, { model: 'last-used', reasoningProfile: 'low' });
  expect(new NewSessionModelPreference(localStorage).read(endpoint)).toEqual({ model: 'last-used', reasoningProfile: 'low' });
  expect(preference.read('ws://other/')).toBeUndefined(); expect(existing).toEqual({ model: 'session-owned', reasoningProfile: 'high' });
  expect(localStorage.getItem('rustx-console-view-v2')).toBeNull();
});

it.each(['/model', 'unrelated prose'])('hero model selection settles the command without consuming unrelated input (%s)', async draft => {
  await server.connect();
  await act(async () => { render(<App client={server.client} workspaceHost={host()}/>); });
  fireEvent.click(screen.getByRole('button', { name: 'Choose Workspace' }));
  await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'Workspace A' })));
  const message = input();
  fireEvent.change(message, { target: { value: draft } });
  if (draft !== '/model') fireEvent.click(screen.getByRole('button', { name: 'Commands' }));
  fireEvent.keyDown(message, { key: 'Tab' });
  fireEvent.click(screen.getByRole('menuitem', { name: 'Model' }));
  await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'fixture/chosen' })));
  await waitFor(() => expect(message.value).toBe(draft === '/model' ? '' : draft));
  expect(screen.queryByRole('menu')).toBeNull();
  expect(document.activeElement).toBe(message);
});

it('a confirmed native model selection seeds the next Session without modifying other Sessions', async () => {
  const projection = { configured: { model: 'fixture/root' }, effective: { model: 'fixture/root' } } as NonNullable<RuntimeClientSnapshot['model']>;
  server.snapshots.set('A', { ...snapshot('A'), model: projection });
  server.snapshots.set('B', { ...snapshot('B'), model: structuredClone(projection) });
  nativeModel();
  server.handlers.set('session/models', () => ({ type: 'models', catalog: { models: [model('fixture/root'), model('fixture/chosen')] } }));
  server.handlers.set('session/model', () => ({ type: 'model', model: server.snapshots.get('A')!.model! }));
  await server.attached('A', 'B');
  localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint, openViews: ['A', 'B'] }));
  await act(async () => { render(<App client={server.client} workspaceHost={host()}/>); });
  fireEvent.click(screen.getByRole('button', { name: 'Model and reasoning' }));
  await waitFor(() => expect(screen.queryByText('Reading native models…')).toBeNull());
  fireEvent.click(screen.getByRole('menuitem', { name: 'Model' }));
  await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'fixture/chosen' })));
  expect(modelPreferences().read(endpoint)).toEqual({ model: 'fixture/chosen' });
  expect(server.client.getSnapshot().views.B.snapshot!.model!.configured).toEqual({ model: 'fixture/root' });
  await act(async () => fireEvent.click(screen.getAllByRole('button', { name: 'New Conversation' })[0]));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Model and reasoning' }).textContent).toContain('fixture/chosen'));
  expect(server.requests.filter(row => row.request.method === 'session/setModel')).toHaveLength(1);
  expect(server.requests.some(row => row.request.method === 'configuration/sourceWrite')).toBe(false);
});

it('an unavailable saved selection remains visibly invalid and cannot submit a silently substituted default', async () => {
  modelPreferences().select(endpoint, { model: 'removed/provider-model', reasoningProfile: 'removed-profile' });
  await server.connect();
  await act(async () => { render(<App client={server.client} workspaceHost={host()}/>); });
  fireEvent.click(screen.getByRole('button', { name: 'Choose Workspace' }));
  await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'Workspace A' })));
  fireEvent.change(input(), { target: { value: 'must not substitute' } });
  expect(screen.getByRole('button', { name: 'Model and reasoning' }).textContent).toContain('removed/provider-model');
  expect(screen.getByText(/removed\/provider-model is unavailable/)).toBeTruthy();
  expect(screen.getByRole('button', { name: 'Send' })).toHaveProperty('disabled', true);
  fireEvent.keyDown(input(), { key: 'Enter' });
  expect(server.requests.some(row => row.request.method === 'session/create')).toBe(false);
  expect(modelPreferences().read(endpoint)!.model).toBe('removed/provider-model');
});

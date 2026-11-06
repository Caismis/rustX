import { act, cleanup, fireEvent, render, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { App } from '../src/app/App';
import type { SessionLocation } from '../src/workspaces/host';
import { cfg3Source } from './cfg3-data';
import { Server, endpoint } from './fixture';
let server: Server;
afterEach(() => { cleanup(); server?.client.disconnect(); localStorage.clear(); vi.restoreAllMocks(); });
function gate<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(done => { resolve = done; }); return { promise, resolve }; }
async function fixture({ held = false, current = false, history = true, preservePreferences = false, hostAuthority = 'fixture-host' } = {}) {
  if (!preservePreferences) localStorage.clear(); server = new Server();
  const listWorkspaces = server.workspaceHost.listWorkspaces;
  server.workspaceHost.listWorkspaces = async () => ({ ...await listWorkspaces(), authorityId: hostAuthority });
  server.summaries.set('A', { updated_at: '2026-11-01T00:00:00Z' }); server.summaries.set('B', { updated_at: '2026-11-06T00:00:00Z' });
  if (!history) server.snapshots.clear();
  const classify = gate<void>(); if (!held) classify.resolve();
  server.workspaceHost.classifyLocations = async paths => { await classify.promise; return paths.map(cwd => ({ authorized: true, workspaceId: cwd === '/workspace/B' ? 'workspace-b' : 'workspace-a' } satisfies SessionLocation)); };
  server.workspaceHost.resolveWorkspace = async id => ({ cwd: id === 'workspace-b' ? '/workspace/B' : '/workspace/A' });
  const capabilities = { inputModalities: ['text' as const], outputModalities: ['text' as const], jsonSchemaOutput: false };
  server.workspaceHost.configureWorkspace = async id => ({ kind: 'read', projection: { ...cfg3Source(), target: { kind: 'workspace', directory: id === 'workspace-b' ? '/workspace/B' : '/workspace/A' }, prospective_approval_mode: 'policy',
    session_models: { kind: 'available', default_model: { model: 'fixture/native' }, catalog: { models: [{ model: 'fixture/native', protocol: 'openai_responses', contextWindow: 8192, maxOutputTokens: 1024, credentialSource: { type: 'environment', variable: 'KEY' }, declaredCapabilities: capabilities, effectiveCapabilities: capabilities, profiles: [] }] } } } });
  if (current) { await server.attached('A'); localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint, openViews: ['A'] })); } else await server.connect();
  const ui = render(<App client={server.client} workspaceHost={server.workspaceHost}/>);
  const picker = () => ui.getByRole('button', { name: 'Choose Workspace' });
  return { ui, picker, classify };
}
it('startup selects the most recently active registered Workspace and Send uses it without opening a picker', async () => {
  const f = await fixture();
  await waitFor(() => expect(f.picker().textContent).toContain('Workspace B'));
  expect(server.requests.some(row => row.request.method === 'session/create')).toBe(false);
  fireEvent.change(f.ui.getByRole('textbox', { name: 'Message' }), { target: { value: 'Use the default Workspace' } });
  await waitFor(() => expect((f.ui.getByRole('button', { name: 'Send' }) as HTMLButtonElement).disabled).toBe(false));
  server.held.add('session/create'); fireEvent.click(f.ui.getByRole('button', { name: 'Send' }));
  const request = await server.waitFor('session/create', 1); expect(request.params).toMatchObject({ settings: { cwd: '/workspace/B' } });
});
it('classification arriving after a manual Workspace choice preserves that choice, text and editor', async () => {
  const f = await fixture({ held: true });
  const editor = f.ui.getByRole('textbox', { name: 'Message' }); fireEvent.change(editor, { target: { value: 'Keep my input' } });
  fireEvent.click(f.picker()); fireEvent.click(await f.ui.findByRole('menuitem', { name: 'Workspace A' }));
  await act(async () => f.classify.resolve());
  await waitFor(() => expect(f.picker().textContent).toContain('Workspace A'));
  expect(f.ui.getByRole('textbox', { name: 'Message' })).toBe(editor); expect((editor as HTMLTextAreaElement).value).toBe('Keep my input');
});
it('New Conversation retains the current Session origin until classification can select its Workspace', async () => {
  const f = await fixture({ held: true, current: true });
  fireEvent.click(f.ui.getAllByRole('button', { name: 'New Conversation' })[0]);
  const editor = f.ui.getByRole('textbox', { name: 'Message' }); fireEvent.change(editor, { target: { value: 'New conversation in A' } });
  await act(async () => f.classify.resolve());
  await waitFor(() => expect(f.picker().textContent).toContain('Workspace A'));
  expect((editor as HTMLTextAreaElement).value).toBe('New conversation in A');
});
it('an empty Session history still selects an existing registration', async () => {
  const f = await fixture({ history: false }); await waitFor(() => expect(f.picker().textContent).toContain('Workspace A'));
});
it('restored Session selection takes precedence over the recent-Workspace default', async () => {
  const f = await fixture({ current: true });
  await waitFor(() => expect(f.ui.container.querySelector('#session-view')?.getAttribute('data-shell-route')).toBe('session'));
  expect(f.ui.container.querySelector('[data-draft-conversation]')).toBeNull();
  expect(server.requests.some(row => row.request.method === 'session/create')).toBe(false);
});
it('remembers an unsent picker choice across a fresh client and Product Host lifetime', async () => {
  const first = await fixture();
  await waitFor(() => expect(first.picker().textContent).toContain('Workspace B'));
  fireEvent.click(first.picker()); fireEvent.click(await first.ui.findByRole('menuitem', { name: 'Workspace A' }));
  expect(server.requests.some(row => row.request.method === 'session/create')).toBe(false);
  first.ui.unmount(); server.client.disconnect();
  const restarted = await fixture({ preservePreferences: true, hostAuthority: 'restarted-host' });
  await waitFor(() => expect(restarted.picker().textContent).toContain('Workspace A'));
  expect(server.requests.some(row => row.request.method === 'session/create')).toBe(false);
});
it('a manual sidebar choice restores the unsent route instead of reopening an older Session', async () => {
  const first = await fixture({ current: true });
  fireEvent.click(await first.ui.findByRole('button', { name: 'Select Workspace Workspace A' }));
  await waitFor(() => expect(first.picker().textContent).toContain('Workspace A'));
  first.ui.unmount(); server.client.disconnect();
  const restarted = await fixture({ preservePreferences: true });
  await waitFor(() => expect(restarted.picker().textContent).toContain('Workspace A'));
  expect(restarted.ui.container.querySelector('#session-view')?.getAttribute('data-shell-route')).toBe('new-conversation');
});

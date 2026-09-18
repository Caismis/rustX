import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it } from 'vitest';
import { App } from '../src/app/App';
import { sessionDisplayTitle } from '../src/bindings/session-title';
import { sessionDeletionNotice } from '../src/bindings/session-deletion';
import { deriveSessionProductState } from '../src/bindings/session-product';
import { Server, endpoint, interaction, snapshot } from './fixture';
import type { RuntimeClientSnapshot, RuntimeClientSessionDeletionResult } from '../../protocol/app-server/v6';

let server: Server;
beforeEach(() => { server = new Server(); localStorage.clear(); });
afterEach(() => { cleanup(); server.client.disconnect(); localStorage.clear(); });
async function mount(ids = ['A', 'B']) {
  await server.attached(...ids);
  localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint, openViews: ids }));
  await act(async () => { render(<App client={server.client} workspaceHost={server.workspaceHost} />); });
}
const row = (id: string) => document.querySelector<HTMLButtonElement>(`button[data-session-id="${id}"]`)!;
const select = async (id: string) => act(async () => fireEvent.click(row(id)));
const methods = () => server.requests.map(({ request }) => request.method);
it.each([
  [{ status: 'deleted', session_id: 'private-id' }, 'Session deleted.'],
  [{ status: 'not_found', session_id: 'private-id' }, 'no longer exists'],
  [{ status: 'stale', session_id: 'private-id' }, 'new deletion confirmation'],
  [{ status: 'committed_cleanup_pending', session_id: 'private-id' }, 'committed, but cleanup is still pending'],
  [{ status: 'committed_durability_uncertain', session_id: 'private-id' }, 'Deletion needs verification'],
  [{ status: 'blocked', session_id: 'private-id', reason: { kind: 'in_use' } }, 'Advanced Session controls'],
  [{ status: 'blocked', session_id: 'private-id', reason: { kind: 'current_session' } }, 'Select another Session'],
  [{ status: 'blocked', session_id: 'private-id', reason: { kind: 'workspace', resource_count: 2 } }, 'Retained workspaces'],
  [{ status: 'blocked', session_id: 'private-id', reason: { kind: 'invalid_ownership' } }, 'could not be verified'],
] as const)('preserves the native deletion outcome without raw DTOs: %j', (result, copy) => {
  const notice = sessionDeletionNotice(result as RuntimeClientSessionDeletionResult);
  expect(notice).toContain(copy); expect(notice).not.toContain('private-id');
});
function canonicalUser(content: string | null): RuntimeClientSnapshot {
  return { ...snapshot(), transcript: { entries: [{ cursor: '1', item: { type: 'message', message: { role: 'user', id: 'canonical-user', source: 'human', content: content === null ? [{ type: 'uploaded_file', batch_id: 'native-batch', name: 'notes.txt' }] : [{ type: 'text', text: content }] } } }] } };
}

it.each([
  [undefined, 'New session'], [{ name: null, preview: null }, 'New session'],
  [{ preview: 'Native first-message preview' }, 'Native first-message preview'],
  [{ name: 'Manual name', preview: 'Native preview' }, 'Manual name'],
])('displays catalog metadata without generating a title: %j', (summary, expected) => {
  expect(sessionDisplayTitle(summary)).toBe(expected);
});

it('uses only the Sidebar for selection; switching preserves concurrent work and Close view releases only its controller', async () => {
  server.snapshots.get('A')!.attempt = { attempt_id: 'attempt-private', phase: { type: 'running' }, turn: 1 };
  await mount();
  expect(screen.getAllByRole('tree', { name: 'Session browser' })).toHaveLength(1);
  expect(screen.getAllByRole('tablist').map(node => node.getAttribute('aria-label'))).toEqual(['Conversation view']);
  const before = methods().length;
  await select('B');
  expect(screen.getByLabelText('Session title').textContent).toBe('Session B');
  expect(row('A').textContent).toContain('Working…');
  await select('A');
  expect(screen.getByLabelText('Session title').textContent).toBe('Session A');
  expect(methods().slice(before).filter(method => ['session/attach', 'session/detach', 'session/unload', 'turn/cancel'].includes(method))).toEqual([]);
  fireEvent.click(screen.getByRole('button', { name: 'Session actions for Session A' }));
  await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'Close Session A view' })));
  expect(server.client.getSnapshot().views.A.attachmentIntent).toBe('released');
  expect(server.snapshots.get('A')!.attempt?.phase.type).toBe('running');
  expect(server.loaded.has('A')).toBe(true);
  expect(methods().slice(before).filter(method => ['session/detach', 'session/unload', 'turn/cancel'].includes(method))).toEqual(['session/detach']);
  expect(row('A')).toBeTruthy();
  expect(screen.getByLabelText('Session title').textContent).toBe('Session B');
});

it('identifies two unnamed previews and two empty Sessions without UUID fallbacks', async () => {
  server.summaries.set('A', { name: null, preview: 'Inspect the parser' });
  server.summaries.set('B', { name: null, preview: 'Write the release notes' });
  await mount();
  expect(row('A').getAttribute('aria-label')).toBe('Open Inspect the parser');
  expect(row('B').getAttribute('aria-label')).toBe('Open Write the release notes');
  expect(screen.getByLabelText('Session title').textContent).toBe('Inspect the parser');
  server.summaries.set('A', { name: null, preview: null }); server.summaries.set('B', { name: null, preview: null });
  await act(async () => server.client.listSessions());
  expect(screen.getAllByRole('button', { name: 'Open New session' })).toHaveLength(2);
  await select('B'); expect(screen.getByLabelText('Session title').textContent).toBe('New session');
  expect(row('A')).not.toBe(row('B'));
});

it('Connection shows product state without a generation counter', async () => {
  await mount();
  fireEvent.click(screen.getByRole('button', { name: 'Connection' }));
  expect(document.querySelector('.connection-status')!.textContent).toBe('Connected');
  expect(document.querySelector('.connection-status small')).toBeNull();
});

it.each([false, true])('refreshes native preview only after a committed canonical user message (file-only: %s)', async fileOnly => {
  server.summaries.set('A', { name: null, preview: null });
  await mount(['A']);
  const lists = () => methods().filter(method => method === 'session/list').length;
  const baseline = lists();
  await act(async () => server.client.send('A', 'Local draft is not a title', [], 'send'));
  expect(lists()).toBe(baseline);
  expect(screen.getByLabelText('Session title').textContent).toBe('New session');
  server.summaries.set('A', { name: null, preview: fileOnly ? null : 'Native canonical preview' });
  await act(async () => server.update('A', canonicalUser(fileOnly ? null : 'Do not summarize this browser text')));
  expect(lists()).toBe(baseline + 1);
  expect(screen.getByLabelText('Session title').textContent).toBe(fileOnly ? 'New session' : 'Native canonical preview');
  expect(row('A').getAttribute('aria-label')).toBe(`Open ${fileOnly ? 'New session' : 'Native canonical preview'}`);
  await act(async () => server.update('A', canonicalUser('Later text')));
  expect(lists()).toBe(baseline + 1);
  expect(methods()).not.toContain('session/name');
});

it('manual naming immediately wins and survives later messages and catalog pagination', async () => {
  server.summaries.set('A', { name: null, preview: 'First native preview' });
  await mount();
  server.handlers.set('session/name', request => {
    if (request.method !== 'session/name') throw new Error('wrong method');
    server.summaries.set('A', { ...server.summaries.get('A'), name: request.params.name });
    return { type: 'session', session: { id: 'A', name: request.params.name, active_node: 'node-A', active_conversation_id: 'conversation-A', node_count: 1, created_at: '0', updated_at: '0' } };
  });
  fireEvent.click(screen.getByRole('button', { name: 'Session actions for First native preview' }));
  fireEvent.click(screen.getByRole('menuitem', { name: 'Rename' }));
  fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'My explicit name' } });
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Save name' })));
  expect(screen.getByLabelText('Session title').textContent).toBe('My explicit name');
  await act(async () => server.update('A', canonicalUser('Later conversation')));
  await act(async () => server.client.listSessions(0, 'Session B'));
  expect(screen.getByLabelText('Session title').textContent).toBe('My explicit name');
  expect(methods().filter(method => method === 'session/name')).toHaveLength(1);
});

it('a delayed older Sidebar page cannot overwrite a newer committed native preview', async () => {
  server.summaries.set('A', { name: null, preview: null });
  await mount(['A']);
  server.held.add('session/list');
  const baseline = methods().filter(method => method === 'session/list').length;
  const oldPage = server.client.listSessions();
  const oldRequest = await server.waitFor('session/list', baseline + 1);
  server.commit(oldRequest);
  server.summaries.set('A', { name: null, preview: 'Committed native preview' });
  let convergence!: Promise<void>;
  await act(async () => { convergence = server.update('A', canonicalUser('Different browser text')); });
  const freshRequest = await server.waitFor('session/list', baseline + 2);
  await act(async () => { server.reply(freshRequest); await convergence; server.reply(oldRequest); await oldPage; });
  expect(screen.getByLabelText('Session title').textContent).toBe('Committed native preview');
  expect(row('A').getAttribute('aria-label')).toBe('Open Committed native preview');
});

it('restores an explicitly named empty Session outside the current catalog page', async () => {
  server.handlers.set('session/list', request => {
    if (request.method !== 'session/list') throw new Error('wrong method');
    const id = request.params.query === 'B' ? 'B' : 'A';
    return { type: 'sessions', residencies: { [id]: 'Unloaded' }, sessions: [{ id, cwd: `/workspace/${id}`, name: id === 'B' ? 'Named empty Session' : 'Session A', active_node: `node-${id}`, updated_at: '2026-09-14T00:00:00Z' }] };
  });
  await mount(['B']);
  expect(screen.getByLabelText('Session title').textContent).toBe('Named empty Session');
  expect(server.client.getSnapshot().sessions.map(row => row.id)).toEqual(['A']);
  expect(server.requests.filter(({ request }) => request.method === 'session/list').map(({ request }) => request.params)).toEqual([{ offset: 0, limit: 32, query: '' }, { offset: 0, limit: 32, query: 'B' }]);
});

it('A stays healthy while B uncertainty remains visible, scoped and never replayed when switching', async () => {
  await mount();
  server.held.add('turn/start');
  const lost = server.client.send('B', 'uncertain B', [], 'send').catch(() => {});
  const request = await server.waitFor('turn/start', 1);
  await act(async () => { server.socket.close(); await lost; await server.connect(); });
  expect(screen.queryByLabelText('Session status')).toBeNull();
  expect(row('B').textContent).toContain('Needs verification');
  expect(row('A').textContent).not.toContain('Needs verification');
  fireEvent.click(screen.getByRole('button', { name: 'Toggle Inspector' }));
  const selected = () => screen.getByRole('region', { name: 'Selected Session diagnostics' });
  expect(within(selected()).getByLabelText('Native diagnostic JSON').textContent).not.toContain(String(request.id));
  expect(JSON.parse(within(selected()).getByLabelText('Native diagnostic JSON').textContent!).uncertain_operations).toEqual([]);
  expect(screen.getByRole('region', { name: 'Global / other Session diagnostics' }).textContent).toContain(String(request.id));
  const before = methods().length;
  await select('B');
  expect(screen.getByLabelText('Session status').textContent).toContain('Needs verification');
  expect(JSON.parse(within(selected()).getByLabelText('Native diagnostic JSON').textContent!).uncertain_operations).toMatchObject([{ id: request.id, sessionId: 'B' }]);
  await select('A');
  expect(methods().slice(before).filter(method => ['turn/start', 'turn/cancel', 'session/unload', 'session/detach', 'session/attach'].includes(method))).toEqual([]);
  expect(methods().filter(method => method === 'turn/start')).toHaveLength(1);
});

it('unscoped uncertainty is global, not assigned to every Session; reviewed diagnostics acknowledge locally outside Inspector', async () => {
  await mount();
  server.held.add('session/create');
  const lost = server.client.request({ method: 'session/create', params: { settings: { cwd: '/workspace/A' } } }, 'session').catch(() => {});
  await server.waitFor('session/create', 1);
  await act(async () => { server.socket.close(); await lost; await server.connect(); });
  const state = server.client.getSnapshot();
  expect(state.uncertain[0].sessionId).toBeUndefined();
  for (const id of ['A', 'B']) expect(deriveSessionProductState(state, state.views[id]).status).toBe('idle');
  expect(screen.getByText(/A global operation needs verification/)).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Toggle Inspector' }));
  expect(JSON.parse(screen.getByLabelText('Native diagnostic JSON').textContent!).uncertain_operations).toEqual([]);
  const before = methods().length;
  fireEvent.click(screen.getByRole('button', { name: 'Connection' }));
  fireEvent.click(screen.getByText('Review uncertain operations'));
  fireEvent.click(screen.getByRole('button', { name: 'I have verified the affected work' }));
  expect(server.client.getSnapshot().uncertain).toEqual([]);
  expect(methods().slice(before)).toEqual([]);
});

it('interaction operation evidence is scoped and cannot be dismissed as a non-interaction diagnostic', async () => {
  const pending = interaction('approval', 'B');
  server.snapshots.get('B')!.pending_interactions = [pending];
  await mount();
  server.held.add('interaction/respond');
  await select('B');
  fireEvent.click(screen.getByRole('button', { name: 'Allow once' }));
  await server.waitFor('interaction/respond', 1);
  await act(async () => { server.socket.close(); await server.connect(); });
  await select('A');
  fireEvent.click(screen.getByRole('button', { name: 'Toggle Inspector' }));
  expect(JSON.parse(screen.getByLabelText('Native diagnostic JSON').textContent!).interactions).toEqual({});
  expect(screen.getByRole('region', { name: 'Selected Session diagnostics' }).textContent).not.toContain('interaction-approval');
  await select('B');
  const evidence = JSON.parse(screen.getByLabelText('Native diagnostic JSON').textContent!);
  expect(Object.values(evidence.interactions)).toEqual([{ sessionId: 'B', status: 'uncertain' }]);
  fireEvent.click(screen.getByRole('button', { name: 'Connection' }));
  expect(screen.queryByText('Review uncertain operations')).toBeNull();
  expect(methods().filter(method => method === 'interaction/respond')).toHaveLength(1);
});

it('32-view capacity is actionable through Sidebar close, without eviction or stopping server work', async () => {
  const ids = Array.from({ length: 33 }, (_, i) => `session-${i}`);
  server.snapshots = new Map(ids.map(id => [id, snapshot(id)]));
  server.workspaceHost.classifyLocations = async cwds => cwds.map(() => ({ authorized: true }));
  await mount(ids.slice(0, 32));
  fireEvent.click(screen.getByRole('button', { name: 'Search Sessions' }));
  await act(async () => fireEvent.change(screen.getByLabelText('Search Session metadata'), { target: { value: 'session-32' } }));
  await select('session-32');
  expect(screen.getByRole('alert').textContent).toContain('Close a view from its Sidebar Session actions');
  expect(server.claims()).toHaveLength(32);
  fireEvent.click(screen.getByRole('button', { name: 'Clear search' }));
  await act(async () => server.client.listSessions(0, ''));
  fireEvent.click(screen.getByRole('button', { name: 'Session actions for Session session-0' }));
  await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'Close Session session-0 view' })));
  fireEvent.click(screen.getByRole('button', { name: 'Search Sessions' }));
  await act(async () => fireEvent.change(screen.getByLabelText('Search Session metadata'), { target: { value: 'session-32' } }));
  await select('session-32');
  expect(screen.getByLabelText('Session title').textContent).toBe('Session session-32');
  expect(server.claims()).toHaveLength(32);
  expect(methods().filter(method => method === 'session/detach')).toHaveLength(1);
  expect(methods()).not.toContain('session/unload'); expect(methods()).not.toContain('turn/cancel');
});

it('deletion shows the catalog title and impact, never raw identity/CAS, while sending the exact revision', async () => {
  server.summaries.set('A', { name: null, preview: 'My first prompt' });
  server.handlers.set('session/deletePreview', () => ({ type: 'deletion', result: { status: 'preview', preview: { session_id: 'A', target_revision: '9007199254740999', owned_node_count: 3, owned_conversation_count: 3, owned_child_count: 1 } } }));
  server.handlers.set('session/delete', () => ({ type: 'deletion', result: { status: 'not_found', session_id: 'A' } }));
  await mount();
  fireEvent.click(screen.getByRole('button', { name: 'Session actions for My first prompt' }));
  await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'Delete Session' })));
  const confirmation = screen.getByRole('region', { name: 'Confirm Session deletion' });
  expect(confirmation.textContent).toContain('Delete My first prompt?');
  expect(confirmation.textContent).toContain('Saved conversations: 3 · History nodes: 3 · Child conversations: 1');
  expect(confirmation.textContent).not.toMatch(/session_id|target_revision|9007199254740999/);
  expect(confirmation.querySelector('pre')).toBeNull();
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Confirm delete' })));
  expect(server.requests.find(({ request }) => request.method === 'session/delete')?.request.params).toEqual({ session_id: 'A', expected_target_revision: '9007199254740999' });
});

it('unlisted restored views cannot strand the finite view capacity', async () => {
  await server.connect();
  const missing = Array.from({ length: 32 }, (_, i) => `missing-${i}`);
  localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint, openViews: missing }));
  await act(async () => { render(<App client={server.client} workspaceHost={server.workspaceHost} />); });
  await select('A');
  expect(screen.getByRole('alert').textContent).toContain('32 Session views');
  const before = methods().length;
  fireEvent.click(screen.getByRole('button', { name: 'View options' }));
  await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'Close all views' })));
  expect(JSON.parse(localStorage.getItem('rustx-console-view-v2')!).openViews).toEqual([]);
  expect(missing.every(id => server.client.getSnapshot().views[id].attachmentIntent === 'released')).toBe(true);
  expect(methods().slice(before)).toEqual([]); // No controller exists to release.
  await select('A');
  expect(screen.getByLabelText('Session title').textContent).toBe('Session A');
  expect(methods()).not.toContain('session/unload'); expect(methods()).not.toContain('turn/cancel');
});

it('Close all views releases observed controllers without stopping their work', async () => {
  server.snapshots.get('A')!.attempt = { attempt_id: 'running-A', phase: { type: 'running' }, turn: 1 };
  await mount();
  const before = methods().length;
  fireEvent.click(screen.getByRole('button', { name: 'View options' }));
  await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'Close all views' })));
  expect(methods().slice(before)).toEqual(['session/detach', 'session/detach']);
  expect(server.claims()).toEqual([]);
  expect(server.loaded.has('A')).toBe(true);
  expect(server.snapshots.get('A')!.attempt?.phase.type).toBe('running');
});

it('reviewing a notice restores capacity at the 64-diagnostic limit without replay', async () => {
  await server.attached('A');
  server.held.add('session/name');
  for (let i = 0; i < 64; i++) {
    const lost = server.client.request({ method: 'session/name', params: { session_id: 'A', name: 'Unacknowledged' } }, 'session').catch(() => {});
    await server.waitFor('session/name', i + 1);
    await act(async () => { server.socket.close(); await lost; await server.connect(); });
  }
  expect(server.client.getSnapshot().uncertain).toHaveLength(64);
  localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint, openViews: ['A'] }));
  await act(async () => { render(<App client={server.client} workspaceHost={server.workspaceHost} />); });
  await expect(server.client.request({ method: 'session/name', params: { session_id: 'A', name: 'Blocked' } }, 'session')).rejects.toThrow();
  expect(methods().filter(method => method === 'session/name')).toHaveLength(64);
  fireEvent.click(screen.getByRole('button', { name: 'Connection' }));
  fireEvent.click(screen.getByText('Review uncertain operations'));
  const before = methods().length;
  fireEvent.click(screen.getAllByRole('button', { name: 'I have verified the affected work' })[0]);
  expect(server.client.getSnapshot().uncertain).toHaveLength(63);
  expect(methods().length).toBe(before);
  const next = server.client.request({ method: 'session/name', params: { session_id: 'A', name: 'Deliberate new request' } }, 'session').catch(() => {});
  await server.waitFor('session/name', 65);
  await act(async () => { server.socket.close(); await next; });
  expect(methods().filter(method => method === 'session/name')).toHaveLength(65);
});

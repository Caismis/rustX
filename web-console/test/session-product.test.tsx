import { translator } from '../src/locale/translation';
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import type { ClientView, SessionView } from '../src/client/contracts';
import { nativeTool } from './native-tool-cases';
import { deriveSessionProductState } from '../src/bindings/session-product';
import { App } from '../src/app/App';
import { Server, settingsReceipt, snapshot } from './fixture';
import { RpcFailure } from '../src/client/errors';

const connected = { connection: 'connected' as const, uncertain: [] };
const activeStatus = {threadId:'conversation-A',revision:'1',runtimeIncarnation:'native-incarnation',status:{type:'active' as const,activeFlags:[]}};
const base = (): SessionView => ({ threadStatus:{...activeStatus,status:{type:'idle'}}, id: 'A', attachment: 'attached', attachmentIntent: 'wanted',
  target: { session_id: 'A', conversation_id: 'conversation-A', runtime_incarnation: 'native-incarnation', attachment_id: 'native-attachment' }, snapshot: snapshot() });
const running = () => ({ attempt_id: 'private-attempt', phase: { type: 'running' as const }, turn: 1 });
const uncertain = { id: 'lost-request', method: 'turn/interrupt' as const, sessionId: 'A', generation: 7 };

it('missing thread status is not evidence of a failed observation', () => {
  const view = { ...base(), threadStatus: undefined };
  const project = (observationState: SessionView['observationState']) => deriveSessionProductState(translator('en'), connected, { ...view, observationState });
  expect(project('following').label).toBeUndefined();
  expect(project('unavailable')).toMatchObject({ severity: 'warning', label: 'Observation unavailable' });
});

it.each(['fork', 'retry'] as const)('%s presents a deliberate handoff as progress while retaining real failures', transition => {
  const tx = translator('en'), view: SessionView = { ...base(), target: undefined, attachment: 'detached', attachmentIntent: 'released' };
  const project = (state: Pick<ClientView, 'connection' | 'uncertain'> = connected, source = view) => deriveSessionProductState(tx, state, source, 'A', transition);
  expect(project()).toMatchObject({ status: 'connecting', severity: 'quiet' });
  expect(project().recovery).toBeUndefined();
  expect(project({ ...connected, connection: 'stale' })).toMatchObject({ status: 'reconnect', recovery: { action: 'connect' } });
  expect(project({ ...connected, uncertain: [uncertain] }).status).toBe('uncertain');
  expect(project(connected, { ...view, snapshot: { ...snapshot(), durability_failure: { operation: 'commit', diagnostic: 'storage-failure' } } }).status).toBe('failure');
  expect(project(connected, { ...base(), attachment: 'error', error: 'Read failed' }).status).toBe('unavailable');
  expect(deriveSessionProductState(tx, connected, view).status).toBe('reconnect');
});

it('an edited-message send is quiet progress, never a regeneration or interruption banner', () => {
  const tx = translator('en');
  const view: SessionView = { ...base(), target: undefined, attachment: 'detached', attachmentIntent: 'wanted' };
  const project = (source = view) => deriveSessionProductState(tx, connected, source, 'A', 'edit');
  // The handoff gap must not claim a regeneration nor surface as a reconnect.
  expect(project()).toEqual({ status: 'connecting', severity: 'quiet', label: undefined });
  expect(project().label).not.toBe(tx('common:session-product.regenerating'));
  expect(project().label).not.toBe(tx('common:session-product.connection-interrupted'));
  expect(project().recovery).toBeUndefined();
  // Real failures still outrank the quiet handoff.
  expect(project({ ...view, attachment: 'error', error: 'Read failed' }).status).toBe('unavailable');
  expect(deriveSessionProductState(tx, { ...connected, connection: 'stale' }, view, 'A', 'edit').status).toBe('reconnect');
});

it.each([
  ['awaiting first checkpoint', { threadStatus: undefined, observationState: 'attached' }, {}, 'connecting', undefined],
  ['loading checkpoint', { threadStatus: undefined, observationState: 'checkpointLoading' }, {}, 'connecting', undefined],
  ['awaiting subscription', { threadStatus: undefined, observationState: 'subscribePending' }, {}, 'connecting', undefined],
  ['missing status after subscription', { threadStatus: undefined, observationState: 'following' }, {}, 'unavailable', undefined],
  ['idle', {}, {}, 'idle', undefined],
  ['working', { threadStatus:activeStatus, snapshot: { ...snapshot(), attempt: running() } }, {}, 'working', undefined],
  ['queued accepted input', { submissions: [{ messageId: 'accepted', content: [], placement: 'queued' }] }, {}, 'idle', undefined],
  ['accepted idle input', { submissions: [{ messageId: 'accepted', content: [], placement: 'transcript' }] }, {}, 'idle', undefined],
  ['unaccepted request is not queued', { inboundRequests: 1 }, {}, 'idle', undefined],
  ['cancellation acknowledgement leaves native work active', { threadStatus:activeStatus, snapshot: { ...snapshot(), attempt: running() }, cancellation: { attemptId: 'private-attempt', status: 'acknowledged' } }, {}, 'working', undefined],
  ['another native receipt does not label stale working as stopping', { threadStatus:activeStatus, snapshot: { ...snapshot(), attempt: running() }, cancellation: { attemptId: 'actual-other-attempt', status: 'acknowledged' } }, {}, 'working', undefined],
  ['connection loss outranks stale work', { threadStatus:activeStatus, snapshot: { ...snapshot(), attempt: running() } }, { connection: 'stale' }, 'reconnect', 'connect'],
  ['incompatible versions require settings, not blind reconnect', {}, { connection: 'incompatible' }, 'failure', 'connection-settings'],
  ['lost attachment', { target: undefined, attachment: 'stale' }, {}, 'reconnect', 'open'],
  ['failed read with target', { attachment: 'stale' }, {}, 'unavailable', 'refresh'],
  ['failed observation after confirmed attach', { attachment: 'error' }, {}, 'unavailable', 'refresh'],
  ['opening is not recovery', { target: undefined, attachment: 'attaching' }, {}, 'connecting', undefined],
  ['cold unattached intent is not connection interruption', { target: undefined, attachment: 'detached', attachmentIntent: 'wanted', snapshot: undefined, threadStatus: undefined }, {}, 'connecting', undefined],
  ['cold attach failure without target preserves unavailability', { target: undefined, attachment: 'error', attachmentIntent: 'wanted', error: 'Attach failed', snapshot: undefined, threadStatus: undefined }, {}, 'unavailable', 'open'],
  ['uncertain outranks reconnect', {}, { connection: 'stale', uncertain: [uncertain] }, 'uncertain', 'connect'],
  ['uncertain survives suggestive settled state', { snapshot: { ...snapshot(), attempt: { ...running(), phase: { type: 'settled', outcome: { type: 'cancelled', reason: 'user_requested' } } } } }, { uncertain: [uncertain] }, 'uncertain', undefined],
  ['durability failure', { snapshot: { ...snapshot(), durability_failure: { operation: 'commit', diagnostic: 'storage-failure' } } }, {}, 'failure', undefined],
  ['uncertainty outranks storage failure', { snapshot: { ...snapshot(), durability_failure: { operation: 'commit', diagnostic: 'storage-failure' } } }, { uncertain: [uncertain] }, 'uncertain', undefined],
  ['storage failure retains reconnect action', { snapshot: { ...snapshot(), durability_failure: { operation: 'commit', diagnostic: 'storage-failure' } } }, { connection: 'stale' }, 'failure', 'connect'],
  ['pending deletion disables ordinary activity', { deleting: true }, {}, 'stopping', undefined],
  ['lost deletion requests verification', { deleting: true }, { uncertain: [uncertain] }, 'uncertain', 'connect'],
  ['failed retirement stays disabled with honest recovery', { deleting: true, error: 'Writer retirement unproven' }, {}, 'uncertain', 'connect'],
  ['pending native mailbox', { snapshot: { ...snapshot(), inbound: { pending: [{ sequence: '2', revision: '3', message: { id: 'native-input', source: 'human', content: [] } }] } } }, {}, 'idle', undefined],
  ['settled cancellation has no noisy status', { snapshot: { ...snapshot(), attempt: { ...running(), phase: { type: 'settled', outcome: { type: 'cancelled', reason: 'user_requested' } } } } }, {}, 'idle', undefined],
  ['terminal history belongs to transcript projection', { snapshot: { ...snapshot(), attempt: { ...running(), phase: { type: 'settled', outcome: { type: 'timed_out' } } } } }, {}, 'idle', undefined],
] as const)('%s maps only authoritative evidence', (_name, patch, connection, status, action) => {
  const view = { ...base(), ...patch } as SessionView;
  const state = { ...connected, ...connection } as Pick<ClientView, 'connection' | 'uncertain'>;
  const before = JSON.stringify({ view, state });
  const result = deriveSessionProductState(translator('en'), state, view);
  expect(result.status).toBe(status); expect(result.recovery?.action).toBe(action);
  expect(JSON.stringify(result)).not.toMatch(/private-attempt|native-incarnation|native-attachment|lost-request/);
  expect(JSON.stringify({ view, state })).toBe(before);
});

let server: Server | undefined;
afterEach(() => { cleanup(); server?.client.disconnect(); localStorage.clear(); });
async function mount() {
  server = new Server();
  server.snapshots.get('A')!.attempt = running();
  await server.attached('A');
  localStorage.setItem('rustx-console-view-v3', JSON.stringify({ endpoint: 'ws://127.0.0.1:8080/', openViews: ['A'], route: { kind: 'session', sessionId: 'A' } }));
  await act(async () => { render(<App client={server!.client} workspaceHost={server!.workspaceHost} />); });
  return server;
}
it('conversation chrome exposes product controls without developer diagnostics', async () => {
  const server = await mount();
  expect(screen.queryByLabelText('Session status')).toBeNull();
  expect(screen.getByText('Deep diving…', {selector:'[role=status]'})).toBeTruthy();
  const product = within(document.querySelector('.session-panel') as HTMLElement);
  for (const label of ['Attach / cold resume', 'Resync', 'Detach', 'Unload runtime', 'private-attempt']) expect(product.queryByText(label)).toBeNull();
  expect(document.querySelector('.attempt-status')).toBeNull();
  expect(screen.queryByRole('button', { name: /Inspector/ })).toBeNull();
  expect(screen.queryByText('More conversation details')).toBeNull();
  expect(screen.queryByRole('complementary', { name: 'Developer inspector' })).toBeNull();
  await act(async () => server.update('A', { ...snapshot(), attempt: { ...running(), phase: { type: 'settled', outcome: { type: 'completed', finish_reason: { type: 'stop' } } } } }));
  expect(screen.queryByLabelText('Session status')).toBeNull();
  expect(screen.queryByLabelText('Native diagnostic JSON')).toBeNull();
});
it('keeps an unconfirmed remote fetch result out of the session chrome after Stop', async () => {
  const server = await mount();
  const tool = nativeTool('fetch', 'web_fetch_exa', {}, []);
  tool.state = { type: 'settled', arguments: '{}', result: {
    status: { type: 'outcome_unknown', detail: 'Remote termination could not be confirmed' }, duration_ms: '0',
  } };
  await act(async () => server.update('A', { ...snapshot(), attempt: {
    ...running(), phase: { type: 'settled', outcome: { type: 'cancelled', reason: 'user_requested' } }, foreground: [tool],
  } }));
  expect(screen.queryByText('Needs verification')).toBeNull();
  expect(screen.queryByRole('button', { name: 'Review outcome' })).toBeNull();
});
it('lost cancellation remains uncertain through reconnect and a settled snapshot, with no replay', async () => {
  const server = await mount(); server.held.add('turn/interrupt');
  fireEvent.click(screen.getByRole('button', { name: 'Stop' }));
  await server.waitFor('turn/interrupt', 1);
  expect(screen.queryByLabelText('Session status')).toBeNull();
  await act(async () => server.socket.close());
  expect(screen.getByLabelText('Session status').textContent).toContain('Needs verification');
  expect(screen.getByRole('button', { name: 'Review outcome' })).toBeTruthy();
  server.snapshots.get('A')!.attempt = { ...running(), phase: { type: 'settled', outcome: { type: 'cancelled', reason: 'user_requested' } } };
  await act(async () => server.connect());
  expect(screen.getByLabelText('Session status').textContent).toContain('Needs verification');
  expect(server.client.getSnapshot().uncertain).toMatchObject([{ method: 'turn/interrupt', sessionId: 'A' }]);
  expect(server.requests.filter(row => row.request.method === 'turn/interrupt')).toHaveLength(1);
});
it('lost authoritative attachment exposes one Open Session action and sends one admission-fenced attach', async () => {
  const server = await mount();
  await act(async () => server.client.release('A'));
  const before = server.requests.length;
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Session' })));
  expect(server.requests.slice(before).filter(row => row.request.method === 'thread/resume')).toHaveLength(1);
  expect(screen.queryByRole('button', { name: 'Open Session' })).toBeNull();
  expect(screen.queryByLabelText('Session status')).toBeNull();
  expect(screen.getByText('Deep diving…', {selector:'[role=status]'})).toBeTruthy();
});
it('failed observation exposes one Refresh observation action which refreshes without attaching or replaying', async () => {
  const server = await mount();
  const original = server.client.getSnapshot().views.A;
  server.handlers.set('session/checkpoint', () => { throw new RpcFailure({ code: -32000, message: 'read unavailable' }); });
  await act(async () => { await expect(server.client.refresh('A')).rejects.toThrow('read unavailable'); });
  expect(screen.getByLabelText('Session status').textContent).toContain('Observation unavailable');
  expect(screen.getByLabelText('Session status').textContent).not.toContain('Connection interrupted');
  const unavailable = server.client.getSnapshot().views.A;
  expect(unavailable.target).toEqual(original.target);
  expect(unavailable.checkpoint).toBe(original.checkpoint);
  expect(unavailable.cursor).toBe(original.cursor);
  server.handlers.delete('session/checkpoint');
  const before = server.requests.length;
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Refresh observation' })));
  const operations = server.requests.slice(before).map(row => row.request.method);
  expect(operations.filter(method => method === 'session/checkpoint')).toHaveLength(1);
  expect(operations).not.toContain('thread/resume');
  expect(operations).not.toContain('inbound/queue');
  expect(operations).not.toContain('turn/interrupt');
  expect(screen.queryByRole('button', { name: 'Refresh observation' })).toBeNull();
});
it('rapid cold switch aborted before attach stays quiet in the header and recovers on explicit return', async () => {
  const s = new Server();
  server = s;
  await s.attached('A');
  localStorage.setItem('rustx-console-view-v3', JSON.stringify({ endpoint: 'ws://127.0.0.1:8080/', openViews: ['A'], route: { kind: 'session', sessionId: 'A' } }));
  await act(async () => { render(<App client={s.client} workspaceHost={s.workspaceHost} />); });

  const notices: string[] = [];
  const header = document.querySelector('#session-view > header')!;
  const observer = new MutationObserver(() => {
    const title = document.querySelector('#session-title')?.textContent;
    if (!title || title === 'New session') notices.push(`title:${String(title)}`);
    const status = document.querySelector('#session-view [aria-label="Session status"]')?.textContent;
    if (status) notices.push(`status:${status}`);
  });
  observer.observe(document.querySelector('#session-view')!, { childList: true, characterData: true, subtree: true });
  try {
    expect(header.className).not.toMatch(/headerBlank/);
    s.held.add('session/read');
    const readsBefore = s.requests.filter(row => row.request.method === 'session/read').length;
    await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Session B' })));
    const read = await s.waitFor('session/read', readsBefore + 1);
    expect(read.method === 'session/read' && read.params.session_id).toBe('B');
    expect(document.querySelector('#session-title')?.textContent).toBe('Session B');
    // Rapidly switch back to warm A before B dispatches thread/resume.
    await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Session A' })));
    expect(document.querySelector('#session-title')?.textContent).toBe('Session A');
    s.held.delete('session/read');
    await act(async () => s.reply(read));
    expect(s.requests.filter(row => row.request.method === 'thread/resume' && s.sessionForThread(row.request.params.threadId) === 'B')).toHaveLength(0);
    expect(s.client.getSnapshot().views.B).toMatchObject({ attachmentIntent: 'released', attachment: 'detached' });
    expect(notices).toEqual([]);

    // Reconnecting must not reopen B after user rapidly switched away.
    await act(async () => { s.socket.close(); await s.connect(); });
    expect(s.requests.filter(row => row.request.method === 'thread/resume' && s.sessionForThread(row.request.params.threadId) === 'B')).toHaveLength(0);

    // Explicitly opening B attaches it once cleanly.
    await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Session B' })));
    expect(document.querySelector('#session-title')?.textContent).toBe('Session B');
    expect(s.requests.filter(row => row.request.method === 'thread/resume' && s.sessionForThread(row.request.params.threadId) === 'B')).toHaveLength(1);
    expect(notices).toEqual([]);
  } finally {
    observer.disconnect();
  }
});
it('unlisted cold session header falls back to its session id while summary is loading', async () => {
  const s = new Server();
  server = s;
  s.snapshots.set('unlisted', snapshot('unlisted'));
  s.handlers.set('session/list', () => ({ type: 'sessions', sessions: [], next_offset: null }));
  s.handlers.set('session/settings', req => settingsReceipt({ cwd: req.method === 'session/settings' && req.params.session_id === 'unlisted' ? '/workspace/A' : '/workspace/B' }, '0'));
  s.summaries.set('unlisted', { cwd: '/workspace/A', name: 'Recovered title' });
  localStorage.setItem('rustx-console-view-v3', JSON.stringify({ endpoint: 'ws://127.0.0.1:8080/', openViews: ['unlisted'], route: { kind: 'session', sessionId: 'unlisted' } }));
  s.held.add('session/summary');
  await act(async () => { render(<App client={s.client} workspaceHost={s.workspaceHost} />); });
  await act(async () => s.connect());
  const read = await s.waitFor('session/summary', 1);
  expect(document.querySelector('#session-title')?.textContent).toBe('unlisted');
  expect(document.querySelector('#session-title')?.textContent).not.toBe('New session');
  await act(async () => s.reply(read));
  expect(document.querySelector('#session-title')?.textContent).toBe('Recovered title');
});

import { translator } from '../src/locale/translation';
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import type { ClientView, SessionView } from '../src/client/contracts';
import { deriveSessionProductState } from '../src/bindings/session-product';
import { App } from '../src/app/App';
import { Server, snapshot } from './fixture';
import { RpcFailure } from '../src/client/errors';

const connected = { connection: 'connected' as const, uncertain: [] };
const base = (): SessionView => ({ id: 'A', attachment: 'attached', attachmentIntent: 'wanted',
  target: { session_id: 'A', conversation_id: 'conversation-A', runtime_incarnation: 'native-incarnation', attachment_id: 'native-attachment' }, snapshot: snapshot() });
const running = () => ({ attempt_id: 'private-attempt', phase: { type: 'running' as const }, turn: 1 });
const uncertain = { id: 'lost-request', method: 'turn/cancel' as const, sessionId: 'A', generation: 7 };

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

it.each([
  ['idle', {}, {}, 'idle', undefined],
  ['working', { snapshot: { ...snapshot(), attempt: running() } }, {}, 'working', undefined],
  ['queued accepted input', { submissions: [{ messageId: 'accepted', content: [] }] }, {}, 'queued', undefined],
  ['unaccepted request is not queued', { inboundRequests: 1 }, {}, 'idle', undefined],
  ['stopping outranks working', { snapshot: { ...snapshot(), attempt: running() }, cancellation: { attemptId: 'private-attempt', status: 'acknowledged' } }, {}, 'stopping', undefined],
  ['another native receipt does not label stale working as stopping', { snapshot: { ...snapshot(), attempt: running() }, cancellation: { attemptId: 'actual-other-attempt', status: 'acknowledged' } }, {}, 'working', undefined],
  ['connection loss outranks stale work', { snapshot: { ...snapshot(), attempt: running() } }, { connection: 'stale' }, 'reconnect', 'connect'],
  ['incompatible versions require settings, not blind reconnect', {}, { connection: 'incompatible' }, 'failure', 'connection-settings'],
  ['lost attachment', { target: undefined, attachment: 'stale' }, {}, 'reconnect', 'open'],
  ['failed read with target', { attachment: 'stale' }, {}, 'unavailable', 'refresh'],
  ['failed observation after confirmed attach', { attachment: 'error' }, {}, 'unavailable', 'refresh'],
  ['opening is not recovery', { target: undefined, attachment: 'attaching' }, {}, 'connecting', undefined],
  ['uncertain outranks reconnect', {}, { connection: 'stale', uncertain: [uncertain] }, 'uncertain', 'connect'],
  ['uncertain survives suggestive settled state', { snapshot: { ...snapshot(), attempt: { ...running(), phase: { type: 'settled', outcome: { type: 'cancelled', reason: 'user_requested' } } } } }, { uncertain: [uncertain] }, 'uncertain', undefined],
  ['durability failure', { snapshot: { ...snapshot(), durability_failure: { operation: 'commit', diagnostic: 'storage-failure' } } }, {}, 'failure', undefined],
  ['uncertainty outranks storage failure', { snapshot: { ...snapshot(), durability_failure: { operation: 'commit', diagnostic: 'storage-failure' } } }, { uncertain: [uncertain] }, 'uncertain', undefined],
  ['storage failure retains reconnect action', { snapshot: { ...snapshot(), durability_failure: { operation: 'commit', diagnostic: 'storage-failure' } } }, { connection: 'stale' }, 'failure', 'connect'],
  ['pending deletion disables ordinary activity', { deleting: true }, {}, 'stopping', undefined],
  ['lost deletion requests verification', { deleting: true }, { uncertain: [uncertain] }, 'uncertain', 'connect'],
  ['failed retirement stays disabled with honest recovery', { deleting: true, error: 'Writer retirement unproven' }, {}, 'uncertain', 'connect'],
  ['pending native mailbox', { snapshot: { ...snapshot(), inbound: { pending: [{ sequence: '2', revision: '3', message: { id: 'native-input', source: 'human', content: [] } }] } } }, {}, 'queued', undefined],
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
  localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint: 'ws://127.0.0.1:8080/', openViews: ['A'] }));
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
it('lost cancellation remains uncertain through reconnect and a settled snapshot, with no replay', async () => {
  const server = await mount(); server.held.add('turn/cancel');
  fireEvent.click(screen.getByRole('button', { name: 'Stop' }));
  await server.waitFor('turn/cancel', 1);
  expect(screen.getByLabelText('Session status').textContent).toBe('Stopping…');
  await act(async () => server.socket.close());
  expect(screen.getByLabelText('Session status').textContent).toContain('Needs verification');
  server.snapshots.get('A')!.attempt = { ...running(), phase: { type: 'settled', outcome: { type: 'cancelled', reason: 'user_requested' } } };
  await act(async () => server.connect());
  expect(screen.getByLabelText('Session status').textContent).toContain('Needs verification');
  expect(server.client.getSnapshot().uncertain).toMatchObject([{ method: 'turn/cancel', sessionId: 'A' }]);
  expect(server.requests.filter(row => row.request.method === 'turn/cancel')).toHaveLength(1);
});
it('lost authoritative attachment exposes one Open Session action and sends one admission-fenced attach', async () => {
  const server = await mount();
  await act(async () => server.client.release('A'));
  const before = server.requests.length;
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Session' })));
  expect(server.requests.slice(before).filter(row => row.request.method === 'session/attach')).toHaveLength(1);
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
  expect(operations).not.toContain('session/attach');
  expect(operations).not.toContain('turn/start');
  expect(operations).not.toContain('turn/cancel');
  expect(screen.queryByRole('button', { name: 'Refresh observation' })).toBeNull();
});

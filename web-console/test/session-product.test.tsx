import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import type { ClientView, SessionView } from '../src/client/app-server';
import { deriveSessionProductState } from '../src/bindings/session-product';
import { App } from '../src/app/App';
import { Server, snapshot } from './fixture';
import { RpcFailure } from '../src/client/app-server';

const connected = { connection: 'connected' as const, uncertain: [] };
const base = (): SessionView => ({ id: 'A', attachment: 'attached', attachmentIntent: 'wanted',
  target: { session_id: 'A', conversation_id: 'conversation-A', runtime_incarnation: 'native-incarnation', attachment_id: 'native-attachment' }, snapshot: snapshot() });
const running = () => ({ attempt_id: 'private-attempt', phase: { type: 'running' as const }, turn: 1 });
const uncertain = { id: 'lost-request', method: 'turn/cancel' as const, sessionId: 'A', generation: 7 };

it.each([
  ['idle', {}, {}, 'idle', undefined],
  ['working', { snapshot: { ...snapshot(), attempt: running() } }, {}, 'working', undefined],
  ['queued accepted input', { submissions: [{ messageId: 'accepted', content: [] }] }, {}, 'queued', undefined],
  ['unaccepted request is not queued', { inboundRequests: 1 }, {}, 'idle', undefined],
  ['stopping outranks working', { snapshot: { ...snapshot(), attempt: running() }, cancellation: { attemptId: 'private-attempt', status: 'acknowledged' } }, {}, 'stopping', undefined],
  ['connection loss outranks stale work', { snapshot: { ...snapshot(), attempt: running() } }, { connection: 'stale' }, 'reconnect', 'connect'],
  ['incompatible versions require settings, not blind reconnect', {}, { connection: 'incompatible' }, 'failure', 'connection-settings'],
  ['lost attachment', { target: undefined, attachment: 'stale' }, {}, 'reconnect', 'open'],
  ['failed read with target', { attachment: 'stale' }, {}, 'reconnect', 'refresh'],
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
  ['timed out is actionable', { snapshot: { ...snapshot(), attempt: { ...running(), phase: { type: 'settled', outcome: { type: 'timed_out' } } } } }, {}, 'failure', undefined],
] as const)('%s maps only authoritative evidence', (_name, patch, connection, status, action) => {
  const view = { ...base(), ...patch } as SessionView;
  const state = { ...connected, ...connection } as Pick<ClientView, 'connection' | 'uncertain'>;
  const before = JSON.stringify({ view, state });
  const result = deriveSessionProductState(state, view);
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
it('ordinary chrome is product-only; Inspector retains exact facts and emits no native operations', async () => {
  const server = await mount();
  expect(screen.getByLabelText('Session status').textContent).toBe('Working…');
  const product = within(document.querySelector('.session-panel') as HTMLElement);
  for (const label of ['Attach / cold resume', 'Resync', 'Detach', 'Unload runtime', 'private-attempt']) expect(product.queryByText(label)).toBeNull();
  expect(document.querySelector('.attempt-status')).toBeNull();
  const baseline = server.requests.length;
  fireEvent.click(screen.getByRole('button', { name: 'Toggle Inspector' }));
  const panel = screen.getByRole('complementary', { name: 'Developer inspector' });
  for (const section of ['Identity', 'Execution', 'Attachment / residency', 'Inbound / interactions', 'Configuration / revisions', 'Recovery / uncertainty', 'Protocol']) expect(within(panel).getByText(section)).toBeTruthy();
  expect(within(panel).getByText('private-attempt')).toBeTruthy();
  fireEvent.click(within(panel).getByText('Complete native runtime facts'));
  const facts = JSON.parse(within(panel).getByLabelText('Native diagnostic JSON').textContent!);
  expect(facts).toMatchObject({ SessionId: 'A', ConversationId: 'conversation-A', attachment_intent: 'wanted', attachment: 'attached', runtime_incarnation: server.client.target('A').runtime_incarnation, attachment_id: server.client.target('A').attachment_id, connection_generation: server.client.getSnapshot().generation, attempt: running(), capability_revision: '0' });
  fireEvent.change(within(panel).getByLabelText('Method filter'), { target: { value: 'turn' } });
  fireEvent.click(within(panel).getByRole('button', { name: 'Pause log' }));
  fireEvent.click(within(panel).getByRole('button', { name: 'Clear log' }));
  fireEvent.click(screen.getByRole('button', { name: 'Close Inspector' }));
  expect(server.requests.slice(baseline)).toEqual([]);
  const settled = { ...snapshot(), resources: { revision: '9007199254740993', inspection: { definitions: [], resource_diagnostics: [], agents: {}, workflows: {}, sources: {}, skills: [], skill_diagnostics: [] } }, capabilities: { revision: '9007199254740994' }, attempt: { ...running(), phase: { type: 'settled' as const, outcome: { type: 'completed' as const, finish_reason: { type: 'stop' as const } } } } };
  await act(async () => server.update('A', settled));
  expect(screen.queryByLabelText('Session status')).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Toggle Inspector' }));
  const exact = JSON.parse(screen.getByLabelText('Native diagnostic JSON').textContent!);
  expect(exact).toMatchObject({ resource_revision: '9007199254740993', capability_revision: '9007199254740994', attempt: { attempt_id: 'private-attempt', phase: { type: 'settled', outcome: { type: 'completed' } } } });
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
  fireEvent.click(screen.getByRole('button', { name: 'Toggle Inspector' }));
  const facts = JSON.parse(screen.getByLabelText('Native diagnostic JSON').textContent!);
  expect(facts.uncertain_operations).toMatchObject([{ method: 'turn/cancel', sessionId: 'A' }]);
  expect(server.requests.filter(row => row.request.method === 'turn/cancel')).toHaveLength(1);
});
it('lost authoritative attachment exposes one Open Session action and sends one admission-fenced attach', async () => {
  const server = await mount();
  await act(async () => server.client.release('A'));
  const before = server.requests.length;
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Session' })));
  expect(server.requests.slice(before).filter(row => row.request.method === 'session/attach')).toHaveLength(1);
  expect(screen.queryByRole('button', { name: 'Open Session' })).toBeNull();
  expect(screen.getByLabelText('Session status').textContent).toBe('Working…');
});
it('failed observation exposes one Retry connection action which refreshes without attaching or replaying', async () => {
  const server = await mount();
  server.handlers.set('session/snapshot', () => { throw new RpcFailure({ code: -32000, message: 'read unavailable' }); });
  await act(async () => { await expect(server.client.refresh('A')).rejects.toThrow('read unavailable'); });
  server.handlers.delete('session/snapshot');
  const before = server.requests.length;
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Retry connection' })));
  const operations = server.requests.slice(before).map(row => row.request.method);
  expect(operations.filter(method => method === 'session/snapshot')).toHaveLength(1);
  expect(operations).not.toContain('session/attach');
  expect(operations).not.toContain('turn/start');
  expect(operations).not.toContain('turn/cancel');
  expect(screen.queryByRole('button', { name: 'Retry connection' })).toBeNull();
});

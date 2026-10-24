import { settingsReceipt } from '../fixture';
import { sourceReceipt } from '../fixture';
// Isolated deterministic wire fixture. Never imported by the production entry.
import { createRoot } from 'react-dom/client';
import { App } from '../../src/app/App';
import { traceRecord } from '../trace-fixture';
import { cfg3Source } from '../cfg3-data';
import { RpcFailure } from '../../src/client/errors';
import { Server, interaction, snapshot, endpoint } from '../fixture';
import '../../src/presentation/theme/base.css';
import '../../src/presentation/theme/gradient-shadow-text.css';
import '../../src/presentation/theme/design-platform.css';
import '../../src/presentation/theme/scrollbar.css';
import '../../src/presentation/theme/corner-shape.css';
import '../../src/presentation/theme/shiki.css';
import '../../src/presentation/theme/reset.css';
import '../../src/app/console.css';
const server = new Server();
for (const method of ['configuration/sourcesRead', 'session/effectiveConfiguration'] as const) server.handlers.set(method, () => { throw new RpcFailure({ code: -32000, message: 'Select Appearance to review the presentation fixture.' }); });
server.snapshots.set('C', snapshot('C'));
server.snapshots.get('A')!.attempt = { attempt_id: 'attempt-A', phase: { type: 'running' }, turn: 1 };
server.snapshots.get('B')!.pending_interactions = [interaction('approval', 'B')];
server.snapshots.get('A')!.messages = [{ role: 'assistant', id: 'message-A', content: [{ type: 'text', text: 'The presentation shell is ready. Sessions and execution remain owned by the rustX App Server.' }] }];
server.snapshots.get('A')!.transcript = { entries: [{ cursor: '1', item: { type: 'message', message: server.snapshots.get('A')!.messages[0] } }] };
if (new URL(location.href).searchParams.has('fork')) {
  server.snapshots.get('A')!.transcript.inherited_through = '1';
  const source = snapshot('C'); source.conversation_id = 'conversation-source-C';
  source.messages = [{ role: 'user', source: 'human', id: 'source-message', content: [{ type: 'text', text: 'Exact fork source' }] }];
  source.transcript.entries = [{ cursor: '1', item: { type: 'message', message: source.messages[0] } }];
  server.nodeSnapshots.set('source-node-C', source);
  server.handlers.set('session/node', request => {
    if (request.method !== 'session/node') throw Error('Node fixture');
    const { session_id: id, node_id: node } = request.params;
    return { type: 'node', node: { id: node, conversation_id: id === 'C' && node === 'source-node-C' ? 'conversation-source-C' : `conversation-${id}`, ordinal: '1', origin: id === 'A' ? { type: 'fork', source_session: 'C', source_node: 'source-node-C', source_surface_revision: '0', source_message: 'source-message', side: 'after' } : { type: 'new' } } };
  });
}

if (new URL(location.href).searchParams.has('editor-seed')) {
  const source = server.snapshots.get('A')!;
  source.transcript.entries![0].completed_response = {closing_locator:{type:'by_message_id',message_id:'message-A'},deferred_fields:[],retry_locator:{type:'by_message_id',message_id:'user-A'}, closing_message_id: 'message-A', origin: {attempt_id_source:null, conversation_id: 'conversation-A', closing_message_id: 'message-A', attempt_id: 'attempt-A' }, completed_at: '2026-09-25T00:00:00Z', surface_revision: '7', retry_message_id: 'user-A' };
  server.handlers.set('session/fork', request => {
    if (request.method !== 'session/fork' || request.params.session_id !== 'A' || request.params.node_id !== 'node-A' || request.params.boundary?.type !== 'by_message_id' || request.params.boundary.message_id !== 'message-A' || request.params.side !== 'after' || request.params.surface_revision !== '7') throw new Error('Different native Fork cut');
    const child = snapshot('seed-child'); server.snapshots.set('seed-child', child); server.nodeSnapshots.set('node-seed-child', child);
    return { type: 'session_transition', session: { id: 'seed-child', active_node: 'node-seed-child', active_conversation_id: child.conversation_id, node_count: 1, created_at: '0', updated_at: '0' }, editor:server.editor({ id: 'seed-child', active_node: 'node-seed-child', active_conversation_id: child.conversation_id, node_count: 1, created_at: '0', updated_at: '0' },[{ type: 'upload', session_id: 'seed-child', batch_id: 'restored-batch', token: 'restored-token' }, { type: 'text', text: 'Native restored Fork draft' }]) };
  });
}

server.workspaceHost.listWorkspaces = async () => ({ authorityId: 'fixture-host', endpoint, workspaces: [{ id: 'project', displayName: 'rustX', displayPath: '/workspace', location: 'project' }], picker: { kind: 'unavailable', reason: 'Fixture has one authorized project' } });
// Reference tests explicitly release the cold display baseline after its first paint.
// Other shell consumers keep the ordinary immediate fixture response.
let releaseAssociations!: () => void;
const associationGate = new Promise<void>(resolve => { releaseAssociations = resolve; });
if (!new URL(location.href).searchParams.has('association-gate')) releaseAssociations();
server.workspaceHost.classifyLocations = async cwds => { await associationGate; return cwds.map(cwd => ({ authorized: true, workspaceId: cwd.endsWith('/C') ? undefined : 'project' })); };
// The shell's registered Workspace supplies the same generated source contract
// as the permission seat. User Settings retains its explicit error fixture.
const settingsTargets: string[] = [];
server.workspaceHost.configureWorkspace = async (id, _endpoint, operation) => {
  settingsTargets.push(id);
  const projection = { ...cfg3Source(), target: { kind: 'workspace' as const, directory: '/workspace' } };
  if (operation.kind === 'write') return { kind: 'write', commit: { acknowledgement: sourceReceipt(projection), reread: { status: 'observed', projection } } };
  if (operation.kind === 'mcp_probe') return {kind:'mcp_probe',result:{id:operation.id,revision:operation.expected_revision,outcome:'reachable'}};
  return { kind: operation.kind, projection };
};
const cold = new URL(location.href).searchParams.get('initial') === 'cold';
if (cold) {
  server.snapshots.get('A')!.attempt = null;
  server.snapshots.get('A')!.transcript.statistics = { turns: '8', steps: '46', completed_responses: '8', model_requests: '46', requests_with_usage: '46', reported_usage: { input_tokens: "900000", output_tokens: "100000", total_tokens: "1000000" } };
  server.handlers.set('session/settings', () => (settingsReceipt({ cwd: '/workspace/A', model: { model: 'DeepSeek/deepseek-flash' } },'0')));
  server.snapshots.get('A')!.trace = { records: [traceRecord(1)] };
  server.held.add('session/attach'); server.held.add('turn/start');
  await server.connect();
} else await server.attached('A', 'B');
localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint, openViews: ['A', 'B'] }));

// Isolated fixture controls, never reachable from the production application.
let replacedAttach: { socket: typeof server.socket; response: import('../../../protocol/app-server/v57').Response } | undefined;
let backgroundCheckpoint: Promise<unknown> | undefined;
let navigationCurrent = () => true;
window.sessionFixture = {
  releaseAssociations,
  holdInput: () => { server.held.add('turn/start'); server.held.add('turn/steer'); },
  settleInput: async () => {
    const row = server.requests.filter(row => row.request.method === 'turn/start' || row.request.method === 'turn/steer').at(-1)!;
    server.reply(row.request, row.socket);
    await server.client.request({ method: 'server/info', params: {} }, 'server_info');
  },
  holdSearch: () => { server.held.add('session/list'); },
  settleSearch: async (query: string) => {
    const row = server.requests.filter(row => row.request.method === 'session/list' && row.request.params.query === query).at(-1)!;
    server.held.delete('session/list'); server.reply(row.request, row.socket);
    await server.client.request({ method: 'server/info', params: {} }, 'server_info');
    await new Promise<void>(resolve => requestAnimationFrame(() => resolve()));
  },
  holdHistory: () => { server.held.add('session/history'); },
  settleHistory: async (success: boolean) => {
    const row = server.requests.filter(row => row.request.method === 'session/history' && row.request.params.session_id === 'C').at(-1)!;
    server.held.delete('session/history');
    if (success) server.reply(row.request, row.socket);
    else row.socket.deliver({ jsonrpc: '2.0', id: row.request.id, error: { code: -32000, message: 'Navigation fixture rejected', data: { kind: 'operation_failed' } } });
    await server.client.request({ method: 'server/info', params: {} }, 'server_info');
    await new Promise<void>(resolve => requestAnimationFrame(() => resolve()));
  },
  allowNodeReads: () => { server.held.delete('session/node'); },
  holdNode: () => { server.held.add('session/node'); },
  settleNode: async (success: boolean) => {
    const row = server.requests.filter(row => row.request.method === 'session/node' && row.request.params.session_id === 'C' && row.request.params.node_id === 'source-node-C').at(-1)!;
    server.held.delete('session/node');
    if (success) server.reply(row.request, row.socket);
    else row.socket.deliver({ jsonrpc: '2.0', id: row.request.id, error: { code: -32000, message: 'Unknown Node', data: { kind: 'unknown_node', session_id: 'C', node_id: 'source-node-C' } } });
    // An ordered wire barrier after delivery, followed by React's presentation
    // frame. No elapsed-time assumption establishes the response ordering.
    await server.client.request({ method: 'server/info', params: {} }, 'server_info');
    await new Promise<void>(resolve => requestAnimationFrame(() => resolve()));
  },
  uncertainUpload: () => { server.handlers.set('session/uploadStatus', () => ({ type: 'upload_status', outcome: { state: 'unresolved' } })); },
  resolveUpload: () => { server.handlers.delete('session/uploadStatus'); },
  wireBarrier: () => server.client.request({ method: 'server/info', params: {} }, 'server_info').then(() => {}),
  requests: () => server.requests.map(row => row.request),
  settingsTargets: () => settingsTargets.slice(),
  waitForNavigation: () => server.client.waitForAttachment('C', () => true).catch(() => {}),
  captureNavigation: () => { const view = server.client.getSnapshot().views.A, observation = view?.attachmentObservation; navigationCurrent = () => server.client.isAttachmentControlCurrent('A', observation); },
  navigationCurrent: () => navigationCurrent(),
  navigationFacts: () => ({ generation: server.client.getSnapshot().generation, claims: server.claims(), views: Object.fromEntries(Object.entries(server.client.getSnapshot().views).map(([id, view]) => [id, { attachment: view.attachment, nodeId: view.nodeId, intent: view.attachmentIntent, target: view.target, authoritative: server.client.isAttachmentControlCurrent(id, view.attachmentObservation) }])) }),
  holdBackgroundCheckpoint: async () => {
    server.held.add('session/checkpoint');
    const count = server.requests.filter(row => row.request.method === 'session/checkpoint').length;
    // An independent read neither retires Following nor installs another cut.
    backgroundCheckpoint = server.client.request({ method: 'session/checkpoint', params: { target: server.client.target('B'), trace_records: [] } }, 'checkpoint');
    void backgroundCheckpoint.catch(() => {});
    await server.waitFor('session/checkpoint', count + 1);
  },
  holdSnapshot: async () => { server.held.add('session/checkpoint'); const count = server.requests.filter(row => row.request.method === 'session/checkpoint').length; void server.client.refresh('B'); await server.waitFor('session/checkpoint', count + 1); },
  settleSnapshot: async () => { server.held.delete('session/checkpoint'); const row = server.requests.filter(row => row.request.method === 'session/checkpoint' && row.request.params.target.session_id === 'B').at(-1)!; server.reply(row.request, row.socket); await backgroundCheckpoint; backgroundCheckpoint = undefined; },
  staleDestination: async () => { server.handlers.set('session/checkpoint', () => { throw new RpcFailure({ code: -32000, message: 'Stale fixture observation', data: { kind: 'operation_failed' } }); }); await server.client.refresh('B').catch(() => {}); server.handlers.delete('session/checkpoint'); },
  replaceConnection: async () => { const row = server.requests.find(row => row.request.method === 'session/attach' && row.request.params.session_id === 'C')!; replacedAttach = { socket: row.socket, response: server.commit(row.request, row.socket) }; await server.client.disconnect(); server.held.delete('session/attach'); await server.connect(); },
  readyNavigation: async () => { const next = structuredClone(server.snapshots.get('B')!); next.pending_interactions = []; await server.update('B', next); },
  holdAttach: () => { server.held.add('session/attach'); },
  settleAttach: (success: boolean, sessionId?: string) => {
    if (replacedAttach) { replacedAttach.socket.deliver(replacedAttach.response); replacedAttach = undefined; return; }
    const row = server.requests.filter(row => row.request.method === 'session/attach' && (!sessionId || row.request.params.session_id === sessionId)).at(-1)!;
    server.held.delete('session/attach');
    if (success) server.reply(row.request, row.socket);
    else row.socket.deliver({ jsonrpc: '2.0', id: row.request.id, error: { code: -32000, message: 'Navigation fixture rejected', data: { kind: 'operation_failed' } } });
  },
  async releaseCold() {
    server.reply(await server.waitFor('session/attach', 1));
    const start = await server.waitFor('turn/start', 1);
    server.reply(start);
    return server.requests.filter(row => row.request.method === 'turn/start').length;
  },
  async activity(id, timestamp) {
    server.summaries.set(id, { ...server.summaries.get(id), updated_at: timestamp });
    server.invalidateSummary(id, server.socket, true);
  },
  async stream(text) {
    const next = structuredClone(server.snapshots.get('A')!);
    next.attempt = { attempt_id: 'attempt-A', phase: { type: 'running' }, turn: 1, in_flight: { message_id: 'stream-A', blocks: [{ type: 'text', block_index: 0, text }] } };
    next.transcript.statistics = { turns: '1', steps: '1', completed_responses: '0', model_requests: '1', requests_with_usage: '0', latest_turn: { attempt_id_deferred:false, attempt_id_source:null, attempt_id: 'attempt-A', started_at: '2026-09-25T00:00:00Z' } };
    await server.update('A', next);
  },
  async presentation(mode) {
    if (mode === 'empty' || mode === 'preview' || mode === 'named' || mode === 'delete') {
      server.summaries.set('A', { name: mode === 'named' ? 'Architecture review' : null, preview: mode === 'empty' ? null : 'Inspect the Session ownership boundary' });
      await server.update('A', snapshot('A'));
      await server.client.listSessions();
    }
    // Enough Sessions in the one Workspace for the Session list to scroll.
    if (mode === 'many') {
      for (let index = 1; index <= 24; index += 1) server.snapshots.set(`S${String(index).padStart(2, '0')}`, snapshot(`S${String(index).padStart(2, '0')}`));
      await server.client.listSessions();
    }
    if (mode === 'delete') server.handlers.set('session/deletePreview', () => ({ type: 'deletion', result: { status: 'preview', preview: { session_id: 'A', target_revision: '9007199254740999', owned_node_count: 3, owned_conversation_count: 3, owned_child_count: 1 } } }));
    if (mode === 'other-uncertain') {
      server.snapshots.get('B')!.pending_interactions = [];
      server.held.add('turn/start');
      const lost = server.client.send('B', 'Verify this operation', [], 'send').catch(() => {});
      await server.waitFor('turn/start', 1);
      server.socket.close(); await lost; await server.connect();
    }
  },
  async state(mode) {
    const next = structuredClone(server.snapshots.get('A')!);
    if (mode === 'idle' || mode === 'queued') next.attempt = null;
    if (mode === 'queued') next.inbound = { pending: [{ sequence: '1', revision: '0', message: { id: 'queued-A', source: 'human', content: [{ type: 'text', text: 'Review the implementation' }] } }] };
    await server.update('A', next);
    if (mode === 'stopping' || mode === 'uncertain') {
      server.held.add('turn/cancel');
      const expected = server.client.cancellationTarget('A')!;
      void server.client.cancelTurn(expected).catch(() => {});
      await server.waitFor('turn/cancel', 1);
    }
    if (mode === 'reconnect' || mode === 'uncertain') server.socket.close();
  },
};
// Static other-Session uncertainty references start from the settled wire state.
// Reconnect interleavings are exercised by the separate product-state and
// workspace-associations browser tests, not incidental screenshot setup paints.
if (new URL(location.href).searchParams.get('initial') === 'other-uncertain') await window.sessionFixture.presentation('other-uncertain');
createRoot(document.getElementById('root')!).render(<App client={server.client} workspaceHost={server.workspaceHost} />);

declare global { interface Window { sessionFixture: { holdInput(): void; settleInput(): Promise<void>; wireBarrier(): Promise<void>; holdHistory(): void; settleHistory(success: boolean): Promise<void>; holdSearch(): void; settleSearch(query: string): Promise<void>; allowNodeReads(): void; holdNode(): void; settleNode(success: boolean): Promise<void>; uncertainUpload(): void; resolveUpload(): void; settingsTargets(): string[]; waitForNavigation(): Promise<void>; captureNavigation(): void; navigationCurrent(): boolean; navigationFacts(): { generation: number; claims: import('../../../protocol/app-server/v57').AttachmentTarget[]; views: Record<string, { attachment: string; nodeId?: string; intent: string; target?: import('../../../protocol/app-server/v57').AttachmentTarget; authoritative: boolean }> }; holdBackgroundCheckpoint(): Promise<void>; holdSnapshot(): Promise<void>; settleSnapshot(): Promise<void>; staleDestination(): Promise<void>; replaceConnection(): Promise<void>; readyNavigation(): Promise<void>; holdAttach(): void; settleAttach(success: boolean, sessionId?: string): void; requests(): import("../../../protocol/app-server/v57").Request[]; releaseCold(): Promise<number>; activity(id: string, timestamp: string): Promise<void>; releaseAssociations(): void; stream(text: string): Promise<void>; state(mode: 'idle' | 'queued' | 'stopping' | 'reconnect' | 'uncertain'): Promise<void>; presentation(mode: 'empty' | 'preview' | 'named' | 'delete' | 'other-uncertain' | 'many'): Promise<void> } } }

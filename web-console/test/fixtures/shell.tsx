// Isolated deterministic wire fixture. Never imported by the production entry.
import { createRoot } from 'react-dom/client';
import { App } from '../../src/app/App';
import { traceRecord } from '../trace-fixture';
import { cfg3Source } from '../cfg3-data';
import { RpcFailure } from '../../src/client/app-server';
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
server.workspaceHost.listWorkspaces = async () => ({ authorityId: 'fixture-host', endpoint, workspaces: [{ id: 'project', displayName: 'rustX', displayPath: '/workspace', location: 'project' }], picker: { kind: 'unavailable', reason: 'Fixture has one authorized project' } });
// Reference tests explicitly release the cold display baseline after its first paint.
// Other shell consumers keep the ordinary immediate fixture response.
let releaseAssociations!: () => void;
const associationGate = new Promise<void>(resolve => { releaseAssociations = resolve; });
if (!new URL(location.href).searchParams.has('association-gate')) releaseAssociations();
server.workspaceHost.classifyLocations = async cwds => { await associationGate; return cwds.map(cwd => ({ authorized: true, workspaceId: cwd.endsWith('/C') ? undefined : 'project' })); };
// The shell's registered Workspace supplies the same generated source contract
// as the permission seat. User Settings retains its explicit error fixture.
server.workspaceHost.configureWorkspace = async (_id, _endpoint, operation) => {
  const projection = { ...cfg3Source(), target: { kind: 'workspace' as const, directory: '/workspace' } };
  if (operation.kind === 'write') return { kind: 'write', commit: { acknowledgement: projection, reread: { status: 'observed', projection } } };
  if (operation.kind === 'mcp_probe') return {kind:'mcp_probe',result:{id:operation.id,revision:operation.expected_revision,outcome:'reachable'}};
  return { kind: operation.kind, projection };
};
const cold = new URL(location.href).searchParams.get('initial') === 'cold';
if (cold) {
  server.snapshots.get('A')!.attempt = null;
  server.snapshots.get('A')!.transcript.statistics = { turns: '8', steps: '46', completed_responses: '8', model_requests: '46', requests_with_usage: '46', reported_usage: { input_tokens: 900000, output_tokens: 100000, total_tokens: 1000000 } };
  server.handlers.set('session/settings', () => ({ type: 'settings', revision: '0', settings: { cwd: '/workspace/A', model: { model: 'DeepSeek/deepseek-flash' } } }));
  server.snapshots.get('A')!.trace = { records: [traceRecord(1)] };
  server.held.add('session/attach'); server.held.add('turn/start');
  await server.connect();
} else await server.attached('A', 'B');
localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint, openViews: ['A', 'B'] }));

// Isolated fixture controls, never reachable from the production application.
window.sessionFixture = {
  releaseAssociations,
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
    next.transcript.statistics = { turns: '1', steps: '1', completed_responses: '0', model_requests: '1', requests_with_usage: '0', latest_turn: { attempt_id: 'attempt-A', started_at: '2026-09-25T00:00:00Z' } };
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

declare global { interface Window { sessionFixture: { releaseCold(): Promise<number>; activity(id: string, timestamp: string): Promise<void>; releaseAssociations(): void; stream(text: string): Promise<void>; state(mode: 'idle' | 'queued' | 'stopping' | 'reconnect' | 'uncertain'): Promise<void>; presentation(mode: 'empty' | 'preview' | 'named' | 'delete' | 'other-uncertain' | 'many'): Promise<void> } } }

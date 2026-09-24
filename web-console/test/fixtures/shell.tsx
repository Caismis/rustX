// Isolated deterministic wire fixture. Never imported by the production entry.
import { createRoot } from 'react-dom/client';
import { App } from '../../src/app/App';
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
server.workspaceHost.listWorkspaces = async () => ({ endpoint, workspaces: [{ id: 'project', displayName: 'rustX', displayPath: '/workspace', location: 'project' }], picker: { kind: 'unavailable', reason: 'Fixture has one authorized project' } });
server.workspaceHost.classifyLocations = async cwds => cwds.map(cwd => ({ authorized: true, workspaceId: cwd.endsWith('/C') ? undefined : 'project' }));
await server.attached('A', 'B');
localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint, openViews: ['A', 'B'] }));
createRoot(document.getElementById('root')!).render(<App client={server.client} workspaceHost={server.workspaceHost} />);

// Isolated fixture controls, never reachable from the production application.
window.sessionFixture = {
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
      void server.client.cancelTurn('A').catch(() => {});
      await server.waitFor('turn/cancel', 1);
    }
    if (mode === 'reconnect' || mode === 'uncertain') server.socket.close();
  },
};
declare global { interface Window { sessionFixture: { state(mode: 'idle' | 'queued' | 'stopping' | 'reconnect' | 'uncertain'): Promise<void>; presentation(mode: 'empty' | 'preview' | 'named' | 'delete' | 'other-uncertain' | 'many'): Promise<void> } } }

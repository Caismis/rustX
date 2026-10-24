import type { MessageBlock } from '../../../protocol/app-server/v57';
import { childConversation, type Server } from '../fixture';

/** Hold a real continuation while the resident Child Chat changes visibility. */
export function childMessageContinuation(server: Server) {
  const message: MessageBlock = { role: 'user', id: 'child-uploads', source: { agent: { agent_id: 'root' } }, content:
    Array.from({ length: 32 }, (_, n) => ({ type: 'uploaded_file', batch_id: 'task-files', name: `child-file-${n}.txt` })) };
  server.held.add('conversation/checkpointPage');
  server.handlers.set('agent/observe', request => {
    if (request.method !== 'agent/observe') throw Error('Expected Child observation');
    const agent = server.snapshots.get('A')!.agents!.find(agent => agent.agent_id === request.params.agent_id)!;
    return childConversation({ entries: [{ cursor: '1', item: { type: 'message', message } }], collections: [
      { domain: 'messages.ledger:1.blocks', revision: '0', total: '33', loaded: '32', next_offset: '32' },
    ] }, agent.agent_id, agent.activation_id, agent.child_conversation_id);
  });
  server.handlers.set('conversation/checkpointPage', request => {
    if (request.method !== 'conversation/checkpointPage' || request.params.agent_id !== 'child-1' || request.params.offset !== 32) throw Error('Expected original Child continuation');
    return { type: 'checkpoint_page', page: { read_cut: { canonical_foreground: null, live_publication: null, source_cut: null, ...request.params.cut }, cursor: request.params.cut.revision, domain: request.params.domain,
      total: '33', next_offset: null, collections: [], items: [{ offset: '32', identity: 'text', value: { type: 'text', text: 'Use all attached sources for the child task.' }, content: [], deferred: false }] } };
  });
}

import { childConversation, checkpoint, type Server } from '../fixture';
import { cfg3Effective } from '../cfg3-data';
import { RpcFailure } from '../../src/client/errors';
import type { Request } from '../../../protocol/app-server/v57';

/** Gate only native reads; all child navigation, cursor ownership and UI are real. */
export function childObservationRecovery(server: Server) {
  const replied = new Set<Request>();
  let reads = 0;
  server.handlers.set('agent/observe', request => {
    if (request.method !== 'agent/observe' || request.params.agent_id !== 'child-0') throw Error('Expected the selected child');
    replied.add(request);
    if (++reads === 2) throw new RpcFailure({ code: -32000, message: 'Operation rejected', data: {
      kind: 'observation_unavailable', diagnostic: 'child observation: Connection reset by peer (os error 104)',
    } });
    const agent = server.snapshots.get('A')!.agents!.find(agent => agent.agent_id === request.params.agent_id)!;
    const result = childConversation({ entries: [{ cursor: '1', item: { type: 'message', message: {
      role: 'assistant', id: 'child-report', content: [{ type: 'text', text: reads === 1
        ? '# Child research report\n\nRetained source evidence.\n\n' + 'Detailed findings and supporting evidence. '.repeat(120)
        : '# Child research report\n\nRecovered canonical source evidence.' }],
    } } }] }, agent.agent_id, agent.activation_id, agent.child_conversation_id);
    if (result.observation.type !== 'checkpoint') throw Error('Expected fixture checkpoint');
    const cursor = reads === 1 ? '10' : '11';
    const model = cfg3Effective().effective_model!;
    result.observation.checkpoint = checkpoint({ ...result.observation.checkpoint.snapshot,
      effective_approval_mode: 'full_access', model: { ...model, effective: { ...model.effective!, model: 'child/own-model' } },
    }, cursor);
    result.observation.checkpoint.read_cut.activation_id = agent.activation_id;
    if (reads === 1) {
      result.observation.source = 'live';
      result.observation.cursor = { agent_id: agent.agent_id, conversation_id: agent.child_conversation_id, activation_id: agent.activation_id, cursor };
      server.held.add('agent/observe');
    }
    return result;
  });
  return () => {
    const pending = server.requests.find(({ request }) => request.method === 'agent/observe' && !replied.has(request));
    if (!pending) throw Error('Expected one pending child read');
    server.reply(pending.request);
  };
}

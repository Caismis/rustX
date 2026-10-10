import type { ConversationTurn, RuntimeClientTranscriptEntry } from '../../../protocol/app-server/v42';
import { childConversation, type Server } from '../fixture';

/** Large child history proves navigation uses native directory/window reads,
 * including turns absent from the watched tail. No production state owner. */
export function childNavigation(server: Server) {
  const conversation = (agentId: string) => server.snapshots.get('A')!.agents!.find(agent => agent.agent_id === agentId)!.child_conversation_id;
  const cut = (agentId: string) => ({ conversation_id: conversation(agentId), journal: '1000', transcript: '1000', mutation_revision: '0' });
  const turn = (agentId: string, n: number): ConversationTurn => ({ id: { conversation_id: conversation(agentId), attempt_id: `child-turn-${n}` }, ordinal: n, cursor: String(n), prompt: `Child task ${n}`, response: `Child report ${n}` });
  const entry = (agentId: string, n: number): RuntimeClientTranscriptEntry => ({ cursor: String(n), item: { type: 'message', message: { role: 'assistant', id: `${agentId}-reply-${n}`, content: [{ type: 'text', text: `## Child report ${n}\n\n` + 'Independent research evidence. '.repeat(n === 130 ? 2 : 25) }] } }, turn_process: { ...turn(agentId, n).id, control_cursor: String(n), final_message_id: `${agentId}-reply-${n}`, message_count: 1, tool_call_count: 0, outcome: 'completed' } });
  server.handlers.set('agent/conversation', request => {
    if (request.method !== 'agent/conversation') throw Error('method');
    return childConversation({ entries: [entry(request.params.agent_id, 130)], next_cursor: '130' }, request.params.agent_id, 'activation-a', conversation(request.params.agent_id));
  });
  server.handlers.set('agent/turns', request => {
    if (request.method !== 'agent/turns') throw Error('method');
    const { agent_id, offset: input } = request.params, offset = input ?? 128;
    return { type: 'conversation_turns', page: { cut: cut(agent_id), total: 130, offset, turns: Array.from({ length: Math.min(64, 130 - offset) }, (_, index) => turn(agent_id, index + offset + 1)) } };
  });
  server.handlers.set('agent/transcript', request => {
    if (request.method !== 'agent/transcript') throw Error('method');
    const { agent_id, at } = request.params;
    const n = at.type === 'turn' ? Number(at.id.attempt_id.slice(11)) : at.type === 'older' ? Math.max(1, Number(at.before) - 4) : at.type === 'newer' ? Number(at.after) + 1 : 127;
    const entries = Array.from({ length: Math.min(4, 131 - n) }, (_, index) => entry(agent_id, n + index));
    return { type: 'transcript_window', window: { cut: cut(agent_id), page: { entries, next_cursor: n > 1 ? String(n) : null }, newer_cursor: n + entries.length < 131 ? String(n + entries.length - 1) : null,
      target: at.type === 'turn' ? at.id : null, target_cursor: at.type === 'turn' ? String(n) : null } };
  });
}

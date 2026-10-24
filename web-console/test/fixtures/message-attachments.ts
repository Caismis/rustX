import type { MessageBlock } from '../../../protocol/app-server/v57';
import { childConversation, type Server } from '../fixture';

/** Historical upload references expose identity/name, never a fabricated size. */
export function messageAttachments(server: Server) {
  const messages: MessageBlock[] = [
    { role: 'user', id: 'compose', source: 'human', timestamp: '2026-10-20T08:40:00Z', content: [
      { type: 'uploaded_file', batch_id: 'compose-batch', name: 'compose.yml' }, { type: 'text', text: '研究一下这个' },
    ] },
    { role: 'assistant', id: 'compose-answer', content: [{ type: 'text', text: '我会先核对这个配置文件。' }] },
    { role: 'user', id: 'files-only', source: 'human', timestamp: '2026-10-20T08:41:00Z', content: [
      { type: 'uploaded_file', batch_id: 'docs-batch', name: '架构说明-包含一个很长的文件名称以验证省略显示.pdf' },
      { type: 'uploaded_file', batch_id: 'docs-batch', name: 'notes.md' }, { type: 'text', text: '' },
    ] },
    { role: 'assistant', id: 'files-answer', content: [{ type: 'text', text: '已收到这两份附件。' }] },
  ];
  server.snapshots.get('A')!.transcript = { entries: messages.map((message, index) => ({ cursor: String(index + 1), item: { type: 'message', message } })) };
  server.handlers.set('agent/observe', request => {
    if (request.method !== 'agent/observe') throw Error('Expected child read');
    const agent = server.snapshots.get('A')!.agents!.find(agent => agent.agent_id === request.params.agent_id)!;
    return childConversation({ entries: [{ cursor: '1', item: { type: 'message', message: { ...messages[0], role: 'user', source: { agent: { agent_id: agent.parent_agent_id } }, content: messages[0]!.role === 'user' ? messages[0]!.content : [] } } }] }, agent.agent_id, agent.activation_id, agent.child_conversation_id);
  });
}

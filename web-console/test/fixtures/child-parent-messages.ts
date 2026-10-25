import type { MessageBlock } from '../../../protocol/app-server/v57';
import { childConversation, type Server } from '../fixture';
import { cfg3Effective } from '../cfg3-data';

export const parentTask = '任务：核对原始资料，确认两个人名之间是否存在关联。\n\n要求：\n1. 检索中英文资料，并记录可靠来源。\n2. 区分已证实的事实、推测和无证据的说法。\n3. 无法确认的内容请明确写出。\n\n输出格式：中文报告，先给结论，再列证据与来源 URL。';
export const parentGuidance = '补充：保留原始链接，不要把同名人物混为一谈。\n\n请继续核验后再给出结论。';

/** Native direct-parent provenance is identical for initial tasks and guidance. */
export function childParentMessages(server: Server) {
  server.handlers.set('agent/observe', request => {
    if (request.method !== 'agent/observe') throw Error('Expected child read');
    const agent = server.snapshots.get('A')!.agents!.find(agent => agent.agent_id === request.params.agent_id)!;
    const messages: MessageBlock[] = [
      { role: 'user', id: 'parent-task', source: { agent: { agent_id: agent.parent_agent_id } }, timestamp: '2026-10-08T07:15:00Z', content: [{ type: 'text', text: parentTask }] },
      { role: 'assistant', id: 'child-progress', content: [{ type: 'text', text: '我会检索并交叉核对原始资料。' }] },
      { role: 'user', id: 'parent-guidance', source: { agent: { agent_id: agent.parent_agent_id } }, timestamp: '2026-10-08T07:16:00Z', content: [{ type: 'text', text: parentGuidance }] },
      { role: 'assistant', id: 'child-report', content: [{ type: 'text', text: '## 核验结果\n\n已完成原始资料核对，以下结论附有可核查来源。' }] },
    ];
    const result = childConversation({ entries: messages.map((message, index) => ({ cursor: String(index + 1), item: { type: 'message', message } })) }, agent.agent_id, agent.activation_id, agent.child_conversation_id);
    if (result.observation.type !== 'checkpoint') throw Error('Expected child checkpoint');
    result.observation.checkpoint.snapshot.model = cfg3Effective().effective_model;
    result.observation.checkpoint.snapshot.effective_approval_mode = 'full_access';
    return result;
  });
}

import type { MessageBlock, RuntimeClientSnapshot, RuntimeClientTranscriptEntry, TurnProcessView } from '../../../protocol/app-server/v74';
import type { Server } from '../fixture';
import { workflowRunsFixture } from './workflow-runs';

/** Same Workflow in two Turns: one call follows reasoning, one owns the header. */
export function workflowTurnsFixture(server: Server, current: RuntimeClientSnapshot, hiddenReply = false, withStatus = false) {
  workflowRunsFixture(server, current);
  const run = current.workflows!.runs[0], work = current.transcript.entries![1];
  const entries: RuntimeClientTranscriptEntry[] = [], runs = [] as typeof current.workflows.runs, cursors: string[] = [];
  for (const [index, reasoning] of [true, false].entries()) {
    const attempt_id = `workflow-turn-${index + 1}`, start = entries.length + 1;
    const user: MessageBlock = { role: 'user', source: 'human', id: `workflow-turn-user-${index}`, content: [{ type: 'text', text: '请再运行一次工作流。' }] };
    const call: MessageBlock = { role: 'assistant', id: `workflow-turn-call-${index}`, content: [{ type: 'tool_call', id: `workflow-turn-tool-${index}`, tool_id: 'workflow:test_flow', name: 'workflow:test_flow', arguments: {} }] };
    const answer: MessageBlock = { role: 'assistant', id: `workflow-turn-answer-${index}`, content: [{ type: 'text', text: '工作流已完成。\n\n> 两轮使用相同的工作流卡片。' }] };
    const owner: TurnProcessView = { ...work.turn_process!, attempt_id, control_cursor: String(start + 1), final_message_id: answer.id,
      final_message_locator: { type: 'by_message_id', message_id: answer.id }, tool_call_count: 1, message_count: reasoning ? hiddenReply ? 4 : 3 : 2 };
    cursors.push(owner.control_cursor);
    entries.push({ cursor: String(start), item: { type: 'message', message: user } });
    if (reasoning) entries.push({ cursor: String(entries.length + 1), turn_process: owner,
      item: { type: 'message', message: { role: 'assistant', id: 'workflow-turn-reasoning', content: [{ type: 'reasoning', text: '先检查请求，再调用工作流。' }] } } });
    if (reasoning && hiddenReply) entries.push({ cursor: String(entries.length + 1), turn_process: owner,
      item: { type: 'message', message: { role: 'assistant', id: 'workflow-turn-intermediate', content: [{ type: 'text', text: '正在准备调用工作流。' }] } } });
    entries.push({ cursor: String(entries.length + 1), turn_process: owner, item: { type: 'message', message: call },
      tool_calls: [{ ...work.tool_calls![0], message_id: call.id, call_id: `workflow-turn-tool-${index}`, occurrence: { locator: { type: 'by_message_id', message_id: call.id }, block_index: 0 }, block_index: 0 }] });
    entries.push({ cursor: String(entries.length + 1), turn_process: owner, item: { type: 'message', message: answer }, completed_response: {
      origin: { conversation_id: owner.conversation_id, attempt_id, attempt_id_source: null, closing_message_id: answer.id }, closing_message_id: answer.id,
      closing_locator: { type: 'by_message_id', message_id: answer.id }, deferred_fields: [], surface_revision: '1',
      completed_at: `2026-11-03T06:3${index}:00Z`, timing: { total_duration_ms: index ? 5000 : 7000 },
    } });
    const next = structuredClone(run);
    next.id = { conversation_id: current.conversation_id, origin: { type: 'tool', attempt_id }, invocation: '1' };
    next.tool_call_id = `workflow-turn-tool-${index}`;
    next.instances = next.instances.map(instance => ({ ...instance, block: { ...instance.block, run: next.id } }));
    runs.push(next);
  }
  current.transcript.entries = entries;
  current.messages = entries.flatMap(entry => entry.item.type === 'message' ? [entry.item.message] : []);
  current.workflows = { revision: '2', runs, omitted_runs: 0 };
  if (withStatus) current.statuses = [{ attempt_id: 'workflow-turn-1', turn: 1, status_message_id: 'workflow-turn-native-status',
    opportunities: { fresh_inbound: { target_message_id: 'workflow-turn-user-0' } },
    sections: [{ type: 'temporal', current_time: '2026-11-03T06:34:00Z', timezone: 'Asia/Shanghai' }], rendered: 'Native time observation' }];
  server.handlers.set('session/turns', () => ({ type: 'conversation_turns', page: {
    cut: { conversation_id: current.conversation_id, journal: String(entries.length), transcript: String(entries.length), mutation_revision: '0', ledger_through: String(entries.length), surface_revision: '1', transcript_total: String(entries.length), pending_through: '0', pending_total: '0' },
    offset: 0, total: '2', turns: [0, 1].map(index => ({ id: { conversation_id: current.conversation_id, attempt_id: `workflow-turn-${index + 1}`, attempt_id_source: null },
      ordinal: index + 1, cursor: cursors[index], prompt: '请再运行一次工作流。', response: '工作流已完成。' })),
  } }));
}

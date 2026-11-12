import type { MessageBlock, RuntimeClientSnapshot, TurnProcessView } from '../../../protocol/app-server/v79';
import type { Server } from '../fixture';
import { conversationPreferences } from '../../src/app/conversation-preferences';

/** Native call identity survives reply arrival and settlement in one Turn. */
export function chatFlowFixture(server: Server, current: RuntimeClientSnapshot) {
  const owner: TurnProcessView = { conversation_id: current.conversation_id, attempt_id: 'flow-attempt', attempt_id_source: null,
    final_message_id: null, final_message_locator: null, final_message_id_deferred: false,
    final_message_id_source: null, outcome: 'running', control_cursor: '2', message_count: 2, tool_call_count: 2,
    started_at: '2026-11-03T01:00:00Z', ended_at: null };
  const user: MessageBlock = { role: 'user', source: 'human', id: 'flow-user', content: [{ type: 'text', text: '梳理对话流的布局，让思考、工具调用和最终回复更容易阅读。' }] };
  const calls = [
    { id: 'flow-read', tool_id: 'tool-read', name: 'read', arguments: { path: 'src/components/conversation/a-long-file-name-for-the-message-stream.tsx' } },
    { id: 'flow-bash', tool_id: 'tool-bash', name: 'bash', arguments: { command: 'pnpm test -- conversation' } },
  ];
  const answer: Extract<MessageBlock, { role: 'assistant' }> = { role: 'assistant', id: 'flow-work', content: [
    { type: 'reasoning', text: '**先核对布局与交互**\n\n保持消息顺序，分别检查工具行、回复间距，以及触屏上的展开反馈。' },
    ...calls.map(call => ({ type: 'tool_call' as const, ...call })),
  ] };
  current.messages = [user, answer];
  current.transcript.entries = [{ cursor: '1', item: { type: 'message', message: user } }, { cursor: '2', turn_process: owner,
    item: { type: 'message', message: answer }, tool_calls: calls.map((call, index) => ({ assembly_complete: true,
      committed_result_deferred: false, occurrence: { locator: { type: 'by_message_id', message_id: answer.id }, block_index: index + 1 },
      message_id: answer.id, block_index: index + 1, call_id: call.id, tool_id: call.tool_id, name: call.name,
      state: index === 1 ? { type: 'running', arguments: JSON.stringify(call.arguments) }
        : { type: 'settled', arguments: JSON.stringify(call.arguments), result: { status: { type: 'success' }, duration_ms: '12', content: [{ type: 'text', text: 'export function Conversation() { return <Chat />; }' }] } },
    })) }];
  current.attempt = { attempt_id: owner.attempt_id, phase: { type: 'running' }, turn: 1 };
  // The outline and transcript describe the same cut, as the native owner does.
  // A zero/empty default outline would incorrectly claim this Turn is missing.
  server.handlers.set('session/turns', () => ({ type: 'conversation_turns', page: {
    cut: { conversation_id: owner.conversation_id, journal: '3', transcript: server.snapshots.get('A')!.transcript.entries!.at(-1)!.cursor, mutation_revision: '0', ledger_through: '3', surface_revision: '1', transcript_total: String(server.snapshots.get('A')!.transcript.entries!.length), pending_through: '0', pending_total: '0' },
    total: '1', offset: 0, turns: [{ id: { conversation_id: owner.conversation_id, attempt_id: owner.attempt_id, attempt_id_source: null }, ordinal: 1, cursor: '2', prompt: '梳理对话流的布局', response: '对话流已整理' }],
  } }));
  const reply = () => {
    const next = structuredClone(server.snapshots.get('A')!);
    if (!next.transcript.entries!.some(row => row.item.type === 'message' && row.item.message.id === 'flow-answer')) next.transcript.entries!.push({ cursor: '3', turn_process: owner,
      item: { type: 'message', message: { role: 'assistant', id: 'flow-answer', content: [{ type: 'text', text: '## 对话流已整理\n\n思考与工具调用保持紧凑，最终回复独立呈现。\n\n- 清晰的过程与回复层级\n- 稳定的运行状态位置\n- 桌面、手机均可展开查看详情' }] } } });
    next.messages = next.transcript.entries!.flatMap(row => row.item.type === 'message' ? [row.item.message] : []);
    return next;
  };
  window.chatFlowFixture = {
    fontSize: value => conversationPreferences().update({ fontSize: value }),
    outlineReads: () => server.requests.filter(row => row.request.method === 'session/turns').length,
    reply: () => server.update('A', reply()),
    async settle() {
      const next = reply(), work = next.transcript.entries![1], entry = next.transcript.entries!.at(-1)!;
      for (const row of next.transcript.entries!.slice(1)) row.turn_process = { ...owner, outcome: 'completed', final_message_id: 'flow-answer',
        final_message_locator: { type: 'by_message_id', message_id: 'flow-answer' }, ended_at: '2026-11-03T01:01:05Z' };
      work.tool_calls = work.tool_calls!.map(tool => ({ ...tool, state: { type: 'settled', arguments: tool.state.arguments,
        result: { status: { type: 'success' }, duration_ms: '12', content: [{ type: 'text', text: 'All conversation checks passed.' }] } } }));
      entry.completed_response = { origin: { conversation_id: owner.conversation_id, attempt_id: owner.attempt_id, attempt_id_source: null, closing_message_id: 'flow-answer' },
        closing_message_id: 'flow-answer', closing_locator: { type: 'by_message_id', message_id: 'flow-answer' }, surface_revision: '1', deferred_fields: [],
        completed_at: '2026-11-03T01:01:05Z', timing: { total_duration_ms: 65000, ttft_ms: 100, generation_ms: 64900, output_tokens_per_second: 10 },
        usage: { input_tokens: '256', output_tokens: '128', total_tokens: '384' } };
      next.attempt = { attempt_id: owner.attempt_id, phase: { type: 'settled', outcome: { type: 'completed', finish_reason: { type: 'stop' } } }, turn: 1 };
      await server.update('A', next);
    },
  };
}

declare global { interface Window { chatFlowFixture: { reply(): Promise<void>; settle(): Promise<void>; fontSize(value: number): void; outlineReads(): number } } }

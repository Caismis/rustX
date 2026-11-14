import type { ForegroundToolExecution, RuntimeClientSnapshot } from '../../../protocol/app-server/v78';
import { interaction, type Server } from '../fixture';
import { workflowReviewHistoryFixture } from './workflow-review-history';

export function questionHistoryFixture(server: Server, current: RuntimeClientSnapshot) {
  workflowReviewHistoryFixture(server, current);
  const entry = current.transcript.entries![1];
  if (entry.item.type !== 'message' || entry.item.message.role !== 'assistant') throw Error('Expected native Tool call entry');
  const questions = [
    { header: 'Direction', question: 'Which direction?', options: [{ label: 'Keep native', description: 'Keep runtime ownership.' }, { label: 'Simplify', description: 'Simplify the interface.', preview: '**UI only** preview' }] },
    { header: 'Checks', question: 'Which checks?', multi_select: true, options: [{ label: 'Unit', description: 'Check the data.' }, { label: 'Browser', description: 'Check the interface.' }] },
    { header: 'Scope', question: 'Which scope?', options: [{ label: 'Desktop', description: 'Desktop views.' }, { label: 'Mobile', description: 'Mobile views.' }] },
  ];
  const id = 'ask-history', block_index = entry.item.message.content.length;
  entry.item.message.content.push({ type: 'tool_call', id, tool_id: 'tool-ask-user', name: 'ask_user', arguments: { arguments: { questions } } });
  const tool: ForegroundToolExecution = { message_id: entry.item.message.id, block_index, call_id: id, tool_id: 'tool-ask-user', name: 'ask_user', assembly_complete: true,
    committed_result_deferred: false, occurrence: { locator: { type: 'by_message_id', message_id: entry.item.message.id! }, block_index },
    state: { type: 'settled', arguments: JSON.stringify({ arguments: { questions } }), result: { status: { type: 'success' }, duration_ms: '1000', content: [{ type: 'json', value: { cancelled: false, answers: [
      { question_index: 0, kind: 'option', answer: 'Simplify' }, { question_index: 1, kind: 'multiple', selected: ['Browser', 'Unit'] },
    ] } }] } } };
  entry.tool_calls!.push(tool);
  for (const entry of current.transcript.entries!) if (entry.turn_process) entry.turn_process.tool_call_count = 3;
  // A new native ask is pending while the reader inspects the earlier record.
  const pending = interaction('questionnaire');
  const kind = pending.body!.kind;
  if (kind.type !== 'questionnaire') throw Error('Expected native questionnaire');
  const args = { questions: kind.questionnaire.questions.map(question => {
    if (question.answer.type !== 'single_choice') throw Error('Expected native choice');
    return { header: question.header, question: question.question, options: question.answer.options };
  }) };
  const prompt = { role: 'user' as const, source: 'human' as const, id: 'pending-user', content: [{ type: 'text' as const, text: 'Ask another question.' }] };
  const message = { role: 'assistant' as const, id: 'pending-ask-message', content: [{ type: 'tool_call' as const, id: 'call-ask', tool_id: 'tool-ask-user', name: 'ask_user', arguments: args }] };
  const live: ForegroundToolExecution = { ...tool, message_id: message.id, block_index: 0, call_id: 'call-ask',
    occurrence: { locator: { type: 'by_message_id', message_id: message.id }, block_index: 0 }, state: { type: 'running', arguments: JSON.stringify({ arguments: args }) } };
  current.pending_interactions = [pending];
  current.attempt = { attempt_id: 'attempt-A', phase: { type: 'running' }, turn: 2, foreground: [live], execution_settings: { resource_revision: '1', approval_mode: 'plan' } };
  current.messages.push(prompt, message);
  current.transcript.entries!.push({ cursor: '8', item: { type: 'message', message: prompt } }, { cursor: '9', item: { type: 'message', message }, tool_calls: [live] });
  server.handlers.set('session/turns', () => ({ type: 'conversation_turns', page: {
    cut: { conversation_id: current.conversation_id, journal: '9', transcript: '9', mutation_revision: '0', ledger_through: '9', surface_revision: '1', transcript_total: '9', pending_through: '0', pending_total: '0' },
    offset: 0, total: '2', turns: [
      { id: { conversation_id: current.conversation_id, attempt_id: 'workflow-attempt', attempt_id_source: null }, ordinal: 1, cursor: '2', prompt: '运行两次工作流', response: '两次调用的结果' },
      { id: { conversation_id: current.conversation_id, attempt_id: 'attempt-A', attempt_id_source: null }, ordinal: 2, cursor: '9', prompt: 'Ask another question.', response: '' },
    ],
  } }));
}

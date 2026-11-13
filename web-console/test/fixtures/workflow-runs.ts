import type { ForegroundToolExecution, MessageBlock, RuntimeClientSnapshot, TurnProcessView } from '../../../protocol/app-server/v79';
import type { Server } from '../fixture';
import { workflowRun } from '../workflow-data';

/** Two explicit native calls, as in the reported completed/cancelled history. */
export function workflowRunsFixture(server: Server, current: RuntimeClientSnapshot) {
  const owner: TurnProcessView = { conversation_id: current.conversation_id, attempt_id: 'workflow-attempt', attempt_id_source: null,
    final_message_id: 'workflow-answer', final_message_locator: { type: 'by_message_id', message_id: 'workflow-answer' }, final_message_id_deferred: false,
    final_message_id_source: null, outcome: 'completed', control_cursor: '2', message_count: 2, tool_call_count: 2 };
  const runs = [false, true].map(cancelled => {
    const run = workflowRun();
    run.id = { conversation_id: current.conversation_id, origin: { type: 'tool', attempt_id: 'workflow-attempt' }, invocation: cancelled ? '2' : '1' };
    run.workflow_id = cancelled ? 'test_flow_cancelled_with_a_long_name_for_layout' : 'test_flow';
    run.tool_call_id = cancelled ? 'workflow-call-2' : 'workflow-call-1';
    run.state = { type: 'settled', outcome: cancelled ? 'cancelled' : 'completed' };
    run.steps_consumed = 3; run.steps_max = 20; run.agents_consumed = 0;
    const block = { run: run.id, definition: { workflow_id: run.workflow_id, blocks: [] }, invocations: [] };
    run.instances = [
      { block, node: 'format_answer', kind: 'template', visit: 1, state: { type: 'settled', outcome: 'completed' } },
      { block, node: 'select_items', kind: 'list', visit: 1, state: { type: 'settled', outcome: 'completed' } },
      { block, node: cancelled ? 'request_user_input_for_final_decision' : 'done', kind: cancelled ? 'review' : 'return', visit: 1, state: run.state },
    ];
    return run;
  });
  const user: MessageBlock = { role: 'user', source: 'human', id: 'workflow-user', content: [{ type: 'text', text: '运行两次工作流并汇总结果。' }] };
  const calls: Extract<MessageBlock, { role: 'assistant' }> = { role: 'assistant', id: 'workflow-work', content: runs.map(run => ({ type: 'tool_call', id: run.tool_call_id!, tool_id: `workflow:${run.workflow_id}`, name: `workflow:${run.workflow_id}`, arguments: { arguments: {} } })) };
  const tools: ForegroundToolExecution[] = runs.map((run, index) => ({ message_id: calls.id, block_index: index, call_id: run.tool_call_id!, tool_id: `workflow:${run.workflow_id}`, name: `workflow:${run.workflow_id}`,
    assembly_complete: true, committed_result_deferred: false, occurrence: { locator: { type: 'by_message_id', message_id: calls.id }, block_index: index },
    state: { type: 'settled', arguments: "{\"arguments\":{}}", result: { status: index ? { type: 'cancelled', reason: 'user_requested', phase: 'during_execution' } : { type: 'success' }, duration_ms: '9000', content: [{ type: 'text', text: index ? 'Cancelled by the user.' : 'bark' }] } } }));
  const answer: MessageBlock = { role: 'assistant', id: 'workflow-answer', content: [{ type: 'text', text: '两次调用的结果：\n\n| 工作流 | 结果 |\n| --- | --- |\n| test_flow | 已完成 |\n| 第二次调用 | 已取消，可能已产生部分副作用 |' }] };
  current.messages = [user, calls, answer]; current.attempt = null;
  current.transcript.entries = [{ cursor: '1', item: { type: 'message', message: user } },
    { cursor: '2', turn_process: owner, item: { type: 'message', message: calls }, tool_calls: tools },
    { cursor: '3', turn_process: owner, item: { type: 'message', message: answer }, completed_response: {
      origin: { conversation_id: owner.conversation_id, attempt_id: owner.attempt_id, attempt_id_source: null, closing_message_id: answer.id }, closing_message_id: answer.id,
      closing_locator: { type: 'by_message_id', message_id: answer.id }, surface_revision: '1', deferred_fields: [], completed_at: '2026-11-03T06:43:00Z', timing: { total_duration_ms: 9000 },
    } }];
  current.workflows = { revision: '1', runs, omitted_runs: 0 };
  server.handlers.set('session/turns', () => ({ type: 'conversation_turns', page: {
    cut: { conversation_id: current.conversation_id, journal: '3', transcript: server.snapshots.get('A')!.transcript.entries!.at(-1)!.cursor, mutation_revision: '0', ledger_through: '3', surface_revision: '1', transcript_total: String(server.snapshots.get('A')!.transcript.entries!.length), pending_through: '0', pending_total: '0' },
    offset: 0, total: '1', turns: [{ id: { conversation_id: owner.conversation_id, attempt_id: owner.attempt_id, attempt_id_source: null }, ordinal: 1, cursor: '2', prompt: '运行两次工作流', response: '两次调用的结果' }],
  } }));
}

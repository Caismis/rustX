import type { ForegroundToolExecution, ToolExecutionResult } from '../../protocol/app-server/v57';

const text = (value: string): ToolExecutionResult['content'] => [{ type: 'text', text: value }];
const json = (value: unknown): ToolExecutionResult['content'] => [{ type: 'json', value: value as never }];
export function nativeTool(id: string, name: string, args: object, content: ToolExecutionResult['content']): ForegroundToolExecution {
  return { call_id: name, tool_id: id, name, message_id: `message-${name}`, block_index: 0, occurrence: { locator: { type: 'by_message_id', message_id: `message-${name}` }, block_index: 0 }, assembly_complete: true, committed_result_deferred: false,
    state: { type: 'settled', arguments: JSON.stringify(args), result: { status: { type: 'success' }, duration_ms: '30000', content } } };
}
export const goalResult = { reference: { id: 'goal-native', revision: 3 }, objective: '核对原生工具的显示与交互', phase: 'blocked', blocked_reason: '等待构建环境恢复', autonomous_round_budget: 10, autonomous_rounds_consumed: 3, origin: { kind: 'runtime_control' }, last_round_message_id: null };
export const jobResult = { job_id: 'exec-native-sleep', tool: 'bash', state: 'succeeded', progress: null, result: { status: { type: 'success' }, duration_ms: 30000, exit_code: 0,
  content: [{ type: 'json', value: { combined: '开始: 2026-10-19 23:03:13 CST\n结束: 2026-10-19 23:03:43 CST\n' } }] } };
export const nativeToolCases = [
  nativeTool('tool-bash', 'bash', { command: 'printf done', description: '核对命令输出' }, json({ combined: 'done\n' })),
  nativeTool('tool-read', 'read', { path: '/workspace/report.md', offset: 5 }, text('report line 5\nreport line 6')),
  nativeTool('tool-write', 'write', { path: '/workspace/report.md', content: 'new report\n' }, text('Successfully wrote 11 bytes to /workspace/report.md')),
  nativeTool('tool-edit', 'edit', { path: '/workspace/report.md', edits: [{ oldText: 'old report', newText: 'new report' }] }, text('Successfully replaced text in /workspace/report.md.')),
  nativeTool('tool-glob', 'glob', { pattern: '*.md' }, text('report.md\nnotes.md')),
  nativeTool('tool-grep', 'grep', { pattern: 'report' }, text('report.md:5: report\nnotes.md:2: report')),
  nativeTool('tool-read-image', 'read_image', { path: '/workspace/snapshot.png' }, [{ type: 'image', artifact_id: 'image-native' }]),
  nativeTool('tool-present', 'present', { files: [{ path: '/workspace/report.md' }] }, text('Presented report.md')),
  nativeTool('tool-ask-user', 'ask_user', { questions: [{ question: '选择展示模式', header: '模式', options: ['标准', '紧凑'] }] }, json({ cancelled: false, answers: [{ kind: 'single', question_index: 0, answer: '标准' }] })),
  nativeTool('tool-job_list', 'job_list', {}, json({ jobs: [{ job_id: jobResult.job_id, tool: 'bash', state: 'succeeded' }], returned: 1, matched: 2, truncated: true, limit: 1 })),
  ...['job_status', 'job_wait', 'job_cancel'].map(name => nativeTool(`tool-${name}`, name, { job_id: jobResult.job_id }, json(jobResult))),
  nativeTool('tool-subagent', 'subagent', { agent: 'explore', title: '核对原生工具', task: '检查源代码中的注册表' }, json({ agent_id: 'child-native', state: 'active', agent: 'explore', activation_id: 'activation-first' })),
  nativeTool('tool-list_agents', 'list_agents', {}, json({ returned: 1, matched: 1, truncated: false, agents: [{ agent_id: 'child-native', title: '核对原生工具', state: 'active', agent: 'explore' }] })),
  nativeTool('tool-send_message', 'send_message', { agent_id: 'child-native', message: '请继续检查工具详情' }, json({ agent_id: 'child-native', activation_id: 'activation-first', resumed: false })),
  nativeTool('tool-wait_agent', 'wait_agent', { agent_id: 'child-native' }, json({ agent_id: 'child-native', activation_id: 'activation-first', outcome: 'succeeded' })),
  nativeTool('tool-interrupt_agent', 'interrupt_agent', { agent_id: 'child-native' }, json({ agent_id: 'child-native', activation_id: 'activation-first', outcome: 'cancelled' })),
  ...['create_goal', 'get_goal', 'update_goal'].map(name => nativeTool(`native.${name}`, name, name === 'create_goal' ? { objective: goalResult.objective } : name === 'update_goal' ? { action: 'blocked', expected: goalResult.reference, reason: goalResult.blocked_reason } : {}, json(goalResult))),
  nativeTool('tool-todo', 'todo', { action: 'update', id: 2, status: 'in_progress' }, [
    { type: 'text', text: 'Updated #2: 检查聊天流和轨迹' },
    { type: 'json', value: { next_id: 3, tasks: [{ id: 1, subject: '核对原生工具注册表', status: 'completed' }, { id: 2, subject: '检查聊天流和轨迹', status: 'in_progress', blocked_by: [1], owner: 'explore' }] } },
  ]),
  nativeTool('tool-workflow-review', 'review', { source: '/workspace/report.md' }, json({ report: 'Workflow-defined output', score: 1 })),
];

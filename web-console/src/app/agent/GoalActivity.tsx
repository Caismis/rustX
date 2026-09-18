import type { ForegroundToolExecution } from '../../../../protocol/app-server/v8';
import css from './GoalActivity.module.css';

/** Historical execution evidence only. The Goal dock owns current state. */
export function goalActivityLabel(tool: ForegroundToolExecution): string | undefined {
  const operation = tool.tool_id === 'native.create_goal' ? 'start' : tool.tool_id === 'native.update_goal' ? 'update' : tool.tool_id === 'native.get_goal' ? 'check' : undefined;
  if (!operation) return undefined;
  const success = tool.state.type === 'settled' && tool.state.result.status.type === 'success';
  if (operation === 'start') return success ? 'Goal started' : 'Starting Goal';
  if (operation === 'check') return success ? 'Goal checked' : 'Checking Goal';
  let action: unknown;
  try { action = JSON.parse(tool.state.arguments)?.action; } catch { /* Arguments may still be assembling. */ }
  return success && action === 'complete' ? 'Goal completed' : success && action === 'blocked' ? 'Goal blocked' : 'Updating Goal';
}

export function GoalActivity({ tool, label }: { tool: ForegroundToolExecution; label: string }) {
  const status = tool.state.type === 'settled' ? tool.state.result.status.type : tool.state.type;
  return <div className={css.activity} data-goal-activity data-tool-call-id={tool.call_id}>
    <span aria-hidden="true">◇</span><span>{label}</span>
    <details><summary>Execution details</summary><pre>{JSON.stringify(tool, null, 2)}</pre></details>
    {status !== 'success' && <small aria-label="Goal activity status">{status.replaceAll('_', ' ')}</small>}
  </div>;
}

import type { Translate } from '../../locale/translation';
import { useTranslation } from '../../locale/react';
import type { ForegroundToolExecution } from '../../../../protocol/app-server/v23';
import css from './GoalActivity.module.css';

/** Historical execution evidence only. The Goal dock owns current state. */
export function goalActivityLabel(tx: Translate, tool: ForegroundToolExecution): string | undefined {
  const operation = tool.tool_id === 'native.create_goal' ? 'start' : tool.tool_id === 'native.update_goal' ? 'update' : tool.tool_id === 'native.get_goal' ? 'check' : undefined;
  if (!operation) return undefined;
  const success = tool.state.type === 'settled' && tool.state.result.status.type === 'success';
  if (operation === 'start') return success ? tx('agent:copy.goal-started') : tx('agent:copy.starting-goal');
  if (operation === 'check') return success ? tx('agent:copy.goal-checked') : tx('agent:copy.checking-goal');
  let action: unknown;
  try { action = JSON.parse(tool.state.arguments)?.action; } catch { /* Arguments may still be assembling. */ }
  return success && action === 'complete' ? tx('agent:copy.goal-completed') : success && action === 'blocked' ? tx('agent:copy.goal-blocked') : tx('agent:copy.updating-goal');
}

export function GoalActivity({ tool, label }: { tool: ForegroundToolExecution; label: string }) {
  const tx = useTranslation();
  const status = tool.state.type === 'settled' ? tool.state.result.status.type : tool.state.type;
  return <div className={css.activity} data-goal-activity data-tool-call-id={tool.call_id}>
    <span aria-hidden="true">◇</span><span>{label}</span>
    <details><summary>{tx('agent:goal-activity.execution-details')}</summary><pre>{JSON.stringify(tool, null, 2)}</pre></details>
    {status !== 'success' && <small aria-label={tx('agent:goal-activity.goal-activity-status')}>{tx(`common:state.${status}`)}</small>}
  </div>;
}

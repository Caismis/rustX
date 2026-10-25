import { ToolCard } from '../../presentation/agent/ToolCard';
import { IconGoalOutline16 } from '../../presentation/primitives/icons';
import { toolCard } from '../../bindings/tools';
import { recordedToolDetails } from '../../bindings/recorded-tool-details';
import type { Translate } from '../../locale/translation';
import { useTranslation } from '../../locale/react';
import type { ForegroundToolExecution } from '../../../../protocol/app-server/v57';

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

export function GoalActivity({ tool, label, inspect }: { tool: ForegroundToolExecution; label: string; inspect?: () => Promise<void> }) {
  const tx = useTranslation(), view = toolCard(tool);
  const body = tool.committed_result ?? (tool.state.type === 'settled' ? tool.state.result : undefined);
  const details = view.state === 'success' && body?.status.type === 'success' ? recordedToolDetails(tool.tool_id, body.content, tx) : undefined;
  return <div data-goal-activity><ToolCard inspect={inspect} tool={{ ...view, title: label, icon: <IconGoalOutline16 size={14}/>, details, summary: details?.summary ?? details?.empty ?? view.summary }}/></div>;
}

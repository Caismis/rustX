import type { ForegroundToolExecution } from '../../../../protocol/app-server/v14';
import { toolCard } from '../../bindings/tools';
import { ToolCard } from '../../presentation/agent/ToolCard';
import { ToolArtifacts } from '../components/Artifact';
import { GoalActivity, goalActivityLabel } from './GoalActivity';
export function Tool({ tool }: { tool: ForegroundToolExecution }) {
  const goal = goalActivityLabel(tool);
  if (goal !== undefined) return <GoalActivity tool={tool} label={goal}/>;
  return <ToolCard tool={{ ...toolCard(tool), artifacts: tool.state.type === 'settled' ? <ToolArtifacts result={tool.state.result}/> : undefined }}/>;
}

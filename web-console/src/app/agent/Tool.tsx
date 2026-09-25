import type { ForegroundToolExecution } from '../../../../protocol/app-server/v22';
import { toolCard } from '../../bindings/tools';
import { ToolCard } from '../../presentation/agent/ToolCard';
import { ToolArtifacts } from '../components/Artifact';
import { DomainActivity, domainActivity } from './DomainActivity';
import { GoalActivity, goalActivityLabel } from './GoalActivity';
export function Tool({ tool }: { tool: ForegroundToolExecution }) {
  if (domainActivity(tool)) return <DomainActivity tool={tool}/>;
  const goal = goalActivityLabel(tool);
  if (goal !== undefined) return <GoalActivity tool={tool} label={goal}/>;
  return <ToolCard tool={{ ...toolCard(tool), artifacts: tool.state.type === 'settled' ? <ToolArtifacts result={tool.state.result}/> : undefined }}/>;
}

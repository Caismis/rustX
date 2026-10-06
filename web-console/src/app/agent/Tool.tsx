import { useTranslation } from '../../locale/react';
import type { ForegroundToolExecution } from '../../../../protocol/app-server/v35';
import { toolCard } from '../../bindings/tools';
import { askUserRow } from '../../bindings/ask-user';
import { QuestionRow } from '../../presentation/agent/QuestionRow';
import { ToolCard } from '../../presentation/agent/ToolCard';
import { ToolArtifacts } from '../components/Artifact';
import { DomainActivity, domainActivity } from './DomainActivity';
import { GoalActivity, goalActivityLabel } from './GoalActivity';
export function Tool({ tool }: { tool: ForegroundToolExecution }) {
  const tx = useTranslation();
  if (domainActivity(tool)) return <DomainActivity tool={tool}/>;
  const question = tool.tool_id === 'tool-ask-user' ? askUserRow(tool) : undefined;
  if (question) return <QuestionRow row={question}/>;
  const goal = goalActivityLabel(tx, tool);
  if (goal !== undefined) return <GoalActivity tool={tool} label={goal}/>;
  return <ToolCard tool={{ ...toolCard(tool), artifacts: tool.state.type === 'settled' ? <ToolArtifacts result={tool.state.result}/> : undefined }}/>;
}

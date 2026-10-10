import { useToolInspection } from './tool-inspection';
import { useTranslation } from '../../locale/react';
import type { ForegroundToolExecution } from '../../../../protocol/app-server/v44';
import { toolCard } from '../../bindings/tools';
import { askUserRow } from '../../bindings/ask-user';
import { QuestionRow } from '../../presentation/agent/QuestionRow';
import { PresentRow } from '../../presentation/agent/PresentRow';
import { presentRow } from '../../bindings/present';
import { ToolCard } from '../../presentation/agent/ToolCard';
import { ToolArtifacts } from '../components/Artifact';
import { DomainActivity, domainActivity } from './DomainActivity';
import { GoalActivity, goalActivityLabel } from './GoalActivity';
export function Tool({ tool }: { tool: ForegroundToolExecution }) {
  const tx = useTranslation(), inspect = useToolInspection(tool);
  if (domainActivity(tool)) return <DomainActivity tool={tool}/>;
  const question = tool.tool_id === 'tool-ask-user' ? askUserRow(tool) : undefined;
  if (question) return <QuestionRow inspect={inspect} row={question}/>;
  // Lifecycle only: delivery cards render from the committed Tool message.
  if (tool.tool_id === 'tool-present') return <PresentRow inspect={inspect} row={presentRow(tool)}/>;
  const goal = goalActivityLabel(tx, tool);
  if (goal !== undefined) return <GoalActivity inspect={inspect} tool={tool} label={goal}/>;
  return <ToolCard inspect={inspect} tool={{ ...toolCard(tool), artifacts: tool.state.type === 'settled' ? <ToolArtifacts result={tool.state.result}/> : undefined }}/>;
}

import { useToolInspection } from './tool-inspection';
import { useTranslation } from '../../locale/react';
import type { ForegroundToolExecution } from '../../../../protocol/app-server/v57';
import { useToolInput } from './tool-source';
import { toolCard } from '../../bindings/tools';
import { askUserRow } from '../../bindings/ask-user';
import { QuestionRow } from '../../presentation/agent/QuestionRow';
import { PresentRow } from '../../presentation/agent/PresentRow';
import { presentRow } from '../../bindings/present';
import { recordedToolDetails } from '../../bindings/recorded-tool-details';
import { IconChecklistOutline14 } from '../../presentation/primitives/icons';
import { ToolCard } from '../../presentation/agent/ToolCard';
import { ToolArtifacts } from '../components/Artifact';
import { DomainActivity, domainActivity } from './DomainActivity';
import { GoalActivity, goalActivityLabel } from './GoalActivity';
export function Tool({ tool: original }: { tool: ForegroundToolExecution }) {
  const input = useToolInput(original), tool = input.tool;
  const tx = useTranslation(), inspect = useToolInspection(tool);
  if (domainActivity(tool)) return <DomainActivity tool={tool}/>;
  const question = tool.tool_id === 'tool-ask-user' ? askUserRow(tool) : undefined;
  if (question) return <QuestionRow inspect={inspect} row={question}/>;
  // Lifecycle only: delivery cards render from the committed Tool message.
  if (tool.tool_id === 'tool-present') return <PresentRow inspect={inspect} row={presentRow(tool)}/>;
  const goal = goalActivityLabel(tx, tool);
  if (goal !== undefined) return <GoalActivity inspect={inspect} tool={tool} label={goal}/>;
  const body = tool.committed_result ?? (tool.state.type === 'settled' ? tool.state.result : undefined);
  const view = toolCard(tool);
  const details = view.state === 'success' && body?.status.type === 'success' ? recordedToolDetails(tool.tool_id, body.content, tx) : undefined;
  return <ToolCard onExpand={input.expand} body={input.pending ? <p role="status">{tx('common:activity.loading')}</p> : input.error ? <p role="alert">{input.error} <button type="button" onClick={input.retry}>{tx('agent:message.retry-tool')}</button></p> : undefined} inspect={inspect} tool={{ ...view, ...(tool.tool_id === 'tool-todo' ? { title: tx('tools:todo.title'), icon: <IconChecklistOutline14/> } : {}), details, summary: details?.summary ?? details?.empty ?? view.summary, artifacts: body ? <ToolArtifacts result={body}/> : undefined }}/>;
}

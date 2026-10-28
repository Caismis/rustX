import { useToolInspection } from './tool-inspection';
import { useTranslation } from '../../locale/react';
import type { ForegroundToolExecution } from '../../../../protocol/app-server/v62';
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
import { createContext, useContext } from 'react';
import { canonicalLocatorKey } from '../../../../protocol/app-server/content-identity';
/** One conversation presentation owns disclosure across process-group reparenting. */
export const ToolDisclosureContext = createContext<{ scope: string; open: (key: string) => boolean; toggle: (key: string) => void } | undefined>(undefined);
export function Tool({ tool: original, messageId }: { tool: ForegroundToolExecution; messageId?: string }) {
  const disclosure = useContext(ToolDisclosureContext), locator = original.occurrence.locator;
  const message = messageId ?? original.message_id ?? (locator.type === 'by_message_id' ? locator.message_id : undefined);
  const key = JSON.stringify([disclosure?.scope, message === undefined ? ['locator', canonicalLocatorKey(locator)] : ['message', message], original.block_index]);
  const open = disclosure?.open(key) ?? false;
  const input = useToolInput(original, open), tool = input.tool;
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
  return <ToolCard disclosure={disclosure ? { open, toggle: () => { if (!open) input.expand(); disclosure.toggle(key); } } : undefined} onExpand={input.expand} body={input.pending ? <p role="status">{tx('common:activity.loading')}</p> : input.error ? <p role="alert">{input.error} <button type="button" onClick={input.retry}>{tx('agent:message.retry-tool')}</button></p> : undefined} inspect={inspect} tool={{ ...view, ...(tool.tool_id === 'tool-todo' ? { title: tx('tools:todo.title'), icon: <IconChecklistOutline14/> } : {}), details, summary: details?.summary ?? details?.empty ?? view.summary, artifacts: body ? <ToolArtifacts result={body}/> : undefined }}/>;
}

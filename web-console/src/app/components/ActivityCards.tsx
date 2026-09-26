import { useTranslation } from '../../locale/react';
import type { RuntimeClientSubagent, WorkflowRunView, WorkflowRunId } from '../../../../protocol/app-server/v23';

import { Badge, SettingsCard } from '../../presentation/settings/SettingsContent';



const detail = (value: string) => value.length > 1024 ? `${value.slice(0, 1024)}…` : value;
export const workflowKey = (id: WorkflowRunId) => JSON.stringify([id.conversation_id, id.attempt_id, id.invocation]);

/** Current native facts only. These cards do not assign historical placement. */
export function SubagentCard({ child }: { child: RuntimeClientSubagent }) {
  const tx = useTranslation();
  const childState: Record<RuntimeClientSubagent['state'], string> = {
  running: tx('common:activity-cards.working'), cancelling: tx('common:session-product.stopping'), publishing_terminal: tx('common:copy.finishing'),
  succeeded: tx('common:copy.completed'), failed: tx('common:copy.failed'), cancelled: tx('common:copy.stopped'), interrupted: tx('common:copy.interrupted'),
};
  const activity = child.observation.activity;
  return <section data-subagent-id={child.subagent_id} aria-label={tx('common:copy.subagent-value', { p0: child.agent })}><SettingsCard title={tx('common:copy.subagent-value', { p0: child.agent })} meta={<Badge>{childState[child.state]}</Badge>}>
    <p>{activity.type === 'waiting' ? tx('common:activity-cards.waiting-for-value', { p0: tx(`common:state.${activity.on.type}`) }) : tx(`common:state.${activity.type}`)}
      {(activity.type === 'model' || activity.type === 'retrying_model') && activity.retry > 0 && <> {tx('common:activity-cards.retry')} {activity.retry}</>}
    </p>
    {child.detail && <p>{detail(child.detail)}</p>}
  </SettingsCard></section>;
}

export function WorkflowCard({ run }: { run: WorkflowRunView }) {
  const tx = useTranslation();
  return <section data-workflow-run-id={workflowKey(run.id)} aria-label={tx('common:activity-cards.workflow-value', { p0: run.workflow_id })}><SettingsCard title={tx('common:activity-cards.workflow-value-2', { p0: run.workflow_id })} meta={<Badge>{run.state.type === 'settled' ? tx(`common:state.${run.state.outcome}`) : tx(`common:state.${run.state.type}`)}</Badge>}>
    <p>{tx('common:activity-cards.steps')} {run.steps_consumed} / {run.steps_max} {tx('common:activity-cards.agents')} {run.agents_consumed}</p>
    {run.state.type === 'waiting' && <p>{tx('common:activity-cards.waiting-for')} {tx(`common:state.${run.state.reason}`)}</p>}
  </SettingsCard></section>;
}

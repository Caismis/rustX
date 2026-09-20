import type { RuntimeClientSubagent, WorkflowRunView, WorkflowRunId } from '../../../../protocol/app-server/v15';

import { Badge, SettingsCard } from '../../presentation/settings/SettingsContent';

const label = (value: string) => value.replaceAll('_', ' ');
const childState: Record<RuntimeClientSubagent['state'], string> = {
  running: 'Working', cancelling: 'Stopping…', publishing_terminal: 'Finishing…',
  succeeded: 'Completed', failed: 'Failed', cancelled: 'Stopped', interrupted: 'Interrupted',
};
const detail = (value: string) => value.length > 1024 ? `${value.slice(0, 1024)}…` : value;
export const workflowKey = (id: WorkflowRunId) => JSON.stringify([id.conversation_id, id.attempt_id, id.invocation]);

/** Current native facts only. These cards do not assign historical placement. */
export function SubagentCard({ child }: { child: RuntimeClientSubagent }) {
  const activity = child.observation.activity;
  return <section data-subagent-id={child.subagent_id} aria-label={`Subagent ${child.agent}`}><SettingsCard title={`Subagent · ${child.agent}`} meta={<Badge>{childState[child.state]}</Badge>}>
    <p>{activity.type === 'waiting' ? `Waiting for ${label(activity.on.type)}` : label(activity.type)}
      {(activity.type === 'model' || activity.type === 'retrying_model') && activity.retry > 0 && <> · Retry {activity.retry}</>}
    </p>
    {child.detail && <p>{detail(child.detail)}</p>}
  </SettingsCard></section>;
}

export function WorkflowCard({ run }: { run: WorkflowRunView }) {
  return <section data-workflow-run-id={workflowKey(run.id)} aria-label={`Workflow ${run.workflow_id}`}><SettingsCard title={`Workflow · ${run.workflow_id}`} meta={<Badge>{run.state.type === 'settled' ? label(run.state.outcome) : label(run.state.type)}</Badge>}>
    <p>Steps {run.steps_consumed} / {run.steps_max} · Agents {run.agents_consumed}</p>
    {run.state.type === 'waiting' && <p>Waiting for {label(run.state.reason)}</p>}
  </SettingsCard></section>;
}

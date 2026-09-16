import type { RuntimeClientSubagent, WorkflowRunView, WorkflowRunId } from '../../../../protocol/app-server/v5';

const label = (value: string) => value.replaceAll('_', ' ');
const detail = (value: string) => value.length > 1024 ? `${value.slice(0, 1024)}…` : value;
export const workflowKey = (id: WorkflowRunId) => JSON.stringify([id.conversation_id, id.attempt_id, id.invocation]);

/** Current native facts only. These cards do not assign historical placement. */
export function SubagentCard({ child }: { child: RuntimeClientSubagent }) {
  const activity = child.observation.activity;
  return <article className="activity-card" data-subagent-id={child.subagent_id} aria-label={`Subagent ${child.subagent_id}`}>
    <header><strong>Subagent · {child.agent}</strong><span>{label(child.state)}</span></header>
    <small>{child.subagent_id}</small>
    <p>{activity.type === 'waiting' ? `Waiting for ${label(activity.on.type)}` : label(activity.type)}
      {activity.type === 'tool' && <> · {activity.tool_id}</>}
      {(activity.type === 'model' || activity.type === 'retrying_model') && activity.retry > 0 && <> · Retry {activity.retry}</>}
    </p>
    {child.detail && <p>{detail(child.detail)}</p>}
  </article>;
}

export function WorkflowCard({ run }: { run: WorkflowRunView }) {
  return <article className="activity-card" data-workflow-run-id={workflowKey(run.id)} aria-label={`Workflow ${workflowKey(run.id)}`}>
    <header><strong>Workflow · {run.workflow_id}</strong><span>{label(run.state.type)}</span></header>
    <small>{run.id.attempt_id} · Run {run.id.invocation}</small>
    <p>Steps {run.steps_consumed} / {run.steps_max} · Agents {run.agents_consumed}</p>
    {run.state.type === 'waiting' && <p>Waiting for {label(run.state.reason)}</p>}
    {run.state.type === 'settled' && <p>Outcome: {label(run.state.outcome)}</p>}
  </article>;
}

import type { WorkflowInstanceView, WorkflowRunView } from '../../../protocol/app-server/v60';

/** Navigation references, not durable Agent owners or locally opened views. */
export interface WorkflowChild {
  agentId: string;
  activationId: string;
  title: string;
  workflowId: string;
  runKey: string;
  instance: WorkflowInstanceView;
}

/** Count actual admitted children once, in native run/node order. */
export function workflowChildren(runs: readonly WorkflowRunView[] | undefined, conversationId: string | undefined): WorkflowChild[] {
  const children: WorkflowChild[] = [], seen = new Set<string>();
  for (const run of runs ?? []) {
    if (run.id.conversation_id !== conversationId) continue;
    for (const instance of run.instances) {
      if (instance.kind !== 'agent' || !instance.node || !instance.child || !instance.child_agent_id || instance.state.type === 'pending') continue;
      const key = JSON.stringify([instance.child_agent_id, instance.child]);
      if (seen.has(key)) continue;
      seen.add(key);
      children.push({ agentId: instance.child_agent_id, activationId: instance.child, title: instance.node,
        workflowId: run.workflow_id, runKey: JSON.stringify([run.id.conversation_id, run.id.attempt_id, run.id.invocation]), instance });
    }
  }
  return children;
}

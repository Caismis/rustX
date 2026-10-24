import type { ReviewSpecification, WorkflowRunView } from '../../protocol/app-server/v57';
export function workflowRun(): WorkflowRunView {
  const id = { conversation_id: 'conversation-A', origin: {type:'tool' as const,attempt_id:'attempt-1'}, invocation: '1' };
  const block = { run: id, definition: { workflow_id: 'file_checkup', blocks: [] }, invocations: [] };
  return { id, workflow_id: 'file_checkup', program_digest: 'native-program', resource_revision: '1', tool_call_id: 'workflow-call',
    state: { type: 'waiting', reason: 'review' }, steps_consumed: 9, steps_max: 10, agents_consumed: 2, model_calls_consumed: 0, omitted_instances: 0,
    instances: [
      { block, kind: 'block', state: { type: 'waiting', reason: 'review' } },
      ...(['read', 'triage', 'analyze', 'gate'] as const).map((node, index) => ({ block, node, visit: 1,
        kind: (['tool', 'branch', 'parallel', 'review'] as const)[index],
        state: index < 3 ? { type: 'settled' as const, outcome: 'completed' as const } : { type: 'waiting' as const, reason: 'review' as const },
      })),
    ],
  };
}
export function workflowReview(): ReviewSpecification {
  const run = workflowRun();
  return { instance: { block: run.instances[0].block, node: 'gate', visit: 1 }, subject: { type: 'plan', content: { overview: '## Review report\n\nThe workflow completed its analysis.', issues: ['Check the release version before publishing.'], candidate: null } }, context: [] };
}

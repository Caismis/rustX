import { useTranslation } from '../../locale/react';
import type { RuntimeClientSnapshot } from '../../../../protocol/app-server/v50';
import { WorkflowCard, workflowKey } from '../components/ActivityCards';

/** Rows follow native identities. An activation change never remounts its Agent. */
export function RuntimeFacts({ snapshot }: { snapshot: Pick<RuntimeClientSnapshot, 'workflows'> }) {
  const tx = useTranslation();
  const workflows = snapshot.workflows.runs;
  if (!workflows.length) return null;
  return <section className="runtime-facts" aria-label={tx('agent:activity.current-activity')}>
    <small>{tx('agent:activity.current-activity')}</small>
    {workflows.map(workflow => <WorkflowCard key={workflowKey(workflow.id)} run={workflow} />)}
  </section>;
}

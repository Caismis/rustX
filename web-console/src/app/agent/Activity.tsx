import { useTranslation } from '../../locale/react';
import type { RuntimeClientSnapshot } from '../../../../protocol/app-server/v40';
import type { AppServerClient } from '../../client/app-server';
import { JobCard, WorkflowCard, workflowKey } from '../components/ActivityCards';

/** Rows follow native identities. An activation change never remounts its Agent. */
export function RuntimeFacts({ snapshot, client, sessionId }: { snapshot: Pick<RuntimeClientSnapshot, 'jobs' | 'agents' | 'workflows'>; client?: AppServerClient; sessionId?: string }) {
  const tx = useTranslation();
  const jobs = snapshot.jobs ?? [];
  const workflows = snapshot.workflows.runs;
  if (!workflows.length && !jobs.length) return null;
  return <section className="runtime-facts" aria-label={tx('agent:activity.current-activity')}>
    <small>{tx('agent:activity.current-activity')}</small>
    {jobs.map(job => <JobCard key={job.job_id} job={job} client={client} sessionId={sessionId}/>)}
    {workflows.map(workflow => <WorkflowCard key={workflowKey(workflow.id)} run={workflow} />)}
  </section>;
}

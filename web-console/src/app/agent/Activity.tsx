import type { RuntimeClientSnapshot } from '../../../../protocol/app-server/v22';
import type { AppServerClient } from '../../client/app-server';
import { AgentCard, JobCard, WorkflowCard, workflowKey } from '../components/ActivityCards';

/** Rows follow native identities. An activation change never remounts its Agent. */
export function RuntimeFacts({ snapshot, client, sessionId }: { snapshot: RuntimeClientSnapshot; client?: AppServerClient; sessionId?: string }) {
  const agents = snapshot.agents ?? [];
  const jobs = snapshot.jobs ?? [];
  const workflows = snapshot.workflows.runs;
  if (!agents.length && !workflows.length && !jobs.length) return null;
  return <section className="runtime-facts" aria-label="Current activity">
    <small>Current activity</small>
    {jobs.map(job => <JobCard key={job.job_id} job={job} client={client} sessionId={sessionId}/>)}
    {agents.map(agent => <AgentCard key={agent.agent_id} agent={agent} client={client} sessionId={sessionId}/>)}
    {workflows.map(workflow => <WorkflowCard key={workflowKey(workflow.id)} run={workflow} />)}
  </section>;
}

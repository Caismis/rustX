import type { RuntimeClientSnapshot } from '../../../../protocol/app-server/v15';
import { json } from '../../bindings/projection';
import { SubagentCard, WorkflowCard, workflowKey } from '../components/ActivityCards';
import { ToolCard } from '../../presentation/agent/ToolCard';
import { ToolArtifacts } from '../components/Artifact';
export function RuntimeFacts({ snapshot }: { snapshot: RuntimeClientSnapshot }) {
  const children = snapshot.subagents ?? [];
  const background = snapshot.background ?? [];
  const workflows = snapshot.workflows.runs;
  if (!children.length && !workflows.length && !background.length) return null;
  return <section className="runtime-facts" aria-label="Current activity">
    <small>Current activity</small>
    {background.map(tool => <ToolCard key={tool.execution_id} tool={{ id: tool.execution_id, identity: 'execution', title: tool.tool_name, variant: 'generic', summary: 'Background task',
      state: tool.state === 'succeeded' ? 'success' : tool.state === 'failed' || tool.state === 'denied' || tool.state === 'timed_out' ? 'failure' : tool.state === 'outcome_unknown' ? 'uncertain' : tool.state,
      output: tool.result ? json(tool.result) : tool.progress ? json(tool.progress) : undefined, artifacts: tool.result ? <ToolArtifacts result={tool.result}/> : undefined }}/>) }
    {children.map(child => <SubagentCard key={child.subagent_id} child={child} />)}
    {workflows.map(workflow => <WorkflowCard key={workflowKey(workflow.id)} run={workflow} />)}
  </section>;
}

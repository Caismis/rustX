import type { ForegroundToolExecution } from '../../../../protocol/app-server/v24';
import { ToolCard } from '../../presentation/agent/ToolCard';
import { toolCard } from '../../bindings/tools';

const names: Record<string, { domain: 'job' | 'agent'; title: string }> = {
  'tool-job_list': { domain: 'job', title: 'Jobs' },
  'tool-job_status': { domain: 'job', title: 'Job status' },
  'tool-job_wait': { domain: 'job', title: 'Wait for Job' },
  'tool-job_cancel': { domain: 'job', title: 'Cancel Job' },
  'tool-subagent': { domain: 'agent', title: 'Create Agent' },
  'tool-list_agents': { domain: 'agent', title: 'Agents' },
  'tool-send_message': { domain: 'agent', title: 'Message Agent' },
  'tool-wait_agent': { domain: 'agent', title: 'Wait for activation' },
  'tool-interrupt_agent': { domain: 'agent', title: 'Interrupt activation' },
};

/** Historical Tool results are evidence, never the current Job/Agent roster. */
export function domainActivity(tool: ForegroundToolExecution) {
  return names[tool.tool_id];
}
export function DomainActivity({ tool }: { tool: ForegroundToolExecution }) {
  const domain = domainActivity(tool)!;
  const view = toolCard(tool);
  let target: string | undefined;
  try {
    const input = JSON.parse(tool.state.arguments);
    target = domain.domain === 'job' ? input.job_id : input.agent_id;
    if (typeof target !== 'string') target = undefined;
  } catch { /* Streaming arguments remain in the disclosure. */ }
  return <div data-activity-domain={domain.domain}><ToolCard tool={{ ...view, title: domain.title,
    summary: target ?? (domain.domain === 'job' ? 'Finite background Tool' : 'Durable child conversation') }}/></div>;
}

import { useTranslation } from '../../locale/react';
import type { TranslationKey } from '../../locale/translation';
import type { ForegroundToolExecution } from '../../../../protocol/app-server/v36';
import { ToolCard } from '../../presentation/agent/ToolCard';
import { toolCard } from '../../bindings/tools';

const names: Record<string, { domain: 'job' | 'agent'; title: TranslationKey }> = {
  'tool-job_list': { domain: 'job', title: 'common:activity.jobs' },
  'tool-job_status': { domain: 'job', title: 'common:activity.job-status' },
  'tool-job_wait': { domain: 'job', title: 'common:activity.wait-job' },
  'tool-job_cancel': { domain: 'job', title: 'common:activity.cancel-job' },
  'tool-subagent': { domain: 'agent', title: 'common:activity.create-agent' },
  'tool-list_agents': { domain: 'agent', title: 'common:activity.agents' },
  'tool-send_message': { domain: 'agent', title: 'common:activity.message-agent' },
  'tool-wait_agent': { domain: 'agent', title: 'common:activity.wait-activation' },
  'tool-interrupt_agent': { domain: 'agent', title: 'common:activity.interrupt-activation' },
};

/** Historical Tool results are evidence, never the current Job/Agent roster. */
export function domainActivity(tool: ForegroundToolExecution) {
  return names[tool.tool_id];
}
export function DomainActivity({ tool }: { tool: ForegroundToolExecution }) {
  const tx = useTranslation();
  const domain = domainActivity(tool)!;
  const view = toolCard(tool);
  let target: string | undefined;
  try {
    const input = JSON.parse(tool.state.arguments);
    target = domain.domain === 'job' ? input.job_id : input.agent_id;
    if (typeof target !== 'string') target = undefined;
  } catch { /* Streaming arguments remain in the disclosure. */ }
  return <div data-activity-domain={domain.domain}><ToolCard tool={{ ...view, title: tx(domain.title),
    summary: target ?? (domain.domain === 'job' ? tx('common:activity.finite-tool') : tx('common:activity.durable-child')) }}/></div>;
}

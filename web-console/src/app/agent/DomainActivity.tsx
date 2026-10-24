import { useToolInspection } from './tool-inspection';
import { IconAgentPresetOutline16, IconChecklistOutline14 } from '../../presentation/primitives/icons';
import { subagentToolDetails } from '../../bindings/subagent-tool-details';
import { useSubagents } from './subagent-context';
import { useTranslation } from '../../locale/react';
import type { TranslationKey } from '../../locale/translation';
import type { ForegroundToolExecution } from '../../../../protocol/app-server/v57';
import { ToolCard } from '../../presentation/agent/ToolCard';
import { toolCard } from '../../bindings/tools';
import { recordedToolDetails } from '../../bindings/recorded-tool-details';

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
  return tool.tool_id==null?undefined:names[tool.tool_id];
}
export function DomainActivity({ tool }: { tool: ForegroundToolExecution }) {
  const inspect = useToolInspection(tool), tx = useTranslation(), scope = useSubagents();
  const domain = domainActivity(tool)!, view = toolCard(tool);
  const body = tool.committed_result ?? (tool.state.type === 'settled' ? tool.state.result : undefined);
  let details = view.state === 'success' && body?.status.type === 'success' ? recordedToolDetails(tool.tool_id, body.content, tx) : undefined;
  const result = subagentToolDetails(tool);
  let target: string | undefined;
  try { const args = JSON.parse(tool.state.arguments); target = typeof args?.job_id === 'string' ? args.job_id : undefined; } catch { /* Streaming arguments. */ }
  if (domain.domain === 'agent' && view.state === 'success' && body?.status.type === 'success') {
    // Human titles and outcomes come only from this receipt. The current roster
    // supplies navigation authority, never replacement historical state/text.
    if (result.items.length || result.count === 0) details = {
      items: result.items.map(item => ({ title: item.name ?? item.agentId ?? item.text ?? '',
        agentId: item.agentId && scope?.agents.some(agent => agent.agent_id === item.agentId) ? item.agentId : undefined,
        subtitle: item.name ? item.agentId : undefined, description: item.text,
        badge: item.status ? { label: tx(item.status), tone: item.tone === 'done' ? 'success' : item.tone === 'error' ? 'error' : item.tone === 'warning' ? 'warning' : item.tone === 'ongoing' ? 'info' : 'neutral' } : undefined,
      })),
      summary: result.count !== undefined ? tx('common:subagents.count', { count: result.count }) : result.task ?? result.target,
      empty: tx('common:subagents.empty'), caption: result.truncated ? tx('common:subagents.truncated', result.truncated) : undefined,
    };
  }
  return <div data-activity-domain={domain.domain}><ToolCard inspect={inspect} onOpenAgent={id => scope?.open(id)} tool={{ ...view,
    icon: domain.domain === 'job' ? <IconChecklistOutline14/> : <IconAgentPresetOutline16 size={14}/>,
    title: tx(domain.title), details, summary: details?.summary ?? details?.empty ?? (view.state === 'failure' || view.state === 'uncertain' || view.state === 'cancelled' ? view.summary : result.task ?? result.target ?? target ?? tx('common:activity.finite-tool')),
  }}/></div>;
}

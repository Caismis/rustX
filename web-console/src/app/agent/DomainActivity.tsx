import { useToolInspection } from './tool-inspection';
import { useState } from 'react';
import { DisclosureRow } from '../../presentation/primitives/DisclosureRow';
import { IconAgentPresetOutline16 } from '../../presentation/primitives/icons';
import { subagentToolDetails } from './subagent-tool-details';
import detailsCss from './AgentToolDetails.module.css';
import { useSubagents } from './subagent-context';
import css from '../../presentation/agent/Tool.module.css';
import { useTranslation } from '../../locale/react';
import type { TranslationKey } from '../../locale/translation';
import type { ForegroundToolExecution } from '../../../../protocol/app-server/v44';
import { ToolCard, ToolInspectionButton } from '../../presentation/agent/ToolCard';
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
  const inspect = useToolInspection(tool);
  const tx = useTranslation(), [expanded, setExpanded] = useState(false), scope = useSubagents();
  const domain = domainActivity(tool)!;
  const view = toolCard(tool);
  if (domain.domain === 'agent') {
    const details = subagentToolDetails(tool);
    const target = details.target, task = details.task;
    const agent = scope?.agents.find(agent => agent.agent_id === target);
    return <div className={css.root} data-activity-domain="agent" data-tool-call-id={tool.call_id}>
      <DisclosureRow icon={<IconAgentPresetOutline16 size={14}/>} title={tx(domain.title)} open={expanded} expandable expandOnRowClick keepContentWhenOpen onToggle={() => setExpanded(value => !value)}
        collapsedContent={<><span className={css.sep}/><span className={css.summary}>{details.count !== undefined ? tx('common:subagents.count', { count: details.count }) : task ?? agent?.agent ?? target ?? tx('common:subagents.list')}</span></>}>
        <div className={detailsCss.root}>
          <div className={detailsCss.caption}><span>{tx('common:subagents.result')}</span>{inspect && <ToolInspectionButton inspect={inspect}/>}</div>
          <ul className={detailsCss.list}>
            {details.items.map((item, index) => {
              const child = scope?.agents.find(agent => agent.agent_id === item.agentId);
              const name = item.name ?? child?.agent ?? item.agentId;
              return <li className={detailsCss.item} key={item.agentId ?? index}>
                <div className={detailsCss.heading}><div className={detailsCss.text}>{item.text ?? (child ? <button className={detailsCss.link} type="button" onClick={() => scope?.open(child.agent_id)}>{name}</button> : name)}</div>
                  {item.status && <span className={detailsCss.status} data-tone={item.tone}>{tx(item.status)}</span>}
                </div>
                {item.text && child && <button className={`${detailsCss.link} ${detailsCss.subtitle}`} type="button" onClick={() => scope?.open(child.agent_id)}>{name}</button>}
                {tool.tool_id === 'tool-list_agents' && <div className={detailsCss.subtitle}>{item.agentId}</div>}
              </li>;
            })}
            {details.count === 0 && <li className={detailsCss.item}>{tx('common:subagents.empty')}</li>}
            {!details.items.length && details.count === undefined && <li className={detailsCss.item}><div className={detailsCss.text}>{task ?? (view.state === 'success' ? tx('common:state.success') : view.output || view.input)}</div></li>}
            {details.truncated && <li className={detailsCss.item}>{tx('common:subagents.truncated', details.truncated)}</li>}
          </ul>
        </div>
      </DisclosureRow>
    </div>;
  }
  let target: string | undefined;
  try { const input = JSON.parse(tool.state.arguments); if (typeof input?.job_id === 'string') target = input.job_id; }
  catch { /* Partial streaming arguments. */ }
  return <div data-activity-domain={domain.domain}><ToolCard inspect={inspect} tool={{ ...view, title: tx(domain.title),
    summary: target ?? tx('common:activity.finite-tool') }}/></div>;
}

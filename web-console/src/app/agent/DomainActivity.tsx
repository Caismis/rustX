import { useState } from 'react';
import { DisclosureRow } from '../../presentation/primitives/DisclosureRow';
import { IconAgentPresetOutline16 } from '../../presentation/primitives/icons';
import { Button } from '../../presentation/primitives/Button';
import { useSubagents } from './subagent-context';
import css from '../../presentation/agent/Tool.module.css';
import own from './InboundMessage.module.css';
import { useTranslation } from '../../locale/react';
import type { TranslationKey } from '../../locale/translation';
import type { ForegroundToolExecution } from '../../../../protocol/app-server/v37';
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
  const tx = useTranslation(), [expanded, setExpanded] = useState(false), scope = useSubagents();
  const domain = domainActivity(tool)!;
  const view = toolCard(tool);
  let target: string | undefined;
  let task: string | undefined;
  try {
    const input = JSON.parse(tool.state.arguments);
    target = domain.domain === 'job' ? input.job_id : input.agent_id;
    task = [input.description, input.prompt, input.task, input.message].find(value => typeof value === 'string');
    if (typeof target !== 'string') target = undefined;
  } catch { /* Streaming arguments remain in the disclosure. */ }
  if (domain.domain === 'agent') {
    if (!target && tool.tool_id === 'tool-subagent' && tool.state.type === 'settled' && tool.state.result.status.type === 'success') {
      for (const block of tool.state.result.content ?? []) {
        if (block.type === 'json' && block.value && typeof block.value === 'object' && !Array.isArray(block.value) && typeof block.value.agent_id === 'string') { target = block.value.agent_id; break; }
      }
    }
    const agent = scope?.agents.find(agent => agent.agent_id === target);
    return <div data-activity-domain="agent" data-tool-call-id={tool.call_id}>
      <DisclosureRow icon={<IconAgentPresetOutline16 size={14}/>} title={tx(domain.title)} open={expanded} expandable expandOnRowClick keepContentWhenOpen onToggle={() => setExpanded(value => !value)}
        collapsedContent={<><span className={css.sep}/><span className={css.summary}>{task ?? agent?.agent ?? target ?? tx('common:subagents.list')}</span></>}>
        <div className={own.body}>
          {agent && <Button size="sm" onClick={() => scope?.open(agent.agent_id)}>{tx('common:subagents.view')}</Button>}
          <small>{tx('common:subagents.result')} · {tx(`common:state.${view.state}`)}</small>
          {task && <p className={css.ioText}>{task}</p>}
          {view.output && <pre className={css.ioText}>{view.output}</pre>}
          {!task && !view.output && <pre className={css.ioText}>{view.input}</pre>}
        </div>
      </DisclosureRow>
    </div>;
  }
  return <div data-activity-domain={domain.domain}><ToolCard tool={{ ...view, title: tx(domain.title),
    summary: target ?? (domain.domain === 'job' ? tx('common:activity.finite-tool') : tx('common:activity.durable-child')) }}/></div>;
}

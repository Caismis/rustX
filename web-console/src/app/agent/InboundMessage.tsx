import { useState } from 'react';
import type { MessageBlock } from '../../../../protocol/app-server/v39';
import { useTranslation } from '../../locale/react';
import { DisclosureRow } from '../../presentation/primitives/DisclosureRow';
import { IconAgentPresetOutline16 } from '../../presentation/primitives/icons';
import { Button } from '../../presentation/primitives/Button';
import { Content } from './Message';
import { useSubagents } from './subagent-context';
import css from '../../presentation/agent/Tool.module.css';
import own from './InboundMessage.module.css';

export const isHumanMessage = (message: MessageBlock): message is Extract<MessageBlock, { role: 'user' }> => message.role === 'user' && message.source === 'human' && (!message.kind || message.kind === 'message');
/** Provider role describes model input; canonical source describes who spoke. */
export function InboundMessage({ message }: { message: Extract<MessageBlock, { role: 'user' }> }) {
  const tx = useTranslation(), [expanded, setExpanded] = useState(false), scope = useSubagents();
  const agentId = typeof message.source === 'object' && 'agent' in message.source ? message.source.agent.agent_id : undefined;
  const agent = scope?.agents.find(agent => agent.agent_id === agentId);
  const title = agentId ? tx('common:subagents.message', { name: agent?.agent ?? tx('common:subagents.list') }) : tx('agent:message.context');
  const text = message.content.flatMap(block => block.type === 'text' ? [block.text] : []).join('\n');
  return <div className={own.root} data-inbound-source={agentId ? 'agent' : 'context'}>
    <DisclosureRow icon={<IconAgentPresetOutline16 size={14}/>} title={title} open={expanded} expandable expandOnRowClick keepContentWhenOpen onToggle={() => setExpanded(value => !value)} collapsedContent={<><span className={css.sep}/><span className={css.summary}>{text}</span></>}>
      <div className={own.body}>{agent && <Button size="sm" onClick={() => scope?.open(agent.agent_id)}>{tx('common:subagents.view')}</Button>}<Content blocks={message.content} markdown/></div>
    </DisclosureRow>
  </div>;
}

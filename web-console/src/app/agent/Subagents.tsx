/* Copyright (c) 2026 DeepSeek. MIT. Header layout adapted from ui-subagent/SubagentHeaderLineage; see PROVENANCE.md. */
import { useState, type ReactNode } from 'react';
import type { RuntimeClientAgent } from '../../../../protocol/app-server/v37';
import type { AppServerClient } from '../../client/app-server';
import { useClientSelector } from '../../client/selectors';
import { useTranslation } from '../../locale/react';
import { Menu } from '../../presentation/primitives/Menu';
import { StateDot, type StateDotState } from '../../presentation/primitives/StateDot';
import { IconChevronDownOutline14, IconChevronRightOutline14 } from '../../presentation/primitives/icons';
import { AgentCard } from '../components/ActivityCards';
import css from './Subagents.module.css';
import conversationCss from '../../presentation/agent/Conversation.module.css';

import { SubagentContext as Context, useSubagents } from './subagent-context';
export function SubagentScope({ client, sessionId, children }: { client: AppServerClient; sessionId?: string; children: ReactNode }) {
  const agents = useClientSelector(client, state => sessionId ? state.views[sessionId]?.snapshot?.agents : undefined) ?? [];
  const owner = `${client.getSnapshot().generation}:${sessionId}`;
  const [selection, setSelection] = useState<{ owner: string; id?: string }>();
  const id = selection?.owner === owner ? selection.id : undefined;
  const open = (id?: string) => setSelection({ owner, id });
  return <Context value={{ client, sessionId, agents, selected: agents.find(agent => agent.agent_id === id), open }}>{children}</Context>;
}
export function agentDot(agent: RuntimeClientAgent): StateDotState {
  if (agent.state === 'unavailable') return 'error';
  if (agent.state === 'active' || agent.state === 'admitting' || agent.state === 'stopping') return agent.observation.activity.type === 'waiting' ? 'warning' : 'ongoing';
  return agent.activation_state === 'failed' ? 'error' : agent.activation_state === 'succeeded' ? 'done' : agent.activation_state === 'cancelled' ? 'warning' : 'idle';
}
export function SubagentHeader({ title }: { title: string }) {
  const tx = useTranslation(), scope = useSubagents(), [expanded, setExpanded] = useState(false);
  if (!scope) return <span id="session-title">{title}</span>;
  const { selected, agents, open } = scope;
  const stateLabel = (agent: RuntimeClientAgent) => agent.state === 'inactive' ? tx(`common:state.${agent.activation_state}`) : tx(`common:activity.${agent.state === 'active' ? 'working' : agent.state === 'admitting' ? 'admitting' : agent.state === 'stopping' ? 'stopping' : 'unavailable'}`);
  return <div className={css.lineage}>
    {selected ? <><button className={css.parent} onClick={() => open()}>{title}</button><span className={css.separator}>/</span></> : <span id="session-title" aria-label={tx(scope.sessionId ? 'agent:conversation-header.session-title' : 'agent:conversation-header.product-title')} className={css.title}>{title}</span>}
    {!!agents.length && <Menu open={expanded} onClose={() => setExpanded(false)} className={css.menu} selection="fill" selectedId={selected?.agent_id} autoFocus
      anchor={<button id={selected ? 'session-title' : undefined} className={css.trigger} aria-label={tx('common:subagents.list')} aria-haspopup="menu" aria-expanded={expanded} onClick={() => setExpanded(value => !value)}>
        {(selected ? selected.state === 'active' || selected.state === 'admitting' : agents.some(agent => agent.state === 'active' || agent.state === 'admitting')) && <StateDot state="ongoing"/>}
        <span>{selected?.agent ?? tx('common:subagents.count', { count: agents.length })}</span><IconChevronDownOutline14/>
      </button>}
      items={agents.map(agent => ({ id: agent.agent_id, label: <span className={css.row} data-agent-id={agent.agent_id} data-agent-state={agent.state}>
        <StateDot state={agentDot(agent)} size={7}/><span className={css.content}><span className={css.name}>{agent.agent}</span><span className={css.secondary}>{stateLabel(agent)}{agent.observation.activity.type === 'tool' ? ` · ${agent.observation.activity.tool_id.replace(/^tool-/, '')}` : ''}</span></span><IconChevronRightOutline14/>
      </span> }))} onSelect={id => { open(id); setExpanded(false); }}/>} 
  </div>;
}
/** Keep the parent mounted so returning restores its reading position and draft. */
export function SubagentSurface({ children }: { children: ReactNode }) {
  const scope = useSubagents();
  return <><div className={css.parentSurface} hidden={!!scope?.selected}>{children}</div>
    {scope?.selected && <div className={conversationCss.scrollBody} data-conversation-scroll><AgentCard key={`${scope.sessionId}:${scope.selected.agent_id}`} agent={scope.selected} client={scope.client} sessionId={scope.sessionId}/></div>}</>;
}

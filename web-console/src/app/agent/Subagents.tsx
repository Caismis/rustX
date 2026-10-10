/* Copyright (c) 2026 DeepSeek. MIT. Header layout adapted from ui-subagent/SubagentHeaderLineage; see PROVENANCE.md. */
import { useEffect, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore, type ReactNode } from 'react';
import type { RuntimeClientAgent, AgentStatistics } from '../../../../protocol/app-server/v40';
import type { AppServerClient } from '../../client/app-server';
import { useClientSelector } from '../../client/selectors';
import { useTranslation } from '../../locale/react';
import { Menu } from '../../presentation/primitives/Menu';
import { StateDot } from '../../presentation/primitives/StateDot';
import { IconChevronDownOutline14, IconChevronRightOutline14 } from '../../presentation/primitives/icons';
import { AgentCard } from '../components/ActivityCards';
import css from './Subagents.module.css';
import conversationCss from '../../presentation/agent/Conversation.module.css';

import { formatDuration, formatTokens } from './token-format';
import { SubagentContext as Context, useSubagents } from './subagent-context';
import { agentDot, agentRunning, agentStatus } from './subagent-state';
import { meterDemand, type MeterScope } from '../../client/agent-meters';

export function agentDuration(metrics: AgentStatistics, now: number) {
  const { settled_ms, active } = metrics.duration;
  return Number(settled_ms) + (active ? Math.max(0, (active.running ? now : Date.parse(active.observed_at)) - Date.parse(active.started_at)) : 0);
}
const NO_AGENTS: RuntimeClientAgent[] = [];
export function SubagentScope({ client, sessionId, children }: { client: AppServerClient; sessionId?: string; children: ReactNode }) {
  const tx = useTranslation();
  const agents = useClientSelector(client, state => sessionId ? state.views[sessionId]?.snapshot?.agents : undefined) ?? NO_AGENTS;
  const generation = useClientSelector(client, state => state.generation);
  const target = useClientSelector(client, state => sessionId ? state.views[sessionId]?.target : undefined);
  const intent = useClientSelector(client, state => sessionId ? state.views[sessionId]?.attachmentIntent : undefined);
  const admission = useClientSelector(client, state => sessionId ? state.views[sessionId]?.attachmentObservation : undefined);
  const owner = JSON.stringify([generation, sessionId, target, admission]);
  const scope = useMemo<MeterScope>(() => ({
    target,
    current: () => !!sessionId && client.isAttachmentObservationCurrent(sessionId, admission),
    inventory: () => (sessionId ? client.getSnapshot().views[sessionId]?.snapshot?.agents : undefined) ?? [],
  }), [client, owner, admission]);
  const [selection, setSelection] = useState<{ owner: string; id?: string; opened: string[] }>();
  const id = selection?.owner === owner ? selection.id : undefined;
  const open = (id?: string) => setSelection(previous => ({ owner, id, opened: [...new Set([...(previous?.owner === owner ? previous.opened : []), ...(id ? [id] : [])])] }));
  const attached = useClientSelector(client, state => sessionId ? state.views[sessionId]?.attachment : undefined);
  const deleting = useClientSelector(client, state => sessionId ? state.views[sessionId]?.deleting : undefined);
  useLayoutEffect(() => () => client.agentMeters.retire(scope), [client, scope]);
  useLayoutEffect(() => { client.agentMeters.update(scope, id); }, [client, scope, agents, attached, intent, deleting, id]);
  const readings = useSyncExternalStore(client.agentMeters.subscribe, client.agentMeters.getSnapshot);
  const metrics: Record<string, AgentStatistics> = {}, metricErrors: Record<string, string> = {};
  if (readings.scope === scope && scope.current()) for (const agent of agents) {
    const reading = readings.readings.get(agent.agent_id);
    if (reading?.demand === meterDemand(agent)) {
      if (reading.metrics) metrics[agent.agent_id] = reading.metrics;
      if (reading.error) metricErrors[agent.agent_id] = reading.error;
    } else if (readings.blocked) metricErrors[agent.agent_id] = tx('agent:usage.reads-unresolved');
  }
  return <Context value={{ client, sessionId, agents, metrics, metricErrors, selected: agents.find(agent => agent.agent_id === id), opened: agents.filter(agent => selection?.owner === owner && selection.opened.includes(agent.agent_id)), open }}>{children}</Context>;
}
/** DSH count menu and sibling switcher, using the shared floating-menu owner. */
function SubagentSwitcherIcon() {
  return <svg width="16" height="16" viewBox="0 0 20 20" fill="none" aria-hidden="true">
    <path d="M5.99951 12.7L8.95546 14.9478C9.40011 15.2859 9.62244 15.455 9.87526 15.488C9.95774 15.4988 10.0413 15.4988 10.1238 15.488C10.3766 15.455 10.5989 15.2859 11.0436 14.9478L13.9995 12.7" stroke="currentColor" strokeWidth="1.5"/>
    <path d="M13.9995 7.7417L11.0436 5.49387C10.5989 5.15574 10.3766 4.98668 10.1238 4.95362C10.0413 4.94283 9.95775 4.94283 9.87527 4.95362C9.62245 4.98668 9.40012 5.15574 8.95547 5.49387L5.99952 7.7417" stroke="currentColor" strokeWidth="1.5"/>
  </svg>;
}
function SubagentMenu({ agents, selected, label, openChild }: { agents: RuntimeClientAgent[]; selected?: string; label: string; openChild: (id: string) => void }) {
  const tx = useTranslation(), scope = useSubagents()!, [expanded, setExpanded] = useState(false), [pinned, setPinned] = useState(false);
  const hover = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const [now, setNow] = useState(Date.now);
  const cancelHover = () => { clearTimeout(hover.current); hover.current = undefined; };
  useEffect(() => cancelHover, []);
  useEffect(() => {
    if (!expanded || !agents.some(agentRunning)) return;
    setNow(Date.now()); const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [expanded, agents]);
  const close = () => { cancelHover(); setExpanded(false); setPinned(false); };
  return <Menu open={expanded} onClose={close} className={css.menu} closeOnPointerLeave={!pinned} dense selection="fill" selectedId={selected} autoFocus={pinned}
    anchor={<button id={selected ? 'session-title' : undefined} className={`${css.trigger} ${selected ? css.switcher : ''}`} aria-label={tx('common:subagents.list')} aria-haspopup="menu" aria-expanded={expanded}
      onMouseEnter={() => { cancelHover(); if (!expanded) hover.current = setTimeout(() => setExpanded(true), 150); }} onMouseLeave={cancelHover}
      onClick={() => { cancelHover(); setPinned(true); setExpanded(true); }}>
      {!selected && agents.some(agentRunning) && <span className={css.activity}><StateDot state="ongoing"/></span>}
      <span>{label}</span>{selected ? <SubagentSwitcherIcon/> : <IconChevronDownOutline14 className={expanded ? css.expanded : undefined}/>}
    </button>}
    items={agents.map(agent => ({ id: agent.agent_id, label: <span className={css.row} data-agent-id={agent.agent_id} data-agent-state={agent.state}>
      <span className={css.rowActivity}><StateDot state={agentDot(agent)} size={7}/></span>
      <span className={css.content}><span className={`${css.name} ${selected === agent.agent_id ? css.current : ''}`}>{agent.agent}</span><span className={css.secondary}>{agentStatus(agent, tx)}{agent.observation.activity.type === 'tool' ? ` · ${agent.observation.activity.tool_id.replace(/^tool-/, '')}` : ''}</span></span>
      <span className={css.metrics}>{scope.metrics[agent.agent_id]?.statistics.reported_usage && <span>{tx('agent:usage.count', { count: formatTokens(scope.metrics[agent.agent_id].statistics.reported_usage!.total_tokens, tx) })}</span>}{scope.metrics[agent.agent_id] && <span>{formatDuration(agentDuration(scope.metrics[agent.agent_id], now), tx)}</span>}</span><IconChevronRightOutline14/>
    </span> }))} onSelect={id => { openChild(id); close(); }}/>
}
export function SubagentHeader({ title }: { title: string }) {
  const tx = useTranslation(), scope = useSubagents();
  if (!scope) return <span id="session-title">{title}</span>;
  const { selected, agents, open } = scope;
  const ancestors: RuntimeClientAgent[] = [];
  let parent = selected && agents.find(agent => agent.agent_id === selected.parent_agent_id);
  while (parent && !ancestors.includes(parent)) { ancestors.unshift(parent); parent = agents.find(agent => agent.agent_id === parent!.parent_agent_id); }
  const siblings = selected ? agents.filter(agent => agent.parent_agent_id === selected.parent_agent_id) : agents.filter(agent => !agents.some(parent => parent.agent_id === agent.parent_agent_id));
  const children = selected ? agents.filter(agent => agent.parent_agent_id === selected.agent_id) : [];
  return <div className={css.lineage}>
    {selected ? <><button className={css.parent} onClick={() => open()}>{title}</button><span className={css.separator}>/</span>
      {ancestors.map(agent => <span key={agent.agent_id} className={css.ancestor}><button className={css.parent} onClick={() => open(agent.agent_id)}>{agent.agent}</button><span className={css.separator}>/</span></span>)}
      <SubagentMenu agents={siblings} selected={selected.agent_id} label={selected.agent} openChild={open}/>
      {!!children.length && <SubagentMenu agents={children} label={tx('common:subagents.count', { count: children.length })} openChild={open}/>}</>
      : <><span id="session-title" aria-label={tx(scope.sessionId ? 'agent:conversation-header.session-title' : 'agent:conversation-header.product-title')} className={css.title}>{title}</span>
        {!!siblings.length && <SubagentMenu agents={siblings} label={tx('common:subagents.count', { count: siblings.length })} openChild={open}/>}</>}
  </div>;
}
/** Retain each visited child's draft and reading position, like a resident DSH conversation. */
export function SubagentSurface({ children, mode = 'chat' }: { children: ReactNode; mode?: 'chat' | 'trajectory' }) {
  const scope = useSubagents();
  return <><div className={css.parentSurface} hidden={!!scope?.selected}>{children}</div>
    {scope?.opened.map(agent => <div key={`${scope.sessionId}:${agent.agent_id}`} hidden={scope.selected?.agent_id !== agent.agent_id} className={`${conversationCss.scrollBody} ${css.childSurface}`} data-conversation-scroll>
      <AgentCard mode={mode} visible={scope.selected?.agent_id === agent.agent_id} agent={agent} metrics={scope.metrics[agent.agent_id]} metricsError={scope.metricErrors[agent.agent_id]} client={scope.client} sessionId={scope.sessionId}/>
    </div>)}</>;
}

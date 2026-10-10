/* Copyright (c) 2026 DeepSeek. MIT. Header layout adapted from ui-subagent/SubagentHeaderLineage; see PROVENANCE.md. */
import { useLayoutEffect, useMemo, useState, useSyncExternalStore, type ReactNode } from 'react';
import type { RuntimeClientAgent, AgentStatistics } from '../../../../protocol/app-server/v43';
import type { AppServerClient } from '../../client/app-server';
import { useClientSelector } from '../../client/selectors';
import { useTranslation } from '../../locale/react';
import { SubagentMenu } from './SubagentCatalog';
import { AgentCard } from '../components/ActivityCards';
import css from './Subagents.module.css';
import conversationCss from '../../presentation/agent/Conversation.module.css';

import { SubagentContext as Context, useSubagents } from './subagent-context';
import { meterDemand, type MeterScope } from '../../client/agent-meters';

const NO_AGENTS: RuntimeClientAgent[] = [];
export function SubagentScope({ client, sessionId, children, openAside }: { client: AppServerClient; sessionId?: string; children: ReactNode; openAside?: (agent: RuntimeClientAgent) => void }) {
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
  return <Context value={{ client, sessionId, agents, metrics, metricErrors, selected: agents.find(agent => agent.agent_id === id), openAside: openAside ? (id: string) => { const agent = agents.find(agent => agent.agent_id === id); if (agent) openAside(agent); } : undefined, opened: agents.filter(agent => selection?.owner === owner && selection.opened.includes(agent.agent_id)), open }}>{children}</Context>;
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
      {ancestors.map(agent => <span key={agent.agent_id} className={css.ancestor}><SubagentMenu agents={agents.filter(sibling => sibling.parent_agent_id === agent.parent_agent_id)} selected={agent.agent_id} label={agent.title} openTitle={() => open(agent.agent_id)} openChild={open}/><span className={css.separator}>/</span></span>)}
      <SubagentMenu agents={siblings} selected={selected.agent_id} label={selected.title} openChild={open}/>
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

/** Sidebar chat uses the same native Agent conversation and resident composer. */
export function SubagentAside({ id, visible }: { id: string; visible: boolean }) {
  const scope = useSubagents(), tx = useTranslation();
  const agent = scope?.agents.find(agent => agent.agent_id === id);
  if (!scope || !agent) return <p role="status">{tx('common:activity.unavailable')}</p>;
  return <section className={`conversation-panel ${conversationCss.body} ${conversationCss.embeddedBody}`} data-content-phase="active">
    <div className={`${conversationCss.scrollBody} ${css.aside}`} data-conversation-scroll><AgentCard embedded visible={visible} agent={agent} metrics={scope.metrics[id]} metricsError={scope.metricErrors[id]} client={scope.client} sessionId={scope.sessionId}/></div>
  </section>;
}

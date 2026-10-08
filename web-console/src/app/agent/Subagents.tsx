/* Copyright (c) 2026 DeepSeek. MIT. Header layout adapted from ui-subagent/SubagentHeaderLineage; see PROVENANCE.md. */
import { useCallback, useEffect, useMemo, useState, type ReactNode } from 'react';
import type { RuntimeClientAgent, AgentStatistics } from '../../../../protocol/app-server/v38';
import { sameTarget, type AppServerClient } from '../../client/app-server';
import { useClientSelector } from '../../client/selectors';
import { useTranslation } from '../../locale/react';
import { Menu } from '../../presentation/primitives/Menu';
import { StateDot, type StateDotState } from '../../presentation/primitives/StateDot';
import { IconChevronDownOutline14, IconChevronRightOutline14 } from '../../presentation/primitives/icons';
import { AgentCard } from '../components/ActivityCards';
import css from './Subagents.module.css';
import conversationCss from '../../presentation/agent/Conversation.module.css';

import { formatDuration, formatTokens } from './token-format';
import { SubagentContext as Context, useSubagents } from './subagent-context';
// Keep background meter reads below the transport lane budget, even with a
// large child list. Superseded queued work checks its lease before dispatch.
function meterQueue() {
  let active = 0;
  const queued: (() => Promise<void>)[] = [];
  const drain = () => {
    while (active < 2 && queued.length) {
      const task = queued.shift()!;
      active++;
      void task().finally(() => { active--; drain(); });
    }
  };
  return (task: () => Promise<void>) => { queued.push(task); drain(); };
}

/** Fetch independent native meters without delaying parent attach or child history. */
function AgentMeter({ client, sessionId, agent, selected, schedule, receive }: { selected: boolean; schedule: ReturnType<typeof meterQueue>; client: AppServerClient; sessionId: string; agent: RuntimeClientAgent; receive: (id: string, metrics?: AgentStatistics, error?: string) => void }) {
  const target = useClientSelector(client, state => state.views[sessionId]?.target);
  const attached = useClientSelector(client, state => state.views[sessionId]?.attachment === 'attached');
  useEffect(() => {
    if (!attached || !target) return;
    let current = true;
    const generation = client.getSnapshot().generation;
    const valid = () => current && client.getSnapshot().generation === generation && sameTarget(client.getSnapshot().views[sessionId]?.target, target);
    schedule(async () => {
      if (!valid()) return;
      try {
        const result = await client.request({ method: 'agent/statistics', params: { target, agent_id: agent.agent_id } }, 'agent_statistics');
        if (valid()) receive(agent.agent_id, result.metrics);
      } catch (error) { if (valid()) receive(agent.agent_id, undefined, error instanceof Error ? error.message : String(error)); }
    });
    return () => { current = false; };
  }, [client, sessionId, target, attached, agent.agent_id, agent.activation_id, agent.state, agent.observation.revision, selected, schedule, receive]);
  return null;
}

export function agentDuration(metrics: AgentStatistics, now: number) {
  const { settled_ms, active } = metrics.duration;
  return Number(settled_ms) + (active ? Math.max(0, (active.running ? now : Date.parse(active.observed_at)) - Date.parse(active.started_at)) : 0);
}
export function SubagentScope({ client, sessionId, children }: { client: AppServerClient; sessionId?: string; children: ReactNode }) {
  const agents = useClientSelector(client, state => sessionId ? state.views[sessionId]?.snapshot?.agents : undefined) ?? [];
  const owner = `${client.getSnapshot().generation}:${sessionId}`;
  const schedule = useMemo(meterQueue, [owner]);
  const [readings, setReadings] = useState<{ owner: string; metrics: Record<string, AgentStatistics>; errors: Record<string, string> }>({ owner, metrics: {}, errors: {} });
  const receive = useCallback((id: string, metrics?: AgentStatistics, error?: string) => setReadings(previous => {
    const next = previous.owner === owner ? previous : { owner, metrics: {}, errors: {} };
    const errors = { ...next.errors };
    if (error) errors[id] = error; else delete errors[id];
    return { owner, metrics: metrics ? { ...next.metrics, [id]: metrics } : next.metrics, errors };
  }), [owner]);
  const metrics = readings.owner === owner ? readings.metrics : {};
  const metricErrors = readings.owner === owner ? readings.errors : {};
  const [selection, setSelection] = useState<{ owner: string; id?: string }>();
  const id = selection?.owner === owner ? selection.id : undefined;
  const open = (id?: string) => setSelection({ owner, id });
  return <Context value={{ client, sessionId, agents, metrics, metricErrors, selected: agents.find(agent => agent.agent_id === id), open }}>{sessionId && agents.map(agent => <AgentMeter key={`${owner}:${agent.agent_id}`} client={client} sessionId={sessionId} agent={agent} selected={id === agent.agent_id} schedule={schedule} receive={receive}/>)}{children}</Context>;
}
export function agentDot(agent: RuntimeClientAgent): StateDotState {
  if (agent.state === 'unavailable') return 'error';
  if (agent.state === 'active' || agent.state === 'admitting' || agent.state === 'stopping') return agent.observation.activity.type === 'waiting' ? 'warning' : 'ongoing';
  return agent.activation_state === 'failed' ? 'error' : agent.activation_state === 'succeeded' ? 'done' : agent.activation_state === 'cancelled' ? 'warning' : 'idle';
}
export function SubagentHeader({ title }: { title: string }) {
  const tx = useTranslation(), scope = useSubagents(), [expanded, setExpanded] = useState(false);
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    if (!expanded) return;
    setNow(Date.now());
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [expanded]);
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
        <StateDot state={agentDot(agent)} size={7}/><span className={css.content}><span className={css.name}>{agent.agent}</span><span className={css.secondary}>{stateLabel(agent)}{agent.observation.activity.type === 'tool' ? ` · ${agent.observation.activity.tool_id.replace(/^tool-/, '')}` : ''}</span></span><span className={css.metrics}>{scope.metrics[agent.agent_id]?.statistics.reported_usage && <span>{tx('agent:usage.count', { count: formatTokens(scope.metrics[agent.agent_id].statistics.reported_usage!.total_tokens, tx) })}</span>}{scope.metrics[agent.agent_id] && <span>{formatDuration(agentDuration(scope.metrics[agent.agent_id], now), tx)}</span>}</span><IconChevronRightOutline14/>
      </span> }))} onSelect={id => { open(id); setExpanded(false); }}/>} 
  </div>;
}
/** Keep the parent mounted so returning restores its reading position and draft. */
export function SubagentSurface({ children }: { children: ReactNode }) {
  const scope = useSubagents();
  return <><div className={css.parentSurface} hidden={!!scope?.selected}>{children}</div>
    {scope?.selected && <div className={conversationCss.scrollBody} data-conversation-scroll><AgentCard key={`${scope.sessionId}:${scope.selected.agent_id}`} agent={scope.selected} metrics={scope.metrics[scope.selected.agent_id]} metricsError={scope.metricErrors[scope.selected.agent_id]} client={scope.client} sessionId={scope.sessionId}/></div>}</>;
}

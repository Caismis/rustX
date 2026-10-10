import type { RuntimeClientAgent, AgentStatistics } from '../../../../protocol/app-server/v42';
import type { useTranslation } from '../../locale/react';
import type { StateDotState } from '../../presentation/primitives/StateDot';

export const agentRunning = (agent: RuntimeClientAgent) => agent.state === 'active' || agent.state === 'admitting' || agent.state === 'stopping';
/** Native owner state takes precedence over the last activation outcome. */
export function agentDot(agent: RuntimeClientAgent): StateDotState {
  if (agent.state === 'unavailable') return 'error';
  if (agentRunning(agent)) return agent.observation.activity.type === 'waiting' ? 'warning' : 'ongoing';
  return agent.activation_state === 'failed' ? 'error' : agent.activation_state === 'succeeded' ? 'done' : agent.activation_state === 'cancelled' ? 'warning' : 'idle';
}
export function agentStatus(agent: RuntimeClientAgent, tx: ReturnType<typeof useTranslation>) {
  if (agent.state === 'inactive') return agent.activation_state === 'succeeded' ? tx('common:subagents.completed') : tx(`common:state.${agent.activation_state}`);
  if (agent.state === 'active' && agent.observation.activity.type === 'waiting') return tx('common:activity.waiting-for', { state: tx(`common:state.${agent.observation.activity.on.type}`) });
  return tx(`common:activity.${agent.state === 'active' ? 'working' : agent.state === 'admitting' ? 'admitting' : agent.state === 'stopping' ? 'stopping' : 'unavailable'}`);
}

export function agentDuration(metrics: AgentStatistics, now: number) {
  const { settled_ms, active } = metrics.duration;
  return Number(settled_ms) + (active ? Math.max(0, (active.running ? now : Date.parse(active.observed_at)) - Date.parse(active.started_at)) : 0);
}

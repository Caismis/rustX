import { useEffect, useState, useSyncExternalStore } from 'react';
import type { RuntimeClientAgent } from '../../../../protocol/app-server/v43';
import type { AppServerClient } from '../../client/app-server';
import type { Observation } from '../../client/session-lifecycle/port';
import { AgentTraceReader } from '../../client/agent-trace';
import { Trajectory } from '../trajectory/Trajectory';
import { useTranslation } from '../../locale/react';
import { Button } from '../../presentation/primitives/Button';
const noSubscription = () => () => {};
export function SubagentTrajectory({ agent, client, sessionId, visible }: { agent: RuntimeClientAgent; client: AppServerClient; sessionId: string; visible: boolean }) {
  const tx = useTranslation();
  const proof = useSyncExternalStore(client.subscribe, () => client.getSnapshot().views[sessionId]?.attachmentObservation);
  const [domain, setDomain] = useState<{ proof: Observation; reader: AgentTraceReader }>();
  const reader = domain?.proof === proof ? domain?.reader : undefined;
  useEffect(() => {
    if (!proof) return;
    const reader = new AgentTraceReader(client, sessionId, proof, agent.agent_id);
    setDomain({ proof, reader });
    return () => reader.retire();
  }, [client, sessionId, proof, agent.agent_id]);
  const cache = useSyncExternalStore(reader?.subscribe ?? noSubscription, () => reader?.snapshot());

  useEffect(() => { if (visible) void reader?.refresh(); }, [reader, visible, agent.activation_id, agent.state, agent.observation.revision]);
  if (!reader || !cache) return null;
  return <>{cache.loading && cache.page.records.length === 0 && <p role="status">{tx('common:activity.reading')}</p>}{cache.error && <Button size="sm" onClick={() => void reader.refresh()}>{tx('trajectory:toolbar.refresh')}</Button>}
    <Trajectory cache={cache} onSelect={reader.select} loadEarlier={() => void reader.earlier()} onLoadDetail={id => void reader.detail(id)}/></>;
}

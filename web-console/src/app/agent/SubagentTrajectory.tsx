import { useEffect, useState, useImperativeHandle, type Ref, useSyncExternalStore } from 'react';
import type { RuntimeClientAgent, TraceToolLocator } from '../../../../protocol/app-server/v44';
import type { AppServerClient } from '../../client/app-server';
import type { Observation } from '../../client/session-lifecycle/port';
import { AgentTraceReader } from '../../client/agent-trace';
import { Trajectory } from '../trajectory/Trajectory';
import { useTranslation } from '../../locale/react';
import { Button } from '../../presentation/primitives/Button';
const noSubscription = () => () => {};
export interface SubagentTraceHandle { locate: (locator: TraceToolLocator) => Promise<boolean> }
export function SubagentTrajectory({ agent, client, sessionId, visible, ref }: { agent: RuntimeClientAgent; client: AppServerClient; sessionId: string; visible: boolean; ref?: Ref<SubagentTraceHandle> }) {
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
  useImperativeHandle(ref, () => ({ locate: locator => reader?.locate(locator) ?? Promise.resolve(false) }), [reader]);
  const cache = useSyncExternalStore(reader?.subscribe ?? noSubscription, () => reader?.snapshot());

  useEffect(() => { if (visible) void reader?.refresh(); }, [reader, visible, agent.activation_id, agent.state, agent.observation.revision]);
  if (!reader || !cache) return null;
  return <>{cache.loading && cache.page.records.length === 0 && <p role="status">{tx('common:activity.reading')}</p>}{cache.error && <Button size="sm" onClick={() => void reader.refresh()}>{tx('trajectory:toolbar.refresh')}</Button>}
    <Trajectory cache={cache} onLatest={() => void reader.latest()} onSelect={reader.select} loadEarlier={() => void reader.earlier()} onLoadDetail={id => void reader.detail(id)}/></>;
}

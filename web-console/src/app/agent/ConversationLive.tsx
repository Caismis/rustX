import type { AppServerClient } from '../../client/app-server';
import { useClientSelector } from '../../client/selectors';
import { AgentTranscript } from './AgentTranscript';
import { RuntimeFacts } from './Activity';
import { ConversationStats } from './ConversationStats';
import { TodoDock } from '../composer/TodoDock';
import { GoalDock } from '../composer/GoalDock';
import { QueueDock } from '../composer/QueueDock';
import { activeAttempt, lineageSwitchSafe } from '../../bindings/projection';
import { todoDock, goalDock, queueRows } from '../../bindings/composer-context';
import { ChatViewport } from '../../presentation/layout/ChatViewport';
import { Trajectory } from '../trajectory/Trajectory';
import type { HistoryAction } from '../commands/native';
import type { CompletedResponseView } from '../../../../protocol/app-server/v22';

export function ConversationLive({ client, sessionId, mode, disabled, onHistorical, run }: {
  client: AppServerClient; sessionId?: string; mode: 'chat' | 'trajectory'; disabled: boolean;
  onHistorical: (id: HistoryAction, response: CompletedResponseView) => void;
  run: (action: () => Promise<unknown>) => void;
}) {
  const view = useClientSelector(client, state => sessionId ? state.views[sessionId] : undefined);
  if (!view) return null;
  return mode === 'trajectory' && view.trace
    ? <Trajectory key={`${view.id}:${view.target?.attachment_id}`} cache={view.trace} onSelect={id => client.selectTrace(view.id, id)} onLoadDetail={id => { void client.loadTraceDetail(view.id, id); }} loadEarlier={() => run(() => client.loadEarlierTrace(view.id))} latest={() => client.latestTrace(view.id)}/>
    : <ChatViewport key={`${view.id}:${view.target?.attachment_id}`}>
      {view.snapshot && <><AgentTranscript snapshot={view.snapshot} history={view.history} loadEarlier={() => run(() => client.loadEarlier(view.id))} latest={() => client.latestTranscript(view.id)} lineageSwitchSafe={lineageSwitchSafe(view)} historicalDisabled={disabled} onHistorical={onHistorical}/><RuntimeFacts snapshot={view.snapshot}/></>}
    </ChatViewport>;
}

export function ConversationDocks({ client, sessionId, disabled }: { client: AppServerClient; sessionId: string; disabled: boolean }) {
  const view = useClientSelector(client, state => state.views[sessionId]);
  if (!view) return null;
  return <>
    <TodoDock state={todoDock(view.snapshot)}/>
    <GoalDock state={goalDock(view.snapshot)} observation={view.snapshot} disabled={disabled} mutate={(expected, mutation) => client.controlGoal(view.id, expected, mutation)}/>
    <QueueDock disabled={disabled} observation={view.snapshot} edit={(expected, text) => client.editInbound(view.id, expected, text)} remove={expected => client.removeInbound(view.id, expected)} rows={queueRows(view.snapshot)} submissions={view.submissions ?? []} running={activeAttempt(view.snapshot)}/>
  </>;
}

export function ConversationTotals({ client, sessionId }: { client: AppServerClient; sessionId: string }) {
  const snapshot = useClientSelector(client, state => state.views[sessionId]?.snapshot);
  return <ConversationStats snapshot={snapshot}/>;
}

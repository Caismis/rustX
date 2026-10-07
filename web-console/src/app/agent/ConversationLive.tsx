import type { AppServerClient } from '../../client/app-server';
import { shallowEqual, useClientSelector } from '../../client/selectors';
import { ModelRetries } from './ModelRetry';
import { AgentTranscript } from './AgentTranscript';
import { RuntimeFacts } from './Activity';
import { ConversationStats } from './UsageStats';
import { TodoDock } from '../composer/TodoDock';
import { GoalDock } from '../composer/GoalDock';
import { QueueDock } from '../composer/QueueDock';
import { activeAttempt, lineageSwitchSafe } from '../../bindings/projection';
import { todoDock, goalDock, queueRows } from '../../bindings/composer-context';
import { ChatViewport } from '../../presentation/layout/ChatViewport';
import { Trajectory } from '../trajectory/Trajectory';
import type { ResponseAction } from '../commands/native';
import type { AttachmentTarget, CompletedResponseView } from '../../../../protocol/app-server/v36';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from '../../locale/react';
import { TurnNavigator } from './TurnNavigator';
import { turnAnchor } from '../../client/transcript';
import { type ForkOrigin, useForkPoint } from './ForkPoint';

export function ConversationLive({ client, sessionId, mode, disabled, onHistorical, onOpenSource, sourceLocation }: {
  client: AppServerClient; sessionId?: string; mode: 'chat' | 'trajectory'; disabled: boolean;
  onHistorical: (id: ResponseAction, response: CompletedResponseView) => void;
  onOpenSource?: (origin: ForkOrigin) => void;
  sourceLocation?: { target: AttachmentTarget; messageId: string };
}) {
  const tx=useTranslation(), viewport=useRef<ChatViewport>(null), [active,setActive]=useState<string | null>();
  const view = useClientSelector(client, state => {
    const view = sessionId ? state.views[sessionId] : undefined;
    if (!view?.snapshot) return undefined;
    const snapshot = view.snapshot;
    return { id: view.id, target: view.target, messages: snapshot.messages, attempt: snapshot.attempt,
      transcript: snapshot.transcript, statuses: snapshot.statuses, conversation_id: snapshot.conversation_id,
      history: view.history, trace: mode === 'trajectory' ? view.trace : undefined,
      safe: lineageSwitchSafe(view), disabled: disabled || view.attachment !== 'attached' || view.attachmentIntent !== 'wanted'
        || !!view.modelMutation || !!snapshot.shutting_down || !!snapshot.durability_failure };
  }, shallowEqual);
  const fork = useForkPoint(client, mode === 'chat' ? view?.target : undefined, view?.history?.page.inherited_through);
  const located = useRef<object | undefined>(undefined);
  useEffect(() => {
    if (!view?.target || mode !== 'chat' || !sourceLocation || sourceLocation.target !== view.target || located.current === sourceLocation) return;
    const intent = viewport.current?.beginNavigation();
    if (!intent) return;
    located.current = sourceLocation;
    const target = view.target;
    void client.navigateMessage(view.id, sourceLocation.messageId, intent.current).then(landed => {
      if (landed) intent.commit(`message:${sourceLocation.messageId}`, () => client.getSnapshot().views[view.id]?.target === target);
    });
  }, [client, view?.target, mode, sourceLocation]);
  if (!view) return null;
  return mode === 'trajectory' && view.trace
    ? <Trajectory key={`${view.id}:${view.target?.attachment_id}`} cache={view.trace} onSelect={id => client.selectTrace(view.id, id)} onLoadDetail={id => { void client.loadTraceDetail(view.id, id); }} loadEarlier={() => void client.loadEarlierTrace(view.id).catch(() => {})}/>
    : <ChatViewport ref={viewport} key={`${view.id}:${view.target?.attachment_id}`} latestLabel={tx('agent:agent-transcript.return-to-latest')}
      overlay={<TurnNavigator key={`rail:${view.id}:${view.target?.attachment_id}`} client={client} sessionId={view.id} active={active} onNavigate={turn=>{
        const intent=viewport.current?.beginNavigation();
        if(intent){
          const work=client.navigateTurn(view.id,turn,intent.current);
          const clientIntent=client.getSnapshot().views[view.id]?.turnNavigation?.intent;
          void work.then(committed=>{if(committed)intent.commit(turnAnchor(turn.id),()=>{
            const current=client.getSnapshot().views[view.id];
            return current?.target===view.target && current.attachment==='attached' && current.turnNavigation?.intent===clientIntent;
          });});
        }
      }}/>}
      latestTurn={view.attempt && view.attempt.phase.type!=='settled' ? turnAnchor({conversation_id:view.conversation_id,attempt_id:view.attempt.attempt_id}) : undefined}
      onActiveTurn={setActive}>
      {fork.error && <div role="alert"><p>{fork.error}</p><button type="button" onClick={fork.retry}>{tx('agent:fork-point.reload')}</button></div>}
      <AgentTranscript requestFeedback={attemptId => <ModelRetries client={client} sessionId={view.id} attemptId={attemptId}/>} snapshot={view} history={view.history} loadEarlier={() => void client.loadEarlier(view.id).catch(() => {})} lineageSwitchSafe={view.safe} historicalDisabled={view.disabled} onHistorical={onHistorical} forkPoint={fork.point} onOpenSource={onOpenSource} sourceDisabled={fork.point?.origin.source_session === view.id && !view.safe}/>
      <ConversationActivity client={client} sessionId={view.id}/>
    </ChatViewport>;
}
function ConversationActivity({ client, sessionId }: { client: AppServerClient; sessionId: string }) {
  const snapshot = useClientSelector(client, state => {
    const value = state.views[sessionId]?.snapshot;
    return value ? { agents: value.agents, jobs: value.jobs, workflows: value.workflows } : undefined;
  }, shallowEqual);
  return snapshot ? <RuntimeFacts snapshot={snapshot} client={client} sessionId={sessionId}/> : null;
}

export function ConversationDocks({ client, sessionId, disabled }: { client: AppServerClient; sessionId: string; disabled: boolean }) {
  const view = useClientSelector(client, state => state.views[sessionId], (a, b) => a === b || !!a && !!b &&
    a.target === b.target && a.submissions === b.submissions && a.snapshot?.goal === b.snapshot?.goal && a.snapshot?.todos === b.snapshot?.todos &&
    a.snapshot?.inbound === b.snapshot?.inbound && activeAttempt(a.snapshot) === activeAttempt(b.snapshot));
  if (!view) return null;
  return <>
    <TodoDock state={todoDock(view.snapshot)}/>
    <GoalDock state={goalDock(view.snapshot)} observation={view.snapshot?.goal ?? undefined} disabled={disabled} mutate={(expected, mutation) => client.controlGoal(view.id, expected, mutation)}/>
    <QueueDock disabled={disabled} observation={view.snapshot?.inbound} edit={(expected, text) => client.editInbound(view.id, expected, text)} remove={expected => client.removeInbound(view.id, expected)} rows={queueRows(view.snapshot)} submissions={view.submissions ?? []} running={activeAttempt(view.snapshot)}/>
  </>;
}

export function ConversationTotals({ client, sessionId }: { client: AppServerClient; sessionId: string }) {
  const facts = useClientSelector(client, state => {
    const view = state.views[sessionId];
    // A detached or disconnected view's last reading is not current occupancy.
    const current = state.connection === 'connected' && view?.attachment === 'attached';
    return { statistics: view?.snapshot?.transcript.statistics, occupancy: current ? view?.snapshot?.context?.last_request_occupancy : undefined };
  }, shallowEqual);
  return <ConversationStats statistics={facts.statistics} occupancy={facts.occupancy}/>;
}

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
import type { CompletedResponseView } from '../../../../protocol/app-server/v38';
import { useRef, useState, useSyncExternalStore } from 'react';
import { UserMessage, AssistantMessage } from '../../presentation/agent/Message';
import pendingCss from './PendingMessage.module.css';
import { AttachmentCard } from '../../presentation/attachments/AttachmentCard';
import { useTranslation } from '../../locale/react';
import { TurnNavigator } from './TurnNavigator';
import { turnAnchor } from '../../client/transcript';

export function ConversationLive({ client, sessionId, mode, disabled, onHistorical }: {
  client: AppServerClient; sessionId?: string; mode: 'chat' | 'trajectory'; disabled: boolean;
  onHistorical: (id: ResponseAction, response: CompletedResponseView) => void;
}) {
  const tx=useTranslation(), viewport=useRef<ChatViewport>(null), [active,setActive]=useState<string | null>();
  const submission = useSyncExternalStore(client.firstSubmissions.subscribe, () => sessionId ? client.firstSubmissions.session(sessionId) : undefined);
  const waiting = !!submission && ['attaching', 'uploading', 'admitting'].includes(submission.phase);
  const tracePreview = useClientSelector(client, state => {
    const view = sessionId ? state.views[sessionId] : undefined;
    return mode === 'trajectory' && view?.attachment === 'attaching' ? view.tracePreview : undefined;
  });
  const view = useClientSelector(client, state => {
    const view = sessionId ? state.views[sessionId] : undefined;
    if (!view) return undefined;
    const preview = view.attachment === 'attaching' ? view.preview : undefined;
    const snapshot = preview ? { messages: [], attempt: null, statuses: [], conversation_id: preview.conversationId, transcript: preview.history.page } : view.snapshot;
    if (!snapshot) return undefined;
    return { id: view.id, target: view.target, messages: snapshot.messages, attempt: snapshot.attempt,
      transcript: snapshot.transcript, statuses: snapshot.statuses, conversation_id: snapshot.conversation_id,
      readingPreview: !!preview, history: preview?.history ?? view.history, trace: mode === 'trajectory' ? view.trace : undefined,
      safe: lineageSwitchSafe(view), disabled: disabled || !client.isAttachmentControlCurrent(view.id, view.attachmentObservation)
        || !!view.modelMutation || !!view.snapshot?.shutting_down || !!view.snapshot?.durability_failure };
  }, shallowEqual);
  if (sessionId && tracePreview) return <Trajectory key={sessionId} cache={tracePreview.cache} onSelect={id => client.selectTrace(sessionId, id)} onLoadDetail={id => { void client.loadTraceDetail(sessionId, id); }} loadEarlier={() => void client.loadEarlierTrace(sessionId).catch(() => {})}/>;
  if (!view) return sessionId ? <ChatViewport latestLabel={tx('agent:agent-transcript.return-to-latest')}><PendingMessage client={client} sessionId={sessionId}/></ChatViewport> : null;
  return mode === 'trajectory' && view.trace
    ? <Trajectory key={`${view.id}:${view.target?.attachment_id}`} cache={view.trace} onSelect={id => client.selectTrace(view.id, id)} onLoadDetail={id => { void client.loadTraceDetail(view.id, id); }} loadEarlier={() => void client.loadEarlierTrace(view.id).catch(() => {})}/>
    : <ChatViewport ref={viewport} key={`${view.id}:${view.target?.attachment_id}`} latestLabel={tx('agent:agent-transcript.return-to-latest')}
      overlay={<TurnNavigator key={`rail:${view.id}:${view.target?.attachment_id}`} client={client} sessionId={view.id} active={active} onNavigate={turn=>{
        let clientIntent: number | undefined;
        const clientCurrent = () => {
          const current = client.getSnapshot().views[view.id];
          return current?.target === view.target && current.attachment === 'attached' && current.turnNavigation?.intent === clientIntent;
        };
        const intent=viewport.current?.beginNavigation(() => { if (clientCurrent()) client.invalidateReading(view.id); });
        if(intent){
          const work=client.navigateTurn(view.id,turn,intent.current);
          clientIntent=client.getSnapshot().views[view.id]?.turnNavigation?.intent;
          void work.then(committed=>{if(committed)intent.commit(turnAnchor(committed.id),clientCurrent);});
        }
      }}/>}
      latestTurn={view.attempt && view.attempt.phase.type!=='settled' ? turnAnchor({conversation_id:view.conversation_id,attempt_id:view.attempt.attempt_id}) : undefined}
      historical={!!view.history?.window} onLatest={() => client.returnToLatest(view.id)} onActiveTurn={setActive}>
      {(view.messages.length > 0 || !!view.transcript.entries?.length || !waiting) && <AgentTranscript requestFeedback={attemptId => <ModelRetries client={client} sessionId={view.id} attemptId={attemptId}/>} snapshot={view} history={view.history} loadLater={() => void client.loadLater(view.id)} loadEarlier={() => void (view.readingPreview ? client.loadEarlierPreview(view.id) : client.loadEarlier(view.id)).catch(() => {})} lineageSwitchSafe={view.safe} historicalDisabled={view.disabled} onHistorical={onHistorical}/>}
      <ConversationActivity client={client} sessionId={view.id}/>
      <PendingMessage client={client} sessionId={view.id}/>
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
    return { statistics: view?.attachment === 'attaching' ? view.statisticsPreview?.statistics ?? view.snapshot?.transcript.statistics : view?.snapshot?.transcript.statistics, occupancy: current ? view?.snapshot?.context?.last_request_occupancy : view?.attachment === 'attaching' ? view.statisticsPreview?.occupancy ?? view.snapshot?.context?.last_request_occupancy : undefined };
  }, shallowEqual);
  return <ConversationStats statistics={facts.statistics} occupancy={facts.occupancy}/>;
}

function PendingMessage({ client, sessionId }: { client: AppServerClient; sessionId: string }) {
  const tx = useTranslation();
  const flow = useSyncExternalStore(client.firstSubmissions.subscribe, () => client.firstSubmissions.session(sessionId));
  if (!flow || !['attaching', 'uploading', 'admitting'].includes(flow.phase)) return null;
  return <div className={pendingCss.root} data-pending-message="">
    <UserMessage label={tx('agent:message.your-message')} attachments={flow.draft.files.map((file, i) => <AttachmentCard key={flow.attachmentIds[i]} name={file.name} image={file.type.startsWith('image/')}/>)}><div className={pendingCss.text}>{flow.draft.text}</div></UserMessage>
    <AssistantMessage label={tx('agent:message.assistant-response')}><p className={pendingCss.waiting} role="status" data-first-submission={flow.phase}><span className={pendingCss.dot} aria-hidden="true"/>{tx(flow.phase === 'attaching' ? 'common:app.connecting' : flow.phase === 'uploading' ? 'common:startup.uploading' : 'common:startup.admitting')}</p></AssistantMessage>
  </div>;
}

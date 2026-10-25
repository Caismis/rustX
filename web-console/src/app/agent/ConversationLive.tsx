import { useComposerContent } from './useComposerContent';
import { JournalSourceProvider } from './ObservationSource';
import { contentBindings } from '../../../../protocol/app-server/projection';
import { useSubagents } from './subagent-context';
import { ToolInspectionContext, useToolTraceNavigation } from './tool-inspection';
import type { AppServerClient } from '../../client/app-server';
import type { SessionView } from '../../client/contracts';
import { shallowEqual, useClientSelector } from '../../client/selectors';
import { useModelRetryFeedback } from './ModelRetry';
import { AgentTranscript } from './AgentTranscript';
import { ContextSeat } from './ContextSeat';
import { CompactionCheckpoint } from './CompactionCheckpoint';
import { useCompactionHistory } from './useCompactionHistory';
import { useWorkflowContent } from './useWorkflowContent';
import { ConversationStats } from './UsageStats';
import { TodoDock } from '../composer/TodoDock';
import { GoalDock } from '../composer/GoalDock';
import { QueueDock } from '../composer/QueueDock';
import { activeAttempt, lineageSwitchSafe } from '../../bindings/projection';
import { todoDock, goalDock, queueRows } from '../../bindings/composer-context';
import { ChatViewport } from '../../presentation/layout/ChatViewport';
import { Trajectory } from '../trajectory/Trajectory';
import type { ResponseAction } from '../commands/native';
import type { CompletedResponseView } from '../../../../protocol/app-server/v57';
import { useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import { responseRetryReady, type ResponseRetry } from './response-retry';
import { UserMessage, AssistantMessage, MessageFile } from '../../presentation/agent/Message';
import pendingCss from './PendingMessage.module.css';
import { AttachmentCard } from '../../presentation/attachments/AttachmentCard';
import { useTranslation } from '../../locale/react';
import { TurnNavigator } from './TurnNavigator';
import { transcriptContent, transcriptPresentation, turnAnchor } from '../../client/transcript';
import { type ForkOrigin, useForkPoint } from './ForkPoint';
import { firstSubmissions } from '../new-conversation/first-submit';

export function ConversationLive({ client, sessionId, mode, disabled, historicalPending, retry, onHistorical, onOpenSource, previewCurrent, onMode }: {
  client: AppServerClient; sessionId?: string; mode: 'chat' | 'trajectory'; disabled: boolean; historicalPending?: boolean;
  onMode?: (mode: 'chat' | 'trajectory') => void;
  retry?: ResponseRetry;
  onHistorical: (id: ResponseAction, response: CompletedResponseView) => void;
  onOpenSource?: (origin: ForkOrigin, beginViewport?: ChatViewport['beginNavigation']) => void;
  previewCurrent?: () => boolean;
}) {
  const scope = useSubagents();
  const tx=useTranslation(), viewport=useRef<ChatViewport>(null), [active,setActive]=useState<string | null>();
  const submission = useSyncExternalStore(firstSubmissions(client).subscribe, () => sessionId ? firstSubmissions(client).session(sessionId) : undefined);
  const waiting = !!submission && ['attaching', 'uploading', 'admitting'].includes(submission.phase);
  const tracePreview = useClientSelector(client, state => {
    const view = sessionId ? state.views[sessionId] : undefined;
    return mode === 'trajectory' && view?.attachment === 'attaching' ? view.tracePreview : undefined;
  });
  const view = useClientSelector(client, state => {
    const native = sessionId ? state.views[sessionId] : undefined;
    const retrying = !!retry && retry.view.id === sessionId && !responseRetryReady(retry, native);
    const view = retrying ? retry.view : native;
    if (!view) return undefined;
    const preview = view.attachment === 'attaching' ? view.preview : undefined;
    const snapshot = preview ? { messages: [], attempt: null, statuses: [], conversation_id: preview.conversationId, transcript: preview.history.page } : view.snapshot;
    if (!snapshot) return undefined;
    return { retrying, publicationHeads: preview || view.history?.window ? undefined : view.publicationHeads, nodeId:view.nodeId,generation:state.generation,content: preview ? preview.history.page.content ?? [] : transcriptContent(view.history,contentBindings(view.snapshot!)), hasCheckpoint: !!view.checkpoint, configurationObservation:state.configurationObservation?.[view.id], id: view.id, target: view.target, observation: view.attachmentObservation, readable: !disabled && client.isAttachmentObservationCurrent(view.id, view.attachmentObservation), messages: snapshot.messages, message_descriptors: 'message_descriptors' in snapshot ? snapshot.message_descriptors : [], publication_settlements: 'publication_settlements' in snapshot ? snapshot.publication_settlements : undefined, attempt: snapshot.attempt,
      transcript: snapshot.transcript, statuses: snapshot.statuses, pending_interactions: 'pending_interactions' in snapshot ? snapshot.pending_interactions : undefined, conversation_id: snapshot.conversation_id,
      readingPreview: !!preview, history: preview?.history ?? view.history, trace: mode === 'trajectory' ? view.trace : undefined,
      safe: !retrying && lineageSwitchSafe(view), disabled: retrying || disabled || !client.isAttachmentControlCurrent(view.id, view.attachmentObservation)
        || !!view.modelMutation || !!view.snapshot?.shutting_down || !!view.snapshot?.durability_failure };
  }, shallowEqual);
  const workflowView = useClientSelector(client, state => sessionId ? state.views[sessionId] : undefined, (a, b) => a === b || !!a && !!b && a.id === b.id && a.target === b.target && a.attachmentObservation === b.attachmentObservation && a.snapshot?.workflows === b.snapshot?.workflows);
  const workflows = useWorkflowContent(client, workflowView, mode === 'chat' && !scope?.selected && !view?.retrying && !view?.readingPreview && !view?.history?.window);
  const contentView = useMemo(() => Symbol('root content view'), [view?.observation, sessionId, mode, scope?.selected]);
  const currentContentView = useRef(contentView); currentContentView.current = contentView;
  const ownsContent = useMemo(() => () => currentContentView.current === contentView && mode === 'chat' && !scope?.selected && !!view?.observation && !!sessionId && client.isAttachmentObservationCurrent(sessionId, view.observation), [contentView, mode, scope?.selected, client, sessionId, view?.observation]);
  const ownsHistory=()=>{
    if(!view?.readingPreview||!view.history?.page.source_cut||currentContentView.current!==contentView||mode!=='chat'||scope?.selected)return false;
    const state=client.getSnapshot(),current=state.views[view.id];
    return state.generation===view.generation&&current?.nodeId===view.nodeId&&current.attachment==='attaching'
      &&current.preview?.history.page.source_cut===view.history.page.source_cut;
  };
  const fork = useForkPoint(client, mode === 'chat' ? view?.target : undefined, view?.history?.page.inherited_through);
  const inspect = useToolTraceNavigation(locator => view ? client.locateToolTrace(view.id, locator) : Promise.resolve(false), onMode, !!view && !view.readingPreview && !scope?.selected, `root:${sessionId}:${view?.target?.attachment_id}`);
  const readingEntries = useMemo(() => view ? transcriptPresentation(view.history, view.transcript) : [], [view?.history, view?.transcript]);
  const requestFeedback = useModelRetryFeedback(client, sessionId ?? '', readingEntries,
    !view?.history?.window ? view?.attempt?.in_flight?.message_id : undefined);
  const checkpoints = useCompactionHistory(client, sessionId, mode === 'chat' && !scope?.selected && !!view && !view.retrying && !view.readingPreview);
  const content = useMemo(() => [...view?.content ?? [], ...(view?.publicationHeads ?? []).flatMap(head => head.content)], [view?.content, view?.publicationHeads]);
  useEffect(() => {
    if (!view?.readable || view.retrying || view.readingPreview || scope?.selected) return;
    void (mode === 'chat' ? client.loadInitialHistory(view.id) : client.loadInitialTrace(view.id)).catch(() => {});
  }, [client, mode, view?.id, view?.target?.attachment_id, view?.readable, view?.retrying, view?.readingPreview, scope?.selected]);
  if (sessionId && tracePreview) return <Trajectory key={sessionId} cache={tracePreview.cache} onSelect={id => client.selectTrace(sessionId, id)} onLoadDetail={id => { void client.loadTraceDetail(sessionId, id); }} loadEarlier={() => void client.loadEarlierTrace(sessionId).catch(() => {})}/>;
  if (!view) return sessionId ? <ChatViewport latestLabel={tx('agent:agent-transcript.return-to-latest')}><PendingMessage client={client} sessionId={sessionId}/></ChatViewport> : null;
  return mode === 'trajectory' && view.trace
    ? <Trajectory key={`${view.id}:${view.target?.attachment_id}`} cache={view.trace} onLatest={() => void client.returnToLatestTrace(view.id).then(() => client.loadInitialTrace(view.id))} onSelect={id => client.selectTrace(view.id, id)} onLoadDetail={id => { void client.loadTraceDetail(view.id, id); }} loadEarlier={() => void client.loadEarlierTrace(view.id).catch(() => {})}/>
    : <JournalSourceProvider historical={view.readingPreview || !!view.history?.window} content={content} messages={view.messages} transcript={view.transcript} history={view.history?.page} source={view.readingPreview&&view.history?.page.source_cut?{client,sessionId:view.id,nodeId:view.nodeId,historyCut:view.history.page.source_cut,cut:()=>undefined,owned:ownsHistory}:view.target ? {client,target:view.target,cut:()=>client.getSnapshot().views[view.id]?.checkpoint?.read_cut,owned:ownsContent} : undefined}><ToolInspectionContext value={inspect}><ChatViewport chromeVisible={!scope?.selected} ref={viewport} key={view.id} resetKey={view.target?.attachment_id} latestLabel={tx('agent:agent-transcript.return-to-latest')}
      overlay={<TurnNavigator key={`rail:${view.id}:${view.target?.attachment_id}`} previewCurrent={previewCurrent} client={client} sessionId={view.id} active={active} onNavigate={turn=>{
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
      latestTurn={view.attempt && ['admitted','running'].includes(view.attempt.phase.type) ? turnAnchor({conversation_id:view.conversation_id,attempt_id:view.attempt.attempt_id,attempt_id_source:view.attempt.attempt_id_source}) : undefined}
      initialHistoryLoading={view.history?.initialLoading} historical={!!view.history?.window} onLatest={() => { client.returnToLatest(view.id); void client.loadInitialHistory(view.id); }} onActiveTurn={setActive}>
      {fork.error && <div role="alert"><p>{fork.error}</p><button type="button" onClick={fork.retry}>{tx('agent:fork-point.reload')}</button></div>}
      {(view.messages.length > 0 || !!view.transcript.entries?.length || !waiting) && <AgentTranscript publicationHeads={view.publicationHeads} retrying={view.retrying} compactions={checkpoints.map(({ head, command }) => ({ head, content: <CompactionCheckpoint head={head} client={client} sessionId={view.id} title={command ? tx('agent:context.command-title') : undefined}/> }))} contextActivity={!view.retrying && !view.readingPreview && <ContextSeat key={`${view.id}:${view.target?.attachment_id}`} client={client} sessionId={view.id}/>} requestFeedback={requestFeedback} snapshot={view} workflows={!view.retrying && !view.readingPreview && !view.history?.window ? workflows.snapshot : undefined} history={view.history} loadLater={() => client.loadLater(view.id)} loadEarlier={() => { viewport.current?.beginNavigation(); void (view.readingPreview ? client.loadEarlierPreview(view.id) : client.loadEarlier(view.id)).catch(() => {}); }} lineageSwitchSafe={view.safe} historicalDisabled={view.disabled || historicalPending} onHistorical={onHistorical} forkPoint={fork.point} onOpenSource={onOpenSource ? origin => {
        // The gesture publishes its exact Shell intent and installs viewport
        // cancellation in this same stack, before any browser scroll can interleave.
        onOpenSource(origin, viewport.current?.beginNavigation);
      } : undefined} sourceDisabled={!view.readable}/>}
      {view.configurationObservation?.error && <div className={pendingCss.root}><p role="alert">{view.configurationObservation.error}</p></div>}
      {workflows.error && <p role="alert">{workflows.error}</p>}
      <PendingMessage client={client} sessionId={view.id}/>
      </ChatViewport></ToolInspectionContext></JournalSourceProvider>;
}
export function ConversationDocks({ client, sessionId, disabled }: { client: AppServerClient; sessionId: string; disabled: boolean }) {
  const todoEvidence = (view: SessionView) => {
    const frontier = view.checkpoint?.collections.find(value => value.domain === 'todos.tasks');
    return frontier && JSON.stringify([frontier.revision, frontier.total, frontier.loaded, frontier.next_offset]);
  };
  const view = useClientSelector(client, state => state.views[sessionId], (a, b) => a === b || !!a && !!b &&
    a.target === b.target && a.attachmentObservation === b.attachmentObservation && a.submissions === b.submissions && a.snapshot?.goal === b.snapshot?.goal && a.snapshot?.todos === b.snapshot?.todos &&
    a.snapshot?.inbound === b.snapshot?.inbound && activeAttempt(a.snapshot) === activeAttempt(b.snapshot) && todoEvidence(a) === todoEvidence(b));
  const content = useComposerContent(client, view);
  if (!view) return null;
  return <>
    {content.error && <p role="alert">{content.error}</p>}
    <TodoDock state={todoDock(content.snapshot, content.todoLoaded ? undefined : view.checkpoint?.collections.find(value => value.domain === 'todos.tasks'))}/>
    <GoalDock state={goalDock(content.snapshot)} observation={view.snapshot?.goal ?? undefined} disabled={disabled} mutate={(expected, mutation) => client.controlGoal(view.id, expected, mutation)} loadObjective={expected => client.goalObjective(view.id, expected)}/>
    <QueueDock disabled={disabled} observation={view.snapshot?.inbound} edit={(expected, text) => client.editInbound(view.id, expected, text)} remove={expected => client.removeInbound(view.id, expected)} rows={queueRows(view.snapshot)} submissions={view.submissions ?? []} running={activeAttempt(view.snapshot)}/>
  </>;
}

export function ConversationTotals({ client, sessionId }: { client: AppServerClient; sessionId: string }) {
  const facts = useClientSelector(client, state => {
    const view = state.views[sessionId];
    // A detached or disconnected view's last reading is not current occupancy.
    const current = state.connection === 'connected' && view?.attachment === 'attached';
    return { preview: !current && view?.attachment === 'attaching' ? view.statisticsPreview : undefined, nodeId: view?.nodeId, generation: state.generation, target:current?view?.target:undefined, observation:current?view?.attachmentObservation:undefined, statistics: view?.attachment === 'attaching' ? view.statisticsPreview?.statistics ?? view.snapshot?.transcript.statistics : view?.snapshot?.transcript.statistics, occupancy: current ? view?.snapshot?.context?.occupancy : view?.attachment === 'attaching' ? view.statisticsPreview?.occupancy ?? view.snapshot?.context?.occupancy : undefined };
  }, shallowEqual);
  return <JournalSourceProvider source={facts.target&&facts.observation ? {
    client,target:facts.target,cut:()=>client.getSnapshot().views[sessionId]?.checkpoint?.read_cut,
    owned:()=>!!facts.observation&&client.isAttachmentObservationCurrent(sessionId,facts.observation),
  }:facts.preview ? {
    client, sessionId, nodeId:facts.preview.nodeId, cut:()=>facts.preview?.cut,
    owned:()=>client.getSnapshot().generation===facts.generation && client.getSnapshot().views[sessionId]?.attachment==='attaching'
      && client.getSnapshot().views[sessionId]?.statisticsPreview===facts.preview
      && client.getSnapshot().views[sessionId]?.nodeId===facts.nodeId,
  }:undefined}><ConversationStats statistics={facts.statistics} occupancy={facts.occupancy}/></JournalSourceProvider>;
}

function PendingMessage({ client, sessionId }: { client: AppServerClient; sessionId: string }) {
  const tx = useTranslation();
  const flow = useSyncExternalStore(firstSubmissions(client).subscribe, () => firstSubmissions(client).session(sessionId));
  if (!flow || !['attaching', 'uploading', 'admitting'].includes(flow.phase)) return null;
  return <div className={pendingCss.root} data-pending-message="">
    <UserMessage label={tx('agent:message.your-message')} attachments={flow.draft.files.length > 0 ? flow.draft.files.map((file, i) => file.type.startsWith('image/')
      ? <AttachmentCard key={flow.attachmentIds[i]} name={file.name} image/>
      : <MessageFile key={flow.attachmentIds[i]} name={file.name} bytes={file.size}/>) : undefined}>{flow.draft.text !== '' ? <div className={pendingCss.text}>{flow.draft.text}</div> : undefined}</UserMessage>
    <AssistantMessage label={tx('agent:message.assistant-response')}><p className={pendingCss.waiting} role="status" data-first-submission={flow.phase}><span className={pendingCss.dot} aria-hidden="true"/>{tx(flow.phase === 'attaching' ? 'common:app.connecting' : flow.phase === 'uploading' ? 'common:startup.uploading' : 'common:startup.admitting')}</p></AssistantMessage>
  </div>;
}

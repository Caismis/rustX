import { useComposerContent } from './useComposerContent';
import { JournalSourceProvider } from './ObservationSource';
import { contentBindings } from '../../../../protocol/app-server/projection';
import { useSubagents } from './subagent-context';
import { ToolInspectionContext, useToolTraceNavigation } from './tool-inspection';
import type { AppServerClient } from '../../client/app-server';
import { isDurableColdRead } from '../../client/cold-reads';
import type { SessionView } from '../../client/contracts';
import { shallowEqual, useClientSelector } from '../../client/selectors';
import { useModelRetryFeedback } from './ModelRetry';
import { AgentTranscript } from './AgentTranscript';
import { ContextSeat } from './ContextSeat';
import { CompactionCheckpoint } from './CompactionCheckpoint';
import { useCompactionHistory } from './useCompactionHistory';
import { sameWorkflowView, useWorkflowContent } from './useWorkflowContent';
import { ConversationStats } from './UsageStats';
import { TodoDock } from '../composer/TodoDock';
import { GoalDock } from '../composer/GoalDock';
import { QueueDock } from '../composer/QueueDock';
import { activeAttempt, lineageSwitchSafe } from '../../bindings/projection';
import { todoDock, goalDock, queueRows } from '../../bindings/composer-context';
import { ChatViewport } from '../../presentation/layout/ChatViewport';
import { Trajectory } from '../trajectory/Trajectory';
import { SessionPanel } from './SessionPanel';
import type { ResponseAction } from '../commands/native';
import type { CanonicalContentLocator, CompletedResponseView, SessionMessageVersion } from '../../../../protocol/app-server/v80';
import { sameTarget } from '../../../../protocol/app-server/attachment-identity';
import { useMessageVersions } from './message-version-reading';
import { useEffect, useMemo, useRef, useState, useSyncExternalStore, type ReactNode } from 'react';
import { responseRetryReady, type ResponseRetry } from './response-retry';
import { UserMessage, AssistantMessage, MessageFile } from '../../presentation/agent/Message';
import pendingCss from './PendingMessage.module.css';
import { AttachmentCard } from '../../presentation/attachments/AttachmentCard';
import { useTranslation } from '../../locale/react';
import { TurnNavigator } from './TurnNavigator';
import { transcriptContent, transcriptPresentation, turnAnchor } from '../../client/transcript';
import { firstSubmissions } from '../new-conversation/first-submit';
import { ConversationRunCanvasHost } from '../workflows/run-canvas/ConversationRunCanvasHost';

export function ConversationLive({ client, sessionId, mode, disabled, historicalPending, retry, forking, onHistorical, previewCurrent, onMode, messageVersionFocus, onRetryMessage, onSwitchMessageVersion }: {
  client: AppServerClient; sessionId?: string; mode: 'chat' | 'trajectory'; disabled: boolean; historicalPending?: boolean; forking?: CompletedResponseView;
  onMode?: (mode: 'chat' | 'trajectory') => void;
  retry?: ResponseRetry;
  onHistorical: (id: ResponseAction, response: CompletedResponseView) => void;
  previewCurrent?: () => boolean;
  messageVersionFocus?: { conversationId: string; messageId?: string };
  onRetryMessage?: (locator: CanonicalContentLocator, replacementText?: string) => void;
  onSwitchMessageVersion?: (version: SessionMessageVersion) => void;
}) {
  const scope = useSubagents();
  const tx=useTranslation(), viewport=useRef<ChatViewport>(null), [active,setActive]=useState<string | null>();
  const submission = useSyncExternalStore(firstSubmissions(client).subscribe, () => sessionId ? firstSubmissions(client).session(sessionId) : undefined);
  const tracePreview = useClientSelector(client, state => {
    const view = sessionId ? state.views[sessionId] : undefined;
    return mode === 'trajectory' && isDurableColdRead(view) ? view?.tracePreview : undefined;
  });
  const readingOwner = useClientSelector(client, state => {
    const native = sessionId ? state.views[sessionId] : undefined;
    const coldReadable = isDurableColdRead(native);
    return { endpoint: state.endpoint, authority: state.authorityId, authorityRevision: state.authorityRevision, node: native?.nodeId, conversation: native?.target?.conversation_id ?? native?.nodeConversationId,
      connected: state.connection === 'connected',
      coldReadable,
      loading: state.connection === 'connected' && !!native && !native.error && !native.deleting && (native.attachment === 'attaching' || native.attachment === 'resynchronizing' || native.observationState === 'checkpointLoading' || !!native.passiveHistory && !native.target && (mode === 'trajectory' ? !native.tracePreview : !native.preview)),
      wanted: !!native && native.attachmentIntent === 'wanted' && !native.deletionCommitted,
      checkpointPending: !!native && !native.snapshot && native.attachmentIntent === 'wanted' && !native.deletionCommitted };
  }, shallowEqual);
  const selectedView = useClientSelector(client, state => {
    const native = sessionId ? state.views[sessionId] : undefined;
    const retrying = !!retry && retry.view.id === sessionId && !responseRetryReady(retry, native);
    const view = retrying ? retry.view : native;
    if (!view) return undefined;
    // A loading preview of the same Conversation cannot replace rows the
    // reader already has. A genuinely different Conversation still changes scope.
    const cold = isDurableColdRead(view);
    const passiveHistory = cold && !!view.passiveHistory;
    const preview = cold && view.preview && (passiveHistory || !view.snapshot || view.preview.conversationId !== view.snapshot.conversation_id) ? view.preview : undefined;
    const snapshot = preview ? { messages: [], attempt: null, statuses: [], conversation_id: preview.conversationId, transcript: preview.history.page } : view.snapshot;
    if (!snapshot) return undefined;
    return { retrying, hasBranches: !!view.summary?.has_branches, publicationHeads: preview || view.history?.window ? undefined : view.publicationHeads, nodeId:view.nodeId,generation:state.generation,content: preview ? preview.history.page.content ?? [] : transcriptContent(view.history,contentBindings(view.snapshot!)), hasCheckpoint: !!view.checkpoint, configurationObservation:state.configurationObservation?.[view.id], id: view.id, target: view.target, observation: view.attachmentObservation, readable: !disabled && client.isAttachmentObservationCurrent(view.id, view.attachmentObservation), inbound: 'inbound' in snapshot ? snapshot.inbound : undefined, messages: snapshot.messages, message_descriptors: 'message_descriptors' in snapshot ? snapshot.message_descriptors : [], publication_settlements: 'publication_settlements' in snapshot ? snapshot.publication_settlements : undefined, attempt: snapshot.attempt, submissions: view.submissions, sendingSubmissions: view.sendingSubmissions,
      transcript: snapshot.transcript, statuses: snapshot.statuses, pending_interactions: 'pending_interactions' in snapshot ? snapshot.pending_interactions : undefined, conversation_id: snapshot.conversation_id,
      readingPreview: !!preview, history: preview?.history ?? view.history, trace: mode === 'trajectory' ? view.trace : undefined,
      safe: !retrying && lineageSwitchSafe(view), disabled: retrying || disabled || !client.isAttachmentControlCurrent(view.id, view.attachmentObservation)
        || !!view.modelMutation || !!view.snapshot?.shutting_down || !!view.snapshot?.durability_failure };
  }, shallowEqual);
  const retainedReading = useRef<{ client: AppServerClient; sessionId?: string; endpoint?: string; authority?: string; authorityRevision?: number; replay?: SessionView; view: NonNullable<typeof selectedView> }>(undefined);
  if (retainedReading.current && (retainedReading.current.client !== client || retainedReading.current.sessionId !== sessionId
    || retainedReading.current.endpoint !== readingOwner.endpoint || retainedReading.current.authority !== readingOwner.authority || retainedReading.current.authorityRevision !== readingOwner.authorityRevision || retainedReading.current.replay !== retry?.view
    || retainedReading.current.view.nodeId !== readingOwner.node || retainedReading.current.view.conversation_id !== readingOwner.conversation)) retainedReading.current = undefined;
  // A retry may install another passive preview before its checkpoint. It must
  // not replace the richer, already released frame of this same Conversation.
  const availableView = selectedView?.readingPreview && readingOwner.checkpointPending
    && (!retainedReading.current?.view.readingPreview || retainedReading.current.view.generation !== selectedView.generation)
    && retainedReading.current?.view.conversation_id === selectedView.conversation_id ? undefined : selectedView;
  if (availableView) retainedReading.current = { client, sessionId, endpoint: readingOwner.endpoint, authority: readingOwner.authority, authorityRevision: readingOwner.authorityRevision, replay: retry?.view, view: availableView };
  const retained = !availableView && readingOwner.checkpointPending ? retainedReading.current?.view : undefined;
  // The wanted logical Conversation owns its presentation even if another
  // transport loss interrupts checkpoint loading. No old target, read admission
  // or control crosses either the offline or target-present loading interval.
  const view = availableView ?? (retained && { ...retained, target: undefined, observation: undefined, readable: false, disabled: true, safe: false, hasCheckpoint: false, configurationObservation: undefined });
  const readingPending = !!retained;
  const switchVersion = scope?.selected ? undefined : onSwitchMessageVersion;
  const messageControls = mode === 'chat' && !!onRetryMessage && !!switchVersion;
  const versions = useMessageVersions(client, switchVersion && view?.hasBranches && view.readable && !view.retrying ? view.target : undefined, view?.nodeId);
  const visitedVersion = useRef<typeof messageVersionFocus>(undefined);
  useEffect(() => {
    const target = view?.target;
    const id = messageVersionFocus?.messageId;
    if (!sessionId || !id || !target || !view.readable || !view.history || !messageControls
      || messageVersionFocus?.conversationId !== target.conversation_id || visitedVersion.current === messageVersionFocus) return;
    const intent = viewport.current?.beginNavigation();
    if (!intent) return;
    visitedVersion.current = messageVersionFocus;
    const current = () => sameTarget(client.getSnapshot().views[sessionId]?.target, target);
    void client.navigateMessage(sessionId, id, () => current() && intent.current()).then(found => {
      if (found) intent.commit(`message:${id}`, current);
    });
  }, [client, sessionId, messageVersionFocus, messageControls, view?.target, view?.readable, view?.history]);
  const waiting = !!submission && (['attaching', 'uploading', 'admitting'].includes(submission.phase) || !!view?.submissions?.some(value => value.messageId === submission.acceptedMessageId));
  const readingScope = useMemo(() => ({ endpoint: readingOwner.endpoint, authority: readingOwner.authority, authorityRevision: readingOwner.authorityRevision, conversation: view?.conversation_id, replay: retry?.view }), [readingOwner.endpoint, readingOwner.authority, readingOwner.authorityRevision, view?.conversation_id, retry?.view]);
  // Detached-reading memory shares the logical reading identity: a remount
  // caused by attachment churn inside the same Conversation restores, while a
  // different Conversation, authority or Session never inherits the offset.
  const scrollMemoryKey = useMemo(() => JSON.stringify([view?.id, readingOwner.endpoint, readingOwner.authority, readingOwner.authorityRevision, view?.conversation_id]), [view?.id, readingOwner.endpoint, readingOwner.authority, readingOwner.authorityRevision, view?.conversation_id]);
  const presentationOwner = useMemo(() => Symbol('released conversation text'), [client, sessionId, readingOwner.endpoint, readingOwner.authority, readingOwner.authorityRevision, view?.nodeId, view?.conversation_id, retry?.view]);
  const workflowView = useClientSelector(client, state => sessionId ? state.views[sessionId] : undefined, sameWorkflowView);
  const standaloneWorkflows = useWorkflowContent(client, workflowView, !scope && !readingPending && !view?.retrying && !view?.readingPreview && !view?.history?.window);
  const workflows = scope?.workflows ?? standaloneWorkflows;
  const contentView = useMemo(() => Symbol('root content view'), [view?.observation, sessionId, mode, scope?.selected]);
  const currentContentView = useRef(contentView); currentContentView.current = contentView;
  const ownsContent = useMemo(() => () => currentContentView.current === contentView && mode === 'chat' && !scope?.selected && !!view?.observation && !!sessionId && client.isAttachmentObservationCurrent(sessionId, view.observation), [contentView, mode, scope?.selected, client, sessionId, view?.observation]);
  const ownsHistory=()=>{
    if(!view?.readingPreview||!view.history?.page.source_cut||currentContentView.current!==contentView||mode!=='chat'||scope?.selected)return false;
    const state=client.getSnapshot(),current=state.views[view.id];
    return state.generation===view.generation&&current?.nodeId===view.nodeId&&isDurableColdRead(current)
      &&current.preview?.history.page.source_cut===view.history.page.source_cut;
  };
  const inspect = useToolTraceNavigation(locator => view ? client.locateToolTrace(view.id, locator) : Promise.resolve(false), onMode, !!view && !readingPending && !view.readingPreview && !scope?.selected, `root:${sessionId}:${view?.target?.attachment_id}`);
  const readingEntries = useMemo(() => view ? transcriptPresentation(view.history, view.transcript) : [], [view?.history, view?.transcript]);
  const requestFeedback = useModelRetryFeedback(client, sessionId ?? '', readingEntries,
    !view?.history?.window ? view?.attempt?.in_flight?.message_id : undefined);
  const keepCompactions = mode === 'chat' && !scope?.selected && readingOwner.wanted && !!view && !view.retrying && !view.readingPreview;
  const checkpoints = useCompactionHistory(client, sessionId, keepCompactions && !readingPending, { owner: presentationOwner, keep: keepCompactions });
  const content = useMemo(() => [...view?.content ?? [], ...(view?.publicationHeads ?? []).flatMap(head => head.content)], [view?.content, view?.publicationHeads]);
  const initialTrace = mode === 'trajectory' ? view?.trace : undefined;
  useEffect(() => {
    if (!view?.readable || view.retrying || view.readingPreview || scope?.selected) return;
    void (mode === 'chat' ? client.loadInitialHistory(view.id) : client.loadInitialTrace(view.id)).catch(() => {});
  }, [client, mode, view?.id, view?.target?.attachment_id, view?.readable, view?.retrying, view?.readingPreview, scope?.selected,
    initialTrace?.epoch, initialTrace?.page.next_cursor, initialTrace?.loading, initialTrace?.error]);
  useEffect(() => {
    if (mode !== 'trajectory' || !sessionId || disabled || scope?.selected || view?.retrying
      || !readingOwner.coldReadable || !readingOwner.connected) return;
    void client.loadInitialTrace(sessionId).catch(() => {});
  }, [client, sessionId, mode, disabled, scope?.selected, view?.retrying,
    readingOwner.coldReadable, readingOwner.connected, readingOwner.node, readingOwner.conversation,
    tracePreview?.conversationId, tracePreview?.cache.epoch, tracePreview?.cache.page.next_cursor, tracePreview?.cache.loading, tracePreview?.cache.error]);
  const liveReadable = view ? (view.readingPreview || !view.target ? true : view.readable) : !!tracePreview;
  const wrapCanvas = (contentNode: ReactNode) => (
    <ConversationRunCanvasHost
      client={client}
      sessionId={sessionId}
      conversationId={view?.conversation_id ?? readingOwner.conversation}
      attachmentTarget={view && !view.readingPreview ? view.target : undefined}
      attachmentObservation={view && !view.readingPreview ? view.observation : undefined}
      ownerCurrent={liveReadable && !view?.retrying}
      liveCheckpointCut={workflowView?.checkpoint?.read_cut}
      historyNodeId={view?.nodeId ?? readingOwner.node}
      traceCache={tracePreview?.cache ?? view?.trace}
      inlineRuns={workflowView?.snapshot?.workflows?.runs}
      liveRuns={workflows.currentSnapshot?.runs}
      liveRunsCurrentCut={workflows.isCurrentCut}
      liveWorkflowView={!scope && !readingPending && !view?.retrying && !view?.readingPreview && !view?.history?.window ? workflowView : undefined}
      pendingInteractions={view?.pending_interactions}
    >
      {contentNode}
    </ConversationRunCanvasHost>
  );
  if (sessionId && tracePreview) return wrapCanvas(<Trajectory key={sessionId} cache={tracePreview.cache} onSelect={id => client.selectTrace(sessionId, id)} onLoadDetail={id => { if (readingOwner.connected) void client.loadTraceDetail(sessionId, id); }} loadEarlier={() => void client.loadEarlierTrace(sessionId).catch(() => {})}/>);
  if (sessionId && mode === 'trajectory' && readingOwner.coldReadable && !view?.trace) return <ChatViewport latestLabel={tx('agent:agent-transcript.return-to-latest')}>
    {readingOwner.loading && !waiting && <div className={pendingCss.root}><ConversationSkeleton label={tx('common:activity.loading')}/></div>}
  </ChatViewport>;
  if (!view) return sessionId ? <ChatViewport latestLabel={tx('agent:agent-transcript.return-to-latest')}>
    {readingOwner.loading && !waiting && <div className={pendingCss.root}><ConversationSkeleton label={tx('common:activity.loading')}/></div>}
    <PendingMessage client={client} sessionId={sessionId}/>
  </ChatViewport> : null;
  return wrapCanvas(mode === 'trajectory' && view.trace
    ? <Trajectory key={`${view.id}:${view.target?.attachment_id}`} cache={view.trace} onLatest={() => void client.returnToLatestTrace(view.id).then(() => client.loadInitialTrace(view.id))} onSelect={id => client.selectTrace(view.id, id)} onLoadDetail={id => { void client.loadTraceDetail(view.id, id); }} loadEarlier={before => void client.loadEarlierTrace(view.id, before).catch(() => {})}
        messageHistory={switchVersion && !view.readingPreview && !view.retrying ? { versions, switchVersion, disabled: view.disabled || !view.safe || historicalPending } : undefined}/>
    : <JournalSourceProvider presentationOwner={presentationOwner} historical={view.readingPreview || !!view.history?.window} content={content} messages={view.messages} transcript={view.transcript} history={view.history?.page} source={readingPending ? undefined : view.readingPreview&&view.history?.page.source_cut?{client,sessionId:view.id,nodeId:view.nodeId,historyCut:view.history.page.source_cut,cut:()=>undefined,owned:ownsHistory}:view.target ? {client,target:view.target,cut:()=>client.getSnapshot().views[view.id]?.checkpoint?.read_cut,owned:ownsContent} : undefined}><ToolInspectionContext value={inspect}><ChatViewport chromeVisible={!scope?.selected} ref={viewport} key={view.id} resetKey={readingScope} memoryKey={scrollMemoryKey} latestLabel={tx('agent:agent-transcript.return-to-latest')}
      overlay={<>{!readingPending && <TurnNavigator key={`rail:${view.id}:${view.target?.attachment_id}`} previewCurrent={previewCurrent} client={client} sessionId={view.id} active={active} onNavigate={turn=>{
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
      }}/>}<SessionPanel key={view.id} client={client} sessionId={view.id}/></>}
      latestTurn={view.attempt && ['admitted','running'].includes(view.attempt.phase.type) ? turnAnchor({conversation_id:view.conversation_id,attempt_id:view.attempt.attempt_id,attempt_id_source:view.attempt.attempt_id_source}) : undefined}
      initialHistoryLoading={readingPending || view.readingPreview || view.history?.initialLoading} historical={!!view.history?.window && !view.readingPreview} onLatest={() => { client.returnToLatest(view.id); void client.loadInitialHistory(view.id); }} onActiveTurn={setActive}>
      {(view.messages.length > 0 || !!view.transcript.entries?.length || !waiting) && <AgentTranscript accepted={[...(view.submissions ?? []), ...(view.sendingSubmissions ?? [])].filter(value => value.messageId !== submission?.acceptedMessageId && value.placement !== 'queued')} publicationHeads={view.publicationHeads} retrying={view.retrying} compactions={checkpoints.map(({ head, command }) => ({ head, content: <CompactionCheckpoint head={head} client={client} sessionId={view.id} readable={!readingPending && view.readable && !view.readingPreview} title={command ? tx('agent:context.command-title') : undefined}/> }))} contextActivity={!readingPending && !view.retrying && !view.readingPreview && <ContextSeat key={`${view.id}:${view.target?.attachment_id}`} client={client} sessionId={view.id}/>} requestFeedback={requestFeedback} snapshot={view} workflows={!readingPending && !view.retrying && !view.readingPreview && !view.history?.window ? workflows.snapshot : undefined} history={view.history} loadLater={readingPending ? undefined : () => client.loadLater(view.id)} loadEarlier={readingPending ? undefined : () => { viewport.current?.beginNavigation(); void (view.readingPreview ? client.loadEarlierPreview(view.id) : client.loadEarlier(view.id)).catch(() => {}); }} lineageSwitchSafe={view.safe} forking={forking} historicalDisabled={view.disabled || historicalPending} onHistorical={onHistorical}
        editedMessage={view.retrying ? retry?.edited : undefined}
        userMessageHistory={messageControls && !view.readingPreview && !view.retrying ? {
          versions, retry: onRetryMessage, switchVersion,
        } : undefined}/>}
      {view.configurationObservation?.error && <div className={pendingCss.root}><p role="alert">{view.configurationObservation.error}</p></div>}
      {workflows.error && <p role="alert">{workflows.error}</p>}
      <PendingMessage client={client} sessionId={view.id}/>
      </ChatViewport></ToolInspectionContext></JournalSourceProvider>);
}
export function ConversationDocks({ client, sessionId, disabled }: { client: AppServerClient; sessionId: string; disabled: boolean }) {
  const first = useSyncExternalStore(firstSubmissions(client).subscribe, () => firstSubmissions(client).session(sessionId));
  const todoEvidence = (view: SessionView) => {
    const frontier = view.checkpoint?.collections.find(value => value.domain === 'todos.tasks');
    return frontier && JSON.stringify([frontier.revision, frontier.total, frontier.loaded, frontier.next_offset]);
  };
  const view = useClientSelector(client, state => state.views[sessionId], (a, b) => a === b || !!a && !!b &&
    a.target === b.target && a.attachmentObservation === b.attachmentObservation && a.submissions === b.submissions && a.sendingSubmissions === b.sendingSubmissions && a.snapshot?.goal === b.snapshot?.goal && a.snapshot?.todos === b.snapshot?.todos &&
    a.snapshot?.inbound === b.snapshot?.inbound && a.checkpoint?.read_cut.source_cut?.read.mutation_revision === b.checkpoint?.read_cut.source_cut?.read.mutation_revision &&
    activeAttempt(a.snapshot) === activeAttempt(b.snapshot) && todoEvidence(a) === todoEvidence(b));
  const content = useComposerContent(client, view);
  if (!view) return null;
  const queuedSubmissions = [
    ...(view.submissions ?? []).filter(value => value.messageId !== first?.acceptedMessageId && value.placement === 'queued'),
    ...(view.sendingSubmissions ?? []).filter(value => value.placement === 'queued'),
  ];
  return <>
    {content.error && <p role="alert">{content.error}</p>}
    <TodoDock state={todoDock(content.snapshot, content.todoLoaded ? undefined : view.checkpoint?.collections.find(value => value.domain === 'todos.tasks'))}/>
    <GoalDock state={goalDock(content.snapshot)} observation={view.snapshot?.goal ?? undefined} disabled={disabled} mutate={(expected, mutation) => client.controlGoal(view.id, expected, mutation)} loadObjective={expected => client.goalObjective(view.id, expected)}/>
    <QueueDock disabled={disabled} observation={view.snapshot?.inbound} readCut={view.checkpoint?.read_cut.source_cut?.read} edit={(expected, text) => client.editInbound(view.id, expected, text)} remove={expected => client.removeInbound(view.id, expected)} steer={expected => client.steerInbound(view.id, expected)} attachmentMetadata={key => client.uploadAttachmentMetadata(view.id, key)} rows={queueRows(view.snapshot)} submissions={queuedSubmissions} running={activeAttempt(view.snapshot)}/>
  </>;
}

export function ConversationTotals({ client, sessionId }: { client: AppServerClient; sessionId: string }) {
  const facts = useClientSelector(client, state => {
    const view = state.views[sessionId];
    // A detached or disconnected view's last reading is not current occupancy.
    const current = state.connection === 'connected' && view?.attachment === 'attached';
    const attaching = view?.attachment === 'attaching';
    const waitingForCheckpoint = current && !view.snapshot && (view.observationState === 'attached' || view.observationState === 'checkpointLoading');
    const preview = attaching || waitingForCheckpoint ? view.statisticsPreview : undefined;
    const pendingConversation = view?.target?.conversation_id ?? view?.nodeConversationId;
    const lastReading = (attaching || waitingForCheckpoint) ? view?.checkpoint?.snapshot : undefined;
    const retained = lastReading && lastReading.conversation_id === pendingConversation ? lastReading : undefined;
    return { preview: attaching ? preview : undefined, nodeId: view?.nodeId, generation: state.generation,
      target: current && view.snapshot ? view.target : undefined, observation: current && view.snapshot ? view.attachmentObservation : undefined,
      reserveDock: !!(retained?.transcript.statistics || retained?.context?.occupancy),
      statistics: preview?.statistics ?? view?.snapshot?.transcript.statistics,
      occupancy: preview?.occupancy ?? (current || attaching ? view?.snapshot?.context?.occupancy : undefined) };
  }, shallowEqual);
  return <JournalSourceProvider source={facts.target&&facts.observation ? {
    client,target:facts.target,cut:()=>client.getSnapshot().views[sessionId]?.checkpoint?.read_cut,
    owned:()=>!!facts.observation&&client.isAttachmentObservationCurrent(sessionId,facts.observation),
  }:facts.preview ? {
    client, sessionId, nodeId:facts.preview.nodeId, cut:()=>facts.preview?.cut,
    owned:()=>client.getSnapshot().generation===facts.generation && client.getSnapshot().views[sessionId]?.attachment==='attaching'
      && client.getSnapshot().views[sessionId]?.statisticsPreview===facts.preview
      && client.getSnapshot().views[sessionId]?.nodeId===facts.nodeId,
  }:undefined}><ConversationStats statistics={facts.statistics} occupancy={facts.occupancy} reserveDock={facts.reserveDock}/></JournalSourceProvider>;
}

function ConversationSkeleton({ label, phase }: { label: string; phase?: string }) {
  return <div className={pendingCss.skeleton} role="status" aria-label={label} data-first-submission={phase}>
    <span aria-hidden="true"/><span aria-hidden="true"/><span aria-hidden="true"/>
  </div>;
}

export function PendingMessage({ client, sessionId }: { client: AppServerClient; sessionId: string }) {
  const tx = useTranslation();
  const flow = useSyncExternalStore(firstSubmissions(client).subscribe, () => firstSubmissions(client).session(sessionId));
  const submissions = useClientSelector(client, state => state.views[sessionId]?.history?.window ? undefined : state.views[sessionId]?.submissions);
  const accepted = submissions?.find(value => value.messageId === flow?.acceptedMessageId);
  if (!flow || !['attaching', 'uploading', 'admitting'].includes(flow.phase) && !accepted) return null;
  const text = accepted?.content ? accepted.content.flatMap(value => value.type === 'text' ? [value.text] : []).join('\n') : flow.draft.text;
  return <div className={pendingCss.root} data-pending-message="" data-provisional-message-id={accepted?.messageId} data-chat-anchor-key={accepted ? 'message:' + accepted.messageId : undefined}>
    <UserMessage label={tx('agent:message.your-message')} attachments={flow.attachments.length > 0 ? flow.attachments.map(file => file.type.startsWith('image/')
      ? <AttachmentCard key={file.id} name={file.name} image/>
      : <MessageFile key={file.id} name={file.name} bytes={file.size}/>) : undefined}>{text !== '' ? <div className={pendingCss.text}>{text}</div> : undefined}</UserMessage>
    <AssistantMessage label={tx('agent:message.assistant-response')}><ConversationSkeleton label={tx('agent:submission.sending')} phase={flow.phase}/></AssistantMessage>
  </div>;
}

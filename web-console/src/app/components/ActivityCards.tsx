/* Copyright (c) 2026 DeepSeek. MIT. Child composer frame adapted from ui-subagent; native observation/control ownership is rustX. See PROVENANCE.md. */
import { useWorkflowContent } from '../agent/useWorkflowContent';
import { JournalSourceProvider } from '../agent/ObservationSource';
import { ChildObservation } from '../../../../protocol/app-server/child-observation';
import { ToolInspectionContext, useToolTraceNavigation } from '../agent/tool-inspection';
import { AgentConversationContext } from '../agent/subagent-context';
import { StateDot } from '../../presentation/primitives/StateDot';
import { displayText } from '../../locale/translation';
import { useTranslation } from '../../locale/react';
import { createPortal } from 'react-dom';
import { useEffect, useLayoutEffect, useContext, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import type { AgentStatistics, RuntimeClientAgent, MethodResult } from '../../../../protocol/app-server/v57';
import type { Observation } from '../../client/session-lifecycle/port';
import { AppServerClient } from '../../client/app-server';
import { ArtifactContext } from './Artifact';
import { PreviewContext } from './ArtifactPreview';
import css from './ActivityCards.module.css';
import { SubagentChat } from '../agent/SubagentChat';
import { ConversationStats } from '../agent/UsageStats';
import { agentDot, agentRunning, agentStatus } from '../agent/subagent-state';
import { SubagentTrajectory, type SubagentTraceHandle } from '../agent/SubagentTrajectory';
import { ComposerSeat } from '../agent/ComposerSeat';
import { AgentComposer } from '../agent/AgentComposer';
import { SubagentObservationNotice } from '../agent/SubagentObservationNotice';
import { PermissionSelect } from '../../presentation/agent/PermissionSelect';
import { ArtifactResources } from '../../client/artifacts';
import { IconRefreshOutline14, IconClockOutline16, IconCloseOutline16 } from '../../presentation/primitives/icons';

import { useActivityRequest } from './useActivityRequest';

const detail = (value: string) => value.length > 1024 ? `${value.slice(0, 1024)}…` : value;
type AgentWait = Extract<MethodResult, { type: 'agent_wait' }>;
type Controls = { client?: AppServerClient; sessionId?: string };
const noSubscription = () => () => {};

/** Durable identity is the React key; the selected transcript survives resume. */
export function AgentCard({ agent, metrics, metricsError, mode = 'chat', visible = true, embedded = false, inspection = false, onMode, close, ...controls }: { agent: RuntimeClientAgent; metrics?: AgentStatistics; metricsError?: string; mode?: 'chat' | 'trajectory'; visible?: boolean; embedded?: boolean; inspection?: boolean; onMode?: (mode: 'chat' | 'trajectory') => void; close?: () => void } & Controls) {
  const tx = useTranslation();
  const trace = useRef<SubagentTraceHandle>(null);
  const [headerHost, setHeaderHost] = useState<HTMLElement | null>(null);
  useLayoutEffect(() => { setHeaderHost(visible && !embedded ? document.getElementById('subagent-header-actions') : null); }, [visible, embedded, agent.agent_id]);
  const request = useActivityRequest(controls);
  const waitRequest = useActivityRequest(controls);
  const interruptRequest = useActivityRequest(controls);
  const [settledActivation, setSettledActivation] = useState<Pick<AgentWait, 'activation_id' | 'outcome'> & { proof: Observation }>();
  const observeSettlement = (result: AgentWait, current: () => boolean) => {
    if (admission) setSettledActivation(previous => current() ? { proof: admission, activation_id: result.activation_id, outcome: result.outcome } : previous);
  };

  const [transcriptRefresh, refreshTranscript] = useState(0);
  const [transcriptLoading, setTranscriptLoading] = useState(false);
  const [transcriptError, setTranscriptError] = useState<{ owner: object; cause: unknown }>();
  const [conversation, setConversation] = useState<ChildObservation>();
  const observationOwner=useRef<ChildObservation|undefined>(undefined);
  const observationRefresh=useRef(0);
  const preview = useContext(PreviewContext);
  const { client, sessionId } = controls;
  const admission = useSyncExternalStore(client?.subscribe ?? noSubscription, () => sessionId ? client?.getSnapshot().views[sessionId]?.attachmentObservation : undefined);
  useEffect(() => { if (admission) { setConversation(undefined); } }, [admission]);
  const inspect = useToolTraceNavigation(locator => trace.current?.locate(locator) ?? Promise.resolve(false), onMode, visible && mode === 'chat' && !!client && !!sessionId, `child:${sessionId}:${agent.agent_id}:${admission?.target.attachment_id}`);
  const artifacts = useMemo(() => client && sessionId && admission ? new ArtifactResources(client, sessionId, agent.agent_id) : undefined, [client, sessionId, admission, agent.agent_id]);
  useEffect(() => () => artifacts?.dispose(), [artifacts]);

  const viewOwner=useMemo(()=>({}),[admission,agent.agent_id,agent.child_conversation_id,agent.activation_id,agent.current_activation,visible,mode]);
  const currentViewOwner=useRef<object|undefined>(viewOwner);currentViewOwner.current=viewOwner;
  useEffect(()=>()=>{if(currentViewOwner.current===viewOwner)currentViewOwner.current=undefined;},[viewOwner]);
  // Event-driven native watch: every response is a coherent child-owned cut.
  // Cursor and activation travel together; no client invents stream identity.
  useEffect(() => {
    if (!visible || (!inspection && mode !== 'chat') || !client || !sessionId || !admission || !client.isAttachmentObservationCurrent(sessionId, admission)) { setTranscriptLoading(false); return; }
    let observing = true; const controller = new AbortController();
    const current = () => observing && currentViewOwner.current===viewOwner && client.isAttachmentObservationCurrent(sessionId, admission);
    setTranscriptLoading(true);
    void (async () => {
      const retained=observationOwner.current;
      const observation=retained&&retained.agentId===agent.agent_id&&retained.conversationId===agent.child_conversation_id&&retained.activationId===agent.activation_id?retained:new ChildObservation(agent.agent_id,agent.child_conversation_id,agent.activation_id);
      observationOwner.current=observation;
      if(observationRefresh.current!==transcriptRefresh){observationRefresh.current=transcriptRefresh;observation.phase="resyncRequired";observation.cursor=undefined;}
      let recoveries = 0;
      while (current()) {
        const result = await client.request({ method: 'agent/observe', params: { target: admission.target, agent_id: agent.agent_id, after: observation.cursor ?? null } }, 'agent_observe', undefined, current, controller.signal);
        if (!current()) return;
        observation.apply(result.observation);
        setConversation(Object.assign(Object.create(Object.getPrototypeOf(observation)), observation));
        setTranscriptError(undefined);
        setTranscriptLoading(false);
        if (observation.phase === 'durableOnly') return;
        if (observation.phase === 'resyncRequired' && ++recoveries > 2) throw new Error('Observation recovery is unavailable. Reopen the conversation.');
      }
    })().catch(cause => { if (current()) { setTranscriptError({ owner: viewOwner, cause }); setTranscriptLoading(false); } });
    return () => { observing = false; controller.abort(); };
  }, [client, sessionId, admission, agent.agent_id, agent.child_conversation_id, agent.activation_id,agent.current_activation,agent.state,agent.activation_state,transcriptRefresh,visible,mode,inspection,viewOwner]);
  const ownsContent = useMemo(() => () => Boolean(visible&&mode==='chat'&&currentViewOwner.current===viewOwner&&client&&sessionId&&admission&&observationOwner.current?.activationId===agent.activation_id&&client.isAttachmentObservationCurrent(sessionId,admission)),[client,sessionId,admission,viewOwner,visible,mode,agent.activation_id]);
  const scopedPreview = useMemo(() => preview ? {
    openPreview: (artifact: Parameters<typeof preview.openPreview>[0]) => {if(artifact.source.kind==='session_file'&&artifact.source.conversationId!==agent.child_conversation_id)throw new Error('Obsolete Child delivery source');return preview.openPreview({ ...artifact, source: { ...artifact.source,agentId:agent.agent_id,...(artifact.source.kind==='session_file'?{owned:ownsContent}:{} ) } });},
    download: (artifact: Parameters<typeof preview.download>[0]) => {if(artifact.source.kind==='session_file'&&artifact.source.conversationId!==agent.child_conversation_id)throw new Error('Obsolete Child delivery source');return preview.download({ ...artifact, source: { ...artifact.source,agentId:agent.agent_id,...(artifact.source.kind==='session_file'?{owned:ownsContent}:{} ) } });},
  } : undefined, [preview,agent.agent_id,agent.child_conversation_id,ownsContent]);
  const workflows = useWorkflowContent(client, admission && sessionId && conversation?.checkpoint ? { id: sessionId, target: admission.target, attachmentObservation: admission, snapshot: conversation.snapshot, checkpoint: conversation.checkpoint } : undefined, visible && mode === 'chat' && conversation?.activationId === agent.activation_id, agent.agent_id);
  const unavailable = agent.state === 'unavailable';
  const acceptsMessage = agent.state === 'active' || agent.state === 'inactive';
  const observationError = transcriptError?.owner === viewOwner ? transcriptError : undefined;
  const childSnapshot = conversation?.activationId === agent.activation_id ? conversation.snapshot : undefined;
  return <AgentConversationContext.Provider value={agent}><ArtifactContext.Provider value={artifacts}><PreviewContext.Provider value={scopedPreview}><section className={css.agentConversation} data-agent-id={agent.agent_id} data-agent-state={agent.state} data-view={mode} data-activation-id={agent.current_activation ?? undefined} aria-label={tx('common:activity.agent-label', { name: agent.agent })}>
    <div hidden={mode !== 'chat'} className={css.agentChat}><JournalSourceProvider content={conversation?.content} messages={conversation?.snapshot?.messages} transcript={conversation?.snapshot?.transcript} source={client&&admission&&conversation?.checkpoint ? {client,target:admission.target,agentId:agent.agent_id,cut:()=>conversation.checkpoint?.read_cut,owned:ownsContent} : undefined}><ToolInspectionContext value={inspect}><SubagentChat client={client} sessionId={sessionId} admission={admission} agent={agent} snapshot={conversation?.snapshot} workflows={workflows.snapshot} visible={visible && mode === 'chat'}>
      {transcriptLoading && !conversation && <p role="status">{tx('common:activity.reading')}</p>}
      {metricsError && <p role="alert">{metricsError}</p>}
      {workflows.error && <p role="alert">{workflows.error}</p>}
      {agent.detail && <p>{detail(agent.detail)}</p>}
      {settledActivation && settledActivation.proof === admission && <small role="status">{settledActivation.outcome ? tx(`common:state.${settledActivation.outcome}`) : tx('common:activity.observed-inactive')}</small>}
      {[request.error, waitRequest.error, interruptRequest.error].filter(Boolean).map((error, index) => <p role="alert" key={index}>{displayText(tx, error!)}</p>)}
    </SubagentChat></ToolInspectionContext></JournalSourceProvider></div>
    {client && sessionId && <div hidden={mode !== 'trajectory'} className={css.agentTrace}><SubagentTrajectory ref={trace} agent={agent} client={client} sessionId={sessionId} visible={visible && mode === 'trajectory'} observationRevision={conversation?.checkpoint?.cursor}/></div>}
    {visible && headerHost && client && createPortal(<div className={css.agentActions}>
      <span className={css.headerStatus} role="status"><StateDot state={agentDot(agent)}/>{agentStatus(agent, tx)}{request.pending && <span>{tx('common:activity.waiting-runtime')}</span>}</span>
      <button type="button" aria-label={tx('common:activity.transcript')} title={tx('common:activity.transcript')} disabled={request.disabled} onClick={() => refreshTranscript(value => value + 1)}><IconRefreshOutline14/></button>
      {!inspection && <button type="button" aria-label={tx('common:activity.wait-activation')} title={tx('common:activity.wait-activation')} disabled={waitRequest.disabled || unavailable} onClick={() => void waitRequest.run(async (client, target, current) => { const result = await client.request({ method: 'agent/wait', params: { target, agent_id: agent.agent_id } }, 'agent_wait', undefined, current); if (current()) observeSettlement(result, current); })}><IconClockOutline16/></button>}
      {close && <button type="button" aria-label={tx('common:subagents.close-view')} title={tx('common:subagents.close-view')} onClick={close}><IconCloseOutline16/></button>}
    </div>, headerHost)}
    <ComposerSeat>
      {observationError && <SubagentObservationNotice error={observationError.cause} loading={transcriptLoading}
        disabled={!client || !sessionId || !client.isAttachmentObservationCurrent(sessionId, admission)} retry={() => refreshTranscript(value => value + 1)}/>}
      {!client && <div role="status"><StateDot state={agentDot(agent)}/>{agentStatus(agent, tx)}</div>}
      {(inspection || unavailable) && <div className={css.readOnly} role="status"><strong>{tx(inspection ? 'common:subagents.one-shot-title' : 'common:subagents.read-only-title')}</strong><span>{tx(inspection ? 'common:subagents.one-shot' : 'common:subagents.read-only')}</span></div>}
      {client && sessionId && !inspection && <div hidden={unavailable}><AgentComposer separateStop
        permission={childSnapshot && <PermissionSelect value={childSnapshot.effective_approval_mode} disabled choose={() => {}}/>}
        messageLabel={tx('common:activity.message-label', { name: agent.title })} busyEnterBehavior="steer" binding={`${sessionId}:${agent.agent_id}`}
        disabled={request.disabled && !request.pending || !acceptsMessage} busy={request.pending} active={agentRunning(agent)}
        uploadPolicy={client.getSnapshot().capabilities?.upload_policy}
        onUpload={(files, operation) => client.upload(sessionId, files, { current: () => client.isAttachmentControlCurrent(sessionId, admission), acknowledged: () => {} }, operation)}
        onReconcile={operation => client.uploadStatus(sessionId, operation)}
        onSend={(message, attachments, _delivery, acknowledged) => request.run(async (client, target, current) => {
          await client.request({ method: 'agent/sendMessage', params: { target, agent_id: agent.agent_id, message, attachments: [...attachments] } }, 'agent_message', () => acknowledged?.(), current);
          if (current()) refreshTranscript(value => value + 1);
        })}
        cancellationAvailable={!interruptRequest.disabled && !unavailable && agent.state !== 'inactive'}
        cancellationScope={admission ? { authority: admission, identity: `${agent.agent_id}:${agent.current_activation ?? agent.activation_id}` } : undefined}
        onCancel={() => void interruptRequest.run(async (client, target, current) => { const result = await client.request({ method: 'agent/interrupt', params: { target, agent_id: agent.agent_id } }, 'agent_interruption_accepted', undefined, current); if (result.agent_id !== agent.agent_id) throw new Error('Agent interruption receipt named another native owner.'); }, false)}
      /></div>}
      <JournalSourceProvider content={conversation?.content} messages={conversation?.snapshot?.messages} transcript={conversation?.snapshot?.transcript} source={client&&admission&&conversation?.checkpoint ? {
        client,target:admission.target,agentId:agent.agent_id,cut:()=>conversation.checkpoint?.read_cut,owned:ownsContent,
      }:undefined}><ConversationStats statistics={metrics?.statistics} occupancy={metrics?.occupancy}/></JournalSourceProvider>
    </ComposerSeat>
  </section></PreviewContext.Provider></ArtifactContext.Provider></AgentConversationContext.Provider>;
}

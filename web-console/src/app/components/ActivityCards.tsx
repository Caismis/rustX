import chatCss from '../../presentation/agent/Chat.module.css';
import { ChatViewport } from '../../presentation/layout/ChatViewport';
import { StateDot } from '../../presentation/primitives/StateDot';
import { displayText, message as uiMessage, type DisplayText } from '../../locale/translation';
import { useTranslation } from '../../locale/react';
import { createPortal } from 'react-dom';
import { useEffect, useLayoutEffect, useState, useSyncExternalStore } from 'react';
import type { AgentStatistics, RuntimeClientAgent, RuntimeClientJob, RuntimeClientTranscriptPage, MethodResult, WorkflowRunView, WorkflowRunId } from '../../../../protocol/app-server/v40';
import type { Observation } from '../../client/session-lifecycle/port';
import { AppServerClient, RpcFailure } from '../../client/app-server';
import { json } from '../../bindings/projection';
import { Badge, SettingsCard } from '../../presentation/settings/SettingsContent';
import { Button } from '../../presentation/primitives/Button';
import { ToolCard } from '../../presentation/agent/ToolCard';
import { ArtifactContext, ToolArtifacts } from './Artifact';
import { PreviewContext } from './ArtifactPreview';
import css from './ActivityCards.module.css';
import { AgentTranscript } from '../agent/AgentTranscript';
import { ConversationStats } from '../agent/UsageStats';
import { agentDot, agentRunning, agentStatus } from '../agent/subagent-state';
import { SubagentTrajectory } from '../agent/SubagentTrajectory';
import { ComposerSeat } from '../agent/ComposerSeat';
import { SubagentComposer } from '../agent/SubagentComposer';
import { IconRefreshOutline14, IconClockOutline16 } from '../../presentation/primitives/icons';

const detail = (value: string) => value.length > 1024 ? `${value.slice(0, 1024)}…` : value;
export const workflowKey = (id: WorkflowRunId) => JSON.stringify([id.conversation_id, id.attempt_id, id.invocation]);
type AgentWait = Extract<MethodResult, { type: 'agent_wait' }>;
type Controls = { client?: AppServerClient; sessionId?: string };
const noSubscription = () => () => {};

/** Typed domain failures are presentation copy; opaque native diagnostics stay raw. */
function activityFailure(cause: unknown): DisplayText {
  const data = cause instanceof RpcFailure ? cause.error.data : undefined;
  switch (data?.kind) {
    case 'agent_not_delivered': return uiMessage('common:activity.not-delivered', { id: data.agent_id });
    case 'agent_delivery_unknown': return uiMessage('common:activity.delivery-unknown', { id: data.agent_id });
    case 'job_publication_abandoned': return uiMessage('common:activity.publication-abandoned', { id: data.job_id });
    case 'agent_settlement': return uiMessage('common:activity.agent-unresolved', { id: data.agent_id });
    default: return cause instanceof Error ? cause.message : String(cause);
  }
}

/** Request presentation only: native state always comes from the next snapshot.
 * A lost response is never retried, and a changed attachment cannot adopt it. */
function useActivityRequest({ client, sessionId }: Controls) {
  const view = useSyncExternalStore(client?.subscribe ?? noSubscription, () => sessionId ? client?.getSnapshot().views[sessionId] : undefined);
  const proof = view?.attachmentObservation;
  const [state, setState] = useState<{ operation: { proof: Observation }; pending: boolean; error?: DisplayText }>();
  const visible = !!client && !!sessionId && client.isAttachmentObservationCurrent(sessionId, proof) && state?.operation.proof === proof;
  const pending = visible && !!state?.pending, error = visible ? state?.error : undefined;
  const enabled = !!client && !!sessionId && client.isAttachmentControlCurrent(sessionId, proof);
  async function run(operation: (client: AppServerClient, target: ReturnType<AppServerClient['target']>, current: () => boolean) => Promise<void>) {
    if (!enabled || pending || !client || !sessionId || !proof) return;
    const target = client.target(sessionId), owner = { proof };
    const current = () => client.isAttachmentObservationCurrent(sessionId, owner.proof);
    if (!current()) return;
    setState({ operation: owner, pending: true });
    try { await operation(client, target, current); if (current()) await client.refresh(sessionId); }
    catch (cause) { if (current()) setState(previous => current() && previous?.operation === owner ? { ...previous, error: activityFailure(cause) } : previous); }
    finally { setState(previous => previous?.operation === owner ? { ...previous, pending: false } : previous); }
  }
  return { run, disabled: !enabled || pending, pending, error };
}

/** Durable identity is the React key; the selected transcript survives resume. */
export function AgentCard({ agent, metrics, metricsError, mode = 'chat', visible = true, ...controls }: { agent: RuntimeClientAgent; metrics?: AgentStatistics; metricsError?: string; mode?: 'chat' | 'trajectory'; visible?: boolean } & Controls) {
  const tx = useTranslation();
  const [headerHost, setHeaderHost] = useState<HTMLElement | null>(null);
  useLayoutEffect(() => { setHeaderHost(visible ? document.getElementById('subagent-header-actions') : null); }, [visible, agent.agent_id]);
  const request = useActivityRequest(controls);
  const waitRequest = useActivityRequest(controls);
  const interruptRequest = useActivityRequest(controls);
  const [settledActivation, setSettledActivation] = useState<Pick<AgentWait, 'activation_id' | 'outcome'> & { proof: Observation }>();
  const observeSettlement = (result: AgentWait, current: () => boolean) => {
    if (admission) setSettledActivation(previous => current() ? { proof: admission, activation_id: result.activation_id, outcome: result.outcome } : previous);
  };
  const [message, setMessage] = useState('');
  const [transcript, setTranscript] = useState<RuntimeClientTranscriptPage>();
  const [transcriptRefresh, refreshTranscript] = useState(0);
  const [transcriptLoading, setTranscriptLoading] = useState(false);
  const [transcriptError, setTranscriptError] = useState<string>();
  const { client, sessionId } = controls;
  const admission = useSyncExternalStore(client?.subscribe ?? noSubscription, () => sessionId ? client?.getSnapshot().views[sessionId]?.attachmentObservation : undefined);
  useEffect(() => { if (admission) setTranscript(undefined); }, [admission]);
  // A selected child view refreshes canonical content when native activation
  // facts change. It never turns a transcript read into a lifecycle decision.
  useEffect(() => {
    if (!client || !sessionId || !admission || !client.isAttachmentObservationCurrent(sessionId, admission)) { setTranscriptLoading(false); return; }
    const target = admission.target;
    let observing = true;
    const current = () => observing && client.isAttachmentObservationCurrent(sessionId, admission);
    setTranscriptLoading(true); setTranscriptError(undefined);
    void client.request({ method: 'agent/transcript', params: { target, agent_id: agent.agent_id, limit: 64 } }, 'transcript', undefined, current)
      .then(result => { if (current()) setTranscript(previous => {
        if (!current()) return previous;
        if (!previous || (previous.entries?.length ?? 0) <= 64) return result.page;
        const latest = new Set(result.page.entries?.map(entry => entry.cursor));
        return { ...result.page, next_cursor: previous.next_cursor, entries: [...(previous.entries ?? []).filter(entry => !latest.has(entry.cursor)), ...(result.page.entries ?? [])] };
      }); })
      .catch(cause => { if (current()) setTranscriptError(cause instanceof Error ? cause.message : String(cause)); })
      .finally(() => { if (current()) setTranscriptLoading(false); });
    return () => { observing = false; };
  }, [client, sessionId, admission, agent.agent_id, agent.activation_id, agent.state, agent.observation.revision, transcriptRefresh]);
  const unavailable = agent.state === 'unavailable';
  const acceptsMessage = agent.state === 'active' || agent.state === 'inactive';
  const readTranscript = (before?: string) => request.run(async (client, target, owned) => {
    const current = () => owned() && !!sessionId && client.isAttachmentObservationCurrent(sessionId, admission);
    const result = await client.request({ method: 'agent/transcript', params: { target, agent_id: agent.agent_id, before, limit: 64 } }, 'transcript', undefined, current);
    if (current()) { setTranscript(previous => !current() ? previous : before && previous ? { ...result.page, entries: [...(result.page.entries ?? []), ...(previous.entries ?? [])] } : result.page); }
  });
  return <section className={css.agentConversation} data-agent-id={agent.agent_id} data-agent-state={agent.state} data-view={mode} data-activation-id={agent.current_activation ?? undefined} aria-label={tx('common:activity.agent-label', { name: agent.agent })}>
    <div hidden={mode !== 'chat'} className={css.agentChat}><ChatViewport latestLabel={tx('agent:agent-transcript.return-to-latest')}><div className={chatCss.column}>
      {transcript?.next_cursor && <Button size="sm" disabled={request.disabled} onClick={() => void readTranscript(transcript.next_cursor!)}>{tx('common:activity.older')}</Button>}
      <ArtifactContext.Provider value={undefined}><PreviewContext.Provider value={undefined}>{transcript && <AgentTranscript snapshot={{ conversation_id: agent.child_conversation_id, messages: [], statuses: [], attempt: null, transcript }} historicalDisabled/>}</PreviewContext.Provider></ArtifactContext.Provider>
      {transcriptLoading && !transcript && <p role="status">{tx('common:activity.reading')}</p>}
      {transcript && !transcript.entries?.length && <p>{tx('common:activity.empty')}</p>}
      {metricsError && <p role="alert">{metricsError}</p>}
      {transcriptError && <p role="alert">{transcriptError}</p>}
      {agent.detail && <p>{detail(agent.detail)}</p>}
      {settledActivation && settledActivation.proof === admission && <small role="status">{settledActivation.outcome ? tx(`common:state.${settledActivation.outcome}`) : tx('common:activity.observed-inactive')}</small>}
      {[request.error, waitRequest.error, interruptRequest.error].filter(Boolean).map((error, index) => <p role="alert" key={index}>{displayText(tx, error!)}</p>)}
    </div></ChatViewport></div>
    {client && sessionId && mode === 'trajectory' && <div className={css.agentTrace}><SubagentTrajectory agent={agent} client={client} sessionId={sessionId} visible={visible && mode === 'trajectory'}/></div>}
    {visible && headerHost && client && createPortal(<div className={css.agentActions}>
      <span className={css.headerStatus} role="status"><StateDot state={agentDot(agent)}/>{agentStatus(agent, tx)}{request.pending && <span>{tx('common:activity.waiting-runtime')}</span>}</span>
      <button type="button" aria-label={tx('common:activity.transcript')} title={tx('common:activity.transcript')} disabled={request.disabled} onClick={() => refreshTranscript(value => value + 1)}><IconRefreshOutline14/></button>
      <button type="button" aria-label={tx('common:activity.wait-activation')} title={tx('common:activity.wait-activation')} disabled={waitRequest.disabled || unavailable} onClick={() => void waitRequest.run(async (client, target, current) => { const result = await client.request({ method: 'agent/wait', params: { target, agent_id: agent.agent_id } }, 'agent_wait', undefined, current); if (current()) observeSettlement(result, current); })}><IconClockOutline16/></button>
    </div>, headerHost)}
    <ComposerSeat>
      {!client && <div role="status"><StateDot state={agentDot(agent)}/>{agentStatus(agent, tx)}</div>}
      {client && <SubagentComposer name={agent.agent} value={message} onChange={setMessage}
        disabled={request.disabled || !acceptsMessage} pending={request.pending}
        send={() => { const submitted = message; void request.run(async (client, target, current) => { await client.request({ method: 'agent/sendMessage', params: { target, agent_id: agent.agent_id, message: submitted } }, 'agent_message', undefined, current); if (current()) { setMessage(value => current() && value === submitted ? '' : value); refreshTranscript(value => current() ? value + 1 : value); } }); }}
        running={agentRunning(agent)}
        interruptDisabled={interruptRequest.disabled || unavailable || agent.state === 'inactive'}
        interrupt={() => void interruptRequest.run(async (client, target, current) => { const result = await client.request({ method: 'agent/interrupt', params: { target, agent_id: agent.agent_id } }, 'agent_wait', undefined, current); if (current()) observeSettlement(result, current); })}
        />}
      <ConversationStats statistics={metrics?.statistics} occupancy={metrics?.occupancy}/>
    </ComposerSeat>
  </section>;
}

export function JobCard({ job, ...controls }: { job: RuntimeClientJob } & Controls) {
  const tx = useTranslation();
  const request = useActivityRequest(controls);
  const waitRequest = useActivityRequest(controls);
  const output = job.result ? (job.result.content ?? []).flatMap(block => block.type === 'text' ? [block.text] : block.type === 'json' ? [block.value && typeof block.value === 'object' && !Array.isArray(block.value) && typeof block.value.combined === 'string' ? block.value.combined : json(block.value)] : []).join('\n') : job.progress ? [job.progress.message, job.progress.completed == null ? undefined : job.progress.total == null ? tx('common:activity.completed', { n: job.progress.completed }) : `${job.progress.completed} / ${job.progress.total}`].filter(Boolean).join('\n') : undefined;
  const active = job.state === 'starting' || job.state === 'running' || job.state === 'cancelling' || job.state === 'publishing_terminal';
  return <section data-job-state={job.state} aria-label={tx('common:activity.job-label', { name: job.tool_name })}>
    <ToolCard tool={{ id: job.job_id, identity: 'job', title: job.tool_name, variant: job.tool_id === 'tool-bash' ? 'bash' : 'generic', summary: job.bash?.description ?? job.bash?.command ?? tx('common:activity.job-title', { id: job.job_id }), input: job.bash?.command,
      state: job.state === 'succeeded' ? 'success' : job.state === 'failed' || job.state === 'denied' || job.state === 'timed_out' ? 'failure' : job.state === 'outcome_unknown' ? 'uncertain' : job.state,
      output, exitCode: job.result?.exit_code, truncated: job.result?.truncation?.truncated,
      artifacts: job.result ? <ToolArtifacts result={job.result}/> : undefined }}/>
    {job.result?.managed_output && <details><summary>{tx('common:activity.retained', { type: job.result.managed_output.type })}</summary>
      {'locator' in job.result.managed_output && <p className={css.identity}>{job.result.managed_output.locator}</p>}
      {'diagnostic' in job.result.managed_output && <p>{detail(job.result.managed_output.diagnostic)}</p>}
    </details>}
    {controls.client && <div className={css.controls}>
      <Button size="sm" variant="outline" disabled={request.disabled} onClick={() => void request.run(async (client, target, current) => { await client.request({ method: 'job/status', params: { target, job_id: job.job_id } }, 'job', undefined, current); })}>{tx('common:activity.status')}</Button>
      <Button size="sm" variant="outline" disabled={waitRequest.disabled || !active} onClick={() => void waitRequest.run(async (client, target, current) => { await client.request({ method: 'job/wait', params: { target, job_id: job.job_id } }, 'job', undefined, current); })}>{tx('common:activity.wait-job')}</Button>
      <Button size="sm" variant="outline" disabled={request.disabled || !active || job.state === 'cancelling'} onClick={() => void request.run(async (client, target, current) => { await client.request({ method: 'job/cancel', params: { target, job_id: job.job_id } }, 'job', undefined, current); })}>{tx('common:activity.cancel-job')}</Button>
      {request.pending && <small role="status">{tx('common:activity.pending')}</small>}
      {waitRequest.pending && <small role="status">{tx('common:activity.settling')}</small>}
      {waitRequest.error && <p role="alert">{displayText(tx, waitRequest.error)}</p>}
      {request.error && <p role="alert">{displayText(tx, request.error)}</p>}
    </div>}
  </section>;
}

export function WorkflowCard({ run }: { run: WorkflowRunView }) {
  const tx = useTranslation();
  return <section data-workflow-run-id={workflowKey(run.id)} aria-label={tx('common:activity.workflow-label', { id: run.workflow_id })}><SettingsCard title={tx('common:activity.workflow-title', { id: run.workflow_id })} meta={<Badge>{run.state.type === 'settled' ? tx(`common:state.${run.state.outcome}`) : tx(`common:state.${run.state.type}`)}</Badge>}>
    <p>{tx('common:activity.workflow-counts', { used: run.steps_consumed, max: run.steps_max, agents: run.agents_consumed })}</p>
    {run.state.type === 'waiting' && <p>{tx('common:activity.waiting-for', { state: tx(`common:state.${run.state.reason}`) })}</p>}
    {run.instances.filter(instance => instance.child).map(instance => <p key={JSON.stringify([instance.block, instance.node, instance.visit])} data-workflow-activation-id={instance.child} className={css.identity}>
      {instance.node ?? tx('common:activity.agent-step')} · {tx('common:activity.activation', { id: instance.child! })} · {instance.state.type === 'settled' ? tx(`common:state.${instance.state.outcome}`) : tx(`common:state.${instance.state.type}`)}
    </p>)}
    {run.omitted_instances > 0 && <small>{tx('common:activity.omitted', { n: run.omitted_instances })}</small>}
  </SettingsCard></section>;
}

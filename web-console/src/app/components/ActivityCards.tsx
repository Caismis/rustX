import { displayText, message as uiMessage, type DisplayText } from '../../locale/translation';
import { useTranslation } from '../../locale/react';
import { useEffect, useState } from 'react';
import type { RuntimeClientAgent, RuntimeClientJob, RuntimeClientTranscriptPage, MethodResult, WorkflowRunView, WorkflowRunId } from '../../../../protocol/app-server/v26';
import { AppServerClient, RpcFailure, sameTarget } from '../../client/app-server';
import { json } from '../../bindings/projection';
import { Badge, SettingsCard } from '../../presentation/settings/SettingsContent';
import { Button } from '../../presentation/primitives/Button';
import { Input } from '../../presentation/primitives/Input';
import { ToolCard } from '../../presentation/agent/ToolCard';
import { ArtifactContext, ToolArtifacts } from './Artifact';
import { PreviewContext } from './ArtifactPreview';
import css from './ActivityCards.module.css';
import { Message } from '../agent/Message';

const detail = (value: string) => value.length > 1024 ? `${value.slice(0, 1024)}…` : value;
export const workflowKey = (id: WorkflowRunId) => JSON.stringify([id.conversation_id, id.attempt_id, id.invocation]);
type AgentWait = Extract<MethodResult, { type: 'agent_wait' }>;
type Controls = { client?: AppServerClient; sessionId?: string };

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
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<DisplayText>();
  const view = sessionId ? client?.getSnapshot().views[sessionId] : undefined;
  const enabled = !!client && !!sessionId && view?.attachment === 'attached' && view.attachmentIntent === 'wanted' && !view.deleting;
  async function run(operation: (client: AppServerClient, target: ReturnType<AppServerClient['target']>, current: () => boolean) => Promise<void>) {
    if (!enabled || pending || !client || !sessionId) return;
    const target = client.target(sessionId), generation = client.getSnapshot().generation;
    const current = () => client.getSnapshot().generation === generation && sameTarget(client.getSnapshot().views[sessionId]?.target, target);
    setPending(true); setError(undefined);
    try { await operation(client, target, current); if (current()) await client.refresh(sessionId); }
    catch (cause) { if (current()) setError(activityFailure(cause)); }
    finally { setPending(false); }
  }
  return { run, disabled: !enabled || pending, pending, error };
}

/** Durable identity is the React key; the selected transcript survives resume. */
export function AgentCard({ agent, ...controls }: { agent: RuntimeClientAgent } & Controls) {
  const tx = useTranslation();
  const request = useActivityRequest(controls);
  const waitRequest = useActivityRequest(controls);
  const interruptRequest = useActivityRequest(controls);
  const [settledActivation, setSettledActivation] = useState<Pick<AgentWait, 'activation_id' | 'outcome'>>();
  const observeSettlement = (result: AgentWait) => setSettledActivation({ activation_id: result.activation_id, outcome: result.outcome });
  const [message, setMessage] = useState('');
  const [transcript, setTranscript] = useState<RuntimeClientTranscriptPage>();
  const [open, setOpen] = useState(false);
  const [transcriptRefresh, refreshTranscript] = useState(0);
  const [transcriptLoading, setTranscriptLoading] = useState(false);
  const [transcriptError, setTranscriptError] = useState<string>();
  const { client, sessionId } = controls;
  // A selected child view refreshes canonical content when native activation
  // facts change. It never turns a transcript read into a lifecycle decision.
  useEffect(() => {
    if (!open || !client || !sessionId) return;
    const view = client.getSnapshot().views[sessionId];
    if (view?.attachment !== 'attached' || !view.target) return;
    const target = view.target, generation = client.getSnapshot().generation;
    let observing = true;
    const current = () => observing && client.getSnapshot().generation === generation && sameTarget(client.getSnapshot().views[sessionId]?.target, target);
    setTranscriptLoading(true); setTranscriptError(undefined);
    void client.request({ method: 'agent/transcript', params: { target, agent_id: agent.agent_id, limit: 64 } }, 'transcript')
      .then(result => { if (current()) setTranscript(result.page); })
      .catch(cause => { if (current()) setTranscriptError(cause instanceof Error ? cause.message : String(cause)); })
      .finally(() => { if (current()) setTranscriptLoading(false); });
    return () => { observing = false; };
  }, [open, client, sessionId, agent.agent_id, agent.activation_id, agent.state, transcriptRefresh]);
  const activity = agent.observation.activity;
  const unavailable = agent.state === 'unavailable';
  const acceptsMessage = agent.state === 'active' || agent.state === 'inactive';
  const readTranscript = (before?: string) => request.run(async (client, target, current) => {
    const result = await client.request({ method: 'agent/transcript', params: { target, agent_id: agent.agent_id, before, limit: 64 } }, 'transcript');
    if (current()) { setTranscript(previous => before && previous ? { ...result.page, entries: [...(result.page.entries ?? []), ...(previous.entries ?? [])] } : result.page); setOpen(true); }
  });
  return <section data-agent-id={agent.agent_id} data-agent-state={agent.state} data-activation-id={agent.current_activation ?? undefined} aria-label={tx('common:activity.agent-label', { name: agent.agent })}>
    <SettingsCard title={tx('common:activity.agent-title', { name: agent.agent })} meta={<Badge>{agent.state === 'active' ? tx('common:activity.working') : agent.state === 'admitting' ? tx('common:activity.admitting') : agent.state === 'stopping' ? tx('common:activity.stopping') : unavailable ? tx('common:activity.unavailable') : tx('common:activity.inactive')}</Badge>}>
      <small className={css.identity}>{tx('common:activity.identity', { agent: agent.agent_id, parent: agent.parent_agent_id, conversation: agent.child_conversation_id })}</small>
      <p>{unavailable ? tx('common:activity.unresolved') : agent.state === 'inactive' ? tx('common:activity.last-state', { state: tx(`common:state.${agent.activation_state}`) }) : agent.state === 'admitting' ? tx('common:activity.preparing') : activity.type === 'waiting' ? tx('common:activity.waiting-for', { state: tx(`common:state.${activity.on.type}`) }) : tx(`common:state.${activity.type}`)}
        {agent.state === 'active' && (activity.type === 'model' || activity.type === 'retrying_model') && activity.retry > 0 && <> · {tx('common:activity.retry', { n: activity.retry })}</>}
      </p>
      <small className={css.identity}>{agent.current_activation ? tx('common:activity.activation', { id: agent.current_activation }) : tx('common:activity.last-activation', { id: agent.activation_id })}</small>
      {agent.detail && <p>{detail(agent.detail)}</p>}
      {controls.client && <>
        <div className={css.controls}>
          <Button size="sm" variant="outline" disabled={request.disabled} onClick={() => { setOpen(true); refreshTranscript(value => value + 1); }}>{tx('common:activity.transcript')}</Button>
          <Button size="sm" variant="outline" disabled={waitRequest.disabled || unavailable} onClick={() => void waitRequest.run(async (client, target, current) => { const result = await client.request({ method: 'agent/wait', params: { target, agent_id: agent.agent_id } }, 'agent_wait'); if (current()) observeSettlement(result); })}>{tx('common:activity.wait-activation')}</Button>
          <Button size="sm" variant="outline" disabled={interruptRequest.disabled || unavailable || agent.state === 'inactive'} onClick={() => void interruptRequest.run(async (client, target, current) => { const result = await client.request({ method: 'agent/interrupt', params: { target, agent_id: agent.agent_id } }, 'agent_wait'); if (current()) observeSettlement(result); })}>{tx('common:activity.interrupt')}</Button>
        </div>
        <form className={css.message} onSubmit={event => { event.preventDefault(); if (!acceptsMessage || !message.trim()) return; const submitted = message; void request.run(async (client, target, current) => { await client.request({ method: 'agent/sendMessage', params: { target, agent_id: agent.agent_id, message: submitted } }, 'agent_message'); if (current()) setMessage(value => value === submitted ? '' : value); }); }}>
          <Input aria-label={tx('common:activity.message-label', { name: agent.agent })} placeholder={agent.state === 'inactive' ? tx('common:activity.resume') : tx('common:activity.message')} value={message} onChange={event => setMessage(event.target.value)} disabled={request.disabled || !acceptsMessage}/>
          <Button size="sm" type="submit" disabled={request.disabled || !acceptsMessage || !message.trim()}>{tx('common:activity.send')}</Button>
        </form>
        {(request.pending || waitRequest.pending || interruptRequest.pending) && <small role="status">{tx('common:activity.waiting-runtime')}</small>}
        {waitRequest.error && <p role="alert">{displayText(tx, waitRequest.error)}</p>}
        {interruptRequest.error && <p role="alert">{displayText(tx, interruptRequest.error)}</p>}
        {settledActivation && <small role="status">{settledActivation.activation_id == null ? tx('common:activity.observed-inactive') : tx('common:activity.activation-result', { id: settledActivation.activation_id, outcome: settledActivation.outcome ? tx(`common:state.${settledActivation.outcome}`) : tx('common:activity.admission-ended') })}</small>}
        {request.error && <p role="alert">{displayText(tx, request.error)}</p>}
      </>}
      {transcriptLoading && <small role="status">{tx('common:activity.reading')}</small>}
      {transcriptError && <p role="alert">{transcriptError}</p>}
      {transcript && <details className={css.transcript} open={open} onToggle={event => setOpen(event.currentTarget.open)}><summary>{tx('common:activity.child-conversation')}</summary>
        {transcript.next_cursor && <Button size="sm" disabled={request.disabled} onClick={() => void readTranscript(transcript.next_cursor!)}>{tx('common:activity.older')}</Button>}
        <ArtifactContext.Provider value={undefined}><PreviewContext.Provider value={undefined}>{(transcript.entries ?? []).map(entry => entry.item.type === 'message' ? <Message key={entry.cursor} message={entry.item.message} tools={entry.tool_calls ?? []}/> : null)}</PreviewContext.Provider></ArtifactContext.Provider>
        {!transcript.entries?.length && <p>{tx('common:activity.empty')}</p>}
      </details>}
    </SettingsCard>
  </section>;
}

export function JobCard({ job, ...controls }: { job: RuntimeClientJob } & Controls) {
  const tx = useTranslation();
  const request = useActivityRequest(controls);
  const waitRequest = useActivityRequest(controls);
  const output = job.result ? (job.result.content ?? []).flatMap(block => block.type === 'text' ? [block.text] : block.type === 'json' ? [block.value && typeof block.value === 'object' && !Array.isArray(block.value) && typeof block.value.combined === 'string' ? block.value.combined : json(block.value)] : []).join('\n') : job.progress ? [job.progress.message, job.progress.completed == null ? undefined : job.progress.total == null ? tx('common:activity.completed', { n: job.progress.completed }) : `${job.progress.completed} / ${job.progress.total}`].filter(Boolean).join('\n') : undefined;
  const active = job.state === 'starting' || job.state === 'running' || job.state === 'cancelling' || job.state === 'publishing_terminal';
  return <section data-job-state={job.state} aria-label={tx('common:activity.job-label', { name: job.tool_name })}>
    <ToolCard tool={{ id: job.job_id, identity: 'job', title: job.tool_name, variant: job.tool_id === 'tool-bash' ? 'bash' : 'generic', summary: tx('common:activity.job-title', { id: job.job_id }),
      state: job.state === 'succeeded' ? 'success' : job.state === 'failed' || job.state === 'denied' || job.state === 'timed_out' ? 'failure' : job.state === 'outcome_unknown' ? 'uncertain' : job.state,
      output, exitCode: job.result?.exit_code, truncated: job.result?.truncation?.truncated,
      artifacts: job.result ? <ToolArtifacts result={job.result}/> : undefined }}/>
    {job.result?.managed_output && <details><summary>{tx('common:activity.retained', { type: job.result.managed_output.type })}</summary>
      {'locator' in job.result.managed_output && <p className={css.identity}>{job.result.managed_output.locator}</p>}
      {'diagnostic' in job.result.managed_output && <p>{detail(job.result.managed_output.diagnostic)}</p>}
    </details>}
    {controls.client && <div className={css.controls}>
      <Button size="sm" variant="outline" disabled={request.disabled} onClick={() => void request.run(async (client, target) => { await client.request({ method: 'job/status', params: { target, job_id: job.job_id } }, 'job'); })}>{tx('common:activity.status')}</Button>
      <Button size="sm" variant="outline" disabled={waitRequest.disabled || !active} onClick={() => void waitRequest.run(async (client, target) => { await client.request({ method: 'job/wait', params: { target, job_id: job.job_id } }, 'job'); })}>{tx('common:activity.wait-job')}</Button>
      <Button size="sm" variant="outline" disabled={request.disabled || !active || job.state === 'cancelling'} onClick={() => void request.run(async (client, target) => { await client.request({ method: 'job/cancel', params: { target, job_id: job.job_id } }, 'job'); })}>{tx('common:activity.cancel-job')}</Button>
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

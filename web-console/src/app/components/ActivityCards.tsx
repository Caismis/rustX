import { useEffect, useState } from 'react';
import type { RuntimeClientAgent, RuntimeClientJob, RuntimeClientTranscriptPage, WorkflowRunView, WorkflowRunId } from '../../../../protocol/app-server/v22';
import { AppServerClient, sameTarget } from '../../client/app-server';
import { json } from '../../bindings/projection';
import { Badge, SettingsCard } from '../../presentation/settings/SettingsContent';
import { Button } from '../../presentation/primitives/Button';
import { Input } from '../../presentation/primitives/Input';
import { ToolCard } from '../../presentation/agent/ToolCard';
import { ArtifactContext, ToolArtifacts } from './Artifact';
import { PreviewContext } from './ArtifactPreview';
import css from './ActivityCards.module.css';
import { Message } from '../agent/Message';

const label = (value: string) => value.replaceAll('_', ' ');
const detail = (value: string) => value.length > 1024 ? `${value.slice(0, 1024)}…` : value;
export const workflowKey = (id: WorkflowRunId) => JSON.stringify([id.conversation_id, id.attempt_id, id.invocation]);
type Controls = { client?: AppServerClient; sessionId?: string };

/** Request presentation only: native state always comes from the next snapshot.
 * A lost response is never retried, and a changed attachment cannot adopt it. */
function useActivityRequest({ client, sessionId }: Controls) {
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string>();
  const view = sessionId ? client?.getSnapshot().views[sessionId] : undefined;
  const enabled = !!client && !!sessionId && view?.attachment === 'attached' && view.attachmentIntent === 'wanted' && !view.deleting;
  async function run(operation: (client: AppServerClient, target: ReturnType<AppServerClient['target']>, current: () => boolean) => Promise<void>) {
    if (!enabled || pending || !client || !sessionId) return;
    const target = client.target(sessionId), generation = client.getSnapshot().generation;
    const current = () => client.getSnapshot().generation === generation && sameTarget(client.getSnapshot().views[sessionId]?.target, target);
    setPending(true); setError(undefined);
    try { await operation(client, target, current); if (current()) await client.refresh(sessionId); }
    catch (cause) { if (current()) setError(cause instanceof Error ? cause.message : String(cause)); }
    finally { setPending(false); }
  }
  return { run, disabled: !enabled || pending, pending, error };
}

/** Durable identity is the React key; the selected transcript survives resume. */
export function AgentCard({ agent, ...controls }: { agent: RuntimeClientAgent } & Controls) {
  const request = useActivityRequest(controls);
  const waitRequest = useActivityRequest(controls);
  const [waitedActivation, setWaitedActivation] = useState<string | null>();
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
  const readTranscript = (before?: string) => request.run(async (client, target, current) => {
    const result = await client.request({ method: 'agent/transcript', params: { target, agent_id: agent.agent_id, before, limit: 64 } }, 'transcript');
    if (current()) { setTranscript(previous => before && previous ? { ...result.page, entries: [...(result.page.entries ?? []), ...(previous.entries ?? [])] } : result.page); setOpen(true); }
  });
  return <section data-agent-id={agent.agent_id} data-agent-state={agent.state} data-activation-id={agent.current_activation ?? undefined} aria-label={`Agent ${agent.agent}`}>
    <SettingsCard title={`Agent · ${agent.agent}`} meta={<Badge>{agent.state === 'active' ? 'Working' : agent.state === 'stopping' ? 'Stopping…' : 'Inactive'}</Badge>}>
      <small className={css.identity}>Agent {agent.agent_id} · Parent {agent.parent_agent_id} · Conversation {agent.child_conversation_id}</small>
      <p>{agent.state === 'inactive' ? `Last activation: ${label(agent.activation_state)}` : activity.type === 'waiting' ? `Waiting for ${label(activity.on.type)}` : label(activity.type)}
        {(activity.type === 'model' || activity.type === 'retrying_model') && activity.retry > 0 && <> · Retry {activity.retry}</>}
      </p>
      <small className={css.identity}>{agent.current_activation ? `Activation ${agent.current_activation}` : `Last activation ${agent.activation_id}`}</small>
      {agent.detail && <p>{detail(agent.detail)}</p>}
      {controls.client && <>
        <div className={css.controls}>
          <Button size="sm" variant="outline" disabled={request.disabled} onClick={() => { setOpen(true); refreshTranscript(value => value + 1); }}>Transcript</Button>
          <Button size="sm" variant="outline" disabled={waitRequest.disabled} onClick={() => void waitRequest.run(async (client, target, current) => { const result = await client.request({ method: 'agent/wait', params: { target, agent_id: agent.agent_id } }, 'agent_wait'); if (current()) setWaitedActivation(result.activation_id ?? null); })}>Wait for activation</Button>
          <Button size="sm" variant="outline" disabled={request.disabled || agent.state !== 'active'} onClick={() => void request.run(async (client, target) => { await client.request({ method: 'agent/interrupt', params: { target, agent_id: agent.agent_id } }, 'agent'); })}>Interrupt</Button>
        </div>
        <form className={css.message} onSubmit={event => { event.preventDefault(); if (!message.trim()) return; const submitted = message; void request.run(async (client, target, current) => { await client.request({ method: 'agent/sendMessage', params: { target, agent_id: agent.agent_id, message: submitted } }, 'agent_message'); if (current()) setMessage(value => value === submitted ? '' : value); }); }}>
          <Input aria-label={`Message Agent ${agent.agent}`} placeholder={agent.state === 'inactive' ? 'Send a message to resume' : 'Message this Agent'} value={message} onChange={event => setMessage(event.target.value)} disabled={request.disabled || agent.state === 'stopping'}/>
          <Button size="sm" type="submit" disabled={request.disabled || agent.state === 'stopping' || !message.trim()}>Send message</Button>
        </form>
        {(request.pending || waitRequest.pending) && <small role="status">Waiting for runtime…</small>}
        {waitRequest.error && <p role="alert">{waitRequest.error}</p>}
        {waitedActivation !== undefined && <small role="status">{waitedActivation === null ? 'Agent was inactive when Wait observed it.' : `Activation ${waitedActivation} settled.`}</small>}
        {request.error && <p role="alert">{request.error}</p>}
      </>}
      {transcriptLoading && <small role="status">Reading child conversation…</small>}
      {transcriptError && <p role="alert">{transcriptError}</p>}
      {transcript && <details className={css.transcript} open={open} onToggle={event => setOpen(event.currentTarget.open)}><summary>Child conversation</summary>
        {transcript.next_cursor && <Button size="sm" disabled={request.disabled} onClick={() => void readTranscript(transcript.next_cursor!)}>Older messages</Button>}
        <ArtifactContext.Provider value={undefined}><PreviewContext.Provider value={undefined}>{(transcript.entries ?? []).map(entry => entry.item.type === 'message' ? <Message key={entry.cursor} message={entry.item.message} tools={entry.tool_calls ?? []}/> : null)}</PreviewContext.Provider></ArtifactContext.Provider>
        {!transcript.entries?.length && <p>No committed messages yet.</p>}
      </details>}
    </SettingsCard>
  </section>;
}

export function JobCard({ job, ...controls }: { job: RuntimeClientJob } & Controls) {
  const request = useActivityRequest(controls);
  const waitRequest = useActivityRequest(controls);
  const output = job.result ? (job.result.content ?? []).flatMap(block => block.type === 'text' ? [block.text] : block.type === 'json' ? [block.value && typeof block.value === 'object' && !Array.isArray(block.value) && typeof block.value.combined === 'string' ? block.value.combined : json(block.value)] : []).join('\n') : job.progress ? [job.progress.message, job.progress.completed == null ? undefined : job.progress.total == null ? `${job.progress.completed} completed` : `${job.progress.completed} / ${job.progress.total}`].filter(Boolean).join('\n') : undefined;
  const active = job.state === 'starting' || job.state === 'running' || job.state === 'cancelling' || job.state === 'publishing_terminal';
  return <section data-job-state={job.state} aria-label={`Job ${job.tool_name}`}>
    <ToolCard tool={{ id: job.job_id, identity: 'job', title: job.tool_name, variant: job.tool_id === 'tool-bash' ? 'bash' : 'generic', summary: `Job · ${job.job_id}`,
      state: job.state === 'succeeded' ? 'success' : job.state === 'failed' || job.state === 'denied' || job.state === 'timed_out' ? 'failure' : job.state === 'outcome_unknown' ? 'uncertain' : job.state,
      output, exitCode: job.result?.exit_code, truncated: job.result?.truncation?.truncated,
      artifacts: job.result ? <ToolArtifacts result={job.result}/> : undefined }}/>
    {job.result?.managed_output && <details><summary>Retained output · {job.result.managed_output.type}</summary>
      {'locator' in job.result.managed_output && <p className={css.identity}>{job.result.managed_output.locator}</p>}
      {'diagnostic' in job.result.managed_output && <p>{detail(job.result.managed_output.diagnostic)}</p>}
    </details>}
    {controls.client && <div className={css.controls}>
      <Button size="sm" variant="outline" disabled={request.disabled} onClick={() => void request.run(async (client, target) => { await client.request({ method: 'job/status', params: { target, job_id: job.job_id } }, 'job'); })}>Status</Button>
      <Button size="sm" variant="outline" disabled={waitRequest.disabled || !active} onClick={() => void waitRequest.run(async (client, target) => { await client.request({ method: 'job/wait', params: { target, job_id: job.job_id } }, 'job'); })}>Wait for Job</Button>
      <Button size="sm" variant="outline" disabled={request.disabled || !active || job.state === 'cancelling'} onClick={() => void request.run(async (client, target) => { await client.request({ method: 'job/cancel', params: { target, job_id: job.job_id } }, 'job'); })}>Cancel Job</Button>
      {request.pending && <small role="status">Request pending…</small>}
      {waitRequest.pending && <small role="status">Waiting for settlement…</small>}
      {waitRequest.error && <p role="alert">{waitRequest.error}</p>}
      {request.error && <p role="alert">{request.error}</p>}
    </div>}
  </section>;
}

export function WorkflowCard({ run }: { run: WorkflowRunView }) {
  return <section data-workflow-run-id={workflowKey(run.id)} aria-label={`Workflow ${run.workflow_id}`}><SettingsCard title={`Workflow · ${run.workflow_id}`} meta={<Badge>{run.state.type === 'settled' ? label(run.state.outcome) : label(run.state.type)}</Badge>}>
    <p>Steps {run.steps_consumed} / {run.steps_max} · Agents {run.agents_consumed}</p>
    {run.state.type === 'waiting' && <p>Waiting for {label(run.state.reason)}</p>}
    {run.instances.filter(instance => instance.child).map(instance => <p key={JSON.stringify([instance.block, instance.node, instance.visit])} data-workflow-activation-id={instance.child} className={css.identity}>
      {instance.node ?? 'Agent step'} · Activation {instance.child} · {instance.state.type === 'settled' ? label(instance.state.outcome) : label(instance.state.type)}
    </p>)}
    {run.omitted_instances > 0 && <small>{run.omitted_instances} older instances omitted</small>}
  </SettingsCard></section>;
}

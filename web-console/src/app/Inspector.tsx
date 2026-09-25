import { SettingsCard, Facts } from '../presentation/settings/SettingsContent';
import css from '../presentation/settings/SettingsContent.module.css';
import { useState, useSyncExternalStore } from 'react';
import type { ClientView, SessionView } from '../client/app-server';
import { filterLog, type ProtocolLog } from '../client/protocol-log';
import { json } from '../bindings/projection';
import { Button } from '../presentation/primitives/Button';
import { Input } from '../presentation/primitives/Input';
export function Inspector({ log: protocolLog, state, view }: { log: ProtocolLog; state: ClientView; view?: SessionView }) {
  const log = useSyncExternalStore(protocolLog.subscribe, protocolLog.getSnapshot);
  const [method, setMethod] = useState('');
  const [session, setSession] = useState('');
  const [kind, setKind] = useState('');
  const [copyState, setCopyState] = useState('');
  const entries = filterLog(log, method, session, kind);
  const snapshot = view?.snapshot;
  const facts = {
    SessionId: view?.id, ConversationId: view?.target?.conversation_id ?? snapshot?.conversation_id,
    runtime_incarnation: view?.target?.runtime_incarnation,
    connection_generation: state.generation, connection: state.connection,
    attachment_intent: view?.attachmentIntent, attachment: view?.attachment, attachment_id: view?.target?.attachment_id,
    residency: view?.attachment === 'attached' || view?.attachment === 'resynchronizing' ? 'loaded'
      : 'not currently observed',
    cursor: view?.cursor, cwd: view?.settings?.cwd,
    attempt: snapshot?.attempt, pending_interactions: snapshot?.pending_interactions,
    job_count: snapshot?.jobs?.length ?? 0, agent_count: snapshot?.agents?.length ?? 0,
    workflows: snapshot?.workflows, model: snapshot?.model,
    settings_evidence: snapshot?.settings_evidence,
    resource_revision: snapshot?.resources?.revision, capability_revision: snapshot?.capabilities.revision,
    approval_mode: snapshot?.effective_approval_mode,
    shutting_down: snapshot?.shutting_down, durability_failure: snapshot?.durability_failure,
    cancellation: view?.cancellation, inbound: snapshot?.inbound, submissions: view?.submissions,
    inbound_requests: view?.inboundRequests, interactions: Object.fromEntries(Object.entries(state.interactionOperations).filter(([, operation]) => view && operation.sessionId === view.id)),
    uncertain_operations: state.uncertain.filter(operation => view && operation.sessionId === view.id), model_mutation: view?.modelMutation,
    goal: snapshot?.goal, todos: snapshot?.todos, statuses: snapshot?.statuses,
    jobs: snapshot?.jobs, agents: snapshot?.agents,
    trace: view?.trace,
    connection_error: state.error, session_error: view?.error,
  };
  return <div className={`inspector ${css.settings}`}>
    <div className="eyebrow">RUSTX / INSPECTOR</div>
    <h2>Runtime facts</h2>
    <p className="muted">Read from the selected Session. Stale values describe the last observation.</p>
    <section aria-label="Selected Session diagnostics">
    <SettingsCard title="Identity"><Facts rows={[["Session ID", view?.id], ["Conversation ID", facts.ConversationId], ["cwd", facts.cwd]]} /></SettingsCard>
    <SettingsCard title="Execution"><Facts rows={[["Attempt ID", snapshot?.attempt?.attempt_id], ["Exact phase", snapshot?.attempt?.phase.type], ["Exact outcome", snapshot?.attempt?.phase.type === 'settled' ? snapshot.attempt.phase.outcome.type : undefined], ["Cancellation request", view?.cancellation?.status]]} />
      <details><summary>Attempt and cancellation evidence</summary><pre>{json({ attempt: snapshot?.attempt, cancellation: view?.cancellation })}</pre></details>
      <details><summary>Jobs, Agents and Workflows</summary><pre>{json({ jobs: facts.jobs, agents: facts.agents, workflows: facts.workflows })}</pre></details>
      {/* `statuses` is the runtime's bounded window of past compositions, oldest
          first — not one current Agent Status value, and not current Todo, Goal or
          Queue state. The raw facts, including each composition's `rendered` text,
          stay here as diagnostics. */}
      <details><summary>Agent Status history · recent compositions</summary><p className="muted">Bounded history of past Agent Status compositions, oldest first. Each is a historical request-scoped fact anchored in the transcript, not current Agent, Todo, Goal or Queue state.</p><pre>{json({ statuses: facts.statuses })}</pre></details>
      <details><summary>Trace and Request Snapshot identities</summary><pre>{json(facts.trace)}</pre></details>
    </SettingsCard>
    <SettingsCard title="Attachment / residency"><Facts rows={[["Attachment intent", facts.attachment_intent], ["Observed attachment", facts.attachment], ["Attachment ID", facts.attachment_id], ["Runtime incarnation", facts.runtime_incarnation], ["Observed residency", facts.residency]]} /></SettingsCard>
    <SettingsCard title="Inbound / interactions"><details><summary>Mailbox and pending interactions</summary><pre>{json({ inbound: facts.inbound, submissions: facts.submissions, inbound_requests: facts.inbound_requests, pending_interactions: facts.pending_interactions, interaction_operations: facts.interactions })}</pre></details><details><summary>Todo and Goal revisions</summary><pre>{json({ goal: facts.goal, todos: facts.todos })}</pre></details></SettingsCard>
    <SettingsCard title="Configuration / revisions"><Facts rows={[["Resource revision", facts.resource_revision], ["Capability revision", facts.capability_revision], ["Approval mode", facts.approval_mode]]} /><details><summary>Settings evidence</summary><pre>{json({ settings: facts.settings_evidence, model: facts.model, mutation: facts.model_mutation })}</pre></details></SettingsCard>
    <SettingsCard title="Recovery / uncertainty"><Facts rows={[["Shutting down", String(facts.shutting_down ?? 'unobserved')], ["Durability failure", json(facts.durability_failure)]]} /><details><summary>Uncertain operations and reconciliation evidence</summary><pre>{json({ uncertain: facts.uncertain_operations, cancellation: facts.cancellation, model_mutation: facts.model_mutation, interactions: facts.interactions, connection_error: facts.connection_error, session_error: facts.session_error })}</pre></details></SettingsCard>
    <SettingsCard title="Protocol"><Facts rows={[["Connection", state.connection], ["Connection generation", state.generation], ["Cursor", facts.cursor]]} /><details><summary>Complete native runtime facts</summary><pre aria-label="Native diagnostic JSON">{json(facts)}</pre></details></SettingsCard>
    <details><summary>Server capabilities</summary><pre>{json(state.capabilities)}</pre></details>
    <details><summary>Explicit Session selections</summary><pre>{json(view?.settings)}</pre></details>
    </section>
    <section aria-label="Global / other Session diagnostics"><h2>Global / other Session diagnostics</h2>
      <p className="muted">Unscoped operations and evidence belonging to other Sessions, not the selected Session.</p>
      <pre>{json({ uncertain: state.uncertain.filter(operation => !view || operation.sessionId !== view.id), interactions: Object.fromEntries(Object.entries(state.interactionOperations).filter(([, operation]) => !view || operation.sessionId !== view.id)) })}</pre>
    </section>
    <h2>Wire protocol · all Sessions</h2>
    <p className="muted">Actual JSON-RPC frames · before view adaptation</p>
    <div className="log-controls">
      <Input aria-label="Method filter" placeholder="Method filter" value={method} onChange={event => setMethod(event.target.value)} />
      <Input aria-label="Session filter" placeholder="SessionId filter" value={session} onChange={event => setSession(event.target.value)} />
      <select aria-label="Message kind filter" value={kind} onChange={event => setKind(event.target.value)}><option value="">All kinds</option><option>request</option><option>response</option><option>notification</option><option>invalid</option></select>
      <div className="row">
        <Button size="sm" variant="outline" onClick={() => protocolLog.pause(!log.paused)}>{log.paused ? 'Resume log' : 'Pause log'}</Button>
        <Button size="sm" variant="outline" onClick={() => protocolLog.clear()}>Clear log</Button>
        <Button size="sm" variant="outline" onClick={() => {
          if (!navigator.clipboard) { setCopyState('Copy failed: clipboard unavailable'); return; }
          void navigator.clipboard.writeText(json(entries)).then(() => setCopyState('Copied JSON'), () => setCopyState('Copy failed: clipboard unavailable'));
        }}>Copy JSON</Button>
      </div>
    </div>
    <p className="muted" role="status">{entries.length} shown · {log.dropped} dropped · {log.truncated} truncated · {log.paused ? 'view frozen; protocol continues' : 'live'} {copyState}</p>
    <p className="muted">300 entries / 1 MiB text budget / 32 KiB per entry. Truncated entries contain only a raw prefix.</p>
    <div className="protocol-log">{[...entries].reverse().map(entry => <details key={entry.sequence}>
      <summary><span className="wire-direction">{entry.direction === 'out' ? '↑' : '↓'}</span> {entry.kind} · {entry.method ?? 'uncorrelated'} <small>g{entry.generation} #{entry.sequence}</small></summary>
      {entry.sessionId && <small>{entry.sessionId}</small>}
      <pre>{entry.json}</pre>{entry.truncated && <strong>Truncated raw frame</strong>}
    </details>)}</div>
  </div>;
}

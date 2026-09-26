import { message } from '../locale/translation';
import { useTranslation, useNotice } from '../locale/react';
import { SettingsCard, Facts } from '../presentation/settings/SettingsContent';
import css from '../presentation/settings/SettingsContent.module.css';
import { useState, useSyncExternalStore } from 'react';
import { useClientSelector, selectClient } from '../client/selectors';
import type { AppServerClient, ClientView, SessionView } from '../client/app-server';
import { filterLog, type ProtocolLog } from '../client/protocol-log';
import { json } from '../bindings/projection';
import { Button } from '../presentation/primitives/Button';
import { Input } from '../presentation/primitives/Input';
export function LiveInspector({ client, sessionId }: { client: AppServerClient; sessionId?: string }) {
  const state = useClientSelector(client, selectClient);
  return <Inspector log={client.log} state={state} view={sessionId ? state.views[sessionId] : undefined}/>;
}
export function Inspector({ log: protocolLog, state, view }: { log: ProtocolLog; state: ClientView; view?: SessionView }) {
  const tx = useTranslation();
  const log = useSyncExternalStore(protocolLog.subscribe, protocolLog.getSnapshot);
  const [method, setMethod] = useState('');
  const [session, setSession] = useState('');
  const [kind, setKind] = useState('');
  const [copyState, setCopyState] = useNotice();
  const entries = filterLog(log, method, session, kind);
  const snapshot = view?.snapshot;
  const facts = {
    SessionId: view?.id, ConversationId: view?.target?.conversation_id ?? snapshot?.conversation_id,
    runtime_incarnation: view?.target?.runtime_incarnation,
    connection_generation: state.generation, connection: state.connection,
    attachment_intent: view?.attachmentIntent, attachment: view?.attachment, attachment_id: view?.target?.attachment_id,
    residency: view?.attachment === 'attached' || view?.attachment === 'resynchronizing' ? 'loaded'
      : tx('inspector:missing'),
    cursor: view?.cursor, cwd: view?.settings?.cwd,
    attempt: snapshot?.attempt, pending_interactions: snapshot?.pending_interactions,
    background_count: snapshot?.background?.length ?? 0, subagent_count: snapshot?.subagents?.length ?? 0,
    workflows: snapshot?.workflows, model: snapshot?.model,
    settings_evidence: snapshot?.settings_evidence,
    resource_revision: snapshot?.resources?.revision, capability_revision: snapshot?.capabilities.revision,
    approval_mode: snapshot?.effective_approval_mode,
    shutting_down: snapshot?.shutting_down, durability_failure: snapshot?.durability_failure,
    cancellation: view?.cancellation, inbound: snapshot?.inbound, submissions: view?.submissions,
    inbound_requests: view?.inboundRequests, interactions: Object.fromEntries(Object.entries(state.interactionOperations).filter(([, operation]) => view && operation.sessionId === view.id)),
    uncertain_operations: state.uncertain.filter(operation => view && operation.sessionId === view.id), model_mutation: view?.modelMutation,
    goal: snapshot?.goal, todos: snapshot?.todos, statuses: snapshot?.statuses,
    background: snapshot?.background, subagents: snapshot?.subagents,
    trace: view?.trace,
    connection_error: state.error, session_error: view?.error,
  };
  return <div className={`inspector ${css.settings}`}>
    <div className="eyebrow">{tx('inspector:inspector.rustx-inspector')}</div>
    <h2>{tx('inspector:inspector.runtime-facts')}</h2>
    <p className="muted">{tx('inspector:inspector.read-from-the-selected-session-stale-values-describe-the-last-ob')}</p>
    <section aria-label={tx('inspector:inspector.selected-session-diagnostics')}>
    <SettingsCard title={tx('inspector:inspector.identity')}><Facts rows={[[tx('inspector:copy.session-id'), view?.id], [tx('inspector:copy.conversation-id'), facts.ConversationId], ["cwd", facts.cwd]]} /></SettingsCard>
    <SettingsCard title={tx('inspector:inspector.execution')}><Facts rows={[[tx('inspector:copy.attempt-id'), snapshot?.attempt?.attempt_id], [tx('inspector:copy.exact-phase'), snapshot?.attempt?.phase.type], [tx('inspector:copy.exact-outcome'), snapshot?.attempt?.phase.type === 'settled' ? snapshot.attempt.phase.outcome.type : undefined], [tx('inspector:copy.cancellation-request'), view?.cancellation?.status]]} />
      <details><summary>{tx('inspector:inspector.attempt-and-cancellation-evidence')}</summary><pre>{json({ attempt: snapshot?.attempt, cancellation: view?.cancellation })}</pre></details>
      <details><summary>{tx('inspector:copy.tools-subagents-and-workflows')}</summary><pre>{json({ background: facts.background, subagents: facts.subagents, workflows: facts.workflows })}</pre></details>
      {/* `statuses` is the runtime's bounded window of past compositions, oldest
          first — not one current Agent Status value, and not current Todo, Goal or
          Queue state. The raw facts, including each composition's `rendered` text,
          stay here as diagnostics. */}
      <details><summary>{tx('inspector:inspector.agent-status-history-recent-compositions')}</summary><p className="muted">{tx('inspector:inspector.bounded-history-of-past-agent-status-compositions-oldest-first-e')}</p><pre>{json({ statuses: facts.statuses })}</pre></details>
      <details><summary>{tx('inspector:inspector.trace-and-request-snapshot-identities')}</summary><pre>{json(facts.trace)}</pre></details>
    </SettingsCard>
    <SettingsCard title={tx('inspector:inspector.attachment-residency')}><Facts rows={[[tx('inspector:copy.attachment-intent'), facts.attachment_intent], [tx('inspector:copy.observed-attachment'), facts.attachment], [tx('inspector:copy.attachment-id'), facts.attachment_id], [tx('inspector:copy.runtime-incarnation'), facts.runtime_incarnation], [tx('inspector:copy.observed-residency'), facts.residency]]} /></SettingsCard>
    <SettingsCard title={tx('inspector:inspector.inbound-interactions')}><details><summary>{tx('inspector:inspector.mailbox-and-pending-interactions')}</summary><pre>{json({ inbound: facts.inbound, submissions: facts.submissions, inbound_requests: facts.inbound_requests, pending_interactions: facts.pending_interactions, interaction_operations: facts.interactions })}</pre></details><details><summary>{tx('inspector:inspector.todo-and-goal-revisions')}</summary><pre>{json({ goal: facts.goal, todos: facts.todos })}</pre></details></SettingsCard>
    <SettingsCard title={tx('inspector:inspector.configuration-revisions')}><Facts rows={[[tx('inspector:copy.resource-revision'), facts.resource_revision], [tx('inspector:copy.capability-revision'), facts.capability_revision], [tx('inspector:copy.approval-mode'), facts.approval_mode]]} /><details><summary>{tx('inspector:inspector.settings-evidence')}</summary><pre>{json({ settings: facts.settings_evidence, model: facts.model, mutation: facts.model_mutation })}</pre></details></SettingsCard>
    <SettingsCard title={tx('inspector:inspector.recovery-uncertainty')}><Facts rows={[[tx('inspector:copy.shutting-down'), String(facts.shutting_down ?? 'unobserved')], [tx('inspector:copy.durability-failure'), json(facts.durability_failure)]]} /><details><summary>{tx('inspector:inspector.uncertain-operations-and-reconciliation-evidence')}</summary><pre>{json({ uncertain: facts.uncertain_operations, cancellation: facts.cancellation, model_mutation: facts.model_mutation, interactions: facts.interactions, connection_error: facts.connection_error, session_error: facts.session_error })}</pre></details></SettingsCard>
    <SettingsCard title={tx('inspector:inspector.protocol')}><Facts rows={[[tx('inspector:copy.connection'), state.connection], [tx('inspector:copy.connection-generation'), state.generation], [tx('inspector:copy.cursor'), facts.cursor]]} /><details><summary>{tx('inspector:inspector.complete-native-runtime-facts')}</summary><pre aria-label={tx('inspector:inspector.native-diagnostic-json')}>{json(facts)}</pre></details></SettingsCard>
    <details><summary>{tx('inspector:inspector.server-capabilities')}</summary><pre>{json(state.capabilities)}</pre></details>
    <details><summary>{tx('inspector:inspector.explicit-session-selections')}</summary><pre>{json(view?.settings)}</pre></details>
    </section>
    <section aria-label={tx('inspector:inspector.global-other-session-diagnostics')}><h2>{tx('inspector:inspector.global-other-session-diagnostics')}</h2>
      <p className="muted">{tx('inspector:inspector.unscoped-operations-and-evidence-belonging-to-other-sessions-not')}</p>
      <pre>{json({ uncertain: state.uncertain.filter(operation => !view || operation.sessionId !== view.id), interactions: Object.fromEntries(Object.entries(state.interactionOperations).filter(([, operation]) => !view || operation.sessionId !== view.id)) })}</pre>
    </section>
    <h2>{tx('inspector:inspector.wire-protocol-all-sessions')}</h2>
    <p className="muted">{tx('inspector:inspector.actual-json-rpc-frames-before-view-adaptation')}</p>
    <div className="log-controls">
      <Input aria-label={tx('inspector:inspector.method-filter')} placeholder={tx('inspector:inspector.method-filter')} value={method} onChange={event => setMethod(event.target.value)} />
      <Input aria-label={tx('inspector:inspector.session-filter')} placeholder={tx('inspector:inspector.sessionid-filter')} value={session} onChange={event => setSession(event.target.value)} />
      <select aria-label={tx('inspector:inspector.message-kind-filter')} value={kind} onChange={event => setKind(event.target.value)}><option value="">{tx('inspector:inspector.all-kinds')}</option><option value="request">{tx('inspector:inspector.request')}</option><option value="response">{tx('inspector:inspector.response')}</option><option value="notification">{tx('inspector:inspector.notification')}</option><option value="invalid">{tx('inspector:inspector.invalid')}</option></select>
      <div className="row">
        <Button size="sm" variant="outline" onClick={() => protocolLog.pause(!log.paused)}>{log.paused ? tx('inspector:inspector.resume-log') : tx('inspector:inspector.pause-log')}</Button>
        <Button size="sm" variant="outline" onClick={() => protocolLog.clear()}>{tx('inspector:inspector.clear-log')}</Button>
        <Button size="sm" variant="outline" onClick={() => {
          if (!navigator.clipboard) { setCopyState(message('inspector:copy.copy-failed-clipboard-unavailable')); return; }
          void navigator.clipboard.writeText(json(entries)).then(() => setCopyState(tx('inspector:copy.copied-json')), () => setCopyState(tx('inspector:copy.copy-failed-clipboard-unavailable')));
        }}>{tx('inspector:inspector.copy-json')}</Button>
      </div>
    </div>
    <p className="muted" role="status">{entries.length} {tx('inspector:inspector.shown')}{' '}{log.dropped} {tx('inspector:inspector.dropped')}{' '}{log.truncated} {tx('inspector:inspector.truncated')}{' '}{log.paused ? tx('inspector:inspector.view-frozen-protocol-continues') : tx('inspector:inspector.live')} {copyState}</p>
    <p className="muted">{tx('inspector:inspector.300-entries-1-mib-text-budget-32-kib-per-entry-truncated-entries')}</p>
    <div className="protocol-log">{[...entries].reverse().map(entry => <details key={entry.sequence}>
      <summary><span className="wire-direction">{entry.direction === 'out' ? '↑' : '↓'}</span> {entry.kind} · {entry.method ?? tx('inspector:inspector.uncorrelated')} <small>{tx('inspector:inspector.g')}{entry.generation} #{entry.sequence}</small></summary>
      {entry.sessionId && <small>{entry.sessionId}</small>}
      <pre>{entry.json}</pre>{entry.truncated && <strong>{tx('inspector:inspector.truncated-raw-frame')}</strong>}
    </details>)}</div>
  </div>;
}

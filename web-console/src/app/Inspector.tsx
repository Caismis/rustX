import { useState, useSyncExternalStore } from 'react';
import type { AppServerClient, ClientView, SessionView } from '../client/app-server';
import { filterLog } from '../client/protocol-log';
import { json } from '../bindings/projection';
import { Button } from '../presentation/primitives/Button';
import { Input } from '../presentation/primitives/Input';
export function Inspector({ client, state, view }: { client: AppServerClient; state: ClientView; view?: SessionView }) {
  const log = useSyncExternalStore(client.log.subscribe, client.log.getSnapshot);
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
      : view?.attachment === 'unloaded' ? 'unloaded (acknowledged)' : 'not currently observed',
    cursor: view?.cursor, cwd: view?.settings?.cwd,
    attempt: snapshot?.attempt, pending_interactions: snapshot?.pending_interactions,
    background_count: snapshot?.background?.length ?? 0, subagent_count: snapshot?.subagents?.length ?? 0,
    workflows: snapshot?.workflows, model: snapshot?.model,
    settings_evidence: snapshot?.settings_evidence, launch_settings: snapshot?.launch_settings,
    settings_lifetimes: snapshot?.settings_lifetimes,
    resource_revision: snapshot?.resources?.revision, capability_revision: snapshot?.capabilities.revision,
    approval_mode: snapshot?.effective_approval_mode, pending_approval_mode: snapshot?.pending_approval_mode,
    shutting_down: snapshot?.shutting_down, durability_failure: snapshot?.durability_failure,
  };
  return <div className="inspector">
    <div className="eyebrow">RUSTX / INSPECTOR</div>
    <h2>Runtime facts</h2>
    <p className="muted">Read from the selected Session. Stale values describe the last observation.</p>
    <details open><summary>Identity & execution</summary><pre aria-label="Runtime facts">{json(facts)}</pre></details>
    <details><summary>Server capabilities</summary><pre>{json(state.capabilities)}</pre></details>
    <details><summary>Explicit Session selections</summary><pre>{json(view?.settings)}</pre></details>
    <h2>Wire protocol</h2>
    <p className="muted">Actual JSON-RPC frames · before view adaptation</p>
    <div className="log-controls">
      <Input aria-label="Method filter" placeholder="Method filter" value={method} onChange={event => setMethod(event.target.value)} />
      <Input aria-label="Session filter" placeholder="SessionId filter" value={session} onChange={event => setSession(event.target.value)} />
      <select aria-label="Message kind filter" value={kind} onChange={event => setKind(event.target.value)}><option value="">All kinds</option><option>request</option><option>response</option><option>notification</option><option>invalid</option></select>
      <div className="row">
        <Button size="sm" variant="outline" onClick={() => client.log.pause(!log.paused)}>{log.paused ? 'Resume log' : 'Pause log'}</Button>
        <Button size="sm" variant="outline" onClick={() => client.log.clear()}>Clear log</Button>
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

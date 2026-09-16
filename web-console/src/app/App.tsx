import { Trajectory } from './Trajectory';
import { useEffect, useMemo, useState, useSyncExternalStore } from 'react';
import type { AppServerClient } from '../client/app-server';
import type { RuntimeClientSessionDeletePreview, UserInputBlock } from '../../../protocol/app-server/v4';
import { CommandPanel, type CommandRequest } from './commands/CommandPanel';
import { NavigationEpoch, createSession } from './commands/native';
import { available, commands } from './commands/registry';
import { activeAttempt, lineageSwitchSafe, json } from '../bindings/projection';
import { goalDock, queueRows, todoDock } from '../bindings/composer-context';
import { ComposerContextStack } from './composer/ComposerContextStack';
import { GoalDock } from './composer/GoalDock';
import { QueueDock } from './composer/QueueDock';
import { TodoDock } from './composer/TodoDock';
import { ArtifactResources } from '../client/artifacts';
import { ArtifactContext } from './components/Artifact';
import { ChatViewport } from '../presentation/layout/ChatViewport';
import { AppFrame } from '../presentation/layout/AppFrame';
import { Sidebar } from './components/Sidebar';
import { InputBar } from './components/InputBar';
import { Button } from '../presentation/primitives/Button';
import { Input } from '../presentation/primitives/Input';
import { Pill } from '../presentation/primitives/Pill';
import { StateDot } from '../presentation/primitives/StateDot';
import { Conversation, Interactions, RuntimeFacts } from './Conversation';
import { Inspector } from './Inspector';

const PREFERENCES = 'rustx-console-view-v1';
function readPreferences(): { endpoint: string; tabs: string[] } {
  try {
    const value = JSON.parse(localStorage.getItem(PREFERENCES) ?? 'null');
    if (value && typeof value.endpoint === 'string' && Array.isArray(value.tabs)) {
      const endpoint = new URL(value.endpoint);
      if (['ws:', 'wss:'].includes(endpoint.protocol) && !endpoint.username && !endpoint.password && !endpoint.search && !endpoint.hash && endpoint.pathname === '/') {
        return { endpoint: endpoint.href, tabs: value.tabs.filter((id: unknown) => typeof id === 'string').slice(0, 32) };
      }
    }
  } catch { /* Preferences are optional presentation, never recovery input. */ }
  return { endpoint: 'ws://127.0.0.1:8080/', tabs: [] };
}
export function App({ client }: { client: AppServerClient }) {
  const state = useSyncExternalStore(client.subscribe, client.getSnapshot);
  const [conversationMode, setConversationMode] = useState<'chat' | 'trajectory'>('chat');
  const [preferences] = useState(readPreferences);
  const [endpoint, setEndpoint] = useState(preferences.endpoint);
  const [token, setToken] = useState('');
  const [tabs, setTabs] = useState<string[]>(preferences.tabs);
  const [selected, selectSession] = useState<string | undefined>(preferences.tabs[0]);
  const [navigation] = useState(() => new NavigationEpoch());
  const [command, setCommand] = useState<{ request: CommandRequest; current: () => boolean; generation: number; sessionId: string; conversationId?: string }>();
  const [restored, setRestored] = useState<{ conversation: string; content: UserInputBlock[] }>();
  const [consumed, setConsumed] = useState<{ id: string; sequence: number }>();
  const setSelected = (id: string | undefined) => { navigation.invalidate(); setCommand(undefined); setRestored(undefined); selectSession(id); };
  // Existing navigation hints may restore wanted views, never a released claim.
  // A detached tab stays visible on this page but is no longer a resume hint.
  const resumeTabs = JSON.stringify(tabs.filter(id => state.views[id]?.attachmentIntent !== 'released'));
  const [cwd, setCwd] = useState('');
  const [error, setError] = useState('');
  const [creating, setCreating] = useState<number>();
  const busy = creating === state.generation || ['connecting', 'reconnecting', 'resynchronizing'].includes(state.connection);
  const [sending, setSending] = useState<Record<string, number>>({});
  const [preview, setPreview] = useState<RuntimeClientSessionDeletePreview>();
  const [offset, setOffset] = useState(0);
  const view = selected ? state.views[selected] : undefined;
  const artifacts = useMemo(() => selected && view?.target ? new ArtifactResources(client, selected) : undefined, [client, selected, view?.target, state.generation]);
  useEffect(() => () => artifacts?.dispose(), [artifacts]);
  const connected = state.connection === 'connected';
  const attached = connected && view?.attachmentIntent === 'wanted' && view.attachment === 'attached';
  const composerDisabled = !attached || !!view?.snapshot?.shutting_down || !!view?.snapshot?.durability_failure;
  const commandOpen = !!command && command.sessionId === selected && command.generation === state.generation && command.current();
  const invokeCommand = (request: CommandRequest) => {
    if (!view || composerDisabled) return;
    const definition = commands.find(item => item.id === request.id);
    if (definition && !available(definition, activeAttempt(view.snapshot), !!goalDock(view.snapshot), lineageSwitchSafe(view))) return;
    if (request.id === 'goal') {
      document.querySelector<HTMLElement>('[aria-label="Goal"] button')?.focus();
      setConsumed(previous => ({ id: 'goal', sequence: (previous?.sequence ?? 0) + 1 }));
      return;
    }
    navigation.invalidate();
    setCommand({ request, current: navigation.capture(), generation: state.generation, sessionId: view.id, conversationId: view.target?.conversation_id });
  };
  const run = (action: () => Promise<unknown>) => {
    const generation = client.getSnapshot().generation; setError('');
    void action().catch(cause => { if (generation === client.getSnapshot().generation) setError(String(cause)); });
  };
  // Subscription cleanup is presentation-only. No component owns socket/runtime life.
  useEffect(() => { client.restoreViews(preferences.tabs); }, [client, preferences]);
  useEffect(() => {
    try {
      // Persist only safe navigation. Never the token, drafts, snapshots or requests.
      const url = new URL(endpoint);
      if (['ws:', 'wss:'].includes(url.protocol) && !url.username && !url.password && !url.search && !url.hash && url.pathname === '/') localStorage.setItem(PREFERENCES, json({ endpoint, tabs: JSON.parse(resumeTabs) }));
    } catch { /* Storage may be disabled in a trusted browser. */ }
  }, [endpoint, resumeTabs]);
  const open = (id: string) => {
    if (!tabs.includes(id) && tabs.length >= 32) { setError('Close a view before opening more than 32 tabs. Explicit detach releases an attachment.'); return; }
    setSelected(id); setTabs(current => current.includes(id) ? current : [...current, id]);
    run(() => client.attach(id));
  };
  const connect = (reconnect: boolean) => {
    setError('');
    const work = client.connect(endpoint, token, reconnect);
    const generation = client.getSnapshot().generation;
    void work.catch(cause => { if (generation === client.getSnapshot().generation) setError(String(cause)); });
  };
  const deletePreview = (id: string) => run(async () => {
    const generation = client.getSnapshot().generation;
    const result = await client.request({ method: 'session/deletePreview', params: { session_id: id } }, 'deletion');
    if (generation !== client.getSnapshot().generation) return;
    if (result.result.status === 'preview') setPreview(result.result.preview);
    else setError(`Delete preview: ${json(result.result)}`);
  });
  return <AppFrame navigation={<Sidebar footer={<>
    <p className="muted">Native App Server · protocol v4</p><a href="https://github.com/Caismis/rustX" target="_blank" rel="noreferrer">rustX source</a>
    <p className="muted">UI source adapted from DeepSeek Harness. <a href="/LICENSE-DeepSeek-Harness.txt" target="_blank" rel="noreferrer">MIT notice</a></p>
  </>}>
    <section className="connection-form" aria-label="Connection">
      <div className="status"><StateDot state={connected ? 'done' : state.connection === 'error' || state.connection === 'incompatible' ? 'error' : state.connection === 'disconnected' ? 'idle' : 'warning'} /><strong>{state.connection}</strong><small>g{state.generation}</small></div>
      <label>WebSocket endpoint<Input aria-label="WebSocket endpoint" value={endpoint} disabled={busy || connected} onChange={event => setEndpoint(event.target.value)} /></label>
      <label>Transport token<Input type="password" autoComplete="off" aria-label="Transport token" value={token} onChange={event => setToken(event.target.value)} /></label>
      <small className="muted">Dedicated socket token; kept in page memory only.</small>
      <div className="row"><Button variant="primary" disabled={busy || connected || !token} onClick={() => connect(false)}>Connect</Button>
        <Button variant="outline" disabled={state.connection === 'disconnected'} onClick={() => client.disconnect()}>Disconnect</Button></div>
      <Button variant="outline" disabled={busy || !token} onClick={() => connect(true)}>Reconnect</Button>
    </section>
    <section className="session-list" aria-label="Sessions">
      <div className="section-head"><h2>Sessions</h2><Button size="sm" disabled={!connected} onClick={() => run(() => client.listSessions(offset))}>Refresh list</Button></div>
      <form onSubmit={event => { event.preventDefault(); const generation = state.generation; const current = navigation.capture(); setCreating(generation); run(async () => {
        try { const result = await createSession(client, cwd, current); if (result && current() && generation === client.getSnapshot().generation) { const id = result.session.id; setSelected(id); setTabs(current => current.includes(id) ? current : [...current, id]); } }
        catch (cause) { if (current()) throw cause; }
        finally { if (generation === client.getSnapshot().generation) setCreating(undefined); }
      }); }}>
        <label>Explicit Session cwd<Input aria-label="Session cwd" placeholder="/absolute/path/to/project" value={cwd} onChange={event => setCwd(event.target.value)} /></label>
        <Button type="submit" variant="outline" disabled={!connected || busy || !cwd.trim()}>Create Session</Button>
      </form>
      {state.sessions.map(session => <div className="session-row" key={session.id}>
        <button className="session-open" aria-label={`Open ${session.name ?? session.id}`} disabled={!connected} onClick={() => open(session.id)}>
          <strong>{session.name ?? session.preview ?? session.id}</strong><small>{session.id}</small>
          <small>durable · {state.views[session.id]?.attachment ?? 'not attached'}</small>
        </button>
        <Button size="sm" aria-label={`Delete ${session.name ?? session.id}`} disabled={!connected} onClick={() => deletePreview(session.id)}>Delete</Button>
      </div>)}
      <div className="row"><Button size="sm" disabled={!connected || offset === 0} onClick={() => { const next = Math.max(0, offset - 32); setOffset(next); run(() => client.listSessions(next)); }}>Previous</Button>
        <Button size="sm" disabled={!connected || state.nextOffset == null} onClick={() => { const next = state.nextOffset!; setOffset(next); run(() => client.listSessions(next)); }}>Next</Button></div>
    </section>
  </Sidebar>} dockLabel="Developer inspector" dock={<Inspector client={client} state={state} view={view} />}>
    <header className="console-header"><div><div className="eyebrow">DEVELOPER WEB CONSOLE</div><h1>Sessions, in motion.</h1></div><Pill>{state.connection}</Pill></header>
    <nav className="tabs" aria-label="Open Session views">{tabs.map(id => <div className="tab" key={id}>
      <Pill role="tab" active={selected === id} aria-selected={selected === id} onClick={() => setSelected(id)}>{state.sessions.find(item => item.id === id)?.name ?? id.slice(0, 16)}</Pill>
      <button className="close-tab" aria-label={`Close view ${id}`} onClick={() => {
        const remaining = tabs.filter(item => item !== id); setTabs(remaining); if (selected === id) setSelected(remaining[0]);
        run(() => client.release(id, false));
      }}>×</button>
    </div>)}</nav>
    {(error || state.error) && <div className="notice error" role="alert">{error || state.error}<Button size="sm" onClick={() => { setError(''); client.clearError(); }}>Dismiss notice</Button></div>}
    {state.uncertain.map(item => <div key={item.id} className="notice" role="status"><strong>Outcome uncertain: {item.method}</strong><p>{item.sessionId ?? 'Session identity not known'} · request {item.id}. No automatic replay. Read authoritative state before deciding what to do.</p>
      {!item.interactionKey && <Button size="sm" onClick={() => client.acknowledgeDiagnostic(item.id)}>Acknowledge diagnostic only</Button>}</div>)}
    {preview && <section className="delete-preview" aria-label="Confirm Session deletion"><h2>Delete {preview.name ?? preview.session_id}?</h2><pre>{json(preview)}</pre>
      <p>This deletes the native ownership graph. An in-use Session may need explicit unload first.</p>
      <div className="row"><Button onClick={() => setPreview(undefined)}>Keep Session</Button><Button variant="primary" disabled={!connected} onClick={() => run(async () => {
        const generation = client.getSnapshot().generation;
        const result = await client.deleteSession(preview.session_id, preview.target_revision);
        if (generation !== client.getSnapshot().generation || !result) return;
        if (result.status === 'deleted' || result.status === 'not_found') {
          const remaining = tabs.filter(id => id !== preview.session_id); setTabs(remaining);
          if (selected === preview.session_id) setSelected(remaining[0]);
        }
        setError(`Deletion result: ${json(result)}`); setPreview(undefined);
      })}>Confirm delete</Button></div>
    </section>}
    {view ? <>
      <section className="session-toolbar"><div><strong>{view.id}</strong><small>{view.settings?.cwd ?? 'cwd unavailable'} · {view.attachment}</small></div>
        <div className="row"><Button size="sm" disabled={!connected} onClick={() => run(() => client.attach(view.id))}>{view.target && view.attachmentIntent === 'wanted' ? 'Resync' : 'Attach / cold resume'}</Button>
          <Button size="sm" disabled={!attached || commandOpen || !lineageSwitchSafe(view)} onClick={() => invokeCommand({ id: 'tree' })}>Session tree</Button>
          <Button size="sm" disabled={!attached} onClick={() => run(() => client.release(view.id, false))}>Detach</Button>
          <Button size="sm" disabled={!attached} onClick={() => run(() => client.release(view.id, true))}>Unload runtime</Button></div>
      </section>
      {view.attachment !== 'attached' && <p className="notice">{view.attachment}: last observed values may be stale. Execution and pending interactions remain server-owned. {view.error}</p>}
      <div className="row" role="tablist" aria-label="Conversation view"><Button role="tab" aria-selected={conversationMode === 'chat'} onClick={() => setConversationMode('chat')}>Chat</Button><Button role="tab" aria-selected={conversationMode === 'trajectory'} onClick={() => setConversationMode('trajectory')}>Trajectory</Button></div>
      <ArtifactContext.Provider value={artifacts}>{conversationMode === 'trajectory' && view.trace ? <Trajectory key={view.id} cache={view.trace} onSelect={id => client.selectTrace(view.id, id)} loadEarlier={() => run(() => client.loadEarlierTrace(view.id))} latest={() => client.latestTrace(view.id)} /> : <ChatViewport key={`${view.id}:${view.target?.attachment_id ?? state.generation}`}>
        {view.snapshot && <><Conversation snapshot={view.snapshot} history={view.history} loadEarlier={() => run(() => client.loadEarlier(view.id))} latest={() => client.latestTranscript(view.id)}
          lineageSwitchSafe={lineageSwitchSafe(view)} historicalDisabled={composerDisabled || commandOpen} onHistorical={(id, messageId) => invokeCommand({ id, messageId })} /><RuntimeFacts snapshot={view.snapshot} />
          <div className="attempt-status" role="status">Attempt: {view.snapshot.attempt ? `${view.snapshot.attempt.attempt_id} · ${view.snapshot.attempt.phase.type}` : 'none observed'}{view.snapshot.attempt?.phase.type === 'settled' && ` · ${view.snapshot.attempt.phase.outcome.type}`}</div>
          <Interactions client={client} state={state} view={view} run={run} />
        </>}
      </ChatViewport>}</ArtifactContext.Provider>
      {/* Keyed by Session: no dock or draft state crosses Session views. */}
      <ComposerContextStack key={view.id}
        todo={<TodoDock state={todoDock(view.snapshot)} />}
        goal={<GoalDock state={goalDock(view.snapshot)} observation={view.snapshot} disabled={composerDisabled}
          mutate={(expected, mutation) => client.controlGoal(view.id, expected, mutation)} />}
        queue={<QueueDock rows={queueRows(view.snapshot)} submissions={view.submissions ?? []} running={activeAttempt(view.snapshot)} />}
        composer={<InputBar key={`${view.snapshot?.conversation_id ?? view.id}:${restored?.conversation === view.snapshot?.conversation_id ? 'restored' : 'draft'}`} initialContent={restored?.conversation === view.snapshot?.conversation_id ? restored?.content : undefined}
          disabled={composerDisabled} busy={sending[view.id] === state.generation} active={activeAttempt(view.snapshot)}
          lineageSwitchSafe={lineageSwitchSafe(view)} hasGoal={!!goalDock(view.snapshot)} onCommand={id => invokeCommand({ id })}
          consumed={consumed}
          onCancel={() => run(() => client.cancelTurn(view.id))} onUpload={files => client.upload(view.id, files)} onSend={async (text, receipts, delivery) => {
            const generation = state.generation; setSending(current => ({ ...current, [view.id]: generation })); setError('');
            try { await client.send(view.id, text, receipts, delivery); return generation === client.getSnapshot().generation; }
            catch (cause) { if (generation === client.getSnapshot().generation) setError(String(cause)); return false; }
            finally { if (generation === client.getSnapshot().generation) setSending(current => { const next = { ...current }; delete next[view.id]; return next; }); }
          }} />} />
      {commandOpen && <CommandPanel key={`${command.generation}:${command.sessionId}:${command.request.id}:${command.request.messageId ?? ''}`} request={command.request} client={client} sessionId={command.sessionId} current={() => command.current() && client.getSnapshot().generation === command.generation}
        succeeded={() => { setConsumed(previous => ({ id: command.request.id, sequence: (previous?.sequence ?? 0) + 1 })); }}
        close={() => {
          if (command.conversationId !== client.getSnapshot().views[command.sessionId]?.snapshot?.conversation_id) setRestored(undefined);
          navigation.invalidate(); setCommand(undefined);
        }} opened={result => {
          if (!command.current() || client.getSnapshot().generation !== command.generation) return;
          setRestored({ conversation: result.session.active_conversation_id, content: result.content });
          navigation.invalidate(); setCommand(undefined); selectSession(result.session.id); setTabs(current => current.includes(result.session.id) ? current : [...current, result.session.id]);
        }} />}

    </> : <div className="empty"><h2>One runtime. Many Sessions.</h2><p>Connect to rustX, then open or create a Session with an explicit cwd.</p><p>Switching or closing views never cancels work.</p></div>}
  </AppFrame>;
}

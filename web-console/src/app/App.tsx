import { navigateTabs } from '../presentation/primitives/tabs';
import { readTheme, applyTheme } from './appearance';
import { Settings } from './settings/Settings';
import { HttpWorkspaceHost, type ProductHostWorkspaces } from '../workspaces/host';
import { WorkspaceNavigation } from '../workspaces/WorkspaceNavigation';
import { createWorkspaceSession, WorkspaceSessionNavigation } from '../workspaces/navigation';
import { Trajectory } from './Trajectory';
import { useEffect, useMemo, useState, useSyncExternalStore } from 'react';
import type { AppServerClient } from '../client/app-server';
import type { RuntimeClientSessionDeletePreview, UserInputBlock } from '../../../protocol/app-server/v6';
import { CommandPanel, type CommandRequest } from './commands/CommandPanel';
import { NavigationEpoch } from './commands/native';
import { available, commands } from './commands/registry';
import { activeAttempt, lineageSwitchSafe, json } from '../bindings/projection';
import { goalDock, queueRows, todoDock } from '../bindings/composer-context';
import { ComposerContextStack } from './composer/ComposerContextStack';
import { GoalDock } from './composer/GoalDock';
import { QueueDock } from './composer/QueueDock';
import { TodoDock } from './composer/TodoDock';
import { ArtifactResources } from '../client/artifacts';
import { ArtifactPreview, PreviewContext, type PreviewArtifact } from './components/ArtifactPreview';
import { ArtifactContext } from './components/Artifact';
import { ChatViewport } from '../presentation/layout/ChatViewport';
import { AppFrame } from '../presentation/layout/AppFrame';
import { SidebarRoot } from '../presentation/sidebar/SidebarRoot';
import { SettingsTrigger } from '../presentation/settings/SettingsRoot';
import { Modal } from '../presentation/primitives/Modal';
import { IconApiOutline14, IconInspectOutline12 } from '../presentation/primitives/icons';
import { RightPanel } from '../presentation/right-panel/RightPanel';
import { AgentComposer } from './agent/AgentComposer';
import { AgentControls } from './agent/AgentControls';
import agentCss from '../presentation/agent/Conversation.module.css';
import { Button } from '../presentation/primitives/Button';
import { Input } from '../presentation/primitives/Input';
import { Pill } from '../presentation/primitives/Pill';
import { StateDot } from '../presentation/primitives/StateDot';
import { AgentTranscript } from './agent/AgentTranscript';
import { Interactions } from './agent/Interactions';
import { RuntimeFacts } from './agent/Activity';
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
const defaultWorkspaceHost = new HttpWorkspaceHost();
export function App({ client, workspaceHost = defaultWorkspaceHost }: { client: AppServerClient; workspaceHost?: ProductHostWorkspaces }) {
  const state = useSyncExternalStore(client.subscribe, client.getSnapshot);
  const [connectionOpen, setConnectionOpen] = useState(client.getSnapshot().connection !== 'connected');
  const [createOpen, setCreateOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [inspectorOpen, setInspectorOpen] = useState(false);
  const [artifactPreview, setArtifactPreview] = useState<{ artifact: PreviewArtifact; resources: ArtifactResources }>();
  const [theme, setTheme] = useState(readTheme);
  useEffect(() => applyTheme(theme), [theme]);
  useEffect(() => { if (state.connection === 'connected') setConnectionOpen(false); }, [state.connection]);
  const [conversationMode, setConversationMode] = useState<'chat' | 'trajectory'>('chat');
  const [preferences] = useState(readPreferences);
  const [endpoint, setEndpoint] = useState(preferences.endpoint);
  const [token, setToken] = useState('');
  const [tabs, setTabs] = useState<string[]>(preferences.tabs);
  const [focus, setFocus] = useState<{ sessionId?: string; workspaceId?: string; generation?: number }>({ sessionId: preferences.tabs[0] });
  const selected = focus.sessionId;
  const workspace = focus.generation === state.generation && state.connection === 'connected' ? focus.workspaceId : undefined;
  const [navigation] = useState(() => new NavigationEpoch());
  const [command, setCommand] = useState<{ request: CommandRequest; current: () => boolean; generation: number; sessionId: string; conversationId?: string }>();
  const [restored, setRestored] = useState<{ conversation: string; content: UserInputBlock[] }>();
  const [consumed, setConsumed] = useState<{ id: string; sequence: number }>();
  const workspaceNavigation = useMemo(() => new WorkspaceSessionNavigation(workspaceHost, client, navigation), [workspaceHost, client, navigation]);
  useEffect(() => client.setAttachmentAdmission(workspaceNavigation.admit), [client, workspaceNavigation]);
  // Existing navigation hints may restore wanted views, never a released claim.
  // A detached tab stays visible on this page but is no longer a resume hint.
  const resumeTabs = JSON.stringify(tabs.filter(id => state.views[id]?.attachmentIntent !== 'released'));
  const [error, setError] = useState('');
  const [creating, setCreating] = useState<number>();
  const busy = creating === state.generation || ['connecting', 'reconnecting', 'resynchronizing'].includes(state.connection);
  const [sending, setSending] = useState<Record<string, number>>({});
  const [preview, setPreview] = useState<RuntimeClientSessionDeletePreview>();

  const view = selected ? state.views[selected] : undefined;
  const artifacts = useMemo(() => selected && view?.target ? new ArtifactResources(client, selected) : undefined, [client, selected, view?.target, state.generation]);
  useEffect(() => () => artifacts?.dispose(), [artifacts]);
  const connected = state.connection === 'connected';
  const attached = connected && view?.attachmentIntent === 'wanted' && view.attachment === 'attached';
  const composerDisabled = !attached || !!view?.modelMutation || !!view?.snapshot?.shutting_down || !!view?.snapshot?.durability_failure;
  const commandOpen = !!command && command.sessionId === selected && command.generation === state.generation && command.current();
  const invokeCommand = (request: CommandRequest | { id: 'new' }) => {
    if (!view || composerDisabled) return;
    const definition = commands.find(item => item.id === request.id);
    if (definition && !available(definition, activeAttempt(view.snapshot), !!goalDock(view.snapshot), lineageSwitchSafe(view))) return;
    if (request.id === 'new') { if (workspace) createInWorkspace(workspace); else setError('Select a Host-authorized Workspace first.'); return; }
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
  // Every Session focus path publishes the same pair. Until native cwd has been
  // classified, its Workspace is explicitly empty rather than inherited.
  const focusSession = (id?: string, options: { attach?: boolean; ready?: () => void; preserveDraft?: boolean } = {}) => {
    navigation.invalidate(); const current = navigation.capture();
    const generation = state.generation;
    setCommand(undefined); if (!options.preserveDraft) setRestored(undefined); setFocus({ sessionId: id });
    if (!id) return;
    client.restoreViews([id]);
    if (!connected) return;
    run(async () => {
      try {
        if (options.attach) await client.attach(id, undefined, current);
        if (!current() || generation !== client.getSnapshot().generation) return;
        const location = await workspaceNavigation.classifySession(id, current);
        if (!current() || generation !== client.getSnapshot().generation || !location) return;
        setFocus({ sessionId: id, workspaceId: location.authorized ? location.workspaceId : undefined, generation });
        options.ready?.();
      } catch (cause) { if (current() && generation === client.getSnapshot().generation) throw cause; }
    });
  };
  useEffect(() => {
    if (connected && selected) focusSession(selected, { preserveDraft: true });
  }, [state.connection, state.generation]);
  const open = (id: string, ready?: () => void) => {
    if (!tabs.includes(id) && tabs.length >= 32) { setError('Close a view before opening more than 32 tabs. Explicit detach releases an attachment.'); return; }
    setTabs(current => current.includes(id) ? current : [...current, id]);
    focusSession(id, { attach: true, ready });
  };
  const createInWorkspace = (id: string) => {
    if (creating === state.generation) return;
    navigation.invalidate(); const current = navigation.capture(); const generation = state.generation;
    setCommand(undefined); setCreating(generation);
    run(async () => {
      try {
        const result = await createWorkspaceSession(workspaceHost, id, client, current);
        if (result && current() && generation === client.getSnapshot().generation) {
          focusSession(result.session.id); setTabs(value => value.includes(result.session.id) ? value : [...value, result.session.id]);
        }
      } catch (cause) { if (current()) throw cause; }
      finally { if (generation === client.getSnapshot().generation) setCreating(undefined); }
    });
  };
  const connect = (reconnect: boolean) => {
    navigation.invalidate(); setError('');
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
  return <AppFrame sidebar={geometry => <SidebarRoot {...geometry} startSession={() => { setCreateOpen(true); }}
    panels={[{ id: 'connection', label: 'Connection', icon: <IconApiOutline14 />, active: connectionOpen, select: () => setConnectionOpen(true) }]}
    browser={(wide, expand) => <WorkspaceNavigation wide={wide} expand={expand} createOpen={createOpen} closeCreate={() => setCreateOpen(false)} host={workspaceHost} client={client} state={state} endpoint={endpoint} navigation={navigation}
      creating={creating === state.generation} metadataChanged={removed => { if (selected) focusSession(selected, { preserveDraft: true }); else if (removed) setFocus(value => value.workspaceId === removed ? {} : value); }}
      workspace={workspace} selected={selected} selectWorkspace={id => { navigation.invalidate(); setCommand(undefined); setRestored(undefined); setFocus({ workspaceId: id, generation: state.generation }); }}
      openSession={open} createSession={createInWorkspace} deleteSession={deletePreview}
      forkSession={id => open(id, () => {
        setCommand({ request: { id: 'fork' }, current: navigation.capture(), generation: client.getSnapshot().generation, sessionId: id, conversationId: client.getSnapshot().views[id]?.target?.conversation_id });
      })} />}
    settings={wide => <SettingsTrigger wide={wide} onClick={() => setSettingsOpen(true)} />} />}
    rightOpen={inspectorOpen || !!(artifactPreview && artifactPreview.resources === artifacts)} rightPanel={geometry => <RightPanel {...geometry} open={inspectorOpen || !!(artifactPreview && artifactPreview.resources === artifacts)} close={() => { setInspectorOpen(false); setArtifactPreview(undefined); }} title={artifactPreview && artifactPreview.resources === artifacts ? 'Artifact preview' : 'Developer inspector'}>{artifactPreview && artifactPreview.resources === artifacts ? <ArtifactPreview key={artifactPreview.artifact.id} artifact={artifactPreview.artifact} resources={artifacts!} /> : <Inspector client={client} state={state} view={view} />}</RightPanel>}
    overlay={<>
      <Modal open={connectionOpen} title="Connection" closeLabel="Close dialog" onClose={() => setConnectionOpen(false)}>
    <section className="connection-form" aria-label="Connection">
      <div className="connection-status"><StateDot state={connected ? 'done' : state.connection === 'error' || state.connection === 'incompatible' ? 'error' : state.connection === 'disconnected' ? 'idle' : 'warning'} /><strong>{state.connection}</strong><small>g{state.generation}</small></div>
      <details open={!connected}><summary>Connection settings</summary>
      <label>WebSocket endpoint<Input aria-label="WebSocket endpoint" value={endpoint} disabled={busy || connected} onChange={event => setEndpoint(event.target.value)} /></label>
      <label>Transport token<Input type="password" autoComplete="off" aria-label="Transport token" value={token} onChange={event => setToken(event.target.value)} /></label>
      <small className="muted">Dedicated socket token; kept in page memory only.</small>
      <Button variant="primary" disabled={busy || connected || !token} onClick={() => connect(false)}>Connect</Button>
      </details>
      <Button variant="outline" disabled={state.connection === 'disconnected'} onClick={() => { navigation.invalidate(); client.disconnect(); }}>Disconnect</Button>
      <Button variant="outline" disabled={busy || !token} onClick={() => connect(true)}>Reconnect</Button>
    </section>
      </Modal>
      {settingsOpen && <Settings key={view?.id} onConnection={() => setConnectionOpen(true)} client={client} sessionId={view?.id} onClose={() => setSettingsOpen(false)} theme={theme} setTheme={setTheme} />}
    </>}>
    <header className="console-header"><strong>{view ? state.sessions.find(item => item.id === view.id)?.name ?? 'Session' : 'rustX'}</strong><div className="row"><span className="status"><strong>{state.connection}</strong></span><Button aria-label="Toggle Inspector" onClick={() => { setArtifactPreview(undefined); setInspectorOpen(value => !value); }}><IconInspectOutline12 /></Button></div></header>
    <nav className="tabs" role="tablist" aria-label="Open Session views" onKeyDown={navigateTabs}>{tabs.map(id => <div className="tab" key={id}>
      <Pill role="tab" aria-label={state.sessions.find(item => item.id === id)?.name ?? id} title={id} id={`session-tab-${id}`} aria-controls="session-view" tabIndex={selected === id ? 0 : -1} active={selected === id} aria-selected={selected === id} onClick={() => focusSession(id)}>{state.sessions.find(item => item.id === id)?.name ?? id.slice(0, 16)}</Pill>
      <button className="close-tab" aria-label={`Close view ${id}`} onClick={() => {
        const remaining = tabs.filter(item => item !== id); setTabs(remaining); if (selected === id) focusSession(remaining[0]);
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
        const current = navigation.capture();
        const result = await client.deleteSession(preview.session_id, preview.target_revision);
        if (generation !== client.getSnapshot().generation || !result) return;
        if (result.status === 'deleted' || result.status === 'not_found') {
          const remaining = tabs.filter(id => id !== preview.session_id); setTabs(value => value.filter(id => id !== preview.session_id));
          if (current() && selected === preview.session_id) focusSession(remaining[0]);
        }
        setError(`Deletion result: ${json(result)}`); setPreview(undefined);
      })}>Confirm delete</Button></div>
    </section>}
    {view ? <section className={`session-panel ${agentCss.root}`} data-phase="active" id="session-view" role="tabpanel" aria-labelledby={`session-tab-${view.id}`}>
      <section className={agentCss.header}><div className={`${agentCss.titleRow} agent-title-row`}><div className={agentCss.titleCluster}><strong aria-label="Session title">{state.sessions.find(session => session.id === view.id)?.name ?? view.id}</strong><small aria-label="Session location and attachment">{view.settings?.cwd ?? 'cwd unavailable'} · {view.attachment}</small></div>
        <div className="row"><Button size="sm" disabled={!connected} onClick={() => focusSession(view.id, { attach: true, preserveDraft: true })}>{view.target && view.attachmentIntent === 'wanted' ? 'Resync' : 'Attach / cold resume'}</Button>
          <Button size="sm" disabled={!attached || commandOpen || !lineageSwitchSafe(view)} onClick={() => invokeCommand({ id: 'tree' })}>Session tree</Button>
          <Button size="sm" disabled={!attached} onClick={() => run(() => client.release(view.id, false))}>Detach</Button>
          <Button size="sm" disabled={!attached} onClick={() => run(() => client.release(view.id, true))}>Unload runtime</Button></div>
      </div>
      <div className={agentCss.tabs} role="tablist" aria-label="Conversation view" onKeyDown={navigateTabs}>{(['chat', 'trajectory'] as const).map(mode => <Button className={`${agentCss.tab} ${conversationMode === mode ? agentCss.tabActive : ""}`} key={mode} role="tab" id={`view-tab-${mode}`} aria-controls="conversation-view" tabIndex={conversationMode === mode ? 0 : -1} aria-selected={conversationMode === mode} onClick={() => setConversationMode(mode)}>{mode === 'chat' ? 'Chat' : 'Trajectory'}</Button>)}</div>
      </section>
      {view.modelMutation && <p className="notice" role="status">Model change {view.modelMutation.status}. Sending is paused until native state is reread.</p>}
      {view.attachment !== 'attached' && <p className="notice">{view.attachment}: last observed values may be stale. Execution and pending interactions remain server-owned. {view.error}</p>}

      <section className={`conversation-panel ${agentCss.body}`} id="conversation-view" role="tabpanel" aria-labelledby={`view-tab-${conversationMode}`} tabIndex={0}>
      <PreviewContext value={artifact => { if (artifacts) { setArtifactPreview({ artifact, resources: artifacts }); setInspectorOpen(false); } }}><ArtifactContext.Provider value={artifacts}>{conversationMode === 'trajectory' && view.trace ? <Trajectory key={view.id} cache={view.trace} onSelect={id => client.selectTrace(view.id, id)} loadEarlier={() => run(() => client.loadEarlierTrace(view.id))} latest={() => client.latestTrace(view.id)} /> : <ChatViewport key={`${view.id}:${view.target?.attachment_id ?? state.generation}`}>
        {view.snapshot && <><AgentTranscript snapshot={view.snapshot} history={view.history} loadEarlier={() => run(() => client.loadEarlier(view.id))} latest={() => client.latestTranscript(view.id)}
          lineageSwitchSafe={lineageSwitchSafe(view)} historicalDisabled={composerDisabled || commandOpen} onHistorical={(id, messageId) => invokeCommand({ id, messageId })} /><RuntimeFacts snapshot={view.snapshot} />
          <div className="attempt-status" role="status">{view.cancellation && <span>{view.cancellation.status === "uncertain" ? "Cancellation outcome uncertain" : "Stop requested; waiting for native settlement"} · </span>}Attempt: {view.snapshot.attempt ? `${view.snapshot.attempt.attempt_id} · ${view.snapshot.attempt.phase.type}` : 'none observed'}{view.snapshot.attempt?.phase.type === 'settled' && ` · ${view.snapshot.attempt.phase.outcome.type}`}</div>

        </>}
      </ChatViewport>}</ArtifactContext.Provider></PreviewContext>
      {/* Keyed by Session: no dock or draft state crosses Session views. */}
      <div className={agentCss.composerSeat}><div hidden={!!view.snapshot?.pending_interactions?.length}><ComposerContextStack key={view.id}
        todo={<TodoDock state={todoDock(view.snapshot)} />}
        goal={<GoalDock state={goalDock(view.snapshot)} observation={view.snapshot} disabled={composerDisabled}
          mutate={(expected, mutation) => client.controlGoal(view.id, expected, mutation)} />}
        queue={<QueueDock key={`${view.id}:${view.target?.attachment_id ?? state.generation}`} disabled={composerDisabled} observation={view.snapshot} edit={(expected, text) => client.editInbound(view.id, expected, text)} remove={expected => client.removeInbound(view.id, expected)} rows={queueRows(view.snapshot)} submissions={view.submissions ?? []} running={activeAttempt(view.snapshot)} />}
        composer={<AgentComposer key={`${view.snapshot?.conversation_id ?? view.id}:${restored?.conversation === view.snapshot?.conversation_id ? 'restored' : 'draft'}`} initialContent={restored?.conversation === view.snapshot?.conversation_id ? restored?.content : undefined}
          disabled={composerDisabled} busy={sending[view.id] === state.generation} active={activeAttempt(view.snapshot)}
          lineageSwitchSafe={lineageSwitchSafe(view)} hasGoal={!!goalDock(view.snapshot)} onCommand={id => invokeCommand({ id })}
          consumed={consumed} stopDisabled={!!view.cancellation}
          model={<AgentControls key={`model:${view.id}`} client={client} view={view} kind="model"/>}
          permission={<AgentControls key={`permission:${view.id}`} client={client} view={view} kind="permission"/>}
          onCancel={() => run(() => client.cancelTurn(view.id))} onUpload={files => client.upload(view.id, files)} onSend={async (text, receipts, delivery) => {
            const generation = state.generation; setSending(current => ({ ...current, [view.id]: generation })); setError('');
            try { await client.send(view.id, text, receipts, delivery); return generation === client.getSnapshot().generation; }
            catch (cause) { if (generation === client.getSnapshot().generation) setError(String(cause)); return false; }
            finally { if (generation === client.getSnapshot().generation) setSending(current => { const next = { ...current }; delete next[view.id]; return next; }); }
          }} />} /></div><Interactions client={client} state={state} view={view} run={run}/></div>
      </section>
      {commandOpen && <CommandPanel key={`${command.generation}:${command.sessionId}:${command.request.id}:${command.request.messageId ?? ''}`} request={command.request} client={client} sessionId={command.sessionId} current={() => command.current() && client.getSnapshot().generation === command.generation}
        succeeded={() => { setConsumed(previous => ({ id: command.request.id, sequence: (previous?.sequence ?? 0) + 1 })); }}
        close={() => {
          if (command.conversationId !== client.getSnapshot().views[command.sessionId]?.snapshot?.conversation_id) setRestored(undefined);
          navigation.invalidate(); setCommand(undefined);
        }} opened={result => {
          if (!command.current() || client.getSnapshot().generation !== command.generation) return;
          focusSession(result.session.id, { ready: () => setRestored({ conversation: result.session.active_conversation_id, content: result.content }) });
          setTabs(current => current.includes(result.session.id) ? current : [...current, result.session.id]);
        }} />}

    </section> : <div className="empty"><h2>One runtime. Many Sessions.</h2><p>Choose New Session to select a Workspace, or open an existing Session from the sidebar.</p><p>Switching or closing views never cancels work.</p></div>}
  </AppFrame>;
}

import { ConversationStats } from './agent/ConversationStats';
import { navigateTabs } from '../presentation/primitives/tabs';
import { readTheme, applyTheme } from './appearance';
import { Settings } from './settings/Settings';
import { HttpWorkspaceHost, type ProductHostWorkspaces } from '../workspaces/host';
import { WorkspaceNavigation } from '../workspaces/WorkspaceNavigation';
import { createWorkspaceSession, WorkspaceSessionNavigation } from '../workspaces/navigation';
import { Trajectory } from './trajectory/Trajectory';
import { useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import { useActorRef, useSelector } from '@xstate/react';
import type { AppServerClient } from '../client/app-server';
import type { RuntimeClientSessionDeletePreview, SourceTarget, UserInputBlock } from '../../../protocol/app-server/v18';
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
import { IconInspectOutline12 } from '../presentation/primitives/icons';
import { ConnectionController } from '../connection/controller';
import { RightPanel } from '../presentation/right-panel/RightPanel';
import { AgentComposer } from './agent/AgentComposer';
import { AgentControls } from './agent/AgentControls';
import agentCss from '../presentation/agent/Conversation.module.css';
import { Button } from '../presentation/primitives/Button';
import { sessionDisplayTitle } from '../bindings/session-title';
import { sessionDeletionNotice } from '../bindings/session-deletion';
import { AgentTranscript } from './agent/AgentTranscript';
import { Interactions } from './agent/Interactions';
import { RuntimeFacts } from './agent/Activity';
import { Inspector } from './Inspector';
import { Menu } from '../presentation/primitives/Menu';
import { deriveSessionProductState } from '../bindings/session-product';
import { SessionStatus } from './SessionStatus';
import { SessionConfiguration } from './SessionConfiguration';
import { userSettingsTarget, workspaceSettingsTarget, type SettingsTarget } from './settings/projection';
import { createOwnerLookup, settingsNavigationMachine } from './settings/machines/navigation';

const PREFERENCES = 'rustx-console-view-v2';
function readPreferences(): { endpoint?: string; openViews: string[] } {
  try {
    const value = JSON.parse(localStorage.getItem(PREFERENCES) ?? 'null');
    if (value && typeof value.endpoint === 'string' && Array.isArray(value.openViews)) return { endpoint: value.endpoint, openViews: [...new Set<string>(value.openViews.filter((id: unknown) => typeof id === 'string'))].slice(0, 32) };
  } catch { /* Preferences are optional presentation, never recovery input. */ }
  return { openViews: [] };
}
const defaultWorkspaceHost = new HttpWorkspaceHost();
export function App({ client, workspaceHost = defaultWorkspaceHost, connection: providedConnection }: { client: AppServerClient; workspaceHost?: ProductHostWorkspaces; connection?: ConnectionController }) {
  const state = useSyncExternalStore(client.subscribe, client.getSnapshot);
  const connection = useMemo(() => providedConnection ?? new ConnectionController(client), [providedConnection, client]);
  const selection = useSyncExternalStore(connection.subscribe, connection.getSnapshot);
  const [createOpen, setCreateOpen] = useState(false);
  // Top-level Settings navigation is an explicit machine, not an epoch counter.
  // Every navigation-affecting decision re-enters its idle state, which stops
  // any owning-Workspace lookup in flight: a stale lookup therefore has no
  // completion path at all, so neither its success nor its failure can
  // overwrite a newer decision, reopen a closed dialog or publish an obsolete
  // error. Async resolution is preparation, never ongoing navigation authority.
  const navigationActor = useActorRef(settingsNavigationMachine, { input: { lookup: createOwnerLookup(workspaceHost, client) } });
  const settingsPage = useSelector(navigationActor, snapshot => snapshot.context.page);
  const settingsTarget = useSelector(navigationActor, snapshot => snapshot.context.target);
  // The detail focused inside the displayed page. Focus is navigation state
  // with one owner; it carries identities only and never an editing draft.
  const settingsFocus = useSelector(navigationActor, snapshot => snapshot.context.page ? snapshot.context.focus[snapshot.context.page] : undefined);
  const navigationError = useSelector(navigationActor, snapshot => snapshot.context.error);
  const openSettings = (target: SettingsTarget) => navigationActor.send({ type: 'OPEN', target });
  const openConnectionSettings = () => navigationActor.send({ type: 'OPEN.CONNECTION' });
  const closeSettings = () => navigationActor.send({ type: 'CLOSE' });
  const [sessionSettingsOpen, setSessionSettingsOpen] = useState(false);
  const [inspectorOpen, setInspectorOpen] = useState(false);
  const [sessionMenuOpen, setSessionMenuOpen] = useState(false);
  const [artifactPreview, setArtifactPreview] = useState<{ artifact: PreviewArtifact; resources: ArtifactResources }>();
  const [theme, setTheme] = useState(readTheme);
  useEffect(() => applyTheme(theme), [theme]);
  const [conversationMode, setConversationMode] = useState<'chat' | 'trajectory'>('chat');
  const [preferences] = useState(readPreferences);
  const endpoint = state.endpoint ?? '';
  const initialViews = preferences.endpoint === state.endpoint ? preferences.openViews : [];
  const [openViews, setOpenViews] = useState<string[]>(initialViews);
  const [focus, setFocus] = useState<{ sessionId?: string; workspaceId?: string; generation?: number }>({ sessionId: initialViews[0] });
  const selected = focus.sessionId;
  const workspace = focus.generation === state.generation && state.connection === 'connected' ? focus.workspaceId : undefined;
  const [navigation] = useState(() => new NavigationEpoch());
  const [command, setCommand] = useState<{ request: CommandRequest; current: () => boolean; generation: number; sessionId: string; conversationId?: string }>();
  const [restored, setRestored] = useState<{ conversation: string; content: UserInputBlock[] }>();
  const [consumed, setConsumed] = useState<{ id: string; sequence: number }>();
  const workspaceNavigation = useMemo(() => new WorkspaceSessionNavigation(workspaceHost, client, navigation), [workspaceHost, client, navigation]);
  useEffect(() => client.setAttachmentAdmission(workspaceNavigation.admit), [client, workspaceNavigation]);
  // Existing navigation hints may restore wanted views, never a released claim.
  // Released views are not restored. Sidebar rows remain catalog-owned.
  const resumeViews = JSON.stringify(openViews.filter(id => state.views[id]?.attachmentIntent !== 'released'));
  const [error, setError] = useState('');
  const [creating, setCreating] = useState<number>();
  const [sending, setSending] = useState<Record<string, number>>({});
  const [preview, setPreview] = useState<RuntimeClientSessionDeletePreview>();
  const [presentationAuthority, setPresentationAuthority] = useState(state.authorityRevision);
  // The client owns authority retirement. This render adjustment retires only
  // browser presentation before any children can reinterpret an old Session ID.
  if (presentationAuthority !== state.authorityRevision) {
    setPresentationAuthority(state.authorityRevision);
    navigation.invalidate(); setOpenViews([]); setFocus({}); setCommand(undefined); setRestored(undefined);
    setPreview(undefined); setError(''); setConsumed(undefined); setSending({}); setCreating(undefined);
    setCreateOpen(false); setSessionMenuOpen(false); setArtifactPreview(undefined);
  }

  // The client owns authority retirement; this retires the browser navigation
  // preparation that belonged to the replaced authority.
  useEffect(() => { navigationActor.send({ type: 'RETIRE' }); }, [navigationActor, state.authorityRevision]);

  const selectedView = selected ? state.views[selected] : undefined;
  const view = selectedView?.deleting && !selectedView.target ? undefined : selectedView;
  const product = deriveSessionProductState(state, view);
  const artifacts = useMemo(() => selected && view?.target ? new ArtifactResources(client, selected) : undefined, [client, selected, view?.target, state.generation]);
  useEffect(() => () => artifacts?.dispose(), [artifacts]);
  const connected = state.connection === 'connected';
  const attached = !view?.deleting && connected && view?.attachmentIntent === 'wanted' && view.attachment === 'attached';
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
  const preferencesApplied = useRef(false);
  useEffect(() => {
    if (!endpoint || preferencesApplied.current) return;
    preferencesApplied.current = true;
    if (preferences.endpoint !== endpoint) return;
    client.restoreViews(preferences.openViews);
    setOpenViews(preferences.openViews); setFocus({ sessionId: preferences.openViews[0] });
    if (client.getSnapshot().connection === 'connected') for (const id of preferences.openViews) {
      if (!client.getSnapshot().views[id]?.target) void client.attach(id).catch(() => {});
    }
  }, [client, endpoint, preferences]);
  useEffect(() => {
    try {
      // Persist only safe navigation. Never the token, drafts, snapshots or requests.
      if (endpoint) localStorage.setItem(PREFERENCES, json({ endpoint, openViews: JSON.parse(resumeViews) }));
    } catch { /* Storage may be disabled in a trusted browser. */ }
  }, [endpoint, resumeViews]);
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
    if (!openViews.includes(id) && openViews.length >= 32) { setError('32 Session views are open. Close a view from its Sidebar Session actions, or use Sidebar View options → Close all views.'); return; }
    setOpenViews(current => current.includes(id) ? current : [...current, id]);
    focusSession(id, { attach: true, ready });
  };
  const closeView = (id: string) => {
    const remaining = openViews.filter(item => item !== id);
    setOpenViews(remaining);
    if (selected === id) focusSession(remaining[0]);
    run(() => client.release(id));
  };
  const closeAllViews = () => {
    const closing = [...openViews];
    setOpenViews([]); focusSession();
    run(() => Promise.all(closing.map(id => client.release(id))));
  };
  const createInWorkspace = (id: string) => {
    if (openViews.length >= 32) { setError('32 Session views are open. Close a view from its Sidebar Session actions, or use Sidebar View options → Close all views.'); return; }
    if (creating === state.generation) return;
    navigation.invalidate(); const current = navigation.capture(); const generation = state.generation;
    setCommand(undefined); setCreating(generation);
    run(async () => {
      try {
        const result = await createWorkspaceSession(workspaceHost, id, client, current);
        if (result && current() && generation === client.getSnapshot().generation) {
          focusSession(result.session.id); setOpenViews(value => value.includes(result.session.id) ? value : [...value, result.session.id]);
        }
      } catch (cause) { if (current()) throw cause; }
      finally { if (generation === client.getSnapshot().generation) setCreating(undefined); }
    });
  };
  // Owner-specific Settings navigation from one native authored source owner.
  // The owner arrives as a `SourceTarget` from `ConfigurationApplication.sources`;
  // nothing here parses an application scope, a Session cwd or a display string.
  // A Workspace owner opens only the exact Product-Host-registered Workspace
  // whose canonical directory the native owner names; an unregistered or
  // revoked one is reported explicitly and never rerouted to User authoring,
  // and no Workspace, Session or runtime is allocated to resolve it.
  const openOwningSettings = (owner: SourceTarget) => {
    setError('');
    if (owner.kind === 'user') openSettings(userSettingsTarget);
    else navigationActor.send({ type: 'OPEN.OWNER', directory: owner.directory });
  };
  const deletePreview = (id: string) => run(async () => {
    const generation = client.getSnapshot().generation;
    const result = await client.request({ method: 'session/deletePreview', params: { session_id: id } }, 'deletion');
    if (generation !== client.getSnapshot().generation) return;
    if (result.result.status === 'preview') setPreview(result.result.preview);
    else setError(sessionDeletionNotice(result.result));
  });
  return <AppFrame sidebar={geometry => <SidebarRoot {...geometry} startSession={() => { setCreateOpen(true); }}
    panels={[]}
    browser={(wide, expand) => <WorkspaceNavigation key={state.authorityRevision ?? 0} wide={wide} expand={expand} createOpen={createOpen} closeCreate={() => setCreateOpen(false)} host={workspaceHost} client={client} state={state} endpoint={endpoint} navigation={navigation}
      creating={creating === state.generation} metadataChanged={removed => { if (selected) focusSession(selected, { preserveDraft: true }); else if (removed) setFocus(value => value.workspaceId === removed ? {} : value); }}
      workspaceSettings={(id, label) => openSettings(workspaceSettingsTarget(id, label))} workspace={workspace} selected={selected} selectWorkspace={id => { navigation.invalidate(); setCommand(undefined); setRestored(undefined); setFocus({ workspaceId: id, generation: state.generation }); }}
      openSession={open} openViews={openViews} closeView={closeView} closeAllViews={closeAllViews} createSession={createInWorkspace} deleteSession={deletePreview}
      forkSession={id => open(id, () => {
        setCommand({ request: { id: 'fork' }, current: navigation.capture(), generation: client.getSnapshot().generation, sessionId: id, conversationId: client.getSnapshot().views[id]?.target?.conversation_id });
      })} />}
    settings={wide => <SettingsTrigger wide={wide} onClick={() => openSettings(userSettingsTarget)} />} />}
    rightOpen={inspectorOpen || !!(artifactPreview && artifactPreview.resources === artifacts)} rightPanel={geometry => <RightPanel {...geometry} open={inspectorOpen || !!(artifactPreview && artifactPreview.resources === artifacts)} close={() => { setInspectorOpen(false); setArtifactPreview(undefined); }} title={artifactPreview && artifactPreview.resources === artifacts ? 'Artifact preview' : 'Developer inspector'}>{artifactPreview && artifactPreview.resources === artifacts ? <ArtifactPreview key={artifactPreview.artifact.id} artifact={artifactPreview.artifact} resources={artifacts!} /> : <Inspector log={client.log} state={state} view={view} />}</RightPanel>}
    overlay={<>
      {settingsPage && <Settings page={settingsPage} focus={settingsFocus} onSelect={page => navigationActor.send({ type: 'SELECT', page })} onFocus={focus => navigationActor.send({ type: 'FOCUS', focus })} connection={connection} client={client} target={settingsTarget} host={workspaceHost} onClose={closeSettings} theme={theme} setTheme={setTheme} />}
    </>}>
    {!view && <header className="console-header"><strong>rustX</strong><Button aria-label="Toggle Inspector" onClick={() => { setArtifactPreview(undefined); setInspectorOpen(value => !value); }}><IconInspectOutline12 /></Button></header>}
    {!connected && !view && <section className="notice" aria-label="Connection recovery"><p>{selection.busy ? 'Connecting…' : 'Unable to connect to rustX'}</p>
      {selection.mode === 'local' && !selection.busy && <p>No local managed connection is available. Reopen the launcher URL or configure a Remote App Server in Settings.</p>}
      <Button disabled={selection.busy} onClick={() => void connection.reconnect()}>Reconnect</Button><Button onClick={openConnectionSettings}>Show details</Button>
    </section>}
    {Object.values(state.views).filter(item => item.deletionRecovery).map(item => <section key={item.id} className="notice" aria-label={`Deletion recovery for ${sessionDisplayTitle(item.summary)}`}>
      <p>{sessionDisplayTitle(item.summary)}: {item.deletionRecovery === 'committed_cleanup_pending' ? 'Session removed. Cleanup is still pending.' : 'Deletion durability needs verification.'}</p>
      <Button disabled={!connected || item.recoveringDeletion} onClick={() => run(async () => {
        const result = await client.recoverSessionDeletion(item.id);
        if (result) setError(sessionDeletionNotice(result));
      })}>{item.recoveringDeletion ? 'Recovering deletion…' : 'Retry deletion recovery'}</Button>
    </section>)}
    {(error || navigationError) && <div className="notice error" role="alert">{error || navigationError}<Button size="sm" onClick={() => { setError(''); navigationActor.send({ type: 'DISMISS' }); }}>Dismiss notice</Button></div>}
    {state.uncertain.some(item => !item.sessionId) && <div className="notice" role="status">A global operation needs verification. Inspect Global / other Session diagnostics and check the affected work before trying again.</div>}
    {preview && <section className="delete-preview" aria-label="Confirm Session deletion"><h2>Delete {sessionDisplayTitle(state.sessions.find(session => session.id === preview.session_id) ?? state.views[preview.session_id]?.summary)}?</h2>
      <p>This permanently deletes the Session and its saved history.</p>
      <p>Saved conversations: {preview.owned_conversation_count} · History nodes: {preview.owned_node_count} · Child conversations: {preview.owned_child_count}</p>
      <p>Any active work will be settled before deletion.</p>
      <div className="row"><Button onClick={() => setPreview(undefined)}>Keep Session</Button><Button variant="primary" disabled={!connected || !!state.views[preview.session_id]?.deleting} onClick={() => run(async () => {
        const generation = client.getSnapshot().generation;
        const current = navigation.capture();
        const result = await client.deleteSession(preview.session_id, preview.target_revision);
        if (generation !== client.getSnapshot().generation || !result) return;
        if (result.status === 'deleted' || result.status === 'not_found' || result.status === 'committed_cleanup_pending') {
          const remaining = openViews.filter(id => id !== preview.session_id); setOpenViews(value => value.filter(id => id !== preview.session_id));
          if (current() && selected === preview.session_id) {
            const next = remaining[0] ?? client.getSnapshot().sessions.find(session => session.id !== preview.session_id)?.id;
            if (next) open(next); else focusSession();
          }
        }
        setError(sessionDeletionNotice(result)); setPreview(undefined);
      })}>Confirm delete</Button></div>
    </section>}
    {view ? <section className={`session-panel ${agentCss.root}`} data-phase="active" id="session-view" role="region" aria-labelledby="session-title">
      <header className={agentCss.header}><div className={`${agentCss.titleRow} agent-title-row`}><div className={agentCss.titleCluster}><strong id="session-title" aria-label="Session title">{sessionDisplayTitle(state.sessions.find(session => session.id === view.id) ?? view.summary)}</strong><small aria-label="Session location" title={view.settings?.cwd}>{view.settings?.cwd ?? 'Location unavailable'}</small></div>
        <div className="row"><Menu open={sessionMenuOpen} onClose={() => setSessionMenuOpen(false)} align="end" autoFocus portal
          anchor={<Button aria-label="Session actions" aria-haspopup="menu" aria-expanded={sessionMenuOpen} onClick={() => setSessionMenuOpen(value => !value)}>•••</Button>}
          items={[{ id: 'settings', label: 'Session settings' }, { id: 'export', label: 'Export', disabled: state.connection !== 'connected' }, { id: 'tree', label: 'Session tree', disabled: !attached || commandOpen || !lineageSwitchSafe(view) }]}
          onSelect={id => { setSessionMenuOpen(false); if (id === 'settings') setSessionSettingsOpen(value => !value); else if (id === 'tree') invokeCommand({ id: 'tree' }); else if (id === 'export') run(() => client.exportSession(view.id)); }} />
          <Button aria-label="Toggle Inspector" aria-expanded={inspectorOpen} onClick={() => { setArtifactPreview(undefined); setInspectorOpen(value => !value); }}><IconInspectOutline12 /></Button></div>
      </div>
      <SessionConfiguration key={`${state.authorityRevision}:${view.id}`} client={client} view={view} openOwningSettings={openOwningSettings} />
      {sessionSettingsOpen && <section aria-label="Session settings"><p>Workspace: {view.settings?.cwd ?? 'Unavailable'}</p><AgentControls key={`settings:${view.id}`} client={client} view={view} /><Button onClick={() => setSessionSettingsOpen(false)}>Close Session settings</Button></section>}
      <div className={agentCss.tabs} role="tablist" aria-label="Conversation view" onKeyDown={navigateTabs}>{(['chat', 'trajectory'] as const).map(mode => <Button className={`${agentCss.tab} ${conversationMode === mode ? agentCss.tabActive : ""}`} key={mode} role="tab" id={`view-tab-${mode}`} aria-controls="conversation-view" tabIndex={conversationMode === mode ? 0 : -1} aria-selected={conversationMode === mode} onClick={() => setConversationMode(mode)}>{mode === 'chat' ? 'Chat' : 'Trajectory'}</Button>)}</div>
      </header>
      <SessionStatus product={product} recover={action => {
        if (action === 'connection-settings') openConnectionSettings();
        else if (action === 'connect') void connection.reconnect();
        else if (action === 'refresh') run(() => client.refresh(view.id));
        else focusSession(view.id, { attach: true, preserveDraft: true });
      }} />

      <section className={`conversation-panel ${agentCss.body}`} id="conversation-view" role="tabpanel" aria-labelledby={`view-tab-${conversationMode}`} tabIndex={0}>
      <PreviewContext value={artifact => { if (artifacts) { setArtifactPreview({ artifact, resources: artifacts }); setInspectorOpen(false); } }}><ArtifactContext.Provider value={artifacts}>{conversationMode === 'trajectory' && view.trace ? <Trajectory key={view.id} cache={view.trace} onSelect={id => client.selectTrace(view.id, id)} onLoadDetail={id => { void client.loadTraceDetail(view.id, id); }} loadEarlier={() => run(() => client.loadEarlierTrace(view.id))} latest={() => client.latestTrace(view.id)} /> : <ChatViewport key={`${view.id}:${view.target?.attachment_id ?? state.generation}`}>
        {view.snapshot && <><AgentTranscript snapshot={view.snapshot} history={view.history} loadEarlier={() => run(() => client.loadEarlier(view.id))} latest={() => client.latestTranscript(view.id)}
          lineageSwitchSafe={lineageSwitchSafe(view)} historicalDisabled={composerDisabled || commandOpen} onHistorical={(id, response) => invokeCommand({ id, response })} /><RuntimeFacts snapshot={view.snapshot} />

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
          consumed={consumed} cancellationAvailable={attached && !view.cancellation && !view.snapshot?.shutting_down && !view.snapshot?.durability_failure}
          model={<AgentControls key={`model:${view.id}`} client={client} view={view}/>}
          onCancel={() => run(() => client.cancelTurn(view.id))} onUpload={files => client.upload(view.id, files)} onSend={async (text, receipts, delivery) => {
            const generation = state.generation; setSending(current => ({ ...current, [view.id]: generation })); setError('');
            try { await client.send(view.id, text, receipts, delivery); return generation === client.getSnapshot().generation; }
            catch (cause) { if (generation === client.getSnapshot().generation) setError(String(cause)); return false; }
            finally { if (generation === client.getSnapshot().generation) setSending(current => { const next = { ...current }; delete next[view.id]; return next; }); }
          }} />} /><ConversationStats snapshot={view.snapshot}/></div><Interactions client={client} state={state} view={view} run={run}/></div>
      </section>
      {commandOpen && <CommandPanel key={`${command.generation}:${command.sessionId}:${command.request.id}:${command.request.messageId ?? ''}`} request={command.request} client={client} sessionId={command.sessionId} current={() => command.current() && client.getSnapshot().generation === command.generation}
        succeeded={() => { setConsumed(previous => ({ id: command.request.id, sequence: (previous?.sequence ?? 0) + 1 })); }}
        close={() => {
          if (command.conversationId !== client.getSnapshot().views[command.sessionId]?.snapshot?.conversation_id) setRestored(undefined);
          navigation.invalidate(); setCommand(undefined);
        }} opened={result => {
          if (!command.current() || client.getSnapshot().generation !== command.generation) return;
          focusSession(result.session.id, { ready: () => setRestored({ conversation: result.session.active_conversation_id, content: result.content }) });
          setOpenViews(current => current.includes(result.session.id) ? current : [...current, result.session.id]);
        }} />}

    </section> : <div className="empty"><h2>What would you like to work on?</h2><p>Choose New Session to select a Workspace, or open an existing Session from the sidebar.</p><p>Switching or closing views never cancels work.</p></div>}
  </AppFrame>;
}

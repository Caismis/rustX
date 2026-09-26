import { message } from '../locale/translation';
import { useTranslation, useNotice } from '../locale/react';
import { SettingsNavigationFeedback } from './settings/SettingsNavigationFeedback';
import { ConversationHeader } from './agent/ConversationHeader';
import { useClientSelector, selectShell, sameValue } from '../client/selectors';
import { ConversationLive } from './agent/ConversationLive';
import { ConversationSeat, ConversationStatus } from './agent/ConversationSeat';
import { readTheme, applyTheme } from './appearance';
import { Settings } from './settings/Settings';
import { HttpWorkspaceHost, type ProductHostWorkspaces } from '../workspaces/host';
import { WorkspaceNavigation } from '../workspaces/WorkspaceNavigation';
import { WorkspaceSessionNavigation } from '../workspaces/navigation';
import { useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import { useActorRef } from '@xstate/react';
import type { AppServerClient } from '../client/app-server';
import type { SourceTarget, UserInputBlock } from '../../../protocol/app-server/v23';
import { CommandPanel, type CommandRequest } from './commands/CommandPanel';
import { NavigationEpoch } from './commands/native';
import { available, commands } from './commands/registry';
import { activeAttempt, lineageSwitchSafe, json } from '../bindings/projection';
import { goalDock } from '../bindings/composer-context';
import { ArtifactResources } from '../client/artifacts';
import { ArtifactPreview, PreviewContext, type PreviewArtifact } from './components/ArtifactPreview';
import { ArtifactContext } from './components/Artifact';
import { AppFrame } from '../presentation/layout/AppFrame';
import { SidebarRoot } from '../presentation/sidebar/SidebarRoot';
import { SettingsTrigger } from '../presentation/settings/SettingsRoot';
import { ConnectionController } from '../connection/controller';
import { RightPanel } from '../presentation/right-panel/RightPanel';
import agentCss from '../presentation/agent/Conversation.module.css';
import { Button } from '../presentation/primitives/Button';
import { sessionDisplayTitle } from '../bindings/session-title';
import { sessionDeletionNotice } from '../bindings/session-deletion';
import { LiveInspector } from './Inspector';
import { SessionDeletion } from './SessionDeletion';
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
  const tx = useTranslation();
  const state = useClientSelector(client, selectShell, sameValue);
  const connection = useMemo(() => providedConnection ?? new ConnectionController(client), [providedConnection, client]);
  const selection = useSyncExternalStore(connection.subscribe, connection.getSnapshot);
  type CenterRoute = { kind: 'new-conversation'; workspaceId?: string } | { kind: 'session'; sessionId: string };
  const [draftBinding, setDraftBinding] = useState(0);
  const [center, setCenter] = useState<CenterRoute>({ kind: 'new-conversation' });
  // Top-level Settings navigation is an explicit machine, not an epoch counter.
  // Every navigation-affecting decision re-enters its idle state, which stops
  // any owning-Workspace lookup in flight: a stale lookup therefore has no
  // completion path at all, so neither its success nor its failure can
  // overwrite a newer decision, reopen a closed dialog or publish an obsolete
  // error. Async resolution is preparation, never ongoing navigation authority.
  const navigationActor = useActorRef(settingsNavigationMachine, { input: { lookup: createOwnerLookup(workspaceHost, client) } });
  const openSettings = (target: SettingsTarget) => navigationActor.send({ type: 'OPEN', target });
  const openConnectionSettings = () => navigationActor.send({ type: 'OPEN.CONNECTION' });
  const [inspectorOpen, setInspectorOpen] = useState(false);
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
  const [error, setError] = useNotice();
  const [deletingSession, setDeletingSession] = useState<string>();
  const [presentationAuthority, setPresentationAuthority] = useState(state.authorityRevision);
  // The client owns authority retirement. This render adjustment retires only
  // browser presentation before any children can reinterpret an old Session ID.
  if (presentationAuthority !== state.authorityRevision) {
    setPresentationAuthority(state.authorityRevision);
    navigation.invalidate(); setOpenViews([]); setFocus({}); setCommand(undefined); setRestored(undefined);
    setDeletingSession(undefined); setError(''); setConsumed(undefined);
    setDraftBinding(value => value + 1);
    setCenter({ kind: 'new-conversation' }); setArtifactPreview(undefined);
  }

  // The client owns authority retirement; this retires the browser navigation
  // preparation that belonged to the replaced authority.
  useEffect(() => { navigationActor.send({ type: 'RETIRE' }); }, [navigationActor, state.authorityRevision]);

  const selectedView = selected ? state.views[selected] : undefined;
  const view = selectedView?.deleting && !selectedView.target ? undefined : selectedView;
  const artifacts = useMemo(() => selected && view?.target ? new ArtifactResources(client, selected) : undefined, [client, selected, view?.target, state.generation]);
  useEffect(() => () => artifacts?.dispose(), [artifacts]);
  const connected = state.connection === 'connected';
  const attached = !view?.deleting && connected && view?.attachmentIntent === 'wanted' && view.attachment === 'attached';
  const commandOpen = !!command && command.sessionId === selected && command.generation === state.generation && command.current();
  const invokeCommand = (request: CommandRequest | { id: 'new' }) => {
    const currentState = client.getSnapshot();
    const view = selected ? currentState.views[selected] : undefined;
    if (!view || currentState.connection !== 'connected' || view.attachment !== 'attached' || view.attachmentIntent !== 'wanted' || view.modelMutation || view.snapshot?.shutting_down || view.snapshot?.durability_failure) return;
    const definition = commands.find(item => item.id === request.id);
    if (definition && !available(definition, activeAttempt(view.snapshot), !!goalDock(view.snapshot), lineageSwitchSafe(view))) return;
    if (request.id === 'new') { createInWorkspace(workspace); return; }
    if (request.id === 'goal') {
      document.querySelector<HTMLElement>('[data-goal-dock] button')?.focus();
      setConsumed(previous => ({ id: 'goal', sequence: (previous?.sequence ?? 0) + 1 }));
      return;
    }
    navigation.invalidate();
    setCommand({ request, current: navigation.capture(), generation: state.generation, sessionId: view.id, conversationId: view.target?.conversation_id });
  };
  const runGlobal = (action: () => Promise<unknown>) => {
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
    if (preferences.openViews[0]) setCenter({ kind: 'session', sessionId: preferences.openViews[0] });
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
  const focusSession = (id?: string, options: { attach?: boolean; ready?: () => void; preserveDraft?: boolean; commitDraft?: boolean } = {}) => {
    if (!options.preserveDraft && !options.commitDraft) setDraftBinding(value => value + 1);
    navigation.invalidate(); const current = navigation.capture();
    const generation = state.generation;
    setCenter(id ? { kind: 'session', sessionId: id } : { kind: 'new-conversation' });
    setCommand(undefined); if (!options.preserveDraft) setRestored(undefined); setFocus({ sessionId: id });
    if (!id) return;
    client.restoreViews([id]);
    if (!connected) return;
    runGlobal(async () => {
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
    if (!openViews.includes(id) && openViews.length >= 32) { setError(message('common:copy.32-session-views-are-open-close-a-view-from-its-sidebar-session-actions-or-use-sidebar-vie')); return; }
    setOpenViews(current => current.includes(id) ? current : [...current, id]);
    focusSession(id, { attach: true, ready });
  };
  const closeView = (id: string) => {
    const remaining = openViews.filter(item => item !== id);
    setOpenViews(remaining);
    if (selected === id) focusSession(remaining[0]);
    runGlobal(() => client.release(id));
  };
  const closeAllViews = () => {
    const closing = [...openViews];
    setOpenViews([]); focusSession();
    runGlobal(() => Promise.all(closing.map(id => client.release(id))));
  };
  const createInWorkspace = (id?: string) => {
    setDraftBinding(value => value + 1);
    navigation.invalidate(); setCommand(undefined); setRestored(undefined); setFocus({ workspaceId: id, generation: state.generation });
    setCenter({ kind: 'new-conversation', workspaceId: id });
  };
  const newConversationCurrent = useMemo(() => navigation.capture(), [navigation, center]);
  // Owner-specific Settings navigation from one native authored source owner.
  // The owner arrives as a `SourceTarget` from `ConfigurationApplication.sources`;
  // nothing here parses an application scope, a Session cwd or a display string.
  // A Workspace owner opens only the exact Product-Host-registered Workspace
  // whose canonical directory the native owner names; an unregistered or
  // revoked one is reported explicitly and never rerouted to User authoring,
  // and no Workspace, Session or runtime is allocated to resolve it.
  const openOwningSettings = (owner: SourceTarget) => {
    if (owner.kind === 'user') openSettings(userSettingsTarget);
    else navigationActor.send({ type: 'OPEN.OWNER', directory: owner.directory });
  };
  return <AppFrame sidebar={geometry => <SidebarRoot {...geometry} startSession={() => createInWorkspace(workspace)}
    panels={[]}
    browser={(wide, expand) => <WorkspaceNavigation key={state.authorityRevision ?? 0} wide={wide} expand={expand} host={workspaceHost} client={client} state={state} endpoint={endpoint} navigation={navigation}
      metadataChanged={removed => { if (selected) focusSession(selected, { preserveDraft: true }); else if (removed) setFocus(value => value.workspaceId === removed ? {} : value); }}
      workspaceSettings={(id, label) => openSettings(workspaceSettingsTarget(id, label))} workspace={workspace} selected={selected} selectWorkspace={createInWorkspace}
      openSession={open} openViews={openViews} closeView={closeView} closeAllViews={closeAllViews} createSession={createInWorkspace} deleteSession={setDeletingSession}
      forkSession={id => open(id, () => {
        setCommand({ request: { id: 'fork' }, current: navigation.capture(), generation: client.getSnapshot().generation, sessionId: id, conversationId: client.getSnapshot().views[id]?.target?.conversation_id });
      })} />}
    settings={wide => <SettingsTrigger wide={wide} onClick={() => openSettings(userSettingsTarget)} />} />}
    rightOpen={inspectorOpen || !!(artifactPreview && artifactPreview.resources === artifacts)} rightPanel={geometry => <RightPanel closeLabel={artifactPreview && artifactPreview.resources === artifacts ? tx('artifacts:preview.close') : tx('common:right-panel.close-inspector')} {...geometry} open={inspectorOpen || !!(artifactPreview && artifactPreview.resources === artifacts)} close={() => { setInspectorOpen(false); setArtifactPreview(undefined); }} title={artifactPreview && artifactPreview.resources === artifacts ? tx('common:app.artifact-preview') : tx('common:app.developer-inspector')}>{artifactPreview && artifactPreview.resources === artifacts ? <ArtifactPreview key={artifactPreview.artifact.id} artifact={artifactPreview.artifact} resources={artifacts!} /> : <LiveInspector client={client} sessionId={view?.id} />}</RightPanel>}
    overlay={<>
      {/* Settings renders the navigation machine's state and nothing else: which
          target, page and detail are open is decided there, including whether
          this target may reach the client-owned Connection surface at all. */}
      <Settings navigation={navigationActor} connection={connection} client={client} host={workspaceHost} theme={theme} setTheme={setTheme} />
    </>}>
    {!connected && !view && <section className="notice" aria-label={tx('common:app.connection-recovery')}><p>{selection.busy ? tx('common:app.connecting') : tx('common:app.unable-to-connect-to-rustx')}</p>
      {selection.mode === 'local' && !selection.busy && <p>{tx('common:app.no-local-managed-connection-is-available-reopen-the-launcher-url')}</p>}
      <Button disabled={selection.busy} onClick={() => void connection.reconnect()}>{tx('common:app.reconnect')}</Button><Button onClick={openConnectionSettings}>{tx('common:app.show-details')}</Button>
    </section>}
    {Object.values(state.views).filter(item => item.deletionRecovery).map(item => <section key={item.id} className="notice" aria-label={tx('common:app.deletion-recovery-for-value', { p0: sessionDisplayTitle(tx, item.summary) })}>
      <p>{sessionDisplayTitle(tx, item.summary)}: {item.deletionRecovery === 'committed_cleanup_pending' ? tx('common:app.session-removed-cleanup-is-still-pending') : tx('common:app.deletion-durability-needs-verification')}</p>
      <Button disabled={!connected || item.recoveringDeletion} onClick={() => runGlobal(async () => {
        const result = await client.recoverSessionDeletion(item.id);
        if (result && result.status !== 'deleted' && result.status !== 'not_found') setError(sessionDeletionNotice(result));
      })}>{item.recoveringDeletion ? tx('common:app.recovering-deletion') : tx('common:app.retry-deletion-recovery')}</Button>
    </section>)}
    {error && <div className="notice error" role="alert">{error}<Button size="sm" onClick={() => setError('')}>{tx('common:app.dismiss-notice')}</Button></div>}
    {state.uncertain.some(item => !item.sessionId) && <div className="notice" role="status">{tx('common:app.a-global-operation-needs-verification-inspect-global-other-sessi')}</div>}
    {deletingSession && <SessionDeletion key={JSON.stringify([state.authorityRevision, deletingSession])} client={client} sessionId={deletingSession} title={sessionDisplayTitle(tx, state.sessions.find(session => session.id === deletingSession) ?? state.views[deletingSession]?.summary)} close={() => setDeletingSession(undefined)} deleted={() => {
      const remaining = openViews.filter(id => id !== deletingSession);
      setOpenViews(remaining);
      if (selected === deletingSession) {
        const next = remaining[0] ?? client.getSnapshot().sessions.find(session => session.id !== deletingSession)?.id;
        if (next) open(next); else focusSession();
      }
    }}/>}
    <section className={`session-panel ${agentCss.root}`} data-phase={view ? 'active' : 'hero'} id="session-view" role="region" aria-labelledby="session-title">
      <ConversationHeader client={client} view={view && { ...view, summary: state.sessions.find(session => session.id === view.id) ?? view.summary }} authorityRevision={state.authorityRevision}
        connected={connected} attached={attached} commandOpen={commandOpen} inspectorOpen={inspectorOpen}
        toggleInspector={() => { setArtifactPreview(undefined); setInspectorOpen(value => !value); }} invokeCommand={invokeCommand}
        settingsFeedback={<SettingsNavigationFeedback navigation={navigationActor}/>} openOwningSettings={openOwningSettings} conversationMode={conversationMode} setConversationMode={setConversationMode}/>

      {view && <ConversationStatus client={client} sessionId={view.id} recover={action => {
        if (action === 'connection-settings') openConnectionSettings();
        else if (action === 'connect') void connection.reconnect();
        else if (action === 'refresh') runGlobal(() => client.refresh(view.id));
        else focusSession(view.id, { attach: true, preserveDraft: true });
      }} />}

      <section className={`conversation-panel ${agentCss.body}`} id="conversation-view" role={view ? 'tabpanel' : undefined} aria-labelledby={view ? `view-tab-${conversationMode}` : undefined} tabIndex={0}>
      <PreviewContext value={artifact => { if (artifacts) { setArtifactPreview({ artifact, resources: artifacts }); setInspectorOpen(false); } }}><ArtifactContext.Provider value={artifacts}><ConversationLive client={client} sessionId={view?.id} mode={conversationMode} disabled={commandOpen} onHistorical={(id, response) => invokeCommand({ id, response })}/></ArtifactContext.Provider></PreviewContext>
      <ConversationSeat client={client} host={workspaceHost} sessionId={view?.id}
        initialWorkspace={workspace ?? (center.kind === 'new-conversation' ? center.workspaceId : undefined)}
        binding={String(draftBinding)} current={newConversationCurrent} consumed={consumed} restored={restored}
        onCommand={id => invokeCommand({ id })}
        opened={id => { setOpenViews(current => current.includes(id) ? current : [...current, id]); focusSession(id, { commitDraft: true }); }}/>

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

    </section>
  </AppFrame>;
}

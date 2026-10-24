import { SubagentScope, SubagentSurface, SubagentAside } from './agent/Subagents';
import { useConversationPreferences } from './conversation-preferences';
import { FileLinks } from '../presentation/markdown/FileLinks';
import { Workbench, type WorkbenchHandle } from './components/Workbench';
import { DetachedFirstSubmissions } from './new-conversation/DetachedFirstSubmissions';
import { message } from '../locale/translation';
import { useTranslation, useNotice } from '../locale/react';
import { SettingsNavigationFeedback } from './settings/SettingsNavigationFeedback';
import { ConversationHeader } from './agent/ConversationHeader';
import { useClientSelector, selectShell, sameValue } from '../client/selectors';
import { ConversationWidthControls } from './agent/ConversationWidthControls';
import { ConversationLive } from './agent/ConversationLive';
import { ConversationSeat, ConversationStatus } from './agent/ConversationSeat';
import { readTheme, applyTheme } from './appearance';
import { Settings } from './settings/Settings';
import type { ProductHostWorkspaces } from '../workspaces/host';
import type { WorkbenchTarget } from '../workspaces/workbench';
import { HttpWorkspaceHost } from '../workspaces/http-host';
import { WorkspaceAuthority } from '../workspaces/authority';
import { WorkspaceAssociations } from '../workspaces/associations';
import { WorkspaceNavigation } from '../workspaces/WorkspaceNavigation';
import { WorkspaceSessionAdmission } from '../workspaces/session-admission';
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import { useActorRef, useSelector } from '@xstate/react';
import { shellNavigationMachine, committedRouteCurrent, draftSessionCurrent, type ShellNavigationActor } from './navigation/machine';
import { HistorySurface } from './navigation/HistorySurface';
import type { AppServerClient } from '../client/app-server';
import type { CompletedResponseView, SourceTarget } from '../../../protocol/app-server/v57';
import type { ForkOrigin } from './agent/ForkPoint';
import { CommandPanel, type CommandRequest } from './commands/CommandPanel';
import { type ResponseAction } from './commands/native';
import { available, commands } from './commands/registry';
import { activeAttempt, lineageSwitchSafe, json } from '../bindings/projection';
import { goalDock } from '../bindings/composer-context';

import { PreviewContext } from './components/ArtifactPreview';
import { PreviewWorkspaceOwner } from './preview-workspace';
import { PreviewWorkspace } from './components/PreviewWorkspace';
import { ArtifactResources } from '../client/artifacts';
import { ArtifactContext } from './components/Artifact';
import { AppFrame, type SidebarGeometry } from '../presentation/layout/AppFrame';
import { SidebarRoot } from '../presentation/sidebar/SidebarRoot';
import { SettingsTrigger } from '../presentation/settings/SettingsRoot';
import { ConnectionController } from '../connection/controller';
import { RightPanel } from '../presentation/right-panel/RightPanel';
import agentCss from '../presentation/agent/Conversation.module.css';
import { Button } from '../presentation/primitives/Button';
import { IconPanelLeftOutline16 } from '../presentation/primitives/icons';
import { sessionDisplayTitle } from '../bindings/session-title';
import { sessionDeletionNotice } from '../bindings/session-deletion';
import { SessionDeletion } from './SessionDeletion';
import { userSettingsTarget, workspaceSettingsTarget, type SettingsTarget } from './settings/projection';
import { createOwnerLookup, settingsNavigationMachine } from './settings/machines/navigation';
import { RepairNotice } from './RepairNotice';
import { inspectionAcknowledger, RepairAcknowledgements } from './repair-notices';

import type { ChatViewport } from '../presentation/layout/ChatViewport';
import { ComposerDraft } from './composer/draft';
import { firstSubmissions } from './new-conversation/first-submit';
import { workflowSurfaceMachine, type WorkflowSurfaceActor } from './workflows/machines/surface';
import { createWorkflowPort } from './workflows/port';
import { WorkflowsSurface } from './workflows/ui/WorkflowsSurface';
import { IconWorkflow } from './workflows/ui/icons';
import workflowCss from './workflows/ui/Workflows.module.css';

const PREFERENCES = 'rustx-console-view-v2';
function readPreferences(): { endpoint?: string; openViews: string[] } {
  try {
    const value = JSON.parse(localStorage.getItem(PREFERENCES) ?? 'null');
    if (value && typeof value.endpoint === 'string' && Array.isArray(value.openViews)) return { endpoint: value.endpoint, openViews: [...new Set<string>(value.openViews.filter((id: unknown) => typeof id === 'string'))].slice(0, 32) };
  } catch { /* Preferences are optional presentation, never recovery input. */ }
  return { openViews: [] };
}
const defaultWorkspaceHost = new HttpWorkspaceHost();
type AppProps = { client: AppServerClient; workspaceHost?: ProductHostWorkspaces; connection?: ConnectionController };
/** Browser-session owners outlive the disposable rendered projection. */
export function App({ client, workspaceHost = defaultWorkspaceHost, connection: providedConnection }: AppProps) {
  const workspaceAuthority = useMemo(() => new WorkspaceAuthority(workspaceHost), [workspaceHost]);
  const admission = useMemo(() => new WorkspaceSessionAdmission(workspaceAuthority, client), [workspaceAuthority, client]);
  useEffect(() => client.setSessionAdmission(admission.admit), [client, admission]);
  const connection = useMemo(() => providedConnection ?? new ConnectionController(client), [providedConnection, client]);
  // Repair acknowledgements of this browser, shared by the notice and by
  // Settings, which completes a View details inspection.
  const repairAcknowledgements = useMemo(() => new RepairAcknowledgements(), []);
  const settings = useActorRef(settingsNavigationMachine, { input: {
    lookup: createOwnerLookup(workspaceHost, client),
    inspected: inspectionAcknowledger(client, repairAcknowledgements),
  } });
  const [preferences] = useState(readPreferences);
  const first = preferences.endpoint === client.getSnapshot().endpoint ? preferences.openViews[0] : undefined;
  const navigation = useActorRef(shellNavigationMachine, { input: { client, settings, route: first && client.isAttachmentControlCurrent(first, client.getSnapshot().views[first]?.attachmentObservation) ? { kind: 'session', sessionId: first } : undefined } });
  const [composerDraft] = useState(() => new ComposerDraft());
  // Browser-lifetime Workflows surface: its open file target and unsaved
  // edits survive leaving the route, and never follow conversation focus.
  // Each port binds the App Server and Product Host authorities current when
  // it is created; transactions keep their port, so they are never retargeted.
  const workflows = useActorRef(workflowSurfaceMachine, { input: { ports: scope => createWorkflowPort({ client, host: () => workspaceHost, hostAuthority: workspaceAuthority.capture(), scope }) } });
  return <AppSurface composerDraft={composerDraft} client={client} workspaceHost={workspaceHost} workspaceAuthority={workspaceAuthority} connection={connection} navigation={navigation} repairAcknowledgements={repairAcknowledgements} workflows={workflows}/>;
}
/** Route rendering and gestures; it owns no native admission or actor lifetime. */
export function AppSurface({ client, workspaceHost, workspaceAuthority, connection, navigation: shell, composerDraft, repairAcknowledgements, workflows }: { composerDraft: ComposerDraft; client: AppServerClient; workspaceHost: ProductHostWorkspaces; workspaceAuthority: WorkspaceAuthority; connection: ConnectionController; navigation: ShellNavigationActor; repairAcknowledgements: RepairAcknowledgements; workflows: WorkflowSurfaceActor }) {
  const fileWorkbench = useRef<WorkbenchHandle>(null);
  const tx = useTranslation();
  const state = useClientSelector(client, selectShell, sameValue);
  const fileAuthority = useSyncExternalStore(workspaceAuthority.subscribe, workspaceAuthority.getRevision);
  const associations = useMemo(() => new WorkspaceAssociations(client, workspaceAuthority), [client, workspaceAuthority]);
  useEffect(() => associations.start(), [associations]);
  const selection = useSyncExternalStore(connection.subscribe, connection.getSnapshot);
  // Top-level Settings navigation is an explicit machine, not an epoch counter.
  // Every navigation-affecting decision re-enters its idle state, which stops
  // any owning-Workspace lookup in flight: a stale lookup therefore has no
  // completion path at all, so neither its success nor its failure can
  // overwrite a newer decision, reopen a closed dialog or publish an obsolete
  // error. Async resolution is preparation, never ongoing navigation authority.
  const navigationActor = shell.getSnapshot().context.settings;
  const settingsOpen = useSelector(navigationActor, snapshot => !!snapshot.context.page || snapshot.matches('resolvingOwner'));
  const previewOwner = useMemo(() => new PreviewWorkspaceOwner(client, workspaceHost, workspaceAuthority), [client, workspaceHost, workspaceAuthority]);
  const previews = useSyncExternalStore(previewOwner.subscribe, previewOwner.getSnapshot);
  useEffect(() => previewOwner.start(), [previewOwner]);
  const [previewFocus, setPreviewFocus] = useState(0);
  const previewOpener = useRef<HTMLElement | null>(null);
  const [workbenchFullscreen, setWorkbenchFullscreen] = useState(false);
  const returnPreviewFocus = () => {
    const opener = previewOpener.current;
    const target = opener?.isConnected && !opener.closest('[inert]') && opener.checkVisibility() ? opener : document.querySelector<HTMLElement>('[data-preview-toggle]') ?? document.querySelector<HTMLElement>('#conversation-view');
    target?.focus({ preventScroll: true });
  };
  const collapsePanel = () => { previewOwner.collapse(); returnPreviewFocus(); };
  const [theme, setTheme] = useState(readTheme);
  useLayoutEffect(() => applyTheme(theme), [theme]);
  const [displayPreferences] = useConversationPreferences();
  useLayoutEffect(() => { document.body.style.setProperty('--dsh-content-font-size', `${displayPreferences.fontSize}px`); }, [displayPreferences.fontSize]);
  const [requestedConversationMode, setConversationMode] = useState<'chat' | 'trajectory'>('chat');
  const conversationMode = displayPreferences.codingView ? requestedConversationMode : 'chat';
  const [preferences] = useState(readPreferences);
  const endpoint = state.endpoint ?? '';
  const initialViews = preferences.endpoint === state.endpoint ? preferences.openViews : [];
  const [openViews, setOpenViews] = useState<string[]>(initialViews);
  const routeState = useSelector(shell, snapshot => snapshot);
  useEffect(() => { const subscription = shell.subscribe(() => client.recheckAdmissions()); return () => subscription.unsubscribe(); }, [shell, client]);
  const previewCurrent = useMemo(() => { const context = routeState.context; return () => shell.getSnapshot().context === context; }, [shell, routeState.context]);
  const center = routeState.context.route;
  const draftBinding = routeState.context.binding;
  const selected = center.kind === 'session' || center.kind === 'session-draft' ? center.sessionId : undefined;
  const command = routeState.context.command;
  const restored = routeState.context.restored;
  const commandCurrent = () => !!command && shell.getSnapshot().context.command === command && shell.getSnapshot().context.route === command.route && client.getSnapshot().generation === command.generation;
  useEffect(() => associations.select(selected), [associations, selected]);
  const selectedCwd = state.sessions.find(row => row.id === selected)?.cwd ?? (selected ? state.views[selected]?.summary?.cwd ?? state.views[selected]?.settings?.cwd : undefined);
  const associatedWorkspace = useSyncExternalStore(associations.subscribe, () => {
    const association = selected ? associations.getSnapshot().entries.get(selected) : undefined;
    return association?.cwd === selectedCwd ? association?.confirmed?.workspaceId : undefined;
  });
  const workspace = selected ? associatedWorkspace : center.kind === 'new-conversation' ? center.workspaceId : undefined;
  const retirePendingSidebarNavigation = useCallback(() => shell.send({ type: 'SIDEBAR.DISMISS' }), [shell]);
  const openSettings = (target: SettingsTarget) => shell.send({ type: 'SETTINGS', destination: { type: 'OPEN', target } });
  const openConnectionSettings = () => shell.send({ type: 'SETTINGS', destination: { type: 'OPEN.CONNECTION' } });
  const [consumed, setConsumed] = useState<{ id: string; sequence: number }>();
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
    setOpenViews([]);
    setDeletingSession(undefined); setError(''); setConsumed(undefined);
  }

  useEffect(() => { if (center.kind === 'session' || center.kind === 'session-draft') { client.restoreViews([center.sessionId]); setOpenViews(open => open.includes(center.sessionId) ? open : [...open, center.sessionId]); } }, [client, center]);

  const selectedView = selected ? state.views[selected] : center.kind === 'new-conversation' && routeState.context.draftSession ? state.views[routeState.context.draftSession.sessionId] : undefined;
  const view = selectedView?.deleting && !selectedView.target ? undefined : selectedView;
  const workbenchTarget: WorkbenchTarget | undefined = view?.summary
    ? { session_id: view.id, active_node: view.summary.active_node }
    : center.kind === 'new-conversation' && workspace ? { workspace_id: workspace } : undefined;
  useLayoutEffect(() => previewOwner.selectSession(selected), [previewOwner, selected]);
  const artifacts = useMemo(() => selected && view?.target ? new ArtifactResources(client, selected) : undefined, [client, selected, view?.target, state.generation, state.authorityRevision, fileAuthority]);
  useEffect(() => () => artifacts?.dispose(), [artifacts]);
  const connected = state.connection === 'connected';
  const attached = center.kind === 'session' && !view?.deleting && connected && view?.attachmentIntent === 'wanted' && view.attachment === 'attached';
  const commandOpen = !!command && command.route.kind === 'session' && command.route.sessionId === selected && commandCurrent();
  /** The selected view when it can take a Session command right now. */
  const commandable = () => {
    if (center.kind !== 'session') return;
    const currentState = client.getSnapshot();
    const view = selected ? currentState.views[selected] : undefined;
    if (view && firstSubmissions(client).session(view.id) && !['admitted', 'discarded'].includes(firstSubmissions(client).session(view.id)!.phase)) return;
    if (!view || currentState.connection !== 'connected' || view.attachment !== 'attached' || view.attachmentIntent !== 'wanted' || view.modelMutation || view.snapshot?.shutting_down || view.snapshot?.durability_failure) return;
    return view;
  };
  const invokeCommand = (request: CommandRequest | { id: 'new' } | { id: 'compact' }) => {
    const view = commandable();
    if (!view) return;
    const definition = commands.find(item => item.id === request.id);
    if (definition && !available(definition, activeAttempt(view.snapshot), !!goalDock(view.snapshot), lineageSwitchSafe(view))) return;
    if (request.id === 'new') { createInWorkspace(workspace); return; }
    if (request.id === 'compact') {
      client.compact(view.id, () => setConsumed(previous => ({ id: 'compact', sequence: (previous?.sequence ?? 0) + 1 })));
      if (command && commandCurrent()) shell.send({ type: 'COMMAND.CLOSE', command });
      return;
    }
    if (request.id === 'goal') {
      document.querySelector<HTMLElement>('[data-goal-dock] button')?.focus();
      setConsumed(previous => ({ id: 'goal', sequence: (previous?.sequence ?? 0) + 1 }));
      return;
    }
    shell.send({ type: 'COMMAND', request });
  };
  // A completed response already names its exact boundary, so its Fork and
  // Retry run on the click, as Harness forkAt does: no confirming chooser.
  const transitionResponse = (action: ResponseAction, response: CompletedResponseView) => {
    const view = commandable();
    if (view) shell.send({ type: 'OPEN', destination: { kind: 'transition', sessionId: view.id, action, response } });
  };
  const runGlobal = (action: () => Promise<unknown>) => {
    const generation = client.getSnapshot().generation; setError('');
    void action().catch(cause => { if (generation === client.getSnapshot().generation) setError(String(cause)); });
  };
  // Subscription cleanup is presentation-only. No component owns socket/runtime life.
  const preferencesApplied = useRef(false);
  useEffect(() => {
    if (!endpoint || state.connection !== 'connected' || settingsOpen || preferencesApplied.current) return;
    preferencesApplied.current = true;
    if (preferences.endpoint !== endpoint || shell.getSnapshot().context.route.kind !== 'new-conversation' || shell.getSnapshot().context.binding !== 0 || shell.getSnapshot().context.pending) return;
    client.restoreViews(preferences.openViews);
    setOpenViews(preferences.openViews);
    if (preferences.openViews[0]) shell.send({ type: 'OPEN', destination: { kind: 'restore-session', sessionId: preferences.openViews[0], sidebar: false } });
    if (client.getSnapshot().connection === 'connected') for (const id of preferences.openViews.filter(id => id !== preferences.openViews[0])) {
      if (!client.getSnapshot().views[id]?.target && (!firstSubmissions(client).session(id) || ['admitted', 'discarded'].includes(firstSubmissions(client).session(id)!.phase))) void client.attach(id).catch(() => {});
    }
  }, [client, shell, endpoint, preferences, state.connection, settingsOpen]);
  useEffect(() => {
    try {
      // Persist only safe navigation. Never the token, drafts, snapshots or requests.
      if (endpoint) localStorage.setItem(PREFERENCES, json({ endpoint, openViews: JSON.parse(resumeViews) }));
    } catch { /* Storage may be disabled in a trusted browser. */ }
  }, [endpoint, resumeViews]);
  /** Navigation away from the Workflows area asks the Workflows surface
   * first, so unsaved semantic edits are never dropped silently. */
  const leaveWorkflows = (proceed: () => void) => {
    if (shell.getSnapshot().context.route.kind === 'workflows') workflows.send({ type: 'LEAVE', proceed });
    else proceed();
  };
  const focusSession = (id?: string) => {
    if (id) shell.send({ type: 'OPEN', destination: { kind: 'session', sessionId: id, sidebar: false } });
    else shell.send({ type: 'DRAFT' });
  };
  const open = (id: string, command?: CommandRequest, sidebar?: SidebarGeometry) => {
    if (!openViews.includes(id) && openViews.length >= 32) { setError(message('common:copy.32-session-views-are-open-close-a-view-from-its-sidebar-session-actions-or-use-sidebar-vie')); return; }
    leaveWorkflows(() => shell.send({ type: 'OPEN', destination: { kind: 'session', sessionId: id, sidebar: !!sidebar?.overlay, command } }));
  };
  const openForkSource = (origin: ForkOrigin, beginViewport?: ChatViewport['beginNavigation']) => {
    if (center.kind === 'session' && newConversationCurrent() && client.isAttachmentObservationCurrent(center.sessionId, client.getSnapshot().views[center.sessionId]?.attachmentObservation)) {
      shell.send({ type: 'OPEN', destination: { kind: 'history', origin, beginViewport } });
    }
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
  const focusDraft = () => document.querySelector<HTMLTextAreaElement>('[data-resident-composer] textarea')?.focus();
  const createInWorkspace = (id?: string) => leaveWorkflows(() => {
    shell.send({ type: 'DRAFT', workspaceId: id }); focusDraft();
  });
  const newConversationCurrent = useMemo(() => committedRouteCurrent(shell), [shell, routeState.context.control, state.generation, state.authorityRevision]);
  const startupCurrent = useMemo(() => routeState.context.draftSession ? draftSessionCurrent(shell, routeState.context.draftSession.intent) : newConversationCurrent, [shell, routeState.context.draftSession, newConversationCurrent]);
  // Owner-specific Settings navigation from one native authored source owner.
  // The owner arrives as a `SourceTarget` from `ConfigurationApplication.sources`;
  // nothing here parses an application scope, a Session cwd or a display string.
  // A Workspace owner opens only the exact Product-Host-registered Workspace
  // whose canonical directory the native owner names; an unregistered or
  // revoked one is reported explicitly and never rerouted to User authoring,
  // and no Workspace, Session or runtime is allocated to resolve it.
  const openOwningSettings = (owner: SourceTarget) => {
    if (owner.kind === 'user') openSettings(userSettingsTarget);
    else shell.send({ type: 'SETTINGS', destination: { type: 'OPEN.OWNER', directory: owner.directory } });
  };
  return <SubagentScope onMode={setConversationMode} client={client} sessionId={view?.id} openAside={agent => { previewOwner.showWorkbench(); fileWorkbench.current?.openAgent(agent.agent_id, agent.title); }}><PreviewContext value={center.kind === 'history' ? undefined : { openPreview: artifact => { previewOpener.current = document.activeElement as HTMLElement; previewOwner.openPreview(artifact); setPreviewFocus(value => value + 1); }, download: artifact => { void previewOwner.download(artifact); } }}><AppFrame sidebarPublication={routeState.context.publication} dismissSidebarLabel={tx('sidebar:toggle.dismiss')} onSidebarDismiss={retirePendingSidebarNavigation} sidebar={geometry => <SidebarRoot {...geometry} startSession={() => { createInWorkspace(workspace); geometry.dismissSidebar(); }}
    panels={[{ id: 'workflows', label: tx('workflows:nav.label'), active: center.kind === 'workflows',
      icon: <IconWorkflow size={geometry.collapsed ? 18 : 16} className={center.kind === 'workflows' ? workflowCss.navActive : undefined} />,
      select: () => { if (center.kind !== 'workflows') shell.send({ type: 'WORKFLOWS' }); geometry.dismissSidebar(); } }]}
    browser={(wide, expand) => <WorkspaceNavigation associations={associations} key={state.authorityRevision ?? 0} wide={wide} expand={expand} host={workspaceHost} client={client} state={state} endpoint={endpoint}
      metadataChanged={removed => { if (removed && center.kind === 'new-conversation' && center.workspaceId === removed) shell.send({ type: 'DRAFT' }); }}
      workspaceSettings={(id, label) => { openSettings(workspaceSettingsTarget(id, label)); geometry.dismissSidebar(); }} workspace={workspace} selected={selected}
      draft={center.kind === 'new-conversation' && workspace ? { workspaceId: workspace, binding: String(draftBinding), focus: focusDraft } : undefined} selectWorkspace={id => { createInWorkspace(id); geometry.dismissSidebar(); }}
      openSession={id => open(id, undefined, geometry)} openViews={openViews} closeView={closeView} closeAllViews={closeAllViews} createSession={id => { createInWorkspace(id); geometry.dismissSidebar(); }} deleteSession={setDeletingSession}
      forkSession={id => leaveWorkflows(() => shell.send({ type: 'OPEN', destination: { kind: 'fork-session', sessionId: id, sidebar: !!geometry.overlay } }))} />}
    settings={wide => <SettingsTrigger wide={wide} onClick={() => { openSettings(userSettingsTarget); geometry.dismissSidebar(); }} />} />}
    rightOpen={previews.mode !== 'collapsed'} rightPanel={geometry => <RightPanel docked={previews.mode !== 'preview'} {...geometry} fullscreen={previews.mode === 'workbench' ? workbenchFullscreen : previews.mode === 'preview' && previews.workspace?.fullscreen} open={previews.mode !== 'collapsed'} title={previews.mode === 'preview' ? tx('artifacts:workspace.title') : tx('artifacts:workbench.start')}>
      <Workbench renderAgent={(id, visible) => <SubagentAside id={id} visible={visible}/>} fileRef={fileWorkbench} key={JSON.stringify([state.generation, selected, workbenchTarget, fileAuthority])} closePanel={collapsePanel} host={workspaceHost} scope={workspaceAuthority.getCatalog()} target={workbenchTarget} visible={previews.mode === 'workbench'} floatingVisible={previews.mode === 'workbench' || previews.mode === 'collapsed'} fullscreen={workbenchFullscreen || !geometry.canShow} toggleFullscreen={() => { if (!geometry.canShow) collapsePanel(); else setWorkbenchFullscreen(value => !value); }}/>
      <PreviewWorkspace openWorkbench={() => previewOwner.showWorkbench()} owner={previewOwner} snapshot={previews} focusRequest={previewFocus} returnFocus={returnPreviewFocus}/>
    </RightPanel>}
    overlay={<>
      {/* Settings renders the navigation machine's state and nothing else: which
          target, page and detail are open is decided there, including whether
          this target may reach the client-owned Connection surface at all. */}
      <Settings associations={associations} navigation={navigationActor} connection={connection} client={client} host={workspaceHost} theme={theme} setTheme={setTheme} />
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
    <DetachedFirstSubmissions client={client} binding={String(draftBinding)} active={!!selected}/>
    {(previews.problem || previews.downloadError) && <div className="notice error" role="alert">{previews.problem ? tx(`artifacts:workspace.limit-${previews.problem}`) : previews.downloadError}<Button size="sm" onClick={() => previewOwner.dismissProblem()}>{tx('common:app.dismiss-notice')}</Button></div>}
    {(error || routeState.context.error) && <div className="notice error" role="alert">{error || routeState.context.error}<Button size="sm" onClick={() => { setError(''); shell.send({ type: 'DISMISS.ERROR' }); }}>{tx('common:app.dismiss-notice')}</Button></div>}
    {/* Native automatic configuration repairs: nonblocking, never a modal,
        and silent for a healthy source. View details opens the exact owner,
        which acknowledges the receipt once it has presented its details. */}
    <RepairNotice client={client} acknowledgements={repairAcknowledgements}
      viewDetails={({ source, authorityId, receiptId }) => shell.send({ type: 'SETTINGS', destination: { type: 'OPEN.REPAIRS', source, inspection: { authorityId, receiptId } } })} />
    {state.uncertain.some(item => !item.sessionId) && <div className="notice" role="status">{tx('common:app.a-global-operation-needs-verification-review-connection-settings')}</div>}
    {deletingSession && <SessionDeletion key={JSON.stringify([state.authorityRevision, deletingSession])} client={client} sessionId={deletingSession} title={sessionDisplayTitle(tx, state.sessions.find(session => session.id === deletingSession) ?? state.views[deletingSession]?.summary)} close={() => setDeletingSession(undefined)} deleted={() => {
      const remaining = openViews.filter(id => id !== deletingSession);
      setOpenViews(remaining);
      if (selected === deletingSession) {
        const next = remaining[0] ?? client.getSnapshot().sessions.find(session => session.id !== deletingSession)?.id;
        if (next) open(next); else focusSession();
      }
    }}/>}
    {center.kind === 'workflows' ? <WorkflowsSurface surface={workflows} authority={workspaceAuthority} /> : <FileLinks.Provider value={center.kind === 'history' ? undefined : file => { previewOwner.showWorkbench(); void fileWorkbench.current?.openFile(file.path, file.line); }}><section className={`session-panel ${agentCss.root}`} data-phase={view || center.kind === 'history' ? 'active' : 'hero'} data-shell-route={center.kind} id="session-view" role="region" aria-labelledby="session-title">
      {center.kind === 'history' ? <HistorySurface key={JSON.stringify([center.sessionId, center.nodeId, center.messageId, center.window.cut])} client={client} route={center} settings={navigationActor} current={newConversationCurrent} back={() => shell.send({ type: 'RETURN', route: center })}/> : <>
      <ConversationHeader workspaceId={workspace} host={workspaceHost} authority={workspaceAuthority} client={client} view={view && { ...view, summary: state.sessions.find(session => session.id === view.id) ?? view.summary }} authorityRevision={state.authorityRevision}
        connected={connected} attached={attached} commandOpen={commandOpen}
        previewToggle={<Button size="sm" className={agentCss.iconButton} data-preview-toggle aria-label={tx(previews.workspace ? 'artifacts:workspace.reopen' : 'artifacts:workbench.toggle')} title={tx('artifacts:workbench.toggle')} aria-expanded={previews.mode === 'preview' || previews.mode === 'workbench'} onClick={event => { previewOpener.current = event.currentTarget; if (previews.mode === 'preview' || previews.mode === 'workbench') collapsePanel(); else { if (previews.workspace) previewOwner.reveal(); else previewOwner.showWorkbench(); setPreviewFocus(value => value + 1); } }}><IconPanelLeftOutline16 className={agentCss.mirrored} /></Button>} invokeCommand={invokeCommand}
        settingsFeedback={<SettingsNavigationFeedback navigation={navigationActor}/>} openOwningSettings={openOwningSettings} conversationMode={conversationMode} setConversationMode={setConversationMode}/>

      {view && <ConversationStatus client={client} sessionId={view.id} recover={action => {
        if (action === 'connection-settings') openConnectionSettings();
        else if (action === 'connect') void connection.reconnect();
        else if (action === 'refresh') runGlobal(() => client.refresh(view.id));
        else focusSession(view.id);
      }} />}

      <section className={`conversation-panel ${agentCss.body}`} id="conversation-view" role={view && displayPreferences.codingView ? 'tabpanel' : undefined} aria-labelledby={view && displayPreferences.codingView ? `view-tab-${conversationMode}` : undefined} tabIndex={0}>
      <ConversationWidthControls active={!!view && conversationMode === 'chat'}/>
      <ArtifactContext.Provider value={artifacts}><SubagentSurface mode={conversationMode}><div className={agentCss.scrollBody} data-conversation-scroll>
      <ConversationLive previewCurrent={previewCurrent} onOpenSource={openForkSource} onMode={setConversationMode} client={client} sessionId={view?.id} mode={conversationMode} retry={routeState.context.retry} historicalPending={routeState.matches('preparing')} disabled={commandOpen} onHistorical={transitionResponse}/>
      <ConversationSeat draftOwner={composerDraft} client={client} host={workspaceHost} authority={workspaceAuthority} associations={associations} sessionId={view?.id}
        initialWorkspace={workspace ?? (center.kind === 'new-conversation' ? center.workspaceId : undefined)}
        workspacePicked={id => { if (newConversationCurrent()) shell.send({ type: 'DRAFT.WORKSPACE', workspaceId: id }); }}
        transitioning={!!routeState.context.retry} binding={String(draftBinding)} current={center.kind !== 'session' && routeState.context.draftSession ? startupCurrent : newConversationCurrent} executable={center.kind === 'session'} consumed={consumed} restored={restored}
        onCommand={id => { if (id !== 'model') invokeCommand({ id }); }}
        preparePublication={() => {
          shell.send({ type: 'CREATE', binding: String(draftBinding), control: routeState.context.control });
          const pending = shell.getSnapshot().context.pending;
          return id => {
            shell.send({ type: 'CREATED', sessionId: id, control: routeState.context.control, generation: state.generation, pending });
            return committedRouteCurrent(shell);
          };
        }}/>

      </div></SubagentSurface></ArtifactContext.Provider>
      </section>
      {commandOpen && command?.route.kind === 'session' && <CommandPanel key={`${command.generation}:${command.route.sessionId}:${command.request.id}:${command.request.messageId ?? ''}`} request={command.request} client={client} sessionId={command.route.sessionId} current={commandCurrent}
        succeeded={() => setConsumed(previous => ({ id: command.request.id, sequence: (previous?.sequence ?? 0) + 1 }))}
        close={() => shell.send({ type: 'COMMAND.CLOSE', command })}
        navigate={choice => { if (!commandCurrent() || command.route.kind !== 'session') return; shell.send({ type: 'OPEN', destination: choice.kind === 'node' ? { ...choice, sessionId: command.route.sessionId } : { kind: 'transition', sessionId: command.route.sessionId, action: choice.action, selection: choice.selection } }); }} />}
      </>}

    </section></FileLinks.Provider>}
  </AppFrame></PreviewContext></SubagentScope>;
}

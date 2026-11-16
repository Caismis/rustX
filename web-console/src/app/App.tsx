import { Toast } from '../presentation/primitives/Toast';
import { IconWarningOutline16 } from '../presentation/primitives/icons';
import { useToastNotice } from '../locale/use-toast-notice';
import { actionError } from '../bindings/action-error';
import { SubagentScope, SubagentSurface, SubagentAside } from './agent/Subagents';
import { useConversationPreferences } from './conversation-preferences';
import { FileLinks } from '../presentation/markdown/FileLinks';
import { Workbench, type WorkbenchHandle } from './components/Workbench';
import { WorkbenchPreviewRetention } from './workbench-previews';
import { PreviewContentCache } from '../client/preview-content';
import { DetachedFirstSubmissions } from './new-conversation/DetachedFirstSubmissions';
import { message } from '../locale/translation';
import { useTranslation } from '../locale/react';
import { SettingsNavigationFeedback } from './settings/SettingsNavigationFeedback';
import { ConversationHeader } from './agent/ConversationHeader';
import { BtwPanel } from './agent/BtwPanel';
import { DocumentTitle } from './DocumentTitle';
import { useClientSelector, selectShell, sameValue } from '../client/selectors';
import { ConversationWidthControls } from './agent/ConversationWidthControls';
import { ConversationLive } from './agent/ConversationLive';
import { ConversationSeat, ConversationStatus } from './agent/ConversationSeat';
import { ConversationQuestionPanels } from './agent/QuestionPanels';
import { readTheme, applyTheme } from './appearance';
import { Settings } from './settings/Settings';
import { OverlayScope } from '../presentation/primitives/DialogSurface';
import type { ProductHostWorkspaces } from '../workspaces/host';
import type { WorkbenchTarget } from '../workspaces/workbench';
import { HttpWorkspaceHost } from '../workspaces/http-host';
import { WorkspaceAuthority } from '../workspaces/authority';
import { WorkspaceAssociations } from '../workspaces/associations';
import { preferredWorkspace } from '../workspaces/preferred';
import { WorkspaceSelectionPreference } from '../workspaces/selection';
import { sameEndpoint } from '../workspaces/endpoint';
import { WorkspaceNavigation } from '../workspaces/WorkspaceNavigation';
import { WorkspaceSessionAdmission } from '../workspaces/session-admission';
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import { useActorRef, useSelector } from '@xstate/react';
import { shellNavigationMachine, isHomeRoute, committedRouteCurrent, draftSessionCurrent, type ShellNavigationActor } from './navigation/machine';
import { navigationPreferencesKey, readNavigationPreferences, restoreRoute, resumableViews, saveRoute } from './navigation/preferences';
import type { AppServerClient } from '../client/app-server';
import type { CompletedResponseView, SourceTarget } from '../../../protocol/app-server/v79';
import { CommandPanel, type CommandRequest } from './commands/CommandPanel';
import { type ResponseAction } from './commands/native';
import { available, commands } from './commands/registry';
import { activeAttempt, lineageSwitchSafe, json } from '../bindings/projection';
import { goalDock } from '../bindings/composer-context';

import { PreviewContext } from './components/ArtifactPreview';
import { ChangesReviewContext } from './changes-review';
import { PreviewWorkspaceOwner } from './preview-workspace';
import { PreviewWorkspace } from './components/PreviewWorkspace';
import { ArtifactResources } from '../client/artifacts';
import { ArtifactContext } from './components/Artifact';
import { AppFrame, type SidebarGeometry } from '../presentation/layout/AppFrame';
import { HomeSidebar } from '../presentation/sidebar/HomeSidebar';
import { PrimaryNavigationRail } from '../presentation/sidebar/PrimaryNavigationRail';
import { ConnectionController } from '../connection/controller';
import { ConnectionFeedback } from './ConnectionFeedback';
import { RightPanel } from '../presentation/right-panel/RightPanel';
import agentCss from '../presentation/agent/Conversation.module.css';
import { Button } from '../presentation/primitives/Button';
import { IconPanelLeftOutline16, IconQuestionOutline14 } from '../presentation/primitives/icons';
import { sessionDisplayTitle } from '../bindings/session-title';
import { sessionDeletionNotice } from '../bindings/session-deletion';
import { SessionDeletion } from './SessionDeletion';
import { userSettingsTarget, workspaceSettingsTarget, type SettingsTarget } from './settings/projection';
import { createOwnerLookup, settingsNavigationMachine } from './settings/machines/navigation';
import { RepairNotice } from './RepairNotice';
import { inspectionAcknowledger, RepairAcknowledgements } from './repair-notices';

import { ComposerDraft } from './composer/draft';
import { firstSubmissions } from './new-conversation/first-submit';
import { draftPreparations } from './new-conversation/preparation';
import { workflowSurfaceMachine, type WorkflowSurfaceActor } from './workflows/machines/surface';
import { createWorkflowPort } from './workflows/port';
import { WorkflowsSurface } from './workflows/ui/WorkflowsSurface';
import { ResourceSurface } from './resources/ResourceSurface';
import {McpIcon} from './resources/mcp/McpIcon';
import {IconAgentPresetOutline16,IconCodeOutline16} from '../presentation/primitives/icons';
import { IconWorkflow } from './workflows/ui/icons';
import { TaskManagerPage } from './scheduler/TaskManagerPage';
import { TaskDetail, TaskTabTitle } from './scheduler/TaskDetail';
import { TaskNavigationProvider, SchedulerNotices } from './scheduler/SessionTasks';
import { taskKey, parseTaskKey, type TaskRef } from './scheduler/model';
import { IconClockOutline16, IconHomeOutline16, IconSettingsOutline16 } from '../presentation/primitives/icons';
import schedulerCss from './scheduler/scheduler.module.css';
import { readShellLayout, saveShellLayout } from './shell-preferences';

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
  const [preferences] = useState(readNavigationPreferences);
  const savedRoute = restoreRoute(preferences, client.getSnapshot());
  const drafts = draftPreparations(client, workspaceHost, workspaceAuthority);
  const [composerDraft] = useState(() => new ComposerDraft());
  // Browser-lifetime Workflows surface: its open file target and unsaved
  // edits survive leaving the route, and never follow conversation focus.
  // Each port binds the App Server and Product Host authorities current when
  // it is created; transactions keep their port, so they are never retargeted.
  const workflows = useActorRef(workflowSurfaceMachine, { input: { ports: scope => createWorkflowPort({ client, host: () => workspaceHost, hostAuthority: workspaceAuthority.capture(), scope }) } });
  const navigation = useActorRef(shellNavigationMachine, { input: { drafts, client, settings, host: workspaceHost, workspaces: workspaceAuthority, workflows, route: savedRoute?.kind === 'session' && !client.isAttachmentControlCurrent(savedRoute.sessionId, client.getSnapshot().views[savedRoute.sessionId]?.attachmentObservation) ? undefined : savedRoute } });
  return <AppSurface composerDraft={composerDraft} client={client} workspaceHost={workspaceHost} workspaceAuthority={workspaceAuthority} connection={connection} navigation={navigation} repairAcknowledgements={repairAcknowledgements} workflows={workflows}/>;
}
/** Route rendering and gestures; it owns no native admission or actor lifetime. */
export function AppSurface({ client, workspaceHost, workspaceAuthority, connection, navigation: shell, composerDraft, repairAcknowledgements, workflows }: { composerDraft: ComposerDraft; client: AppServerClient; workspaceHost: ProductHostWorkspaces; workspaceAuthority: WorkspaceAuthority; connection: ConnectionController; navigation: ShellNavigationActor; repairAcknowledgements: RepairAcknowledgements; workflows: WorkflowSurfaceActor }) {
  const fileWorkbench = useRef<WorkbenchHandle>(null);
  const [initialSidebarPreference] = useState(readShellLayout);
  const tx = useTranslation();
  const state = useClientSelector(client, selectShell, sameValue);
  const fileAuthority = useSyncExternalStore(workspaceAuthority.subscribe, workspaceAuthority.getRevision);
  const associations = useMemo(() => new WorkspaceAssociations(client, workspaceAuthority), [client, workspaceAuthority]);
  useEffect(() => associations.start(), [associations]);
  // Settings owns internal target/section/detail navigation; Shell owns visibility.
  const navigationActor = shell.getSnapshot().context.settings;
  const previewCache = useMemo(() => new PreviewContentCache(), [client, workspaceHost, workspaceAuthority]);
  const previewOwner = useMemo(() => new PreviewWorkspaceOwner(client, workspaceHost, workspaceAuthority, previewCache), [client, workspaceHost, workspaceAuthority, previewCache]);
  const workbenchPreviews = useMemo(() => new WorkbenchPreviewRetention(client, workspaceAuthority, previewCache), [client, workspaceAuthority, previewCache]);
  useEffect(() => workbenchPreviews.start(), [workbenchPreviews]);
  const previews = useSyncExternalStore(previewOwner.subscribe, previewOwner.getSnapshot);
  useEffect(() => previewOwner.start(), [previewOwner]);
  const [previewFocus, setPreviewFocus] = useState(0);
  const [previewReturnFocus, setPreviewReturnFocus] = useState(0);
  const previewOpener = useRef<HTMLElement | null>(null);
  const [workbenchFullscreen, setWorkbenchFullscreen] = useState(false);
  const returnPreviewFocus = () => setPreviewReturnFocus(value => value + 1);
  useLayoutEffect(() => {
    if (!previewReturnFocus) return;
    const opener = previewOpener.current;
    const target = opener?.isConnected && !opener.closest('[inert]') && opener.checkVisibility() ? opener : (opener?.hasAttribute('data-side-question-toggle') ? document.querySelector<HTMLElement>('[data-side-question-toggle]') : null) ?? document.querySelector<HTMLElement>('[data-preview-toggle]') ?? document.querySelector<HTMLElement>('#conversation-view');
    target?.focus({ preventScroll: true });
  }, [previewReturnFocus]);
  const collapsePanel = () => { previewOwner.collapse(); returnPreviewFocus(); };
  const [theme, setTheme] = useState(readTheme);
  useLayoutEffect(() => applyTheme(theme), [theme]);
  const [displayPreferences] = useConversationPreferences();
  useLayoutEffect(() => { document.body.style.setProperty('--dsh-content-font-size', `${displayPreferences.fontSize}px`); }, [displayPreferences.fontSize]);
  const [requestedConversationMode, setConversationMode] = useState<'chat' | 'trajectory'>('chat');
  const conversationMode = displayPreferences.codingView ? requestedConversationMode : 'chat';
  const [preferences] = useState(readNavigationPreferences);
  const [workspaceSelection] = useState(() => {
    let storage: Storage | undefined;
    try { storage = globalThis.localStorage; } catch { /* Browser storage may be disabled. */ }
    return new WorkspaceSelectionPreference(storage);
  });
  const endpoint = state.endpoint ?? '';
  const initialViews = sameEndpoint(preferences.endpoint, state.endpoint) ? resumableViews(preferences.openViews, state) : [];
  const [openViews, setOpenViews] = useState<string[]>(initialViews);
  const routeState = useSelector(shell, snapshot => snapshot);
  useEffect(() => { const subscription = shell.subscribe(() => client.recheckAdmissions()); return () => subscription.unsubscribe(); }, [shell, client]);
  const previewCurrent = useMemo(() => { const context = routeState.context; return () => shell.getSnapshot().context === context; }, [shell, routeState.context]);
  const primaryRoute = routeState.context.route;
  const home = isHomeRoute(primaryRoute);
  const center = routeState.context.home;
  const settingsOpen = primaryRoute.kind === 'settings';
  const draftBinding = routeState.context.binding;
  const selected = center.kind === 'session' || center.kind === 'session-draft' ? center.sessionId : undefined;
  const command = routeState.context.command;
  const restored = routeState.context.restored;
  const commandCurrent = () => !!command && shell.getSnapshot().context.command === command && shell.getSnapshot().context.route === command.route && client.getSnapshot().generation === command.generation;
  const draftWorkspaceSession = center.kind === 'new-conversation' ? routeState.context.draftWorkspaceSession : undefined;
  useEffect(() => associations.select(selected ?? draftWorkspaceSession), [associations, selected, draftWorkspaceSession]);
  const workspaceAssociations = useSyncExternalStore(associations.subscribe, associations.getSnapshot);
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
  const resumeViews = JSON.stringify(resumableViews(openViews, state));
  const [error, setError] = useToastNotice();
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
  const sideQuestionAvailable = !!view && (!!client.getSnapshot().capabilities?.delivery_access || !!client.auxiliary.snapshot(view.id));
  const openSideQuestion = () => { previewOpener.current = document.activeElement as HTMLElement; previewOwner.showWorkbench(); fileWorkbench.current?.openSideQuestion(); };
  const workbenchTarget: WorkbenchTarget | undefined = view?.summary
    ? { session_id: view.id, active_node: view.summary.active_node }
    : center.kind === 'new-conversation' && workspace ? { workspace_id: workspace } : undefined;
  useLayoutEffect(() => previewOwner.selectSession(selected), [previewOwner, selected]);
  const artifacts = useMemo(() => selected && view?.target ? new ArtifactResources(client, selected) : undefined, [client, selected, view?.target, state.generation, state.authorityRevision, fileAuthority]);
  useEffect(() => () => artifacts?.dispose(), [artifacts]);
  const connected = state.connection === 'connected';
  const attached = home && center.kind === 'session' && !view?.deleting && connected && view?.attachmentIntent === 'wanted' && view.attachment === 'attached';
  const commandOpen = !!command && command.route.kind === 'session' && command.route.sessionId === selected && commandCurrent();
  /** The selected view when it can take a Session command right now. */
  const commandable = () => {
    if (!home || center.kind !== 'session') return;
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
  const destination = routeState.context.pending?.destination;
  const sessionTransition = routeState.context.retry ? { sessionId: routeState.context.retry.view.id, action: 'retry' as const }
    : destination?.kind === 'fork-session' ? { sessionId: destination.sessionId, action: 'fork' as const }
    : destination?.kind === 'transition' && (destination.action === 'fork' || destination.action === 'retry') ? { sessionId: destination.sessionId, action: destination.action } : undefined;
  const forkingResponse = destination?.kind === 'transition' && destination.action === 'fork' ? destination.response : undefined;
  const runGlobal = (action: () => Promise<unknown>, current: () => boolean = () => true) => {
    const generation = client.getSnapshot().generation; setError('');
    void action().catch(cause => { if (generation === client.getSnapshot().generation && current()) setError(actionError(cause)); });
  };
  // Subscription cleanup is presentation-only. No component owns socket/runtime life.
  const [preferencesApplied, setPreferencesApplied] = useState(false);
  useEffect(() => {
    if (!endpoint || state.connection !== 'connected' || settingsOpen || preferencesApplied) return;
    setPreferencesApplied(true);
    const owner = shell.getSnapshot().context, native = client.getSnapshot();
    const route = restoreRoute(preferences, native);
    if (!route || owner.intentRevision !== 0 || owner.pending) return;
    const views = resumableViews(preferences.openViews, native);
    client.restoreViews(views);
    setOpenViews(views);
    if (route.kind === 'session') shell.send({ type: 'OPEN', destination: { kind: 'restore-session', sessionId: route.sessionId, sidebar: false } });
    else if (!sameValue(owner.route, route)) {
      if (route.kind === 'workflows') shell.send({ type: 'WORKFLOWS' });
      else if (route.kind === 'schedules') shell.send({ type: 'SCHEDULES' });
      else if (route.kind === 'new-conversation') shell.send({ type: 'DRAFT', workspaceId: route.workspaceId });
      else shell.send({ type: 'RESOURCES', module: route.kind });
    }
    if (client.getSnapshot().connection === 'connected') for (const id of views.filter(id => route.kind !== 'session' || id !== route.sessionId)) {
      if (!client.getSnapshot().views[id]?.target && (!firstSubmissions(client).session(id) || ['admitted', 'discarded'].includes(firstSubmissions(client).session(id)!.phase))) void client.attach(id).catch(() => {});
    }
  }, [client, shell, endpoint, preferences, state.connection, settingsOpen, preferencesApplied]);
  const resumeRoute = JSON.stringify(saveRoute(routeState.context, JSON.parse(resumeViews)));
  useEffect(() => {
    // Restore saved views before choosing a draft: selecting a Workspace owns
    // real preparation, even while its loading UI stays silent.
    if (!endpoint || state.connection !== 'connected' || settingsOpen || !preferencesApplied) return;
    const context = shell.getSnapshot().context;
    if (context.control !== routeState.context.control || context.route.kind !== 'new-conversation' || context.route.workspaceId || context.pending) return;
    const workspaceId = preferredWorkspace(associations.getSnapshot(), client.getSnapshot(), context.draftWorkspaceSession, workspaceSelection.read(client.getSnapshot().endpoint));
    if (workspaceId) shell.send({ type: 'DRAFT.WORKSPACE', workspaceId });
  }, [associations, workspaceAssociations, workspaceSelection, client, shell, routeState.context.control, center, endpoint, state.connection, settingsOpen, preferencesApplied]);
  useEffect(() => {
    try {
      // Initialization cannot overwrite the saved route before restoration has
      // either published it or yielded to a newer explicit navigation intent.
      // Read the actor now: an earlier effect may have started restoration
      // after this render captured its placeholder route.
      const current = shell.getSnapshot().context;
      if (current !== routeState.context || current.pending?.destination.kind === 'restore-session') return;
      if (endpoint && preferencesApplied) localStorage.setItem(navigationPreferencesKey, json({ endpoint, openViews: JSON.parse(resumeViews), route: JSON.parse(resumeRoute) }));
    } catch { /* Storage may be disabled in a trusted browser. */ }
  }, [shell, routeState.context, endpoint, resumeViews, resumeRoute, preferencesApplied]);
  const focusSession = (id?: string) => {
    if (id) shell.send({ type: 'OPEN', destination: { kind: 'session', sessionId: id, sidebar: false } });
    else shell.send({ type: 'DRAFT' });
  };
  const open = (id: string, command?: CommandRequest, sidebar?: SidebarGeometry, node?: { nodeId: string; conversationId: string }) => {
    if (!openViews.includes(id) && openViews.length >= 32) { setError(message('common:copy.32-session-views-are-open-close-a-view-from-its-sidebar-session-actions-or-use-sidebar-vie')); return; }
    shell.send({ type: 'OPEN', destination: { kind: 'session', sessionId: id, sidebar: !!sidebar?.overlay, command, ...node } });
  };
  const closeView = (id: string) => {
    const remaining = openViews.filter(item => item !== id);
    setOpenViews(remaining);
    if (selected === id) focusSession(remaining[0]);
    runGlobal(() => client.release(id));
  };
  const closeAllViews = () => {
    const closing = [...openViews];
    setOpenViews([]); shell.send({ type: 'DRAFT' });
    runGlobal(() => Promise.all(closing.map(id => client.release(id))));
  };
  const focusDraft = () => document.querySelector<HTMLTextAreaElement>('[data-resident-composer] textarea')?.focus();
  const rememberWorkspace = (id: string) => {
    const native = client.getSnapshot(), catalog = workspaceAuthority.getCatalog(), route = shell.getSnapshot().context.route;
    if (!native.endpoint || !sameEndpoint(catalog?.endpoint, native.endpoint) || !catalog?.workspaces.some(row => row.id === id)
      || route.kind !== 'new-conversation' || route.workspaceId !== id) return;
    const intent = workspaceSelection.select(native.endpoint, id);
    if (intent) runGlobal(() => workspaceHost.selectWorkspace({ authorityId: catalog.authorityId, endpoint: catalog.endpoint }, id, intent), () => workspaceSelection.current(intent));
    // Commit the unsent route at the gesture too, so an immediate refresh
    // cannot restore an older Session before React's persistence effect runs.
    try { localStorage.setItem(navigationPreferencesKey, json({ endpoint: native.endpoint, openViews: resumableViews(openViews, native), route: saveRoute(shell.getSnapshot().context, openViews) })); } catch { /* Storage disabled. */ }
  };
  const createInWorkspace = (id = workspace, remember = false) => {
    shell.send({ type: 'DRAFT', workspaceId: id });
    if (remember && id) rememberWorkspace(id);
    focusDraft();
  };
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
  const openTaskSession = (task: TaskRef) => open(task.session_id, undefined, undefined, { nodeId: task.node_id, conversationId: task.id.conversation_id });
  return <TaskNavigationProvider value={{ openTask: (task, title) => { previewOwner.showWorkbench(); fileWorkbench.current?.openTask(taskKey(task), title); }, openTasks: () => shell.send({ type: 'SCHEDULES' }) }}><SubagentScope onMode={setConversationMode} client={client} sessionId={view?.id} openAside={(id, title) => { previewOwner.showWorkbench(); fileWorkbench.current?.openAgent(id, title); }}><PreviewContext value={{ openPreview: artifact => { previewOpener.current = document.activeElement as HTMLElement; previewOwner.openPreview(artifact); setPreviewFocus(value => value + 1); }, download: artifact => { void previewOwner.download(artifact); } }}><ChangesReviewContext value={request => { previewOpener.current=document.activeElement as HTMLElement; previewOwner.showWorkbench(); fileWorkbench.current?.openChanges(request); }}><AppFrame home={home} initialSidebarPreference={initialSidebarPreference} onSidebarPreferenceChange={saveShellLayout} sidebarPublication={routeState.context.publication} dismissSidebarLabel={tx('sidebar:toggle.dismiss')} onSidebarDismiss={retirePendingSidebarNavigation}
    primary={geometry => <PrimaryNavigationRail label={tx('sidebar:navigation.label')} activeId={home ? 'home' : primaryRoute.kind}
      items={[
        { id: 'home', label: tx('sidebar:navigation.home'), icon: <IconHomeOutline16 size={18}/>, controls: 'home-sidebar', expanded: geometry.sidebarExpanded, select: () => { if (home) geometry.expandSidebar(); else shell.send({ type: 'HOME' }); } },
        { id: 'workflows', label: tx('workflows:nav.label'), icon: <IconWorkflow size={18}/>, select: () => shell.send({ type: 'WORKFLOWS' }) },
        { id: 'subagents', label: tx('resources:subagents.nav'), icon: <IconAgentPresetOutline16/>, select: () => shell.send({type:'RESOURCES',module:'subagents'}) },
        { id: 'mcp', label: tx('resources:mcp.title'), icon: <McpIcon/>, select: () => shell.send({type:'RESOURCES',module:'mcp'}) },
        { id: 'skills', label: tx('resources:skills.title'), icon: <IconCodeOutline16/>, select: () => shell.send({type:'RESOURCES',module:'skills'}) },
        { id: 'schedules', label: tx('sidebar:navigation.schedules'), icon: <IconClockOutline16 size={18}/>, select: () => shell.send({ type: 'SCHEDULES' }) },
      ]}
      settings={{ id: 'settings', label: tx('settings:settings.settings'), icon: <IconSettingsOutline16 size={18}/>, select: () => shell.send({ type: 'SETTINGS' }) }}
      feedback={<ConnectionFeedback client={client} connection={connection} wide={false} settings={openConnectionSettings}/>}/>}
    sidebar={geometry => <HomeSidebar width={geometry.width} toggleSidebar={geometry.toggleSidebar} startSession={() => { createInWorkspace(workspace); geometry.dismissSidebar(); }}
      browser={<WorkspaceNavigation transition={sessionTransition} associations={associations} key={state.authorityRevision ?? 0} host={workspaceHost} client={client} state={state} endpoint={endpoint}
        metadataChanged={removed => { if (removed && center.kind === 'new-conversation' && center.workspaceId === removed) shell.send({ type: 'DRAFT' }); }}
        workspaceSettings={(id, label) => { openSettings(workspaceSettingsTarget(id, label)); geometry.dismissSidebar(); }} workspace={workspace} selected={selected}
        draft={center.kind === 'new-conversation' && workspace ? { workspaceId: workspace, binding: String(draftBinding), focus: focusDraft } : undefined} selectWorkspace={id => { createInWorkspace(id, true); geometry.dismissSidebar(); }}
        openSession={id => open(id, undefined, geometry)} openViews={openViews} closeView={closeView} closeAllViews={closeAllViews} createSession={id => { createInWorkspace(id, true); geometry.dismissSidebar(); }} deleteSession={setDeletingSession}
        forkSession={id => shell.send({ type: 'OPEN', destination: { kind: 'fork-session', sessionId: id, sidebar: !!geometry.overlay } })} />}/>}
    rightOpen={home && previews.mode !== 'collapsed'} rightPanel={geometry => <RightPanel docked={previews.mode !== 'preview'} {...geometry} fullscreen={previews.mode === 'workbench' ? workbenchFullscreen : previews.mode === 'preview' && previews.workspace?.fullscreen} open={home && previews.mode !== 'collapsed'} title={previews.mode === 'preview' ? tx('artifacts:workspace.title') : tx('artifacts:workbench.start')}>
      <Workbench renderSideQuestion={sideQuestionAvailable ? visible => <BtwPanel client={client} sessionId={view!.id} visible={visible} close={collapsePanel}/> : undefined} renderTaskTitle={(id, fallback) => <TaskTabTitle client={client} identity={id} fallback={fallback}/>} renderTask={id => { const task = parseTaskKey(id); return task && <div className={schedulerCss.taskTab}><TaskDetail client={client} task={task} openSession={openTaskSession}/></div>; }} renderAgent={(id, visible) => <SubagentAside id={id} visible={visible}/>} fileRef={fileWorkbench} key={JSON.stringify([state.generation, state.authorityRevision, selected, view?.target, workbenchTarget, fileAuthority])} closePanel={resetPresentation=>{if(resetPresentation)setWorkbenchFullscreen(false);collapsePanel();}} host={workspaceHost} previews={workbenchPreviews} scope={workspaceAuthority.getCatalog()} target={workbenchTarget} active={home} visible={previews.mode === 'workbench'} floatingVisible={previews.mode === 'workbench' || previews.mode === 'collapsed'} fullscreen={workbenchFullscreen || !geometry.canShow} toggleFullscreen={() => { if (!geometry.canShow) {setWorkbenchFullscreen(false);collapsePanel();} else setWorkbenchFullscreen(value => !value); }}/>
      <PreviewWorkspace active={home} openSideQuestion={sideQuestionAvailable ? openSideQuestion : undefined} openWorkbench={() => previewOwner.showWorkbench()} owner={previewOwner} snapshot={previews} focusRequest={previewFocus} returnFocus={returnPreviewFocus}/>
    </RightPanel>}
    overlay={<><SchedulerNotices client={client}/><SettingsNavigationFeedback navigation={navigationActor}/></>}>
    {Object.values(state.views).filter(item => item.deletionRecovery).map(item => <section key={item.id} className="notice" aria-label={tx('common:app.deletion-recovery-for-value', { p0: sessionDisplayTitle(tx, item.summary) })}>
      <p>{sessionDisplayTitle(tx, item.summary)}: {item.deletionRecovery === 'committed_cleanup_pending' ? tx('common:app.session-removed-cleanup-is-still-pending') : tx('common:app.deletion-durability-needs-verification')}</p>
      <Button disabled={!connected || item.recoveringDeletion} onClick={() => runGlobal(async () => {
        const result = await client.recoverSessionDeletion(item.id);
        if (result && result.status !== 'deleted' && result.status !== 'not_found') setError(sessionDeletionNotice(result));
      })}>{item.recoveringDeletion ? tx('common:app.recovering-deletion') : tx('common:app.retry-deletion-recovery')}</Button>
    </section>)}
    <DetachedFirstSubmissions client={client} binding={String(draftBinding)} active={!!selected}/>
    {(previews.problem || previews.downloadError) && <Toast key={previews.problem ?? previews.downloadError} text={previews.problem ? tx(`artifacts:workspace.limit-${previews.problem}`) : tx(previews.downloadError === 'capacity' ? 'artifacts:workspace.download-capacity' : 'artifacts:workspace.download-failed')} holdMs={6000} icon={<IconWarningOutline16/>} actions={[{ label: tx('common:app.dismiss-notice'), onClick: () => previewOwner.dismissProblem() }]} onDone={() => previewOwner.dismissProblem()}/> }
    {(error || routeState.context.error) && <Toast key={error ? `action:${error.sequence}` : routeState.context.error} text={error?.text ?? routeState.context.error!} holdMs={6000} icon={<IconWarningOutline16/>} actions={[{ label: tx('common:app.dismiss-notice'), onClick: () => { setError(''); shell.send({ type: 'DISMISS.ERROR' }); } }]} onDone={() => { setError(''); shell.send({ type: 'DISMISS.ERROR' }); }}/> }
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
    <DocumentTitle client={client} sessionId={home || settingsOpen ? view?.id : undefined}/>
    <OverlayScope className="primary-page" active={settingsOpen}>
      <Settings visible={settingsOpen} associations={associations} navigation={navigationActor} connection={connection} client={client} host={workspaceHost} theme={theme} setTheme={setTheme} />
    </OverlayScope>
    {primaryRoute.kind === 'schedules' && <TaskManagerPage client={client} host={workspaceHost} authority={workspaceAuthority} openSession={openTaskSession}/>}
    {primaryRoute.kind === 'workflows' && <WorkflowsSurface surface={workflows} authority={workspaceAuthority} />}
    {(['subagents','mcp','skills'] as const).map(module=><OverlayScope key={`${module}:${state.authorityRevision}`} className="primary-page" active={primaryRoute.kind===module}><ResourceSurface module={module} visible={primaryRoute.kind===module} client={client} host={workspaceHost} authority={workspaceAuthority} associations={associations} initialWorkspace={workspace}/></OverlayScope>)}
    <div className="primary-page" hidden={!home} inert={!home}><FileLinks.Provider value={file => { previewOwner.showWorkbench(); void fileWorkbench.current?.openFile(file.path, file.line); }}><section className={`session-panel ${agentCss.root}`} data-phase={view ? 'active' : 'hero'} data-shell-route={center.kind} id="session-view" role="region" aria-labelledby={view ? 'session-title' : undefined} aria-label={view ? undefined : tx('agent:conversation-header.product-title')}>
      <ConversationHeader host={workspaceHost} authority={workspaceAuthority} client={client} view={view && { ...view, summary: state.sessions.find(session => session.id === view.id) ?? view.summary }} authorityRevision={state.authorityRevision}
        connected={connected} attached={attached} commandOpen={commandOpen}
        previewToggle={previews.mode === 'collapsed' && <>{sideQuestionAvailable && <Button size="sm" className={agentCss.iconButton} data-side-question-toggle aria-label={tx('agent:btw.title')} title={tx('agent:btw.title')} onClick={openSideQuestion}><IconQuestionOutline14/></Button>}<Button size="sm" className={agentCss.iconButton} data-preview-toggle aria-label={tx(previews.workspace ? 'artifacts:workspace.reopen' : 'artifacts:workbench.toggle')} title={tx('artifacts:workbench.toggle')} aria-expanded={false} onClick={event => { previewOpener.current = event.currentTarget; if (previews.workspace) previewOwner.reveal(); else previewOwner.showWorkbench(); setPreviewFocus(value => value + 1); }}><IconPanelLeftOutline16 className={agentCss.mirrored} /></Button></>} invokeCommand={invokeCommand}
        openOwningSettings={openOwningSettings} conversationMode={conversationMode} setConversationMode={setConversationMode}/>

      {view && <ConversationStatus client={client} sessionId={view.id} transition={sessionTransition?.sessionId === view.id ? sessionTransition.action : undefined} recover={action => {
        if (action === 'connection-settings') openConnectionSettings();
        else if (action === 'connect') void connection.reconnect();
        else if (action === 'refresh') runGlobal(() => client.refresh(view.id));
        else {
          const current = committedRouteCurrent(shell);
          const route = shell.getSnapshot().context.route;
          const presentationCurrent = () => current() && shell.getSnapshot().context.route === route;
          runGlobal(async () => { await client.retryInitialization(view.id, current); if (presentationCurrent()) focusSession(view.id); }, presentationCurrent);
        }
      }} />}

      <section className={`conversation-panel ${agentCss.body}`} id="conversation-view" role={view && displayPreferences.codingView ? 'tabpanel' : undefined} aria-labelledby={view && displayPreferences.codingView ? `view-tab-${conversationMode}` : undefined} tabIndex={0}>
      <ConversationWidthControls active={!!view && conversationMode === 'chat'}/>
      <ConversationQuestionPanels client={client} sessionId={view?.id}>
      <ArtifactContext.Provider value={artifacts}><SubagentSurface mode={conversationMode}><div className={agentCss.scrollBody} data-conversation-scroll>
      <ConversationLive previewCurrent={previewCurrent} onMode={setConversationMode} client={client} sessionId={view?.id} mode={conversationMode} retry={routeState.context.retry} forking={forkingResponse} historicalPending={routeState.matches('preparing')} disabled={commandOpen} onHistorical={transitionResponse}/>
      <ConversationSeat visible={home} intakeCommitted={center.kind !== 'session-draft'} draftOwner={composerDraft} client={client} host={workspaceHost} authority={workspaceAuthority} associations={associations} sessionId={view?.id}
        initialWorkspace={workspace ?? (center.kind === 'new-conversation' ? center.workspaceId : undefined)}
        workspacePicked={id => { if (newConversationCurrent()) { shell.send({ type: 'DRAFT.WORKSPACE', workspaceId: id }); rememberWorkspace(id); } }}
        transitioning={!!routeState.context.retry} binding={String(draftBinding)} current={center.kind !== 'session' && routeState.context.draftSession ? startupCurrent : newConversationCurrent} executable={home && center.kind === 'session'} consumed={consumed} restored={restored}
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
      </ConversationQuestionPanels>
      </section>
      {commandOpen && command?.route.kind === 'session' && <CommandPanel key={`${command.generation}:${command.route.sessionId}:${command.request.id}:${command.request.messageId ?? ''}`} request={command.request} client={client} sessionId={command.route.sessionId} current={commandCurrent}
        succeeded={() => setConsumed(previous => ({ id: command.request.id, sequence: (previous?.sequence ?? 0) + 1 }))}
        close={() => shell.send({ type: 'COMMAND.CLOSE', command })}
        navigate={choice => { if (!commandCurrent() || command.route.kind !== 'session') return; shell.send({ type: 'OPEN', destination: choice.kind === 'node' ? { ...choice, sessionId: command.route.sessionId } : { kind: 'transition', sessionId: command.route.sessionId, action: choice.action, selection: choice.selection } }); }} />}

    </section></FileLinks.Provider></div>
  </AppFrame></ChangesReviewContext></PreviewContext></SubagentScope></TaskNavigationProvider>;
}

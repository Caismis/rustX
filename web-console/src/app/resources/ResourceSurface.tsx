import { useCallback, useEffect, useLayoutEffect, useRef, useState, useSyncExternalStore } from 'react';
import { useSelector } from '@xstate/react';
import clsx from 'clsx';
import { Tabs, TabList, Tab, TabPanel } from 'react-aria-components';
import type { SkillDescriptor, SourceMutation, SourceScope, SourceSettings, WorkspaceCatalogAuthority } from '../../../../protocol/app-server/v81';
import { withWorkflowOperation } from '../../../../protocol/app-server/workflow-documents';
import { bindResourceTransfer } from '../../workspaces/resource-transfer';
import type { AppServerClient } from '../../client/app-server';
import type { ProductHostWorkspaces } from '../../workspaces/host';
import type { WorkspaceAuthority } from '../../workspaces/authority';
import { sameEndpoint } from '../../workspaces/endpoint';
import { clientCatalogWorkspace, type WorkspaceAssociations } from '../../workspaces/associations';
import { useTranslation } from '../../locale/react';
import { CatalogSearch, CatalogViewSwitch, type CatalogView } from '../../presentation/catalog/CatalogControls';
import catalogCss from '../../presentation/catalog/Catalog.module.css';
import { Button } from '../../presentation/primitives/Button';
import { Menu } from '../../presentation/primitives/Menu';
import { IconChevronDownOutline14, IconFolderClose16, IconPlusOutline16, IconRefreshOutline16 } from '../../presentation/primitives/icons';
import settings from '../../presentation/settings/SettingsContent.module.css';
import { SettingsActorContext, useSettingsTarget } from '../settings/machines/react';
import { SourceContext } from '../settings/source-context';
import { catalogIdentities, catalogProfiles, configAuthoring, settingsTargetKey, userSettingsTarget, workspaceSettingsTarget, type SettingsTarget } from '../settings/projection';
import { SettingsFeedback, SettingsNotice } from '../settings/SettingsFeedback';
import { createWorkflowPort, readToolCatalog, USER_SCOPE } from '../workflows/port';
import { McpActions, McpDetail, McpPage } from './mcp/McpPage';
import { useMcpProbes } from './mcp/useMcpProbes';
import { TransferDialog, type TransferIntent, type TransferReopenReason } from './shared/ResourceTransfer';
import { ResourceAvailability } from './shared/ResourceAvailability';
import { ResourceBrowser } from './shared/ResourceBrowser';
import type { SkillDisplayWriter } from './skills/SkillDisplayDialog';
import { AgentEditor, type AgentDraft } from './subagents/AgentEditor';
import type { LoadMcpTools } from './subagents/AgentToolFields';
import { SubagentList } from './subagents/SubagentList';
import type { McpEditorDraft, McpFocus, ResourceModule } from './types';
import css from './ResourceSurface.module.css';

interface Props {
  module: ResourceModule; visible: boolean; client: AppServerClient; host: ProductHostWorkspaces;
  authority: WorkspaceAuthority; associations: WorkspaceAssociations; initialWorkspace?: string;
  executionSession?: { id: string; cwd: string };
}

/** Retain each module's presentation independently of its source transactions. */
type TransferNotice = {
  scope: string;
  name: string;
  operation: TransferIntent['operation'];
  fromLabel: string;
  destination: string;
  destinationLabel: string;
  reason: TransferReopenReason;
};

export function ResourceSurface(props: Props) {
  const [visited, setVisited] = useState(props.visible);
  if (props.visible && !visited) setVisited(true);
  return (visited || props.visible) && <ResourcePage {...props} />;
}

function ResourcePage({ module, visible, client, host, authority, associations, initialWorkspace, executionSession }: Props) {
  const tx = useTranslation();
  const catalog = useSyncExternalStore(authority.subscribe, authority.getCatalog);
  useEffect(() => { if (visible) void authority.observe().catch(() => {}); }, [authority, visible]);
  const [tab, setTab] = useState<'workspace' | 'global'>(initialWorkspace ? 'workspace' : 'global');
  const [workspaceId, setWorkspace] = useState(initialWorkspace);
  const chosen = useRef(!!initialWorkspace);
  const workspaces = catalog?.workspaces ?? [];
  const linked = useSyncExternalStore(associations.subscribe, associations.getSnapshot);
  const sessionAssociation = executionSession && linked.entries.get(executionSession.id);
  const clientWorkspace = clientCatalogWorkspace(sessionAssociation);
  useEffect(() => {
    if (chosen.current) return;
    if (clientWorkspace && workspaces.some(row => row.id === clientWorkspace)) {
      chosen.current = true; setWorkspace(clientWorkspace); setTab('workspace');
    } else if (workspaces.length && (!executionSession || (sessionAssociation?.status !== 'pending' && !!sessionAssociation))) {
      chosen.current = true; setWorkspace(workspaces[0].id);
    }
  }, [clientWorkspace, executionSession, sessionAssociation, workspaces]);
  const workspace = workspaces.find(row => row.id === workspaceId);
  const showingWorkspace = tab === 'workspace';
  const target = showingWorkspace && workspaceId ? workspaceSettingsTarget(workspaceId, workspace?.displayName ?? workspaceId) : userSettingsTarget;
  const available = !showingWorkspace || !!workspace;
  const catalogModule = module === 'mcp' || module === 'skills';
  const sessionCandidate = catalogModule && showingWorkspace && workspace && workspaceId && executionSession
    && clientWorkspace === workspaceId
    && sessionAssociation?.cwd === executionSession.cwd
    && linked.catalog?.authorityId === catalog?.authorityId && sameEndpoint(linked.catalog?.endpoint, catalog?.endpoint)
    ? executionSession : undefined;
  const awaitingAssociation = catalogModule && showingWorkspace && !!executionSession && workspaceId === initialWorkspace
    && (!sessionAssociation || sessionAssociation.status === 'pending' || sessionAssociation.status === 'refreshing');
  const hostOwner = catalog && JSON.stringify([catalog.authorityId, catalog.endpoint]);
  const [catalogRefresh, setCatalogRefresh] = useState(0);
  const candidateOwner = `${client.getSnapshot().endpoint}|${client.getSnapshot().authorityRevision}`;
  const boundKey = JSON.stringify([candidateOwner, hostOwner, client.getSnapshot().generation, workspaceId, workspace?.location, sessionCandidate?.id, sessionCandidate?.cwd, catalogRefresh]);
  const activeBoundKey = useRef(boundKey);
  activeBoundKey.current = boundKey;
  const [boundObservation, setBoundObservation] = useState<{ key: string; authority?: WorkspaceCatalogAuthority; source?: SourceSettings['user_mcp']; error?: string }>();
  const [boundSkillObservation, setBoundSkillObservation] = useState<{ key: string; status: 'loading' | 'ready' | 'failed'; authority?: WorkspaceCatalogAuthority; clientAuthority?: boolean }>();
  const boundSource = sessionCandidate && module === 'mcp' && boundObservation?.key === boundKey ? boundObservation : undefined;
  const workspaceSkills = showingWorkspace && module === 'skills';
  const boundSkillState = workspaceSkills && boundSkillObservation?.key === boundKey ? boundSkillObservation : undefined;
  const observeSkills = (state: { status: 'loading' | 'ready' | 'failed'; authority?: WorkspaceCatalogAuthority; clientAuthority?: boolean }) =>
    setBoundSkillObservation(prev => prev && prev.key === boundKey && prev.status === state.status && prev.authority === state.authority && prev.clientAuthority === state.clientAuthority
      ? prev : { key: boundKey, ...state });
  const authoringConfirmed = !!sessionCandidate && (module === 'mcp' ? boundSource?.authority === 'authoring' : boundSkillState?.authority === 'authoring');
  const bound = sessionCandidate && (module === 'mcp' ? boundSource?.authority === 'client' && !!boundSource.source : boundSkillState?.authority === 'client')
    ? sessionCandidate : undefined;
  const { actor, transport } = useSettingsTarget(client, target, host, visible && available && !awaitingAssociation && (!sessionCandidate || authoringConfirmed), showingWorkspace ? hostOwner : undefined);
  const { actor: globalActor } = useSettingsTarget(client, userSettingsTarget, host, visible && showingWorkspace && module !== 'skills');
  const globalObservation = useSelector(globalActor, state => state.context.observation);
  const globalSource = useSelector(globalActor, state => state.context.observation ?? state.context.staleObservation);
  const globalError = useSelector(globalActor, state => state.context.readError);
  const globalEditable = !!globalObservation && !globalError && transport.connection === 'connected';
  const observation = useSelector(actor, state => state.context.observation);
  const source = useSelector(actor, state => state.context.observation ?? state.context.staleObservation);
  const readError = useSelector(actor, state => state.context.readError);
  const maintenanceError = useSelector(actor, state => state.context.maintenanceError);
  const convergenceError = useSelector(actor, state => state.context.convergenceError);
  const busy = useSelector(actor, state => state.matches({ mutation: 'submitting' }) || state.matches({ maintenance: 'reconciling' }));
  const [workspaceMenu, setWorkspaceMenu] = useState(false);
  const [query, setQuery] = useState('');
  const [view, setView] = useState<CatalogView>('cards');
  const [detailScope, setDetailScope] = useState<SourceScope>();
  const [pendingTransfer, setPendingTransfer] = useState<{ intent: TransferIntent; skillScope?: string; token: number; fromLabel: string }>();
  const [transferNotices, setTransferNotices] = useState<ReadonlyMap<number, TransferNotice>>(() => new Map());
  const [owedTransferRefresh, setOwedTransferRefresh] = useState<string>();
  const transferSequence = useRef(0);
  const liveTransfer = useRef<number | undefined>(undefined);
  const [mcpFocus, setMcpFocus] = useState<McpFocus>();
  const mcpDrafts = useRef(new Map<string, { draft: McpEditorDraft; hostOwner?: string }>());
  const [resourceFocus, setResourceFocus] = useState<string>();
  const [newAgent, setNewAgent] = useState<AgentDraft>();
  const owner = `${transport.endpoint}|${transport.authorityRevision}`;
  const directory = observation?.target.kind === 'workspace' ? observation.target.directory : undefined;
  const lifetime = JSON.stringify([owner, settingsTargetKey(target), showingWorkspace ? hostOwner : undefined, showingWorkspace ? workspace?.location : undefined]);
  const [presentedOwner, setPresentedOwner] = useState(owner);
  const [presentedHost, setPresentedHost] = useState(hostOwner);
  const agentEditor = module === 'subagents' && (newAgent !== undefined || resourceFocus !== undefined);
  const detail = !!mcpFocus && (!bound || detailScope === 'user');
  const boundSession = bound?.id;
  const boundDirectory = bound?.cwd;
  const probeSession = target.kind === 'user' ? executionSession?.id : boundSession;
  const boundTarget = boundDirectory ? { kind: 'workspace' as const, directory: boundDirectory } : undefined;
  useEffect(() => {
    if (!visible || module !== 'mcp' || !sessionCandidate || transport.connection !== 'connected') return;
    const hostAuthority = authority.capture();
    const before = client.getSnapshot();
    const registration = hostAuthority?.catalog.workspaces.find(row => row.id === workspaceId);
    if (!registration || !sameEndpoint(before.endpoint, hostAuthority?.scope.endpoint)) return;
    let currentRead = true;
    const candidateId = sessionCandidate.id;
    const candidateCwd = sessionCandidate.cwd;
    const current = () => {
      const live = client.getSnapshot();
      const association = associations.getSnapshot().entries.get(candidateId);
      const row = authority.getCatalog()?.workspaces.find(entry => entry.id === workspaceId);
      return currentRead && activeBoundKey.current === boundKey && !!hostAuthority?.current()
        && row?.location === registration.location
        && clientCatalogWorkspace(association) === workspaceId && association?.cwd === candidateCwd
        && live.connection === 'connected' && live.generation === before.generation
        && live.endpoint === before.endpoint && live.authorityRevision === before.authorityRevision;
    };
    const controller = new AbortController();
    void client.request({ method: 'mcp/sourceRead', params: {
      target: { kind: 'workspace', directory: candidateCwd }, execution_session: candidateId,
    } }, 'mcp_source', undefined, current, controller.signal).then(({ authority: resolvedAuthority, source }) => {
      if (current()) setBoundObservation({ key: boundKey, authority: resolvedAuthority, source: source ?? undefined });
    }, error => {
      if (current()) setBoundObservation({ key: boundKey, error: String(error) });
    });
    return () => { currentRead = false; controller.abort(); };
  }, [associations, authority, boundKey, client, module, sessionCandidate, transport.connection, visible, workspaceId]);
  const probes = useMcpProbes(client, host, target, observation, visible && available && module === 'mcp' && !mcpFocus,
    associations, probeSession, boundTarget && { target: boundTarget, document: boundSource?.source });
  const scope = target.kind;
  const config = configAuthoring(source, scope);
  const revision = config.state === 'structured' ? config.revision : undefined;
  const editable = available && !bound && !awaitingAssociation && (!sessionCandidate || authoringConfirmed) && !!observation && !readError && transport.connection === 'connected';
  const skillTransferScope = module === 'skills' && visible && showingWorkspace && editable && directory
    && boundSkillState?.status === 'ready' && boundSkillState.authority === 'authoring' ? JSON.stringify([lifetime, boundKey, directory]) : undefined;
  const activeTransfer = pendingTransfer && (pendingTransfer.intent.family !== 'skill' || (
    !!skillTransferScope && pendingTransfer.skillScope === skillTransferScope
  )) ? pendingTransfer : undefined;
  const liveTransferWorkspace = useRef(lifetime);
  useLayoutEffect(() => {
    liveTransferWorkspace.current = lifetime;
    liveTransfer.current = activeTransfer?.token;
    return () => { liveTransfer.current = undefined; };
  }, [activeTransfer, lifetime]);
  const title = tx(`resources:${module}.title`);
  const scopeLabel = showingWorkspace ? workspace?.displayName : tx('resources:scope.global');
  const resetScope = () => {
    setMcpFocus(undefined);
    setDetailScope(undefined);
    setPendingTransfer(undefined);
    setResourceFocus(undefined);
    setNewAgent(undefined);
    setQuery('');
  };
  if (pendingTransfer && !activeTransfer) setPendingTransfer(undefined);
  if (owner !== presentedOwner) {
    mcpDrafts.current.clear();
    setPresentedOwner(owner);
    resetScope();
  }
  if (hostOwner !== presentedHost) {
    setPresentedHost(hostOwner);
    for (const [key, entry] of mcpDrafts.current) {
      if (entry.hostOwner !== undefined) mcpDrafts.current.delete(key);
    }
    if (showingWorkspace && presentedHost !== undefined) resetScope();
  }
  const changeCreationScope = (next: SettingsTarget, draft: AgentDraft) => {
    const { tools, workflows: _workflows, skills: _skills, ...fields } = draft;
    setNewAgent(tools?.builtin?.length ? { ...fields, tools: { builtin: tools.builtin } } : fields);
    setQuery('');
    setTab(next.kind === 'user' ? 'global' : 'workspace');
    if (next.kind === 'workspace') { chosen.current = true; setWorkspace(next.id); }
  };
  const mcpToolsLane = useRef({});
  const loadMcpTools = useCallback<LoadMcpTools>(async (scope, server, owner, signal) => {
    const hostAuthority = authority.capture();
    const currentHost = hostAuthority && JSON.stringify([hostAuthority.scope.authorityId, hostAuthority.scope.endpoint]);
    if (scope === 'workspace' && (!workspaceId || !directory || !hostAuthority || currentHost !== hostOwner)) return;
    const port = createWorkflowPort({ client, host: () => host, hostAuthority, expectedWorkspaceDirectory: directory, executionSession,
      scope: scope === 'workspace' && workspaceId ? { kind: 'workspace', workspaceId } : USER_SCOPE });
    const list = async () => {
      const catalog = await readToolCatalog(port, server, owner, { signal, owns: () => scope === 'user' || !!hostAuthority?.current() });
      return catalog.sources.find(entry => entry.source_id === server);
    };
    return scope === 'workspace' ? withWorkflowOperation(mcpToolsLane.current, list) : list();
  }, [client, host, authority, workspaceId, directory, hostOwner, executionSession]);
  const listTitle = useRef<HTMLHeadingElement>(null);
  const returnFocus = useRef(false);
  useEffect(() => {
    if (!agentEditor && !detail && returnFocus.current) {
      returnFocus.current = false;
      listTitle.current?.focus({ preventScroll: true });
    }
  }, [agentEditor, detail]);
  const editingGlobal = showingWorkspace && detailScope === 'user';
  const editorSource = editingGlobal ? globalSource : source;
  const editorActor = editingGlobal ? globalActor : actor;
  const editorTarget = editingGlobal ? userSettingsTarget : target;
  const editorEditable = editingGlobal ? globalEditable : editable;
  const editorAvailable = editingGlobal || available;
  const editorScopeLabel = editingGlobal ? tx('resources:scope.global') : scopeLabel;
  const editorError = editingGlobal ? globalError : readError;
  const closeAgent = () => { returnFocus.current = true; setNewAgent(undefined); setResourceFocus(undefined); setDetailScope(undefined); };
  const focusMcp = (next?: McpFocus, owner: SourceScope = scope) => {
    if (!next) returnFocus.current = true;
    setDetailScope(next ? owner : undefined);
    setMcpFocus(next);
  };
  const openAgent = (name: string, owner: SourceScope) => {
    setDetailScope(owner);
    setResourceFocus(name);
  };
  const mcpDraftKey = mcpFocus ? JSON.stringify([owner, editorTarget.kind === 'workspace' ? lifetime : 'user', editorSource?.target, mcpFocus.name ?? null]) : undefined;
  const editorConfig = configAuthoring(editorSource, editorTarget.kind);
  const refresh = () => {
    if (!sessionCandidate) actor.send({ type: 'REFRESH' });
    if (catalogModule) setCatalogRefresh(value => value + 1);
    if (showingWorkspace && module !== 'skills') globalActor.send({ type: 'REFRESH' });
  };
  const refreshCurrentWorkspace = useRef(refresh);
  useLayoutEffect(() => { refreshCurrentWorkspace.current = refresh; });
  const refreshTransferredWorkspace = (scope: string, token: number) => {
    if (scope !== liveTransferWorkspace.current) return;
    // A refresh would revoke a newer editable dialog. Let that dialog
    // close first, then refresh the workspace that owned the old request.
    if (liveTransfer.current !== undefined && liveTransfer.current !== token) setOwedTransferRefresh(scope);
    else refreshCurrentWorkspace.current();
  };
  useEffect(() => {
    if (!owedTransferRefresh || activeTransfer) return;
    setOwedTransferRefresh(undefined);
    if (owedTransferRefresh === lifetime) refreshCurrentWorkspace.current();
  }, [activeTransfer, lifetime, owedTransferRefresh]);
  const bindSkillWrite = (skill: SkillDescriptor, expected_revision?: string) => {
    if (!expected_revision || !editable) return;
    const { endpoint, authorityRevision } = client.getSnapshot();
    const hostAuthority = authority.capture();
    return async (mutation: SourceMutation) => {
      const current = () => {
        const snapshot = client.getSnapshot();
        return snapshot.endpoint === endpoint && snapshot.authorityRevision === authorityRevision && snapshot.connection === 'connected';
      };
      const unavailable = () => new Error(tx('resources:scope.unavailable'));
      if (!current()) throw unavailable();
      if (skill.provenance.source === 'user') {
        await client.request({ method: 'configuration/sourceWrite', params: { target: { kind: 'user' }, expected_revision, mutation } },
          'sources_written', undefined, current);
        return;
      }
      if (!workspaceId || !endpoint || !directory || !hostAuthority?.current()
        || !sameEndpoint(hostAuthority.scope.endpoint, endpoint) || !host?.configureWorkspace) throw unavailable();
      const result = await host.configureWorkspace(workspaceId, endpoint, {
        kind: 'write', expected_directory: directory, expected_revision, mutation,
      });
      if (result.kind !== 'write') throw unavailable();
    };
  };
  const bindSkillDisplayWrite = (skill: SkillDescriptor): SkillDisplayWriter | undefined => {
    const write = bindSkillWrite(skill, skill.display?.revision ?? undefined);
    if (write) return (authored, upload) => write({ kind: 'skill_display', name: skill.catalog.name, authored, upload });
  };
  const bindSkillDelete = (skill: SkillDescriptor) => {
    const write = bindSkillWrite(skill, skill.operation_revision ?? undefined);
    if (write) return () => write({ kind: 'skill_delete', name: skill.catalog.name });
  };
  const resourceNames = (shown: SourceSettings | undefined, owner: SourceScope) => module === 'subagents'
    ? shown?.agents.filter(agent => agent.scope === owner).map(agent => agent.name) ?? []
    : Object.keys((owner === 'user' ? shown?.user_mcp : shown?.workspace_mcp)?.authored ?? {});
  const startTransfer = (from: SourceScope, name: string, revision: string, operation: TransferIntent['operation'], skillNames?: ReadonlySet<string>) => {
    if (!showingWorkspace || !workspace || !directory || !editable || (module === 'skills' ? !skillTransferScope : !globalEditable)) return;
    const destination = from === 'user' ? resourceNames(source, 'workspace') : resourceNames(globalSource, 'user');
    const names = from === 'user' ? resourceNames(globalSource, 'user') : resourceNames(source, 'workspace');
    const token = ++transferSequence.current;
    const fromLabel = from === 'user' ? tx('resources:scope.global') : workspace.displayName;
    setPendingTransfer({ token, skillScope: skillTransferScope, fromLabel, intent: {
      from, name, revision, operation, family: module === 'skills' ? 'skill' : module === 'subagents' ? 'agent' : 'mcp', directory,
      destinationLabel: from === 'user' ? workspace.displayName : tx('resources:scope.global'),
      taken: skillNames ?? new Set([...names.filter(id => id !== name), ...destination]),
      transfer: bindResourceTransfer(client, () => host, authority.capture(), workspace.id,
        module === 'skills' ? () => liveTransfer.current === token : undefined),
    } });
  };
  const transfer = showingWorkspace && workspace ? startTransfer : undefined;
  const transferDisabled = !directory || !editable || (module === 'skills' ? !skillTransferScope : !globalEditable) || busy || !host.configureWorkspace;
  const renderResources = (shown: SourceSettings, owner: SourceScope, inherited = false) => {
    const config = configAuthoring(shown, owner);
    const revision = config.state === 'structured' ? config.revision : undefined;
    const group = inherited ? tx('resources:scope.inherited') : undefined;
    const workspaceRevision = inherited && scope === 'workspace' ? configAuthoring(source, scope) : undefined;
    const localNames = inherited ? new Set(resourceNames(source, 'workspace')) : undefined;
    const canOverride = inherited && scope === 'workspace' && editable
      ? (name: string) => !localNames?.has(name)
      : undefined;
    const workspaceAvailability = workspaceRevision?.state === 'structured' && source
      ? (name: string, valid?: boolean) => localNames?.has(name) ? undefined
        : <SettingsActorContext value={actor}><SourceContext value={source}>
          <ResourceAvailability family={module === 'subagents' ? 'agent' : 'mcp'} name={name} valid={valid}
            source={source} scope="workspace" revision={workspaceRevision.revision} />
        </SourceContext></SettingsActorContext>
      : undefined;
    return module === 'mcp'
      ? <McpPage source={shown} scope={owner} revision={revision} onFocus={next => focusMcp(next, owner)}
        query={query} view={view} group={group} probeStates={inherited ? undefined : probes.states}
        testConnection={probes.testConnection} probeAvailable={!inherited && probes.available}
        onOverride={canOverride ? name => focusMcp({ kind: 'mcp', name }, 'workspace') : undefined} canOverride={canOverride}
        onTransfer={transfer} transferDisabled={transferDisabled} workspaceAvailability={workspaceAvailability} />
      : <SubagentList source={shown} scope={owner} revision={revision} query={query} view={view} group={group}
        onOpen={name => openAgent(name, owner)} onOverride={canOverride ? name => openAgent(name, 'workspace') : undefined}
        canOverride={canOverride} onTransfer={transfer} transferDisabled={transferDisabled} workspaceAvailability={workspaceAvailability} />;
  };

  const lifecycle = awaitingAssociation ? 'loading'
    : workspaceSkills && (sessionCandidate || boundSkillState?.authority === 'client' || boundSkillState?.clientAuthority)
      && (!authoringConfirmed || boundSkillState?.status !== 'ready')
      ? (boundSkillState?.status ?? (readError ? 'failed' : 'loading'))
    : sessionCandidate && !authoringConfirmed
      ? (boundSource?.authority === 'client' && boundSource.source ? 'ready' : boundSource?.error ? 'failed' : 'loading')
      : (available && observation && !readError ? 'ready' : readError ? 'failed' : 'loading');

  return <section className={`primary-page ${catalogCss.surface}`} aria-label={title} data-resource-module={module}
    data-lifecycle={lifecycle} data-catalog-authority={visible ? boundSkillState?.authority : undefined} hidden={!visible} inert={!visible}>
    {agentEditor ? <div className={clsx(catalogCss.library, css.detailEditor)} key={lifetime}><SettingsFeedback active={visible}>
      {[editorError, maintenanceError, convergenceError].filter(Boolean).map((error, index) => <SettingsNotice text={error} key={index} />)}
      {editorAvailable && editorSource ? <SettingsActorContext value={editorActor}><SourceContext value={editorSource}>
        <AgentEditor source={editorSource} target={editorTarget} host={host} name={resourceFocus} initialDraft={newAgent} editable={editorEditable}
          client={client} executionSession={editorSource.target.kind === 'workspace' && executionSession?.cwd === editorSource.target.directory ? executionSession?.id : undefined}
          discover={visible && editorEditable} onScopeChange={changeCreationScope} onClose={closeAgent} loadMcpTools={loadMcpTools}
          models={catalogIdentities(editorSource, editorTarget.kind, 'models')} profiles={catalogProfiles(editorSource, editorTarget.kind)} />
      </SourceContext></SettingsActorContext> : <>
        <Button size="sm" onClick={closeAgent}>{tx('resources:cancel')}</Button>
        <p className={catalogCss.state} role="status">{tx(editorAvailable ? 'resources:loading' : 'resources:scope.unavailable')}</p>
      </>}
    </SettingsFeedback></div> : detail ? <div className={clsx(catalogCss.library, css.detailEditor)} key={lifetime}>
      <div className={clsx(settings.settings, css.mcpDetail)}>
        {[editorError, maintenanceError, convergenceError].filter(Boolean).map((error, index) => <p className={catalogCss.banner} role="alert" key={index}>{error}</p>)}
        {editorSource && mcpFocus && <SettingsActorContext value={editorActor}><SourceContext value={editorSource}>
          <fieldset className={css.editor} disabled={!editorEditable}>
            <McpDetail source={editorSource} scope={editorTarget.kind} revision={editorConfig.state === 'structured' ? editorConfig.revision : undefined}
              focus={mcpFocus} onClose={() => focusMcp(undefined)} scopeControl={<span>{editorScopeLabel}</span>}
              editorDraft={mcpDraftKey ? mcpDrafts.current.get(mcpDraftKey)?.draft : undefined}
              onEditorDraftChange={draft => {
                if (!mcpDraftKey) return;
                if (draft) mcpDrafts.current.set(mcpDraftKey, { draft, hostOwner: editorTarget.kind === 'workspace' ? hostOwner : undefined });
                else mcpDrafts.current.delete(mcpDraftKey);
              }} />
          </fieldset>
        </SourceContext></SettingsActorContext>}
      </div>
    </div> : <div className={catalogCss.library}>
      <header className={clsx(catalogCss.pageHead, css.heading)}>
        <div><h1 ref={listTitle} tabIndex={-1}>{title}</h1><p className={catalogCss.muted}>{tx(`resources:${module}.description`)}</p></div>
        {module === 'mcp' && <McpActions disabled={!editable || !(scope === 'user' ? source?.user_mcp : source?.workspace_mcp)?.authored} onFocus={focusMcp} />}
        {module === 'subagents' && <Button variant="primary" icon={<IconPlusOutline16 />} disabled={!editable} onClick={() => setNewAgent({})}>{tx('resources:subagents.new')}</Button>}
      </header>
      <Tabs selectedKey={tab} onSelectionChange={key => { resetScope(); setTab(key as typeof tab); }}>
        <TabList className={catalogCss.tabs} aria-label={tx('resources:scope.label')}>
          {(['workspace', 'global'] as const).map(value => <Tab key={value} id={value} className={clsx(catalogCss.tab, css.tab)}>{tx(`resources:scope.${value}`)}</Tab>)}
        </TabList>
        <TabPanel id={tab}>
          <div className={clsx(catalogCss.controls, css.controls, !showingWorkspace && catalogCss.globalControls)}>
            {showingWorkspace ? <div className={catalogCss.controlsLeft}>
              <span className={catalogCss.subtle}>{tx('workflows:scope.choose-workspace')}</span>
              <Menu open={workspaceMenu} autoFocus onClose={() => setWorkspaceMenu(false)} selectedId={workspaceId}
                items={workspaces.map(row => ({ id: row.id, label: row.displayName, icon: <IconFolderClose16 /> }))}
                onSelect={id => { chosen.current = true; resetScope(); setWorkspace(id); setWorkspaceMenu(false); }}
                anchor={<button type="button" className={catalogCss.chooser} aria-label={tx('resources:scope.choose')}
                  aria-haspopup="menu" aria-expanded={workspaceMenu} onClick={() => setWorkspaceMenu(value => !value)}>
                  <span className={catalogCss.chooserValue}><IconFolderClose16 size={14} /><span className={css.workspaceName}>{workspace?.displayName ?? tx('resources:scope.choose')}</span></span><IconChevronDownOutline14 />
                </button>} />
            </div> : <p className={catalogCss.context}>{scopeLabel}</p>}
            <div className={clsx(catalogCss.controlsRight, css.tools)}>
              <CatalogSearch label={tx(module === 'mcp' ? 'settings:mcp.search' : 'resources:search')} value={query} onChange={setQuery} />
              <CatalogViewSwitch view={view} onChange={setView} />
              <Button size="sm" variant="outline" className={css.refresh} aria-label={tx('resources:refresh')} title={tx('resources:refresh')}
                disabled={!available || busy || transport.connection !== 'connected'} onClick={refresh}><IconRefreshOutline16 /></Button>
            </div>
          </div>
          {awaitingAssociation ? <p className={catalogCss.state} role="status">{tx('resources:loading')}</p>
          : sessionCandidate && module === 'skills' ? <SettingsActorContext value={actor}>
            <SourceContext value={authoringConfirmed ? source : undefined}>
              {authoringConfirmed && [readError, maintenanceError, convergenceError].filter(Boolean).map((error, index) => <p className={catalogCss.banner} role="alert" key={index}>{error}</p>)}
              <fieldset className={css.editor} disabled={authoringConfirmed && !!source && !editable}>
                <ResourceBrowser key={visible ? boundKey : undefined} active={visible} client={client} source={source} target={{ kind: 'workspace', directory: sessionCandidate.cwd }} scope="workspace" revision={revision}
                  query={query} view={view} refresh={catalogRefresh} onRefresh={refresh} workspaceLabel={workspace?.displayName} executionSession={sessionCandidate.id}
                  bindDisplayWrite={bindSkillDisplayWrite} bindDelete={bindSkillDelete} onTransfer={transfer} transferDisabled={transferDisabled}
                  onCatalogState={observeSkills} />
              </fieldset>
            </SourceContext>
          </SettingsActorContext> : sessionCandidate && !authoringConfirmed && module === 'mcp' ? (
            boundSource?.authority === 'client' && boundSource.source ? <McpPage workspaceDocument={boundSource.source} scope="workspace" query={query} view={view}
              probeStates={probes.states} testConnection={probes.testConnection} probeAvailable={probes.available} />
              : boundSource?.error ? <p className={catalogCss.banner} role="alert">{boundSource.error}</p>
                : <p className={catalogCss.state} role="status">{tx('resources:loading')}</p>
          ) : !available ? <p className={catalogCss.state} role="status">{tx(workspaceId ? 'resources:scope.unavailable' : 'resources:scope.empty')}</p> : <>
            {[readError, maintenanceError, convergenceError].filter(Boolean).map((error, index) => <p className={catalogCss.banner} role="alert" key={index}>{error}</p>)}
            {!source ? <p className={catalogCss.state} role="status">{tx('resources:loading')}</p> : <SettingsActorContext value={actor}><SourceContext value={source}>
              <fieldset className={css.editor} disabled={!editable && boundSkillState?.authority !== 'client'} key={lifetime}>
                {module === 'skills' ? <ResourceBrowser key={workspaceSkills && visible ? boundKey : undefined} active={visible} client={client}
                  source={source} target={source.target} scope={scope} revision={revision}
                  query={query} view={view} refresh={catalogRefresh} onRefresh={refresh} workspaceLabel={workspace?.displayName}
                  bindDisplayWrite={bindSkillDisplayWrite} bindDelete={bindSkillDelete} onTransfer={transfer} transferDisabled={transferDisabled}
                  onCatalogState={workspaceSkills ? observeSkills : undefined} /> : renderResources(source, scope)}
              </fieldset>
            </SourceContext></SettingsActorContext>}
          </>}
          {showingWorkspace && module !== 'skills' && (globalError ? <p className={catalogCss.banner} role="alert">{globalError}</p>
            : globalSource ? <SettingsActorContext value={globalActor}><SourceContext value={globalSource}>
              <fieldset className={clsx(css.editor, catalogCss.scopeGroup)} disabled={!globalEditable}>{renderResources(globalSource, 'user', true)}</fieldset>
            </SourceContext></SettingsActorContext> : <p className={catalogCss.state} role="status">{tx('resources:loading')}</p>)}
        </TabPanel>
      </Tabs>
    </div>}
    {[...transferNotices].filter(([, notice]) => notice.scope === lifetime).map(([token, notice]) =>
      <div className={catalogCss.banner} role="alert" key={token}>
        <p><strong>{notice.name}</strong> {tx(`resources:transfer.notice-${notice.reason}`)}</p>
        <p>{tx(notice.operation === 'move' ? 'resources:transfer.notice-move-target' : 'resources:transfer.notice-copy-target', {
          p0: notice.fromLabel, p1: notice.destination, p2: notice.destinationLabel,
        })}</p>
        <Button size="sm" variant="outline" onClick={() => setTransferNotices(current => {
          const remaining = new Map(current);
          remaining.delete(token);
          return remaining;
        })}>{tx('workflows:dialog.close')}</Button>
      </div>)}
    {activeTransfer && <TransferDialog key={activeTransfer.token} intent={activeTransfer.intent}
      close={() => setPendingTransfer(current => current?.token === activeTransfer.token ? undefined : current)}
      refresh={activeTransfer.intent.family === 'skill' ? () => refreshTransferredWorkspace(lifetime, activeTransfer.token) : refresh}
      onReopen={activeTransfer.intent.family === 'skill' ? (reason, destination) => {
        const { token, intent, fromLabel } = activeTransfer;
        setTransferNotices(current => new Map(current).set(token, {
          scope: lifetime, name: intent.name, operation: intent.operation, fromLabel,
          destination, destinationLabel: intent.destinationLabel, reason,
        }));
        setPendingTransfer(current => current?.token === token ? undefined : current);
        refreshTransferredWorkspace(lifetime, token);
      } : undefined} />}
  </section>;
}

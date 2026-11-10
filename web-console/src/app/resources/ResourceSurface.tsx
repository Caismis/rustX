import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from 'react';
import { useSelector } from '@xstate/react';
import clsx from 'clsx';
import { Tabs, TabList, Tab, TabPanel } from 'react-aria-components';
import type { AppServerClient } from '../../client/app-server';
import type { ProductHostWorkspaces } from '../../workspaces/host';
import type { WorkspaceAuthority } from '../../workspaces/authority';
import type { WorkspaceAssociations } from '../../workspaces/associations';
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
import { McpActions, McpPage } from './mcp/McpPage';
import { useMcpProbes } from './mcp/useMcpProbes';
import { ResourceBrowser } from './shared/ResourceBrowser';
import { AgentEditor, type AgentDraft } from './subagents/AgentEditor';
import type { LoadMcpTools } from './subagents/AgentToolFields';
import { SubagentList } from './subagents/SubagentList';
import type { McpEditorDraft, McpFocus, ResourceModule } from './types';
import css from './ResourceSurface.module.css';

interface Props {
  module: ResourceModule; visible: boolean; client: AppServerClient; host: ProductHostWorkspaces;
  authority: WorkspaceAuthority; associations: WorkspaceAssociations; initialWorkspace?: string;
}

/** Retain each module's presentation independently of its source transactions. */
export function ResourceSurface(props: Props) {
  const [visited, setVisited] = useState(props.visible);
  if (props.visible && !visited) setVisited(true);
  return (visited || props.visible) && <ResourcePage {...props} />;
}

function ResourcePage({ module, visible, client, host, authority, associations, initialWorkspace }: Props) {
  const tx = useTranslation();
  const catalog = useSyncExternalStore(authority.subscribe, authority.getCatalog);
  useEffect(() => { if (visible) void authority.observe().catch(() => {}); }, [authority, visible]);
  const [tab, setTab] = useState<'workspace' | 'global'>(initialWorkspace ? 'workspace' : 'global');
  const [workspaceId, setWorkspace] = useState(initialWorkspace);
  const chosen = useRef(!!initialWorkspace);
  const workspaces = catalog?.workspaces ?? [];
  useEffect(() => {
    if (!chosen.current && workspaces.length) { chosen.current = true; setWorkspace(workspaces[0].id); }
  }, [workspaces]);
  const workspace = workspaces.find(row => row.id === workspaceId);
  const showingWorkspace = tab === 'workspace';
  const target = showingWorkspace && workspaceId ? workspaceSettingsTarget(workspaceId, workspace?.displayName ?? workspaceId) : userSettingsTarget;
  const available = !showingWorkspace || !!workspace;
  const hostOwner = catalog && JSON.stringify([catalog.authorityId, catalog.endpoint]);
  const { actor, transport } = useSettingsTarget(client, target, host, visible && available, showingWorkspace ? hostOwner : undefined);
  const observation = useSelector(actor, state => state.context.observation);
  const source = useSelector(actor, state => state.context.observation ?? state.context.staleObservation);
  const readError = useSelector(actor, state => state.context.readError);
  const maintenanceError = useSelector(actor, state => state.context.maintenanceError);
  const convergenceError = useSelector(actor, state => state.context.convergenceError);
  const busy = useSelector(actor, state => state.matches({ mutation: 'submitting' }) || state.matches({ maintenance: 'reconciling' }));
  const [workspaceMenu, setWorkspaceMenu] = useState(false);
  const [query, setQuery] = useState('');
  const [view, setView] = useState<CatalogView>('cards');
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
  const detail = !!mcpFocus;
  const probes = useMcpProbes(client, host, target, observation, visible && available && module === 'mcp' && !mcpFocus, associations);
  const scope = target.kind;
  const config = configAuthoring(source, scope);
  const revision = config.state === 'structured' ? config.revision : undefined;
  const editable = available && !!observation && !readError && transport.connection === 'connected';
  const title = tx(`resources:${module}.title`);
  const scopeLabel = showingWorkspace ? workspace?.displayName : tx('resources:scope.global');
  const resetScope = () => {
    setMcpFocus(undefined);
    setResourceFocus(undefined);
    setNewAgent(undefined);
    setQuery('');
  };
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
  const loadMcpTools = useCallback<LoadMcpTools>(async (scope, server, owner, owns) => {
    const hostAuthority = authority.capture();
    const currentHost = hostAuthority && JSON.stringify([hostAuthority.scope.authorityId, hostAuthority.scope.endpoint]);
    if (scope === 'workspace' && (!workspaceId || !directory || !hostAuthority || currentHost !== hostOwner)) return;
    const port = createWorkflowPort({ client, host: () => host, hostAuthority, expectedWorkspaceDirectory: directory,
      scope: scope === 'workspace' && workspaceId ? { kind: 'workspace', workspaceId } : USER_SCOPE });
    const catalog = await readToolCatalog(port, server, owner, () => owns() && (scope === 'user' || !!hostAuthority?.current()));
    return catalog.sources.find(entry => entry.source_id === server);
  }, [client, host, authority, workspaceId, directory, hostOwner]);
  const listTitle = useRef<HTMLHeadingElement>(null);
  const returnFocus = useRef(false);
  useEffect(() => {
    if (!agentEditor && !detail && returnFocus.current) {
      returnFocus.current = false;
      listTitle.current?.focus({ preventScroll: true });
    }
  }, [agentEditor, detail]);
  const closeAgent = () => { returnFocus.current = true; setNewAgent(undefined); setResourceFocus(undefined); };
  const focusMcp = (next?: McpFocus) => {
    if (!next) returnFocus.current = true;
    setMcpFocus(next);
  };
  const mcpDraftKey = mcpFocus ? JSON.stringify([lifetime, source?.target, mcpFocus.name ?? null]) : undefined;
  const mcpPage = module === 'mcp' && source && <McpPage source={source} scope={scope} revision={revision} focus={mcpFocus} onFocus={focusMcp}
    editorDraft={mcpDraftKey ? mcpDrafts.current.get(mcpDraftKey)?.draft : undefined}
    onEditorDraftChange={draft => {
      if (!mcpDraftKey) return;
      if (draft) mcpDrafts.current.set(mcpDraftKey, { draft, hostOwner: showingWorkspace ? hostOwner : undefined });
      else mcpDrafts.current.delete(mcpDraftKey);
    }}
    scopeControl={<span>{scopeLabel}</span>} query={query} view={view} probeStates={probes.states} testConnection={probes.testConnection} probeAvailable={probes.available} />;

  return <section className={`primary-page ${catalogCss.surface}`} aria-label={title} data-resource-module={module}
    data-lifecycle={available && observation && !readError ? 'ready' : readError ? 'failed' : 'loading'} hidden={!visible} inert={!visible}>
    {agentEditor ? <div className={clsx(catalogCss.library, css.detailEditor)} key={lifetime}><SettingsFeedback active={visible}>
      {[readError, maintenanceError, convergenceError].filter(Boolean).map((error, index) => <SettingsNotice text={error} key={index} />)}
      {available && source ? <SettingsActorContext value={actor}><SourceContext value={source}>
        <AgentEditor source={source} target={target} host={host} name={resourceFocus} initialDraft={newAgent} editable={editable}
          client={client} discover={visible && editable} onScopeChange={changeCreationScope} onClose={closeAgent} loadMcpTools={loadMcpTools}
          models={catalogIdentities(source, scope, 'models')} profiles={catalogProfiles(source, scope)} />
      </SourceContext></SettingsActorContext> : <>
        <Button size="sm" onClick={closeAgent}>{tx('resources:cancel')}</Button>
        <p className={catalogCss.state} role="status">{tx(available ? 'resources:loading' : 'resources:scope.unavailable')}</p>
      </>}
    </SettingsFeedback></div> : detail ? <div className={clsx(catalogCss.library, css.detailEditor)} key={lifetime}>
      <div className={clsx(settings.settings, css.mcpDetail)}>
        {[readError, maintenanceError, convergenceError].filter(Boolean).map((error, index) => <p className={catalogCss.banner} role="alert" key={index}>{error}</p>)}
        {source && <SettingsActorContext value={actor}><SourceContext value={source}>
          <fieldset className={css.editor} disabled={!editable}>{mcpPage}</fieldset>
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
                disabled={!available || busy || transport.connection !== 'connected'} onClick={() => actor.send({ type: 'REFRESH' })}><IconRefreshOutline16 /></Button>
            </div>
          </div>
          {!available ? <p className={catalogCss.state} role="status">{tx(workspaceId ? 'resources:scope.unavailable' : 'resources:scope.empty')}</p> : <>
            {[readError, maintenanceError, convergenceError].filter(Boolean).map((error, index) => <p className={catalogCss.banner} role="alert" key={index}>{error}</p>)}
            {!source ? <p className={catalogCss.state} role="status">{tx('resources:loading')}</p> : <SettingsActorContext value={actor}><SourceContext value={source}>
              <fieldset className={css.editor} disabled={!editable} key={lifetime}>
                {module === 'mcp' ? mcpPage
                  : module === 'subagents' ? <SubagentList source={source} scope={scope} revision={revision} query={query} view={view} onOpen={setResourceFocus} />
                  : <ResourceBrowser client={client} source={source} scope={scope} revision={revision}
                    query={query} view={view} focus={resourceFocus} onFocus={setResourceFocus} />}
              </fieldset>
            </SourceContext></SettingsActorContext>}
          </>}
        </TabPanel>
      </Tabs>
    </div>}
  </section>;
}

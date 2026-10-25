/* Copyright (c) 2026 DeepSeek. MIT. Harness guide and DockLayout port; see PROVENANCE.md. */
import { lazy, Suspense, useMemo, useEffect, useImperativeHandle, useLayoutEffect, type ReactNode, type Ref, useRef, useState, useSyncExternalStore } from 'react';
import type { ProductHostWorkspaces, WorkspaceAuthorityScope } from '../../workspaces/host';
import type { WorkbenchRequest, WorkbenchTerminal, WorkbenchTarget } from '../../workspaces/workbench';
import { useTranslation } from '../../locale/react';
import { WorkspaceFiles, WorkspaceFile, fileTabView } from './WorkbenchFiles';
import { FileTypeIcon } from '../../presentation/primitives/FileTypeIcon';
import { IconPanelLeftOutline16, IconChevronDownOutline14, IconAgentPresetOutline16 } from '../../presentation/primitives/icons';
import { Menu } from '../../presentation/primitives/Menu';
import { Tooltip } from '../../presentation/primitives/Tooltip';
import { observeComposition } from '../../presentation/primitives/keyboard-composition';
import panelCss from '../../presentation/right-panel/SidebarRight.module.css';
import { CompassGlyph, GuideArtworkFiles, PluginArtworkTerminal, TerminalIcon, FullscreenGlyph, ExitFullscreenGlyph } from './WorkbenchIcons';
import { readSidebarLayout, writeSidebarLayout } from './workbench-persistence';
import { DockController } from '../../presentation/dockkit/engine/controller';
import { DockLayout } from '../../presentation/dockkit/components/DockSurface';
import { dockPaneIds, findTabPane, getPane } from '../../presentation/dockkit/engine/tree';
import type { LayoutState, PaneId, TabId, TabRecord } from '../../presentation/dockkit/contract/types';
import type { DockIntents } from '../../presentation/dockkit/contract/adapter';
import css from '../../presentation/right-panel/Workbench.module.css';
import { previewDemands } from '../../client/session-files';
import type { WorkbenchCall } from './WorkbenchFiles';
import { PREVIEW_POLICY } from '../../client/preview-policy';
import type { WorkbenchPreviewRetention } from '../workbench-previews';
import { ChangesReview } from './ChangesReview';
import type { ChangesReviewRequest } from '../changes-review';
const TerminalView = lazy(() => import('./WorkbenchTerminal'));
const guideTab = (id: TabId): TabRecord => ({id,kind:'start',contentId:`start:${id}`,title:''});
export interface WorkbenchHandle { openFile(path: string, line?: number): Promise<boolean>; openAgent(id: string, title: string): void; openChanges(request:ChangesReviewRequest):void }
export function Workbench({ renderAgent, fileRef, host, previews, scope, target, visible, floatingVisible, fullscreen, toggleFullscreen, closePanel }: {
  renderAgent?: (id: string, visible: boolean) => ReactNode; fileRef?: Ref<WorkbenchHandle>; host: ProductHostWorkspaces; previews: WorkbenchPreviewRetention; scope?: WorkspaceAuthorityScope; target?: WorkbenchTarget; visible: boolean; floatingVisible: boolean; fullscreen: boolean; toggleFullscreen: () => void; closePanel: (resetPresentation?: boolean) => void;
}) {
  const tx = useTranslation();
  const session = target && 'session_id' in target ? target : undefined;
  const workspace = target && 'workspace_id' in target ? target.workspace_id : undefined;
  const storageKey = JSON.stringify([scope?.endpoint,scope?.authorityId,session?.session_id,session?.active_node,...(workspace ? [workspace] : [])]);
  const [dock] = useState(() => new DockController({makeInitialTab:guideTab,makePaneTab:guideTab,settle:true,restored:readSidebarLayout(storageKey)}));
  const {state} = useSyncExternalStore(dock.subscribe,dock.getSnapshot,dock.getSnapshot);
  useEffect(() => {
    const minted = Math.max(0,...Object.keys(state.nodes).concat(Object.keys(state.tabs)).map(id=>Number(id.replace(/^[a-z]+/,''))));
    writeSidebarLayout(storageKey,{layout:state,minted,history:{entries:[],cursor:0}});
  },[state,storageKey]);
  const root = useRef<HTMLDivElement>(null);
  const focusRequest = useRef<{ paneId: PaneId; origin: Element | null; onRemoval?: boolean } | undefined>(undefined);
  const room = useRef<ReadonlyMap<PaneId, { row: boolean }>>(new Map());
  useLayoutEffect(() => {
    const request = focusRequest.current;
    if (!request) return;
    focusRequest.current = undefined;
    if (request.onRemoval && request.origin?.isConnected) return;
    // A later deliberate focus choice takes precedence over an async close/open.
    if (document.activeElement !== request.origin && document.activeElement !== document.body) return;
    const panes = [...root.current?.querySelectorAll<HTMLElement>('[data-dockkit-pane], [data-dockkit-float]') ?? []]
      .filter(pane => !pane.closest('[hidden], [aria-hidden="true"], [inert]') && pane.checkVisibility());
    const pane = panes.find(pane => (pane.dataset.dockkitPane ?? pane.dataset.dockkitFloat) === request.paneId)
      ?? panes.find(pane => pane.hasAttribute('data-dockkit-pane-active') || pane.hasAttribute('data-dockkit-float-active')) ?? panes[0];
    pane?.focus({ preventScroll: true });
  }, [state, visible, floatingVisible]);
  const retained = useMemo(() => previews.acquire(scope, target), [previews, storageKey]);
  const views = retained.views, cache = previews.cache;
  useEffect(() => () => { if (retained.current()) for (const view of retained.views.values()) if (view.preview) { view.preview.owner.deactivate(); cache.retain(view.preview.owner); } }, [retained, cache]);
  useEffect(() => { for (const [id, view] of views) if (!state.tabs[id]) { view.preview?.dispose(); views.delete(id); } }, [state.tabs, views]);
  const reviews = useRef(new Map<string,ChangesReviewRequest>());
  const navigation = useRef(0);
  const viewFor = (id:TabId) => { let view=views.get(id); if(!view) {view=fileTabView(cache, retained.current);views.set(id,view);}return view; };
  const [terminals,setTerminals] = useState<WorkbenchTerminal[]>([]), [shells,setShells] = useState<string[]>([]);
  const [shell,setShell] = useState(''),[error,setError] = useState(''),[busy,setBusy] = useState(false),[menu,setMenu] = useState<PaneId>();
  const pendingCreate = useRef<{id:string;shell:string}|null>(null), lifetime=useRef<AbortController|null>(null);
  useEffect(()=>{lifetime.current=new AbortController();return()=>lifetime.current?.abort();},[]);
  useEffect(()=>dock.setExpanded(visible),[dock,visible]);
  const call=useMemo<WorkbenchCall>(() => {
    const call: WorkbenchCall = async (request:WorkbenchRequest,signal?:AbortSignal) => {
      if(!host.workbench || !scope || !target) throw new Error('preview_unavailable');
      const active = signal ?? lifetime.current?.signal ?? new AbortController().signal;
      const work = () => host.workbench!(scope,{target,request},active);
      if (request.kind === 'office') { const demand = previewDemands(host, scope); return demand.documents.run(active, () => demand.privateReads.run(active, work)); }
      return request.kind === 'preview' ? previewDemands(host, scope).privateReads.run(active, work) : work();
    };
    if (host.observeWorkspaceFile && scope && target) call.observe = (path, changed, signal) => previewDemands(host, scope).privateReads.run(signal, () => host.observeWorkspaceFile!(scope, { target, request: { kind: 'preview', path, read: { kind: 'bytes', range: { offset: 0, length: 1 } } } }, changed, signal));
    return call;
  },[host,scope?.authorityId,scope?.endpoint,session?.session_id,session?.active_node,workspace]);
  const available=!!host.workbench && !!scope && !!target;
  const open=(paneId:PaneId,tab:{kind:string;path?:string;id?:string;title?:string})=>{
    if (tab.kind === 'file' && !Object.values(state.tabs).some(existing => existing.contentId === `file:${tab.path}`) && Object.values(state.tabs).filter(existing => existing.kind === 'file').length >= PREVIEW_POLICY.tabsPerSession) { setError(tx('artifacts:workspace.tabs')); return; }
    navigation.current++;
    focusRequest.current = { paneId, origin: document.activeElement };
    const before=dock.getSnapshot().state;
    const pane=getPane(before,paneId), active=pane.activeTabId && before.tabs[pane.activeTabId];
    const contentId=tab.kind==='file' ? `file:${tab.path}` : tab.kind==='files' ? `files:${paneId}` : tab.id ?? crypto.randomUUID();
    dock.openContent({paneId,kind:tab.kind,contentId,title:tab.title ?? tab.path ?? ''});
    if(active && active.kind==='start') dock.closeTab(active.id);
  };
  useImperativeHandle(fileRef, () => ({ openChanges: request => {
    const id = `changes:${request.target.attachment_id}:${request.attemptId}`;
    reviews.current.set(id, request);
    open(dock.activeDockPaneId(), {kind:'changes',id,title:tx('artifacts:changes.title',{count:request.summary.total})});
    setFileFocus(value=>value+1);
  }, openAgent: (id, title) => open(dock.activeDockPaneId(), { kind: 'subagent', id, title }), openFile: async (path, line) => {
    const ticket = ++navigation.current, signal = lifetime.current?.signal;
    setError('');
    try {
      const result = await call({ kind: 'resolve', path });
      if (signal?.aborted || ticket !== navigation.current || !result.path) return false;
      open(dock.activeDockPaneId(), { kind: 'file', path: result.path });
      if (line !== undefined) {
        const tab = Object.values(dock.getSnapshot().state.tabs).find(t => t.contentId === `file:${result.path}`)!;
        const view = viewFor(tab.id); view.mode = 'code'; view.line = line; view.focusRevision = (view.focusRevision ?? 0) + 1;
        // Remount just this view so a repeated line reference is a fresh navigation.
        setFileFocus(n => n + 1);
      }
      return true;
    } catch (cause) { if (!signal?.aborted && ticket === navigation.current) setError(String(cause)); return false; }
  }}));
  const [, setFileFocus] = useState(0);
  useEffect(()=>{
    if(!visible || !available) return;
    const abort=new AbortController();
    void call({kind:'terminals'},abort.signal).then(value=>{
      if(abort.signal.aborted)return;
      for(const tab of Object.values(dock.getSnapshot().state.tabs)) if(tab.kind==='terminal' && !value.terminals?.some(t=>t.id===tab.contentId))dock.closeTab(tab.id);
      if(!Object.keys(dock.getSnapshot().state.tabs).length)dock.addTab(dock.activeDockPaneId());
      setTerminals(value.terminals ?? []);setShells(value.shells ?? []);setShell(old=>old || value.shells?.[0] || '');
      const focused = getPane(dock.getSnapshot().state,dock.activeDockPaneId()).activeTabId;
      for(const terminal of value.terminals ?? []) if(!Object.values(dock.getSnapshot().state.tabs).some(tab=>tab.contentId===terminal.id))
        dock.openContent({kind:'terminal',contentId:terminal.id,title:terminal.shell.split('/').at(-1) ?? ''});
      if(focused && dock.getSnapshot().state.tabs[focused])dock.focusTab(focused);
    },cause=>{if(!abort.signal.aborted)setError(String(cause));});
    return()=>abort.abort();
  },[call,visible,available,dock]);
  const create=async(paneId:PaneId,selectedShell=shell)=>{
    if(busy || !selectedShell)return;
    const admittedNavigation = ++navigation.current;
    setBusy(true);setError('');const pending=pendingCreate.current ??= {id:crypto.randomUUID(),shell:selectedShell};
    try {const value=await call({kind:'create',...pending});if(!lifetime.current?.signal.aborted){pendingCreate.current=null;setTerminals(value.terminals ?? []);
      if(navigation.current === admittedNavigation) open(paneId,{kind:'terminal',id:pending.id});
      else {
        // A newer file/tab selection owns focus even when PTY creation finishes later.
        const current=dock.getSnapshot().state;
        const focused=getPane(current,current.activePaneId).activeTabId;
        dock.openContent({kind:'terminal',contentId:pending.id,title:pending.shell.split('/').at(-1) ?? ''});
        if(focused)dock.focusTab(focused);
      }}}
    catch(cause){if(!lifetime.current?.signal.aborted)setError(String(cause));}
    finally{if(!lifetime.current?.signal.aborted)setBusy(false);}
  };
  const canSplitPane = (id: PaneId) => dockPaneIds(dock.getSnapshot().state).length < 2 && room.current.get(id)?.row !== false;
  const splitPane = (id: PaneId) => {
    if (!canSplitPane(id)) return;
    const origin = document.activeElement;
    dock.splitPane(id);
    focusRequest.current = { paneId: dock.getSnapshot().state.activePaneId, origin };
  };
  const close=async(id:TabId)=>{
    const before=dock.getSnapshot().state, tab=before.tabs[id];
    if(!tab || !canCloseTab(before,id))return;
    const pane = findTabPane(before,id), origin = document.activeElement;
    const keepFocus = root.current?.contains(origin) || origin?.closest('[data-dockkit-tab-menu]');
    try {
      if(tab.kind==='terminal'){const value=await call({kind:'close',id:tab.contentId});if(lifetime.current?.signal.aborted)return;setTerminals(value.terminals ?? []);}
      const current = dock.getSnapshot().state;
      if (!current.tabs[id]) return;
      const lastDocked = soleDockedTab(current,id);
      if (lastDocked) dock.setExpanded(false);
      if (keepFocus) focusRequest.current = { paneId: pane.id, origin };
      dock.closeTab(id);views.get(id)?.preview?.dispose();views.delete(id);
      if(lastDocked){dock.setMode('push');closePanel(true);}
      else if (!visible && !dock.getSnapshot().state.floats.length) closePanel();
    }catch(cause){if(!lifetime.current?.signal.aborted)setError(String(cause));}
  };
  // Guide and file-browser pages are unique per pane; returning a floated page
  // focuses the existing page there, as Harness does for page kinds.
  const arrive = (id: TabId, paneId: PaneId, move: () => void) => {
    const current=dock.getSnapshot().state, tab=current.tabs[id], origin=document.activeElement, retainFocus=root.current?.contains(origin);
    const existing=['start','files'].includes(tab.kind) ? getPane(current,paneId).tabs.find(other=>other!==id && current.tabs[other].kind===tab.kind) : undefined;
    if(existing){dock.closeTab(id);views.get(id)?.preview?.dispose();views.delete(id);dock.focusTab(existing);}else move();
    if(retainFocus)focusRequest.current={paneId:dock.getSnapshot().state.activePaneId,origin,onRemoval:true};
  };
  const intents:DockIntents={
    focusTab:id=>{navigation.current++;dock.focusTab(id);},focusPane:id=>dock.focusPane(id),
    splitPane,
    addTab:id=>{navigation.current++;focusRequest.current={paneId:id,origin:document.activeElement};const pane=getPane(dock.getSnapshot().state,id);const guide=pane.tabs.find(t=>dock.getSnapshot().state.tabs[t].kind==='start');if(guide)dock.focusTab(guide);else dock.addTab(id);},
    closeTab:id=>{void close(id);}, duplicateTab:id=>dock.duplicateTab(id),
    floatTab:(id,rect)=>{const origin=document.activeElement,retainFocus=root.current?.contains(origin),paneId=dock.floatTab(id,rect);if(retainFocus)focusRequest.current={paneId,origin,onRemoval:true};},
    unfloatPane:id=>{const origin=document.activeElement,pane=dock.activeDockPaneId(),tab=getPane(dock.getSnapshot().state,id).tabs[0]!;focusRequest.current={paneId:pane,origin};arrive(tab,pane,()=>dock.unfloatPane(id));},placeTab:(id,p,index)=>arrive(id,p,()=>dock.placeTab(id,p,index)),
    dropTab:(id,p,zone)=>{if(zone==='center')arrive(id,p,()=>dock.dropTab(id,p,zone));else if(canSplitPane(p)){const origin=document.activeElement,retainFocus=root.current?.contains(origin);dock.dropTab(id,p,zone);if(retainFocus)focusRequest.current={paneId:dock.getSnapshot().state.activePaneId,origin,onRemoval:true};}},
    moveFloat:(id,x,y)=>dock.moveFloat(id,x,y),resizeFloat:(id,rect)=>dock.resizeFloat(id,rect),resizeSplit:(id,sizes)=>dock.resizeSplit(id,sizes),
  };
  useEffect(()=>{
    if(!visible && !floatingVisible)return;
    const composition=observeComposition(document);
    const key=(e:KeyboardEvent)=>{
      if(e.defaultPrevented||composition.guards(e)||document.querySelector('[role="dialog"][aria-modal="true"], [role="menu"]'))return;
      const focused = document.activeElement?.closest<HTMLElement>('[data-dockkit-pane], [data-dockkit-float]');
      if (focused && root.current?.contains(focused)) {
        const paneId=(focused.dataset.dockkitPane ?? focused.dataset.dockkitFloat) as PaneId;
        const pane=getPane(dock.getSnapshot().state,paneId);
        const primary=isMac ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey;
        if (primary && !e.shiftKey) {
          if (!e.altKey && e.code==='Backslash' && pane.host==='dock') { e.preventDefault(); if(!e.repeat)splitPane(paneId); return; }
          if (e.altKey && e.code==='Enter' && pane.host==='dock') { e.preventDefault(); if(!e.repeat)toggleFullscreen(); return; }
          // Web keeps the browser's close-tab shortcut; page close adds Alt.
          if (e.altKey && e.code==='KeyW' && pane.activeTabId) { e.preventDefault(); if(!e.repeat){if(canCloseTab(dock.getSnapshot().state,pane.activeTabId))void close(pane.activeTabId);else closePanel();} return; }
        }
      }
      if(!visible||!available)return;
      if(e.ctrlKey&&!e.altKey&&!e.metaKey&&e.code==='Backquote'){e.preventDefault();void create(dock.activeDockPaneId());}
      if(e.altKey&&(e.metaKey||e.ctrlKey)&&e.code==='KeyP'){e.preventDefault();open(dock.activeDockPaneId(),{kind:'files'});}
    };
    window.addEventListener('keydown',key,true);return()=>{window.removeEventListener('keydown',key,true);composition.dispose();};
  });
  const isMac=/Mac/.test(navigator.platform);
  const title=(tab:TabRecord)=>tab.kind==='changes'||tab.kind==='subagent'?tab.title:tab.kind==='file'?tab.title.split('/').at(-1):tab.kind==='terminal'?terminals.find(t=>t.id===tab.contentId)?.shell.split('/').at(-1) ?? tab.title:tx(tab.kind==='start'?'artifacts:workbench.start':'artifacts:workbench.file-tab');
  const visibleFiles = Object.values(state.tabs).filter(tab => {
    const pane = findTabPane(state, tab.id);
    // Collapsing the dock preserves its selected body (notably HTML iframe state).
    // Preview mode retires both surfaces; retained content still shares the body cap.
    return tab.kind === 'file' && pane.activeTabId === tab.id && (pane.host === 'float' ? floatingVisible : visible || floatingVisible);
  }).sort((left, right) => Number(findTabPane(state, right.id).id === state.activePaneId) - Number(findTabPane(state, left.id).id === state.activePaneId)).slice(0, PREVIEW_POLICY.activeBodies);
  const body=(tab:TabRecord)=>{
    const pane=findTabPane(state,tab.id);
    return <div className={css.body}>
    {tab.kind==='changes' ? (reviews.current.has(tab.contentId) && <ChangesReview request={reviews.current.get(tab.contentId)!} visible={pane.activeTabId===tab.id && (pane.host==='float'?floatingVisible:visible)}/>) : tab.kind==='subagent'?renderAgent?.(tab.contentId, pane.activeTabId === tab.id && (pane.host === 'float' ? floatingVisible : visible)):tab.kind==='start'? <div className={css.guide}>
        <span className={css.hero}><CompassGlyph size={56}/></span>
        <button type="button" className={css.entry} disabled={!available} aria-keyshortcuts={isMac ? 'Alt+Meta+P' : 'Alt+Control+P'} onClick={() => open(pane.id, { kind: 'files' })}><span className={css.entryIcon}><GuideArtworkFiles size={26}/></span><span className={css.entryText}><span className={css.entryTitle}>{tx('artifacts:workbench.files')}</span><span className={css.entryDescription}>{tx('artifacts:workbench.files-description')}</span></span><ShortcutKeys keys={isMac ? ['⌥', '⌘', 'P'] : ['Alt', 'Ctrl', 'P']}/></button>
        <div className={`${css.entry} ${css.terminalEntry}`}><button type="button" className={css.launch} aria-label={tx('artifacts:workbench.new-terminal')} aria-keyshortcuts="Control+`" disabled={!available || busy || !shell} onClick={() => void create(pane.id)}/><span className={css.entryIcon}><PluginArtworkTerminal size={26}/></span><span className={css.entryText}><span className={css.titleRow}><span className={css.entryTitle}>{tx('artifacts:workbench.new-terminal')}</span><Menu open={menu === pane.id} autoFocus align="end" items={shells.map(s => ({ id: s, label: s.split('/').at(-1)! }))} selectedId={shell} onClose={() => setMenu(undefined)} onSelect={s => { setShell(s); setMenu(undefined); void create(pane.id, s); }} anchor={<button type="button" className={css.shellTrigger} disabled={busy || !shells.length} aria-label={tx('artifacts:workbench.shell')} aria-haspopup="menu" aria-expanded={menu === pane.id} onClick={() => setMenu(menu === pane.id ? undefined : pane.id)}><IconChevronDownOutline14/></button>}/></span><span className={css.entryDescription}>{tx('artifacts:workbench.terminal-description')}</span></span><ShortcutKeys keys={[isMac ? '⌃' : 'Ctrl', '`']}/></div>
      </div> :tab.kind==='files'?<WorkspaceFiles call={call} view={viewFor(tab.id)} onOpen={path=>open(pane.id,{kind:'file',path})}/>:tab.kind==='file'?visibleFiles.some(file => file.id === tab.id) ? <WorkspaceFile key={`${tab.id}:${viewFor(tab.id).focusRevision ?? 0}`} onOpen={path=>open(pane.id,{kind:'file',path})} path={tab.title} call={call} view={viewFor(tab.id)}/> : <p role="status">{tx('artifacts:document.capacity')}</p> :<Suspense fallback={<p>{tx('artifacts:workbench.loading')}</p>}><TerminalView id={tab.contentId} call={call}/></Suspense>}
    </div>;
  };
  const usesWorkspaceTools = Object.values(state.tabs).some(tab => !['subagent','changes'].includes(tab.kind) && findTabPane(state, tab.id).activeTabId === tab.id);
  return <div ref={root} className={css.root} hidden={!visible && !floatingVisible} data-workbench>
    {visible&&error&&<p role="alert" className={css.notice}>{error}</p>}
    {visible&&!available&&usesWorkspaceTools&&<p className={css.notice}>{tx('artifacts:workbench.unavailable')}</p>}
    <DockLayout state={state} active={visible || floatingVisible} canSplit={dockPaneIds(state).length<2} dropZones="horizontal" minPaneFraction={0.2} intents={intents}
      canAddTab={id=>!getPane(state,id).tabs.some(t=>state.tabs[t].kind==='start')}
      canCloseTab={id=>canCloseTab(state,id)} onRoom={fits=>{room.current=fits;}}
      labels={{emptyPane:tx('artifacts:workbench.empty-pane'),splitPane:tx('artifacts:workbench.split'),splitPaneDisabled:tx('artifacts:workbench.split-limit'),splitPaneNarrow:tx('artifacts:workbench.split-room'),splitPaneShortcut:isMac?'Meta+\\':'Control+\\',splitPaneKeys:isMac?['⌘','\\']:['Ctrl','+','\\'],closeTab:tx('artifacts:workbench.close-tab'),closeTabShortcut:isMac?'Alt+Meta+W':'Alt+Control+W',closeTabKeys:isMac?['⌥','⌘','W']:['Ctrl','+','Alt','+','W'],addTab:tx('artifacts:workbench.add-tab'),dockFloat:tx('artifacts:workbench.dock'),closeFloat:tx('artifacts:workbench.close-tab'),dropZone:{center:tx('artifacts:workbench.drop-center'),left:tx('artifacts:workbench.drop-left'),right:tx('artifacts:workbench.drop-right'),top:tx('artifacts:workbench.drop-top'),bottom:tx('artifacts:workbench.drop-bottom')}}}
      keepMounted={tab=>tab.kind==='subagent'} renderTab={body} renderTabTitle={tab=><span className={css.tabTitle}>{tab.kind==='subagent'?<IconAgentPresetOutline16/>:tab.kind==='start'?<CompassGlyph size={16}/>:tab.kind==='terminal'?<TerminalIcon/>:tab.kind==='files'?<FileTypeIcon kind="folder" size={16}/>:<FileTypeIcon path={tab.title} size={16}/>}<span>{title(tab)}</span></span>}
      chrome={<>
        <Tooltip label={tx(fullscreen?'artifacts:workbench.restore':'artifacts:workbench.fullscreen')} shortcutKeys={isMac?['⌥','⌘','↩']:['Ctrl','+','Alt','+','Enter']} side="bottom" delayMs={500}>
          <button type="button" className={panelCss.iconButton} aria-label={tx(fullscreen?'artifacts:workbench.restore':'artifacts:workbench.fullscreen')} aria-keyshortcuts={isMac?'Alt+Meta+Enter':'Alt+Control+Enter'} data-sidebar-right-mode={fullscreen?'push':'fullscreen'} onClick={toggleFullscreen}>{fullscreen?<ExitFullscreenGlyph/>:<FullscreenGlyph/>}</button>
        </Tooltip>
        <Tooltip label={tx('artifacts:workbench.collapse')} side="bottom" delayMs={500}>
          <button type="button" className={panelCss.iconButton} aria-label={tx('artifacts:workbench.collapse-label')} data-sidebar-right-toggle onClick={()=>closePanel()}><IconPanelLeftOutline16 className={panelCss.collapseGlyph}/></button>
        </Tooltip>
      </>}/>
  </div>;
}
function ShortcutKeys({ keys }: { keys: string[] }) { return <span className={css.shortcut}>{keys.map(key => <kbd key={key}>{key}</kbd>)}</span>; }

/** The lone docked guide stays available even while independent panels float. */
function soleDockedTab(state: LayoutState, id: TabId): boolean {
  const pane=findTabPane(state,id);
  return pane.host==='dock' && pane.tabs.length===1 && dockPaneIds(state).length===1;
}
function canCloseTab(state: LayoutState, id: TabId): boolean {
  const tab=state.tabs[id];
  return !!tab && !(tab.kind==='start' && soleDockedTab(state,id));
}

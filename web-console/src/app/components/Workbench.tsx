/* Copyright (c) 2026 DeepSeek. MIT. Harness guide and DockLayout port; see PROVENANCE.md. */
import { lazy, Suspense, useCallback, useEffect, useImperativeHandle, type Ref, useRef, useState, useSyncExternalStore } from 'react';
import type { ProductHostWorkspaces, WorkspaceAuthorityScope } from '../../workspaces/host';
import type { DesktopTarget } from '../../workspaces/desktop';
import type { WorkbenchRequest, WorkbenchTerminal } from '../../workspaces/workbench';
import { useTranslation } from '../../locale/react';
import { WorkspaceFiles, WorkspaceFile, fileTabView, type FileTabView } from './WorkbenchFiles';
import { FileTypeIcon } from '../../presentation/primitives/FileTypeIcon';
import { IconPanelLeftOutline16, IconChevronDownOutline14 } from '../../presentation/primitives/icons';
import { Menu } from '../../presentation/primitives/Menu';
import { CompassGlyph, GuideArtworkFiles, PluginArtworkTerminal, TerminalIcon, FullscreenGlyph, ExitFullscreenGlyph } from './WorkbenchIcons';
import { readSidebarLayout, writeSidebarLayout } from './workbench-persistence';
import { DockController } from '../../presentation/dockkit/engine/controller';
import { DockLayout } from '../../presentation/dockkit/components/DockSurface';
import { dockPaneIds, findTabPane, getPane } from '../../presentation/dockkit/engine/tree';
import type { PaneId, TabId, TabRecord } from '../../presentation/dockkit/contract/types';
import type { DockIntents } from '../../presentation/dockkit/contract/adapter';
import css from '../../presentation/right-panel/Workbench.module.css';
const TerminalView = lazy(() => import('./WorkbenchTerminal'));
const guideTab = (id: TabId): TabRecord => ({id,kind:'start',contentId:`start:${id}`,title:''});
export interface WorkbenchHandle { openFile(path: string, line?: number): Promise<boolean> }
export function Workbench({ fileRef, host, scope, target, visible, floatingVisible, fullscreen, toggleFullscreen, closePanel }: {
  fileRef?: Ref<WorkbenchHandle>; host: ProductHostWorkspaces; scope?: WorkspaceAuthorityScope; target?: DesktopTarget; visible: boolean; floatingVisible: boolean; fullscreen: boolean; toggleFullscreen: () => void; closePanel: () => void;
}) {
  const tx = useTranslation();
  const storageKey = JSON.stringify([scope?.endpoint,scope?.authorityId,target?.session_id,target?.active_node]);
  const [dock] = useState(() => new DockController({makeInitialTab:guideTab,makePaneTab:guideTab,settle:true,restored:readSidebarLayout(storageKey)}));
  const {state} = useSyncExternalStore(dock.subscribe,dock.getSnapshot,dock.getSnapshot);
  useEffect(() => {
    const minted = Math.max(0,...Object.keys(state.nodes).concat(Object.keys(state.tabs)).map(id=>Number(id.replace(/^[a-z]+/,''))));
    writeSidebarLayout(storageKey,{layout:state,minted,history:{entries:[],cursor:0}});
  },[state,storageKey]);
  const views = useRef(new Map<TabId,FileTabView>());
  const navigation = useRef(0);
  const viewFor = (id:TabId) => { let view=views.current.get(id); if(!view) {view=fileTabView();views.current.set(id,view);}return view; };
  const [terminals,setTerminals] = useState<WorkbenchTerminal[]>([]), [shells,setShells] = useState<string[]>([]);
  const [shell,setShell] = useState(''),[error,setError] = useState(''),[busy,setBusy] = useState(false),[menu,setMenu] = useState<PaneId>();
  const pendingCreate = useRef<{id:string;shell:string}|null>(null), lifetime=useRef<AbortController|null>(null);
  useEffect(()=>{lifetime.current=new AbortController();return()=>lifetime.current?.abort();},[]);
  useEffect(()=>dock.setExpanded(visible),[dock,visible]);
  const call=useCallback(async(request:WorkbenchRequest,signal?:AbortSignal)=>{
    if(!host.workbench || !scope || !target) throw new Error(tx('artifacts:workbench.unavailable'));
    return host.workbench(scope,{target,request},signal ?? lifetime.current?.signal);
  },[host,scope?.authorityId,scope?.endpoint,target?.session_id,target?.active_node,tx]);
  const available=!!host.workbench && !!scope && !!target;
  const open=(paneId:PaneId,tab:{kind:string;path?:string;id?:string})=>{
    navigation.current++;
    const before=dock.getSnapshot().state;
    const pane=getPane(before,paneId), active=pane.activeTabId && before.tabs[pane.activeTabId];
    const contentId=tab.kind==='file' ? `file:${tab.path}` : tab.kind==='files' ? `files:${paneId}` : tab.id ?? crypto.randomUUID();
    dock.openContent({paneId,kind:tab.kind,contentId,title:tab.path ?? ''});
    if(active && active.kind==='start') dock.closeTab(active.id);
  };
  useImperativeHandle(fileRef, () => ({ openFile: async (path, line) => {
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
  const close=async(id:TabId)=>{
    const tab=dock.getSnapshot().state.tabs[id];if(!tab)return;
    if(tab.kind==='start' && Object.keys(dock.getSnapshot().state.tabs).length===1)return;
    try {
      if(tab.kind==='terminal'){const value=await call({kind:'close',id:tab.contentId});if(lifetime.current?.signal.aborted)return;setTerminals(value.terminals ?? []);}
      const lastContent = tab.kind !== 'start' && Object.keys(dock.getSnapshot().state.tabs).length === 1;
      dock.closeTab(id);views.current.delete(id);
      if(lastContent)closePanel();
    }catch(cause){if(!lifetime.current?.signal.aborted)setError(String(cause));}
  };
  const intents:DockIntents={
    focusTab:id=>{navigation.current++;dock.focusTab(id);},focusPane:id=>dock.focusPane(id),
    splitPane:id=>{if(dockPaneIds(dock.getSnapshot().state).length<2)dock.splitPane(id);},
    addTab:id=>{navigation.current++;const pane=getPane(dock.getSnapshot().state,id);const guide=pane.tabs.find(t=>dock.getSnapshot().state.tabs[t].kind==='start');if(guide)dock.focusTab(guide);else dock.addTab(id);},
    closeTab:id=>{void close(id);}, duplicateTab:id=>dock.duplicateTab(id),
    floatTab:(id,rect)=>{dock.floatTab(id,rect);if(!dockPaneIds(dock.getSnapshot().state).some(p=>getPane(dock.getSnapshot().state,p).tabs.length))dock.addTab(dock.activeDockPaneId());},
    unfloatPane:id=>dock.unfloatPane(id),placeTab:(id,p,index)=>dock.placeTab(id,p,index),
    dropTab:(id,p,zone)=>{if(zone==='center'||dockPaneIds(dock.getSnapshot().state).length<2)dock.dropTab(id,p,zone);},
    moveFloat:(id,x,y)=>dock.moveFloat(id,x,y),resizeFloat:(id,rect)=>dock.resizeFloat(id,rect),resizeSplit:(id,sizes)=>dock.resizeSplit(id,sizes),
  };
  useEffect(()=>{
    if(!visible)return;
    const key=(e:KeyboardEvent)=>{
      if(e.defaultPrevented||e.isComposing||!available)return;
      if(e.ctrlKey&&!e.altKey&&!e.metaKey&&e.code==='Backquote'){e.preventDefault();void create(dock.activeDockPaneId());}
      if(e.altKey&&(e.metaKey||e.ctrlKey)&&e.code==='KeyP'){e.preventDefault();open(dock.activeDockPaneId(),{kind:'files'});}
    };
    window.addEventListener('keydown',key,true);return()=>window.removeEventListener('keydown',key,true);
  });
  const isMac=/Mac/.test(navigator.platform);
  const title=(tab:TabRecord)=>tab.kind==='file'?tab.title.split('/').at(-1):tab.kind==='terminal'?terminals.find(t=>t.id===tab.contentId)?.shell.split('/').at(-1) ?? tab.title:tx(tab.kind==='start'?'artifacts:workbench.start':'artifacts:workbench.file-tab');
  const body=(tab:TabRecord)=>{
    const pane=findTabPane(state,tab.id);
    return <div className={css.body} onKeyDown={e=>{if(tab.kind!=='terminal'&&e.key==='Escape'&&!e.defaultPrevented){e.preventDefault();if(fullscreen)toggleFullscreen();else closePanel();}}}>
    {tab.kind==='start'? <div className={css.guide}>
        <span className={css.hero}><CompassGlyph size={56}/></span>
        <button type="button" className={css.entry} disabled={!available} aria-keyshortcuts={isMac ? 'Alt+Meta+P' : 'Alt+Control+P'} onClick={() => open(pane.id, { kind: 'files' })}><span className={css.entryIcon}><GuideArtworkFiles size={26}/></span><span className={css.entryText}><span className={css.entryTitle}>{tx('artifacts:workbench.files')}</span><span className={css.entryDescription}>{tx('artifacts:workbench.files-description')}</span></span><ShortcutKeys keys={isMac ? ['⌥', '⌘', 'P'] : ['Alt', 'Ctrl', 'P']}/></button>
        <div className={`${css.entry} ${css.terminalEntry}`}><button type="button" className={css.launch} aria-label={tx('artifacts:workbench.new-terminal')} aria-keyshortcuts="Control+`" disabled={!available || busy || !shell} onClick={() => void create(pane.id)}/><span className={css.entryIcon}><PluginArtworkTerminal size={26}/></span><span className={css.entryText}><span className={css.titleRow}><span className={css.entryTitle}>{tx('artifacts:workbench.new-terminal')}</span><Menu open={menu === pane.id} autoFocus align="end" items={shells.map(s => ({ id: s, label: s.split('/').at(-1)! }))} selectedId={shell} onClose={() => setMenu(undefined)} onSelect={s => { setShell(s); setMenu(undefined); void create(pane.id, s); }} anchor={<button type="button" className={css.shellTrigger} disabled={busy || !shells.length} aria-label={tx('artifacts:workbench.shell')} aria-haspopup="menu" aria-expanded={menu === pane.id} onClick={() => setMenu(menu === pane.id ? undefined : pane.id)}><IconChevronDownOutline14/></button>}/></span><span className={css.entryDescription}>{tx('artifacts:workbench.terminal-description')}</span></span><ShortcutKeys keys={[isMac ? '⌃' : 'Ctrl', '`']}/></div>
      </div> :tab.kind==='files'?<WorkspaceFiles call={call} view={viewFor(tab.id)} onOpen={path=>open(pane.id,{kind:'file',path})}/>:tab.kind==='file'?<WorkspaceFile key={`${tab.id}:${viewFor(tab.id).focusRevision ?? 0}`} onOpen={path=>open(pane.id,{kind:'file',path})} path={tab.title} call={call} view={viewFor(tab.id)}/>:<Suspense fallback={<p>{tx('artifacts:workbench.loading')}</p>}><TerminalView id={tab.contentId} call={call}/></Suspense>}
    </div>;
  };
  return <div className={css.root} hidden={!visible && (!floatingVisible || !state.floats.length)} data-workbench>
    {error&&<p role="alert" className={css.notice}>{error}</p>}
    {!available&&<p className={css.notice}>{tx('artifacts:workbench.unavailable')}</p>}
    <DockLayout state={state} active={visible || floatingVisible && state.floats.length > 0} canSplit={dockPaneIds(state).length<2} dropZones="horizontal" minPaneFraction={0.2} intents={intents}
      canAddTab={id=>!getPane(state,id).tabs.some(t=>state.tabs[t].kind==='start')}
      canCloseTab={id=>state.tabs[id].kind!=='start'||Object.keys(state.tabs).length>1}
      labels={{emptyPane:tx('artifacts:workbench.start'),splitPane:tx('artifacts:workbench.split'),splitPaneDisabled:tx('artifacts:workbench.split'),splitPaneNarrow:tx('artifacts:workbench.split'),closeTab:tx('artifacts:workbench.close-tab'),addTab:tx('artifacts:workbench.add-tab'),dockFloat:tx('artifacts:workbench.dock'),closeFloat:tx('artifacts:workbench.close-tab'),dropZone:{center:tx('artifacts:workbench.dock'),left:tx('artifacts:workbench.split'),right:tx('artifacts:workbench.split'),top:tx('artifacts:workbench.split'),bottom:tx('artifacts:workbench.split')}}}
      renderTab={body} renderTabTitle={tab=><span className={css.tabTitle}>{tab.kind==='start'?<CompassGlyph size={16}/>:tab.kind==='terminal'?<TerminalIcon/>:tab.kind==='files'?<FileTypeIcon kind="folder" size={16}/>:<FileTypeIcon path={tab.title} size={16}/>}<span>{title(tab)}</span></span>}
      renderTabMenuItems={(tab,dismiss)=><button role="menuitem" onClick={()=>{dismiss();intents.floatTab(tab.id);}}>{tx('artifacts:workbench.float')}</button>}
      chrome={<div className={css.chrome}><button type="button" className={css.iconButton} aria-label={tx(fullscreen?'artifacts:workbench.restore':'artifacts:workbench.fullscreen')} onClick={toggleFullscreen}>{fullscreen?<ExitFullscreenGlyph/>:<FullscreenGlyph/>}</button><button type="button" className={css.iconButton} aria-label={tx('artifacts:workbench.toggle')} onClick={closePanel}><IconPanelLeftOutline16 className={css.mirrored}/></button></div>}/>
  </div>;
}
function ShortcutKeys({ keys }: { keys: string[] }) { return <span className={css.shortcut}>{keys.map(key => <kbd key={key}>{key}</kbd>)}</span>; }

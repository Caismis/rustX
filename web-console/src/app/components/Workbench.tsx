/* Copyright (c) 2026 DeepSeek. MIT. Guide layout adapted from ui-sidebar-right GuideBody; see PROVENANCE.md. */
import { lazy, Suspense, useCallback, useEffect, useRef, useState } from 'react';
import type { ProductHostWorkspaces, WorkspaceAuthorityScope } from '../../workspaces/host';
import type { DesktopTarget } from '../../workspaces/desktop';
import type { WorkbenchRequest, WorkbenchTerminal } from '../../workspaces/workbench';
import { useTranslation } from '../../locale/react';
import { WorkspaceFiles, WorkspaceFile, fileTabView, type FileTabView } from './WorkbenchFiles';
import { FileTypeIcon } from '../../presentation/primitives/FileTypeIcon';
import { IconPanelLeftOutline16, IconPlusOutline16, IconCloseOutline16, IconChevronDownOutline14 } from '../../presentation/primitives/icons';
import { Menu } from '../../presentation/primitives/Menu';
import { CompassGlyph, GuideArtworkFiles, PluginArtworkTerminal, TerminalIcon, FullscreenGlyph, ExitFullscreenGlyph } from './WorkbenchIcons';
import css from '../../presentation/right-panel/Workbench.module.css';
const TerminalView = lazy(() => import('./WorkbenchTerminal'));
type Tab = { id: string; kind: 'start' | 'files' | 'terminal' | 'file'; path?: string; view?: FileTabView };
const sameTab = (a: Tab, b: Tab) => a.kind === b.kind && (a.kind !== 'file' || a.path === b.path);
type Pane = { id: string; tabs: Tab[]; selected: string };
const guidePane = (): Pane => { const id = crypto.randomUUID(); return { id, tabs: [{ id, kind: 'start' }], selected: id }; };
export function Workbench({ host, scope, target, visible, fullscreen, toggleFullscreen, closePanel }: {
  host: ProductHostWorkspaces; scope?: WorkspaceAuthorityScope; target?: DesktopTarget; visible: boolean; fullscreen: boolean; toggleFullscreen: () => void; closePanel: () => void;
}) {
  const tx = useTranslation();
  const [panes, setPanes] = useState<Pane[]>(() => [guidePane()]);
  const [focused, setFocused] = useState<string>();
  const [terminals, setTerminals] = useState<WorkbenchTerminal[]>([]), [shells, setShells] = useState<string[]>([]);
  const [shell, setShell] = useState(''), [error, setError] = useState(''), [busy, setBusy] = useState(false);
  const [menu, setMenu] = useState<string>(), [width, setWidth] = useState(0), [ratio, setRatio] = useState(50);
  const root = useRef<HTMLDivElement>(null);
  const pendingCreate = useRef<{ id: string; shell: string } | null>(null);
  const lifetime = useRef<AbortController | null>(null);
  useEffect(() => { lifetime.current = new AbortController(); return () => lifetime.current?.abort(); }, []);
  useEffect(() => { if (!visible || !root.current) return; const observer = new ResizeObserver(([entry]) => setWidth(entry.contentRect.width)); observer.observe(root.current); return () => observer.disconnect(); }, [visible]);
  const call = useCallback(async (request: WorkbenchRequest, signal?: AbortSignal) => {
    if (!host.workbench || !scope || !target) throw new Error(tx('artifacts:workbench.unavailable'));
    return host.workbench(scope, { target, request }, signal ?? lifetime.current?.signal);
  }, [host, scope?.authorityId, scope?.endpoint, target?.session_id, target?.active_node, tx]);
  useEffect(() => {
    if (!visible || !scope || !target || !host.workbench) return;
    const abort = new AbortController();
    void call({ kind: 'terminals' }, abort.signal).then(value => {
      if (abort.signal.aborted) return; setTerminals(value.terminals ?? []); setShells(value.shells ?? []); setShell(previous => previous || value.shells?.[0] || '');
      setPanes(current => { const known = new Set(current.flatMap(p => p.tabs.map(t => t.id))); const restored = (value.terminals ?? []).filter(t => !known.has(t.id)).map(t => ({ id: t.id, kind: 'terminal' as const })); return restored.length ? current.map((p, i) => i ? p : { ...p, tabs: [...p.tabs, ...restored] }) : current; });
    }, cause => { if (!abort.signal.aborted) setError(String(cause)); });
    return () => abort.abort();
  }, [call, visible]);
  const available = !!host.workbench && !!scope && !!target;
  const open = (paneId: string, tab: Tab) => setPanes(current => current.map(p => {
    if (p.id !== paneId) return p;
    const existing = tab.kind !== 'terminal' && p.tabs.find(t => sameTab(t, tab));
    if (existing) return { ...p, selected: existing.id };
    const active = p.tabs.find(t => t.id === p.selected);
    if (tab.kind === 'files' || tab.kind === 'file') tab = { ...tab, view: fileTabView() };
    return { ...p, selected: tab.id, tabs: active?.kind === 'start' ? p.tabs.map(t => t.id === active.id ? tab : t) : [...p.tabs, tab] };
  }));
  const create = async (paneId: string, selectedShell = shell) => {
    if (busy || !selectedShell) return;
    setBusy(true); setError(''); const pending = pendingCreate.current ??= { id: crypto.randomUUID(), shell: selectedShell };
    try { const value = await call({ kind: 'create', ...pending }); if (!lifetime.current?.signal.aborted) { pendingCreate.current = null; setTerminals(value.terminals ?? []); open(paneId, { id: pending.id, kind: 'terminal' }); } }
    catch (cause) { if (!lifetime.current?.signal.aborted) setError(String(cause)); }
    finally { if (!lifetime.current?.signal.aborted) setBusy(false); }
  };
  const close = async (paneId: string, tab: Tab) => {
    setError('');
    try {
      if (tab.kind === 'terminal') { const value = await call({ kind: 'close', id: tab.id }); if (lifetime.current?.signal.aborted) return; setTerminals(value.terminals ?? []); }
      setPanes(current => current.map(p => { if (p.id !== paneId) return p; const tabs = p.tabs.filter(t => t.id !== tab.id); if (!tabs.length) { const guide = guidePane(); return { ...guide, id: p.id }; } return { ...p, tabs, selected: p.selected === tab.id ? tabs[Math.max(0, p.tabs.findIndex(t => t.id === tab.id) - 1)].id : p.selected }; }));
    } catch (cause) { if (!lifetime.current?.signal.aborted) setError(String(cause)); }
  };
  const activePane = panes.find(p => p.id === focused) ?? panes[0];
  useEffect(() => {
    if (!visible) return;
    const key = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.isComposing || !available) return;
      if (e.ctrlKey && !e.altKey && !e.metaKey && e.code === 'Backquote') { e.preventDefault(); void create(activePane.id); }
      if (e.altKey && (e.metaKey || e.ctrlKey) && e.code === 'KeyP') { e.preventDefault(); open(activePane.id, { id: crypto.randomUUID(), kind: 'files' }); }
    };
    window.addEventListener('keydown', key, true); return () => window.removeEventListener('keydown', key, true);
  });
  const split = () => { setRatio(50); if (panes.length === 2) setPanes([{ ...activePane, tabs: [...activePane.tabs, ...panes.find(p => p.id !== activePane.id)!.tabs.filter(t => t.kind === 'terminal' || !activePane.tabs.some(a => sameTab(a, t)))] }]); else { const next = guidePane(); setPanes([...panes, next]); setFocused(next.id); } };
  const isMac = /Mac/.test(navigator.platform);
  return <div ref={root} className={css.root} hidden={!visible} data-workbench>
    {panes.map((pane, paneIndex) => { const tab = pane.tabs.find(t => t.id === pane.selected)!; const paneVisible = visible && (panes.length === 1 || width >= 640 || pane.id === activePane.id); const quiet = pane.tabs.length === 1 && tab.kind === 'start'; return <section key={pane.id} className={css.pane} data-workbench-pane hidden={panes.length > 1 && width < 640 && pane.id !== activePane.id} style={{ flex: panes.length === 2 && width >= 640 ? `0 0 ${paneIndex ? 100 - ratio : ratio}%` : '1' }} onFocusCapture={() => setFocused(pane.id)} onPointerDown={() => setFocused(pane.id)}>
      <header className={css.strip}>
        <div className={css.tabs} role="tablist" aria-label={tx('artifacts:workbench.tabs')} onKeyDown={e => { if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(e.key)) return; const buttons = [...e.currentTarget.querySelectorAll<HTMLButtonElement>('[role=tab]')]; let i = buttons.indexOf(document.activeElement as HTMLButtonElement); if (i < 0) return; e.preventDefault(); i = e.key === 'Home' ? 0 : e.key === 'End' ? buttons.length - 1 : (i + (e.key === 'ArrowRight' ? 1 : -1) + buttons.length) % buttons.length; buttons[i].click(); buttons[i].focus(); }}>
          {pane.tabs.map(t => <div className={css.tab} key={t.id} data-active={pane.selected === t.id} data-quiet={quiet}>
            <button type="button" role="tab" aria-selected={pane.selected === t.id} tabIndex={pane.selected === t.id ? 0 : -1} onClick={() => setPanes(current => current.map(p => p.id === pane.id ? { ...p, selected: t.id } : p))}>{t.kind === 'start' ? <CompassGlyph className={css.titleIcon}/> : t.kind === 'files' ? <FileTypeIcon kind="folder" size={16}/> : t.kind === 'file' ? <FileTypeIcon path={t.path!} size={16}/> : <TerminalIcon/>}<span title={t.path}>{t.kind === 'file' ? t.path!.split('/').at(-1) : t.kind === 'terminal' ? terminals.find(term => term.id === t.id)?.shell.split('/').at(-1) ?? tx('artifacts:workbench.terminal') : tx(t.kind === 'start' ? 'artifacts:workbench.start' : 'artifacts:workbench.file-tab')}</span></button>
            {!quiet && <button type="button" className={css.tabClose} aria-label={t.kind === 'terminal' ? tx('artifacts:workbench.close-terminal', { p0: terminals.findIndex(term => term.id === t.id) + 1 }) : tx('artifacts:workbench.close-tab')} onClick={() => void close(pane.id, t)}><IconCloseOutline16 size={14}/></button>}
          </div>)}
        </div>
        {!pane.tabs.some(t => t.kind === 'start') && <button type="button" className={css.iconButton} aria-label={tx('artifacts:workbench.add-tab')} onClick={() => open(pane.id, { id: crypto.randomUUID(), kind: 'start' })}><IconPlusOutline16/></button>}
        <span className={css.fill}/>
        <button type="button" className={css.iconButton} disabled={panes.length === 1 && width < 640} aria-label={tx(panes.length === 2 ? 'artifacts:workbench.merge' : 'artifacts:workbench.split')} onClick={split}><IconPanelLeftOutline16/></button>
        {(paneIndex === panes.length - 1 || width < 640) && <div className={css.chrome}><button type="button" className={css.iconButton} aria-label={tx(fullscreen ? 'artifacts:workbench.restore' : 'artifacts:workbench.fullscreen')} onClick={toggleFullscreen}>{fullscreen ? <ExitFullscreenGlyph/> : <FullscreenGlyph/>}</button><button type="button" className={css.iconButton} aria-label={tx('artifacts:workbench.toggle')} onClick={closePanel}><IconPanelLeftOutline16 className={css.mirrored}/></button></div>}
      </header>
      {error && pane.id === activePane.id && <p role="alert" className={css.notice}>{error}</p>}
      {!available && <p className={css.notice}>{tx('artifacts:workbench.unavailable')}</p>}
      <div className={css.body} onKeyDown={e => { if (tab.kind !== 'terminal' && e.key === 'Escape' && !e.defaultPrevented) { e.preventDefault(); if (fullscreen) toggleFullscreen(); else closePanel(); } }}>
      {tab.kind === 'start' ? <div className={css.guide}>
        <span className={css.hero}><CompassGlyph size={56}/></span>
        <button type="button" className={css.entry} disabled={!available} aria-keyshortcuts={isMac ? 'Alt+Meta+P' : 'Alt+Control+P'} onClick={() => open(pane.id, { id: crypto.randomUUID(), kind: 'files' })}><span className={css.entryIcon}><GuideArtworkFiles size={26}/></span><span className={css.entryText}><span className={css.entryTitle}>{tx('artifacts:workbench.files')}</span><span className={css.entryDescription}>{tx('artifacts:workbench.files-description')}</span></span><ShortcutKeys keys={isMac ? ['⌥', '⌘', 'P'] : ['Alt', 'Ctrl', 'P']}/></button>
        <div className={`${css.entry} ${css.terminalEntry}`}><button type="button" className={css.launch} aria-label={tx('artifacts:workbench.new-terminal')} aria-keyshortcuts="Control+`" disabled={!available || busy || !shell} onClick={() => void create(pane.id)}/><span className={css.entryIcon}><PluginArtworkTerminal size={26}/></span><span className={css.entryText}><span className={css.titleRow}><span className={css.entryTitle}>{tx('artifacts:workbench.new-terminal')}</span><Menu open={menu === pane.id} autoFocus align="end" items={shells.map(s => ({ id: s, label: s.split('/').at(-1)! }))} selectedId={shell} onClose={() => setMenu(undefined)} onSelect={s => { setShell(s); setMenu(undefined); void create(pane.id, s); }} anchor={<button type="button" className={css.shellTrigger} disabled={busy || !shells.length} aria-label={tx('artifacts:workbench.shell')} aria-haspopup="menu" aria-expanded={menu === pane.id} onClick={() => setMenu(menu === pane.id ? undefined : pane.id)}><IconChevronDownOutline14/></button>}/></span><span className={css.entryDescription}>{tx('artifacts:workbench.terminal-description')}</span></span><ShortcutKeys keys={[isMac ? '⌃' : 'Ctrl', '`']}/></div>
      </div> : tab.kind === 'files' ? paneVisible && <WorkspaceFiles key={tab.id} call={call} view={tab.view!} onOpen={path => open(pane.id, { id: crypto.randomUUID(), kind: 'file', path })}/> : tab.kind === 'file' ? paneVisible && <WorkspaceFile key={tab.id} path={tab.path!} call={call} view={tab.view!}/> : paneVisible && <Suspense fallback={<p>{tx('artifacts:workbench.loading')}</p>}><TerminalView key={tab.id} id={tab.id} call={call}/></Suspense>}
      </div>
      {paneIndex === 0 && panes.length === 2 && width >= 640 && <div className={css.divider} role="separator" aria-label={tx('artifacts:workbench.resize')} aria-orientation="vertical" aria-valuenow={Math.round(ratio)} tabIndex={0} onKeyDown={e => { if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') { e.preventDefault(); setRatio(v => Math.min(70, Math.max(30, v + (e.key === 'ArrowRight' ? 2 : -2)))); } }} onPointerDown={e => { e.preventDefault(); e.currentTarget.setPointerCapture(e.pointerId); }} onPointerMove={e => { if (e.currentTarget.hasPointerCapture(e.pointerId) && root.current) { const b = root.current.getBoundingClientRect(); setRatio(Math.min(70, Math.max(30, (e.clientX - b.left) / b.width * 100))); } }}/ >}
    </section>; })}
  </div>;
}
function ShortcutKeys({ keys }: { keys: string[] }) { return <span className={css.shortcut}>{keys.map(key => <kbd key={key}>{key}</kbd>)}</span>; }

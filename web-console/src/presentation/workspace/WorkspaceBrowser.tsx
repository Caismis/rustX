import { useTranslation } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Adapted WorkspaceBrowser composition; see PROVENANCE.md. */
import { useRef, useState, type ReactNode } from 'react';
import clsx from 'clsx';
import { IconSearchOutline16, IconCloseFill14, IconProjectAddOutline16, IconEllipsisOutline16 } from '../primitives/icons';
import { Menu } from '../primitives/Menu';
import { Tooltip } from '../primitives/Tooltip';
import { ProjectRowItem, SessionNodeItem, SearchResultItem } from './Rows';
import type { GroupNode, SessionNode } from './types';
import css from './WorkspaceBrowser.module.css';

/** Native adapters supply facts/gestures. This component owns only visual state. */
export function WorkspaceBrowser({ wide, expand, groups, sessions, selected, query, search, open, rename, fork, remove, closeView, closeAllViews,
  selectWorkspace, create, renameWorkspace, removeWorkspace, workspaceSettings, addWorkspace, refresh, previous, next, notices, renderSession = (_node, render) => render(_node) }: {
  renderSession?: (node: SessionNode, render: (node: SessionNode) => ReactNode) => ReactNode;
  workspaceSettings?: (id: string, label: string) => void;
  wide: boolean; expand: () => void; groups: readonly GroupNode[]; sessions: readonly SessionNode[]; selected?: string;
  query: string; search: (text: string) => void; open: (id: string) => void; rename: (id: string) => void;
  closeView?: (id: string) => void;
  closeAllViews?: () => void;
  fork: (id: string) => void; remove: (id: string) => void; selectWorkspace: (id: string) => void; create: (id: string) => void;
  renameWorkspace: (id: string, title: string) => void; removeWorkspace: (id: string, title: string) => void;
  addWorkspace?: () => void; refresh: () => void; previous?: () => void; next?: () => void; notices?: ReactNode;
}) {
  const tx = useTranslation();
  const [searchExpanded, setSearchExpanded] = useState(false), [flat, setFlat] = useState(false), [menu, setMenu] = useState(false);
  const [collapsed, setCollapsed] = useState<string[]>([]);
  const searchInput = useRef<HTMLInputElement>(null);
  // The Session tree is where keyboard navigation continues when a row's
  // actions menu loses its anchor: the row scrolled out of the tree, which
  // stays rendered and visible around it. Programmatically focusable, never a
  // Tab stop of its own; Tab from it enters its rows.
  const tree = useRef<HTMLDivElement>(null);
  const row = (node: SessionNode) => renderSession(node, node => <SessionNodeItem key={node.id} node={node} currentId={selected} now={Date.now()} onOpen={open} onRename={rename} onFork={fork} onDelete={remove} onClose={node.viewOpen ? closeView : undefined} flat={flat || !!query} menuFocusOwner={tree} t={tx} />);
  return <section className={clsx(css.root, !wide && css.rail)} aria-label={tx('workspace:workspace-browser.workspaces-and-sessions')}>
    <div className={css.sectionHeader}>
      {wide && <span className={clsx(css.sectionLabel, css.wide, searchExpanded && css.sectionLabelHidden)}>{flat ? tx('workspace:workspace-browser.sessions') : tx('workspace:workspace-browser.workspaces')}</span>}
      {wide && <div className={clsx(css.searchSlot, searchExpanded && css.searchSlotExpanded)}><div className={clsx(css.search, searchExpanded && css.searchExpanded)} onClick={() => { setSearchExpanded(true); searchInput.current?.focus(); }}>
        <button type="button" className={css.searchButton} aria-label={tx('workspace:workspace-browser.search-sessions')} aria-expanded={searchExpanded} onClick={() => setSearchExpanded(true)}><IconSearchOutline16 size={searchExpanded ? 11 : 14} /></button>
        <input ref={searchInput} className={css.searchInput} aria-label={tx('workspace:workspace-browser.search-session-metadata')} placeholder={tx('workspace:workspace-browser.search-sessions-2')} maxLength={256} value={query} tabIndex={searchExpanded ? 0 : -1} onChange={e => search(e.target.value)} onKeyDown={e => { if (e.key === 'Escape') { search(''); setSearchExpanded(false); } }} />
        {searchExpanded && <button type="button" className={css.clearButton} aria-label={tx('workspace:workspace-browser.clear-search')} onClick={e => { e.stopPropagation(); search(''); setSearchExpanded(false); }}><IconCloseFill14 /></button>}
      </div></div>}
      <div className={clsx(css.headerActions, wide && searchExpanded && css.headerActionsHidden)}>
        {wide && <Menu open={menu} onClose={() => setMenu(false)} autoFocus anchor={<button type="button" className={css.iconButton} aria-label={tx('workspace:workspace-browser.view-options')} onClick={() => setMenu(!menu)}><IconEllipsisOutline16 /></button>}
          items={[{ id: 'group', label: flat ? tx('workspace:workspace-browser.grouped-view') : tx('workspace:workspace-browser.flat-view') }, { id: 'refresh', label: tx('workspace:workspace-browser.refresh-list') }, ...(closeAllViews ? [{ id: 'close-views', label: tx('workspace:workspace-browser.close-all-views') }] : [])]}
          onSelect={id => { setMenu(false); if (id === 'group') setFlat(!flat); else if (id === 'close-views') closeAllViews?.(); else refresh(); }} />}
        {addWorkspace && <Tooltip label={tx('workspace:workspace-navigation.add-workspace')}><button type="button" className={css.iconButton} aria-label={tx('workspace:workspace-navigation.add-workspace')} onClick={addWorkspace}><IconProjectAddOutline16 size={wide ? 16 : 18} /></button></Tooltip>}
      </div>
    </div>
    {!wide && <button type="button" className={css.searchButton} aria-label={tx('workspace:workspace-browser.search-sessions')} onClick={() => { expand(); setSearchExpanded(true); }}><IconSearchOutline16 size={18} /></button>}
    <div className={css.listArea}>{wide && <div className={clsx(css.treeBody, css.wide)}><div ref={tree} className={css.list} role="tree" aria-label={tx('workspace:workspace-browser.session-browser')} tabIndex={-1}>
      {notices}
      {query ? sessions.map(node => renderSession(node, node => <SearchResultItem key={node.id} result={{ ...node, workspace: groups.find(group => group.sessions.some(session => session.id === node.id))?.label ?? '' }} currentId={selected} onOpen={open} t={tx} />)) : flat || groups.length === 0 ? sessions.map(row) : groups.map(group => <div className={css.groupSection} key={group.key} data-workspace-group={group.key}>
        <ProjectRowItem group={{ ...group, expanded: !collapsed.includes(group.key) }} menuFocusOwner={tree} t={tx}
          onToggle={() => setCollapsed(value => value.includes(group.key) ? value.filter(id => id !== group.key) : [...value, group.key])}
          onSelect={() => group.workspaceId && selectWorkspace(group.workspaceId)} onCreate={() => group.workspaceId && create(group.workspaceId)}
          actions={group.workspaceId ? { settings: workspaceSettings ? () => workspaceSettings(group.workspaceId!, group.label) : undefined, rename: () => renameWorkspace(group.workspaceId!, group.label), delete: () => removeWorkspace(group.workspaceId!, group.label) } : undefined} />
        {!collapsed.includes(group.key) && group.sessions.map(row)}
      </div>)}
      {!flat && !query && groups.length > 0 && sessions.some(session => !groups.some(group => group.sessions.some(member => member.id === session.id))) && <details><summary>{tx('workspace:workspace-browser.sessions-outside-registered-workspaces')}</summary>{sessions.filter(session => !groups.some(group => group.sessions.some(member => member.id === session.id))).map(row)}</details>}
      {!sessions.length && <p className={css.empty}>{query ? tx('workspace:workspace-browser.no-matching-sessions') : tx('workspace:workspace-browser.no-sessions-yet')}</p>}
    </div><div className={css.fade} /></div>}</div>
    {wide && (previous || next) && <div><button className={css.sessionOverflowButton} disabled={!previous} onClick={previous}>{tx('workspace:workspace-browser.previous')}</button><button className={css.sessionOverflowButton} disabled={!next} onClick={next}>{tx('workspace:workspace-browser.next')}</button></div>}
  </section>;
}

/* Copyright (c) 2026 DeepSeek. MIT. Adapted WorkspaceBrowser composition; see PROVENANCE.md. */
import { useRef, useState, type ReactNode } from 'react';
import clsx from 'clsx';
import { IconSearchOutline16, IconCloseFill14, IconProjectAddOutline16, IconEllipsisOutline16 } from '../primitives/icons';
import { Menu } from '../primitives/Menu';
import { Tooltip } from '../primitives/Tooltip';
import { ProjectRowItem, SessionNodeItem, SearchResultItem } from './Rows';
import type { GroupNode, SessionNode } from './types';
import { t } from '../locale/translate';
import css from './WorkspaceBrowser.module.css';

/** Native adapters supply facts/gestures. This component owns only visual state. */
export function WorkspaceBrowser({ wide, expand, groups, sessions, selected, query, search, open, rename, fork, remove,
  selectWorkspace, create, renameWorkspace, removeWorkspace, addWorkspace, refresh, previous, next, notices }: {
  wide: boolean; expand: () => void; groups: readonly GroupNode[]; sessions: readonly SessionNode[]; selected?: string;
  query: string; search: (text: string) => void; open: (id: string) => void; rename: (id: string, title: string) => void;
  fork: (id: string) => void; remove: (id: string) => void; selectWorkspace: (id: string) => void; create: (id: string) => void;
  renameWorkspace: (id: string, title: string) => void; removeWorkspace: (id: string, title: string) => void;
  addWorkspace?: () => void; refresh: () => void; previous?: () => void; next?: () => void; notices?: ReactNode;
}) {
  const [searchExpanded, setSearchExpanded] = useState(false), [flat, setFlat] = useState(false), [menu, setMenu] = useState(false);
  const [collapsed, setCollapsed] = useState<string[]>([]);
  const searchInput = useRef<HTMLInputElement>(null);
  const row = (node: SessionNode) => <SessionNodeItem key={node.id} node={node} currentId={selected} now={Date.now()} onOpen={open} onRename={rename} onFork={fork} onDelete={remove} flat={flat || !!query} t={t} />;
  return <section className={clsx(css.root, !wide && css.rail)} aria-label="Workspaces and Sessions">
    <div className={css.sectionHeader}>
      {wide && <span className={clsx(css.sectionLabel, css.wide, searchExpanded && css.sectionLabelHidden)}>{flat ? 'Sessions' : 'Workspaces'}</span>}
      {wide && <div className={clsx(css.searchSlot, searchExpanded && css.searchSlotExpanded)}><div className={clsx(css.search, searchExpanded && css.searchExpanded)} onClick={() => { setSearchExpanded(true); searchInput.current?.focus(); }}>
        <button type="button" className={css.searchButton} aria-label="Search Sessions" aria-expanded={searchExpanded} onClick={() => setSearchExpanded(true)}><IconSearchOutline16 size={searchExpanded ? 11 : 14} /></button>
        <input ref={searchInput} className={css.searchInput} aria-label="Search Session metadata" placeholder="Search Sessions…" maxLength={256} value={query} tabIndex={searchExpanded ? 0 : -1} onChange={e => search(e.target.value)} onKeyDown={e => { if (e.key === 'Escape') { search(''); setSearchExpanded(false); } }} />
        {searchExpanded && <button type="button" className={css.clearButton} aria-label="Clear search" onClick={e => { e.stopPropagation(); search(''); setSearchExpanded(false); }}><IconCloseFill14 /></button>}
      </div></div>}
      <div className={clsx(css.headerActions, wide && searchExpanded && css.headerActionsHidden)}>
        {wide && <Menu open={menu} onClose={() => setMenu(false)} portal autoFocus anchor={<button type="button" className={css.iconButton} aria-label="View options" onClick={() => setMenu(!menu)}><IconEllipsisOutline16 /></button>}
          items={[{ id: 'group', label: flat ? 'Grouped view' : 'Flat view' }, { id: 'refresh', label: 'Refresh list' }]}
          onSelect={id => { setMenu(false); if (id === 'group') setFlat(!flat); else refresh(); }} />}
        {addWorkspace && <Tooltip label="Add Workspace"><button type="button" className={css.iconButton} aria-label="Add Workspace" onClick={addWorkspace}><IconProjectAddOutline16 size={wide ? 16 : 18} /></button></Tooltip>}
      </div>
    </div>
    {!wide && <button type="button" className={css.searchButton} aria-label="Search Sessions" onClick={() => { expand(); setSearchExpanded(true); }}><IconSearchOutline16 size={18} /></button>}
    <div className={css.listArea}>{wide && <div className={clsx(css.treeBody, css.wide)}><div className={css.list} role="tree" aria-label="Session browser">
      {notices}
      {query ? sessions.map(node => <SearchResultItem key={node.id} result={{ ...node, workspace: groups.find(group => group.sessions.some(session => session.id === node.id))?.label ?? '' }} currentId={selected} onOpen={open} t={t} />) : flat ? sessions.map(row) : groups.map(group => <div className={css.groupSection} key={group.key} data-workspace-group={group.key}>
        <ProjectRowItem group={{ ...group, expanded: !collapsed.includes(group.key) }} t={t}
          onToggle={() => setCollapsed(value => value.includes(group.key) ? value.filter(id => id !== group.key) : [...value, group.key])}
          onSelect={() => group.workspaceId && selectWorkspace(group.workspaceId)} onCreate={() => group.workspaceId && create(group.workspaceId)}
          actions={group.workspaceId ? { rename: () => renameWorkspace(group.workspaceId!, group.label), delete: () => removeWorkspace(group.workspaceId!, group.label) } : undefined} />
        {!collapsed.includes(group.key) && group.sessions.map(row)}
      </div>)}
      {!sessions.length && <p className={css.empty}>{query ? 'No matching Sessions' : 'No Sessions yet'}</p>}
    </div><div className={css.fade} /></div>}</div>
    {wide && (previous || next) && <div><button className={css.sessionOverflowButton} disabled={!previous} onClick={previous}>Previous</button><button className={css.sessionOverflowButton} disabled={!next} onClick={next}>Next</button></div>}
  </section>;
}

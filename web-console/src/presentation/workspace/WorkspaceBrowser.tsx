import { useTranslation } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Adapted WorkspaceBrowser composition; see PROVENANCE.md. */
import { useEffect, useRef, useState, type ReactNode } from 'react';
import clsx from 'clsx';
import { IconSearchOutline16, IconCloseFill14, IconProjectAddOutline16, IconEllipsisOutline16 } from '../primitives/icons';
import { Menu } from '../primitives/Menu';
import { Tooltip } from '../primitives/Tooltip';
import { ProjectRowItem, SessionNodeItem, SearchResultItem, DraftConversationRow } from './Rows';
import type { DraftConversation, GroupNode, SessionNode } from './types';
import { workspaceBrowserCss } from './workspaceBrowserCss';
import { Input as TextInput } from '@/presentation/primitives/Input';

/** Native adapters supply facts/gestures. This component owns only visual state. */
export function WorkspaceBrowser({ unclassified = [], groups, sessions, selected, draft, query, search, open, rename, fork, remove, pin, copyId, closeView, closeAllViews,
  selectWorkspace, create, renameWorkspace, removeWorkspace, workspaceSettings, addWorkspace, addingWorkspace, refresh, previous, next, notices, renderSession = (_node, render) => render(_node) }: {
  draft?: DraftConversation;
  unclassified?: readonly string[];
  renderSession?: (node: SessionNode, render: (node: SessionNode) => ReactNode) => ReactNode;
  workspaceSettings?: (id: string, label: string) => void;
  groups: readonly GroupNode[]; sessions: readonly SessionNode[]; selected?: string;
  query: string; search: (text: string) => void; open: (id: string) => void; rename: (id: string, returnFocus?: HTMLElement | null) => void;
  closeView?: (id: string) => void;
  pin?: (id: string) => void; copyId?: (id: string) => void;
  closeAllViews?: () => void;
  fork: (id: string) => void; remove: (id: string) => void; selectWorkspace: (id: string) => void; create: (id: string) => void;
  renameWorkspace: (id: string, title: string) => void; removeWorkspace: (id: string, title: string) => void;
  addWorkspace?: () => void; addingWorkspace?: boolean; refresh: () => void; previous?: () => void; next?: () => void; notices?: ReactNode;
}) {
  const tx = useTranslation();
  const [searchExpanded, setSearchExpanded] = useState(false), [flat, setFlat] = useState(false), [menu, setMenu] = useState(false);
  const [collapsed, setCollapsed] = useState<string[]>([]);
  const draftGroup = groups.find(group => group.workspaceId === draft?.workspaceId)?.key;
  useEffect(() => {
    if (draftGroup) setCollapsed(value => value.filter(key => key !== draftGroup));
  }, [draftGroup, draft?.binding]);
  const draftRow = draft && <DraftConversationRow focus={draft.focus}/>;
  const searchInput = useRef<HTMLInputElement>(null);
  // The Session tree is where keyboard navigation continues when a row's
  // actions menu loses its anchor: the row scrolled out of the tree, which
  // stays rendered and visible around it. Programmatically focusable, never a
  // Tab stop of its own; Tab from it enters its rows.
  const tree = useRef<HTMLDivElement>(null);
  const row = (node: SessionNode) => renderSession(node, node => <SessionNodeItem key={node.id} node={node} currentId={selected} now={Date.now()} onOpen={open} onRename={rename} onFork={fork} onDelete={remove} onPin={pin} onCopyId={copyId} menuFocusOwner={tree} t={tx} />);
  return <section className={workspaceBrowserCss.root} aria-label={tx('workspace:workspace-browser.workspaces-and-sessions')}>
    <div className={workspaceBrowserCss.sectionHeader}>
      {<span className={clsx(workspaceBrowserCss.sectionLabel, workspaceBrowserCss.wide, searchExpanded && workspaceBrowserCss.sectionLabelHidden)}>{flat ? tx('workspace:workspace-browser.sessions') : tx('workspace:workspace-browser.workspaces')}</span>}
      {<div className={clsx(workspaceBrowserCss.searchSlot, searchExpanded && workspaceBrowserCss.searchSlotExpanded)}><div className={clsx(workspaceBrowserCss.search, searchExpanded && workspaceBrowserCss.searchExpanded)} onClick={() => { setSearchExpanded(true); searchInput.current?.focus(); }}>
        <button type="button" className={workspaceBrowserCss.searchButton} aria-label={tx('workspace:workspace-browser.search-sessions')} aria-expanded={searchExpanded} onClick={() => setSearchExpanded(true)}><IconSearchOutline16 size={searchExpanded ? 11 : 14} /></button>
        <TextInput unstyled ref={searchInput} className={workspaceBrowserCss.searchInput} aria-label={tx('workspace:workspace-browser.search-session-metadata')} placeholder={tx('workspace:workspace-browser.search-sessions-2')} maxLength={256} value={query} tabIndex={searchExpanded ? 0 : -1} onChange={e => search(e.target.value)} onKeyDown={e => { if (e.key === 'Escape') { search(''); setSearchExpanded(false); } }} />
        {searchExpanded && <button type="button" className={workspaceBrowserCss.clearButton} aria-label={tx('workspace:workspace-browser.clear-search')} onClick={e => { e.stopPropagation(); search(''); setSearchExpanded(false); }}><IconCloseFill14 /></button>}
      </div></div>}
      <div className={clsx(workspaceBrowserCss.headerActions, searchExpanded && workspaceBrowserCss.headerActionsHidden)}>
        {<Menu open={menu} onClose={() => setMenu(false)} autoFocus listClassName={workspaceBrowserCss.viewOptionsMenu} anchor={<button type="button" className={workspaceBrowserCss.iconButton} aria-label={tx('workspace:workspace-browser.view-options')} onClick={() => setMenu(!menu)}><IconEllipsisOutline16 /></button>}
          items={[{ id: 'group', label: flat ? tx('workspace:workspace-browser.grouped-view') : tx('workspace:workspace-browser.flat-view') }, { id: 'refresh', label: tx('workspace:workspace-browser.refresh-list') },
            ...(closeView && sessions.some(node => node.viewOpen) ? [{ id: 'close-view', label: tx('workspace:workspace-browser.close-view'), submenu: sessions.filter(node => node.viewOpen).map(node => ({ id: `close:${node.id}`, label: node.title })) }] : []),
            ...(closeAllViews ? [{ id: 'close-views', label: tx('workspace:workspace-browser.close-all-views') }] : [])]}
          onSelect={id => { setMenu(false); if (id === 'group') setFlat(!flat); else if (id === 'close-views') closeAllViews?.(); else if (id.startsWith('close:')) closeView?.(id.slice(6)); else refresh(); }} />}
        {addWorkspace && <Tooltip label={tx('workspace:workspace-navigation.add-workspace')}><button type="button" className={workspaceBrowserCss.iconButton} aria-label={tx('workspace:workspace-navigation.add-workspace')} disabled={addingWorkspace} aria-busy={addingWorkspace || undefined} onClick={addWorkspace}><IconProjectAddOutline16 size={16} /></button></Tooltip>}
      </div>
    </div>

    <div className={workspaceBrowserCss.listArea}>{<div className={clsx(workspaceBrowserCss.treeBody, workspaceBrowserCss.wide)}><div ref={tree} className={workspaceBrowserCss.list} role="tree" aria-label={tx('workspace:workspace-browser.session-browser')} tabIndex={-1}>
      {notices}
      {query ? sessions.map(node => renderSession(node, node => <SearchResultItem key={node.id} result={{ ...node, workspace: groups.find(group => group.sessions.some(session => session.id === node.id))?.label ?? '' }} currentId={selected} onOpen={open} t={tx} />)) : flat || groups.length === 0 ? <>{draftRow}{sessions.map(row)}</> : groups.map(group => <div className={workspaceBrowserCss.groupSection} key={group.key} data-workspace-group={group.key}>
        <ProjectRowItem group={{ ...group, expanded: !collapsed.includes(group.key) }} menuFocusOwner={tree} t={tx}
          onToggle={() => setCollapsed(value => value.includes(group.key) ? value.filter(id => id !== group.key) : [...value, group.key])}
          onSelect={() => group.workspaceId && selectWorkspace(group.workspaceId)} onCreate={() => group.workspaceId && create(group.workspaceId)}
          actions={group.workspaceId ? { settings: workspaceSettings ? () => workspaceSettings(group.workspaceId!, group.label) : undefined, rename: () => renameWorkspace(group.workspaceId!, group.label), delete: () => removeWorkspace(group.workspaceId!, group.label) } : undefined} />
        {!collapsed.includes(group.key) && <>{group.key === draftGroup && draftRow}{group.sessions.map(row)}</>}
      </div>)}
      {!flat && !query && groups.length > 0 && sessions.some(session => !unclassified.includes(session.id) && !groups.some(group => group.sessions.some(member => member.id === session.id))) && <details><summary>{tx('workspace:workspace-browser.sessions-outside-registered-workspaces')}</summary>{sessions.filter(session => !unclassified.includes(session.id) && !groups.some(group => group.sessions.some(member => member.id === session.id))).map(row)}</details>}
      {!flat && !query && groups.length > 0 && unclassified.length > 0 && <section aria-label={tx('workspace:association.unclassified')}><p>{tx('workspace:association.unclassified')}</p>{sessions.filter(session => unclassified.includes(session.id)).map(row)}</section>}
      {!sessions.length && (!draft || !!query) && <p className={workspaceBrowserCss.empty}>{query ? tx('workspace:workspace-browser.no-matching-sessions') : tx('workspace:workspace-browser.no-sessions-yet')}</p>}
    </div><div className={workspaceBrowserCss.fade} /></div>}</div>
    {(previous || next) && <div><button className={workspaceBrowserCss.sessionOverflowButton} disabled={!previous} onClick={previous}>{tx('workspace:workspace-browser.previous')}</button><button className={workspaceBrowserCss.sessionOverflowButton} disabled={!next} onClick={next}>{tx('workspace:workspace-browser.next')}</button></div>}
  </section>;
}

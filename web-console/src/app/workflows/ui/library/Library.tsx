import { useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import { useSelector } from '@xstate/react';
import clsx from 'clsx';
import type { WorkflowFileSummary } from '../../../../../../protocol/app-server/v57';
import { useTranslation } from '../../../../locale/react';
import { Button } from '../../../../presentation/primitives/Button';
import { Menu } from '../../../../presentation/primitives/Menu';
import { IconChevronDownOutline14, IconCopyOutline16, IconEditOutline16, IconEllipsisOutline16, IconGlobeOutline14, IconPlusOutline16, IconSearchOutline16, IconTrashOutline16, IconWarningOutline16 } from '../../../../presentation/primitives/icons';
import type { WorkspaceAuthority } from '../../../../workspaces/authority';
import type { WorkflowSurfaceActor } from '../../machines/surface';
import { USER_SCOPE, type WorkflowScope } from '../../port';
import { IconFile, IconFolder, IconGrid, IconList } from '../icons';
import { FailureText } from '../notices';
import { LibraryDialog } from './dialogs';
import css from '../Workflows.module.css';

const VIEW = 'rustx-workflows-library-view';
function readView(): 'cards' | 'list' {
  try { return localStorage.getItem(VIEW) === 'list' ? 'list' : 'cards'; } catch { return 'cards'; }
}

function useModified() {
  const tx = useTranslation();
  return useMemo(() => {
    const format = new Intl.DateTimeFormat(tx.language, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' });
    return (ms: number | null | undefined) => ms == null ? '' : format.format(ms);
  }, [tx.language]);
}

/** Level 1: the Workflow files physically owned by one selected scope.
 * Registered Workspaces come from the Product Host catalog only; choosing
 * one changes this library, never a conversation or execution context. */
export function WorkflowLibrary({ surface, authority }: { surface: WorkflowSurfaceActor; authority: WorkspaceAuthority }) {
  const tx = useTranslation();
  const library = useSelector(surface, snapshot => snapshot.context.library);
  const state = useSelector(library, snapshot => snapshot);
  const { scope, observation, listFailure, operationFailure, confirmation } = state.context;
  const listing = observation?.listing;
  // While a read replaces the shown listing, nothing new starts from it.
  const settled = state.matches('ready');
  // Every time Level 1 is shown it lists the selected scope afresh.
  useEffect(() => library.send({ type: 'ACTIVATE' }), [library]);
  const catalog = useSyncExternalStore(authority.subscribe, authority.getCatalog);
  useEffect(() => { if (!catalog) void authority.observe().catch(() => {}); }, [catalog, authority]);
  const workspaces = catalog?.workspaces ?? [];
  const [tab, setTab] = useState<'workspace' | 'global'>(scope.kind === 'workspace' ? 'workspace' : 'global');
  const chosen = useRef(scope.kind === 'workspace');
  // Until the author picks a tab, the first registered Workspace is shown.
  useEffect(() => {
    if (chosen.current || !workspaces.length) return;
    chosen.current = true;
    setTab('workspace');
    library.send({ type: 'SCOPE', scope: { kind: 'workspace', workspaceId: workspaces[0].id } });
  }, [workspaces, library]);
  const lastWorkspace = useRef(scope.kind === 'workspace' ? scope.workspaceId : undefined);
  if (scope.kind === 'workspace') lastWorkspace.current = scope.workspaceId;
  const select = (next: WorkflowScope) => { chosen.current = true; library.send({ type: 'SCOPE', scope: next }); };
  const chooseTab = (next: 'workspace' | 'global') => {
    chosen.current = true;
    setTab(next);
    if (next === 'global') select(USER_SCOPE);
    else {
      const id = workspaces.find(row => row.id === lastWorkspace.current)?.id ?? workspaces[0]?.id;
      if (id) select({ kind: 'workspace', workspaceId: id });
    }
  };
  const [query, setQuery] = useState('');
  const [view, setView] = useState(readView);
  const chooseView = (next: 'cards' | 'list') => { setView(next); try { localStorage.setItem(VIEW, next); } catch { /* view preference only */ } };
  const modified = useModified();

  const workspace = scope.kind === 'workspace' ? workspaces.find(row => row.id === scope.workspaceId) : undefined;
  const labelOf = (shown: WorkflowScope) => shown.kind === 'user' ? tx('workflows:scope.global-files') : workspaces.find(row => row.id === shown.workspaceId)?.displayName ?? shown.workspaceId;
  const scopeLabel = scope.kind === 'user' ? tx('workflows:scope.global-files') : workspace?.displayName ?? '';
  const showingWorkspace = tab === 'workspace';
  const revoked = showingWorkspace && scope.kind === 'workspace' && !!catalog && !workspace;
  const files = useMemo(() => {
    const needle = query.trim().toLowerCase();
    const rows = listing?.files ?? [];
    return needle ? rows.filter(file => [file.file_name, file.description ?? '', file.relative_path].some(text => text.toLowerCase().includes(needle))) : rows;
  }, [listing, query]);
  // The library machine binds a confirmation or an opened editor to the
  // accepted observation it starts from; the UI only names the file.
  const open = (file: WorkflowFileSummary) => library.send({ type: 'OPEN', file: file.file_name });
  const confirm = (kind: 'create' | 'copy' | 'delete', file?: WorkflowFileSummary) => library.send({ type: 'CONFIRM', kind, file: file?.file_name });
  const actions = (file: WorkflowFileSummary) => [
    { id: 'open', label: tx('workflows:library.open'), icon: <IconEditOutline16 />, disabled: !settled || !file.id || !file.revision },
    { id: 'copy', label: tx('workflows:library.copy'), icon: <IconCopyOutline16 />, disabled: !settled || !file.id || !!file.diagnostic },
    { type: 'separator' as const, id: 'separator' },
    { id: 'delete', label: tx('workflows:library.delete'), icon: <IconTrashOutline16 />, danger: true, disabled: !settled || !file.id || !file.revision },
  ];
  const act = (file: WorkflowFileSummary, action: string) => {
    if (action === 'open') open(file);
    else if (action === 'copy') confirm('copy', file);
    else if (action === 'delete') confirm('delete', file);
  };

  let body;
  if (showingWorkspace && catalog && !workspaces.length) body = <p className={css.state}>{tx('workflows:library.no-workspaces')}</p>;
  else if (revoked || listFailure?.kind === 'unavailable') body = <div className={css.state} role="alert"><p>{tx('workflows:library.unavailable')}</p><Button variant="outline" size="sm" onClick={() => library.send({ type: 'REFRESH' })}>{tx('workflows:library.retry')}</Button></div>;
  else if (state.matches('failed') && listFailure) body = <div className={css.state} role="alert"><p>{tx('workflows:library.read-failed', { p0: listFailure.kind === 'failed' ? listFailure.message : listFailure.kind })}</p><Button variant="outline" size="sm" onClick={() => library.send({ type: 'REFRESH' })}>{tx('workflows:library.retry')}</Button></div>;
  else if (!listing) body = <p className={css.state} role="status">{tx('workflows:library.loading')}</p>;
  else if (listing.diagnostic) body = <p className={css.state} role="alert">{tx('workflows:library.directory-invalid', { p0: listing.diagnostic.reason })}</p>;
  else if (!files.length) body = <p className={css.state}>{query.trim() ? tx('workflows:library.empty-search') : tx('workflows:library.empty')}</p>;
  else if (view === 'cards') body = <ul className={css.cards} aria-label={tx('workflows:library.title')}>
    {files.map(file => <li key={file.file_name} className={clsx(css.card, file.diagnostic && css.invalidCard)}>
      <div className={css.cardHead}>
        <button type="button" className={css.cardOpen} onClick={() => open(file)} disabled={!settled || !file.id || !file.revision} aria-label={file.id ?? file.file_name}>
          <span className={css.fileTile}><IconFile size={18} /></span><span className={css.cardTitle}>{file.id ?? file.file_name}</span>
        </button>
        <CardMenu label={tx('workflows:library.actions', { p0: file.file_name })} items={actions(file)} onSelect={action => act(file, action)} />
      </div>
      <FileFacts file={file} />
      <div className={css.cardFoot}><span className={css.path}><IconFile size={14} /><span>{file.relative_path}</span></span><span className={css.time}>{modified(file.modified_ms)}</span></div>
    </li>)}
  </ul>;
  else body = <table className={css.table}>
    <thead><tr><th>{tx('workflows:library.column-file')}</th><th>{tx('workflows:library.column-description')}</th><th>{tx('workflows:library.column-location')}</th><th>{tx('workflows:library.column-modified')}</th><th aria-label={tx('workflows:library.actions', { p0: '' })} /></tr></thead>
    <tbody>{files.map(file => <tr key={file.file_name}>
      <td><button type="button" className={css.rowOpen} onClick={() => open(file)} disabled={!settled || !file.id || !file.revision}>{file.id ?? file.file_name}</button></td>
      <td><FileFacts file={file} compact /></td>
      <td><code>{file.relative_path}</code></td><td>{modified(file.modified_ms)}</td>
      <td><CardMenu label={tx('workflows:library.actions', { p0: file.file_name })} items={actions(file)} onSelect={action => act(file, action)} /></td>
    </tr>)}</tbody>
  </table>;

  return <section className={css.library} aria-labelledby="workflows-title">
    <header className={css.pageHead}>
      <div><h1 id="workflows-title">{tx('workflows:library.title')}</h1><p className={css.muted}>{tx('workflows:library.subtitle')}</p></div>
      <Button variant="primary" icon={<IconPlusOutline16 />} disabled={!settled || !listing || (showingWorkspace && (!workspace || revoked))} onClick={() => confirm('create')}>{tx('workflows:library.new')}</Button>
    </header>
    <div className={css.tabs} role="tablist" aria-label={tx('workflows:library.scopes')}>
      {(['workspace', 'global'] as const).map(name => <button key={name} type="button" role="tab" aria-selected={tab === name} className={clsx(css.tab, tab === name && css.tabActive)} onClick={() => chooseTab(name)}>
        {tx(name === 'workspace' ? 'workflows:scope.workspace' : 'workflows:scope.global')}
      </button>)}
    </div>
    <div className={css.controls}>
      <div className={css.controlsLeft}>
        <span className={css.subtle}>{tx(showingWorkspace ? 'workflows:scope.choose-workspace' : 'workflows:scope.file-scope')}</span>
        {showingWorkspace
          ? <WorkspaceChooser workspaces={workspaces} selected={workspace?.id} onSelect={id => select({ kind: 'workspace', workspaceId: id })} />
          : <span className={css.chooser} aria-live="polite"><span className={css.chooserValue}><IconGlobeOutline14 />{tx('workflows:scope.global-files')}</span></span>}
      </div>
      <div className={css.controlsRight}>
        <label className={css.search}><IconSearchOutline16 /><input type="search" value={query} onChange={event => setQuery(event.target.value)} placeholder={tx('workflows:library.search')} aria-label={tx('workflows:library.search')} /></label>
        <div className={css.viewSwitch} role="group" aria-label={tx('workflows:library.view')}>
          <button type="button" aria-pressed={view === 'cards'} aria-label={tx('workflows:library.view-cards')} className={clsx(view === 'cards' && css.viewActive)} onClick={() => chooseView('cards')}><IconGrid size={15} /></button>
          <button type="button" aria-pressed={view === 'list'} aria-label={tx('workflows:library.view-list')} className={clsx(view === 'list' && css.viewActive)} onClick={() => chooseView('list')}><IconList size={15} /></button>
        </div>
      </div>
    </div>
    {listing && !(showingWorkspace && !workspace) && <p className={css.context}>{scopeLabel}<span aria-hidden> · </span>{listing.files.length === 1 ? tx('workflows:library.count-one') : tx('workflows:library.count', { p0: listing.files.length })}</p>}
    {operationFailure && <div className={css.banner} role="alert">
      {operationFailure.failure.kind === 'uncertain'
        ? tx('workflows:library.operation-uncertain', { p0: tx(`workflows:library.${operationFailure.operation.kind === 'create' ? 'new' : operationFailure.operation.kind}`) })
        : <FailureText failure={operationFailure.failure} operation={tx(`workflows:library.${operationFailure.operation.kind === 'create' ? 'new' : operationFailure.operation.kind}`)} />}
      {!confirmation && <Button size="sm" onClick={() => library.send({ type: 'DISMISS' })}>{tx('workflows:notice.dismiss')}</Button>}
    </div>}
    {body}
    {confirmation && <LibraryDialog key={confirmation.seq} confirmation={confirmation} scopeLabel={labelOf(confirmation.observation.port.scope)} library={library} />}
  </section>;
}

function FileFacts({ file, compact }: { file: WorkflowFileSummary; compact?: boolean }) {
  const tx = useTranslation();
  if (file.diagnostic) {
    return <p className={clsx(css.cardDescription, css.invalid, compact && css.compact)}>
      <IconWarningOutline16 /><span><strong>{file.revision ? tx('workflows:library.invalid') : tx('workflows:library.unreadable')}</strong> · {file.diagnostic.reason}{file.diagnostic.path !== '$' && <> <code>{file.diagnostic.path}</code></>}</span>
    </p>;
  }
  return <p className={clsx(css.cardDescription, compact && css.compact)}>
    {file.description}
    {file.shadows_global && <span className={css.badge} title={tx('workflows:library.shadows-global-detail')}>{tx('workflows:library.shadows-global')}</span>}
  </p>;
}

function CardMenu({ label, items, onSelect }: { label: string; items: Parameters<typeof Menu>[0]['items']; onSelect: (id: string) => void }) {
  const [open, setOpen] = useState(false);
  return <Menu open={open} onClose={() => setOpen(false)} align="end" autoFocus items={items} onSelect={id => { setOpen(false); onSelect(id); }}
    anchor={<button type="button" className={css.dots} aria-label={label} aria-haspopup="menu" aria-expanded={open} onClick={() => setOpen(value => !value)}><IconEllipsisOutline16 /></button>} />;
}

/** A searchable chooser over the Product Host's registered Workspaces. */
function WorkspaceChooser({ workspaces, selected, onSelect }: { workspaces: readonly { id: string; displayName: string; displayPath: string }[]; selected?: string; onSelect: (id: string) => void }) {
  const tx = useTranslation();
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState('');
  const [active, setActive] = useState(0);
  const trigger = useRef<HTMLButtonElement>(null);
  const matches = workspaces.filter(row => `${row.displayName} ${row.displayPath}`.toLowerCase().includes(query.trim().toLowerCase()));
  const current = workspaces.find(row => row.id === selected);
  const close = (focus = true) => { setOpen(false); setQuery(''); setActive(0); if (focus) trigger.current?.focus(); };
  const choose = (id: string) => { onSelect(id); close(); };
  return <div className={css.chooserWrap}>
    <button ref={trigger} type="button" className={css.chooser} aria-haspopup="listbox" aria-expanded={open} onClick={() => setOpen(value => !value)}
      aria-label={tx('workflows:scope.chooser-label', { p0: current?.displayName ?? '' })}>
      <span className={css.chooserValue}><IconFolder size={15} />{current?.displayName ?? tx('workflows:scope.choose-workspace')}</span><IconChevronDownOutline14 />
    </button>
    {open && <div className={css.popover} onKeyDown={event => {
      if (event.key === 'Escape') { event.stopPropagation(); close(); }
      else if (event.key === 'ArrowDown') { event.preventDefault(); setActive(index => Math.min(index + 1, matches.length - 1)); }
      else if (event.key === 'ArrowUp') { event.preventDefault(); setActive(index => Math.max(index - 1, 0)); }
      else if (event.key === 'Enter' && matches[active]) { event.preventDefault(); choose(matches[active].id); }
    }} onBlur={event => { if (!event.currentTarget.contains(event.relatedTarget)) close(false); }}>
      <label className={css.search}><IconSearchOutline16 /><input autoFocus value={query} onChange={event => { setQuery(event.target.value); setActive(0); }} placeholder={tx('workflows:scope.search-workspaces')} aria-label={tx('workflows:scope.search-workspaces')} aria-controls="workflow-workspaces" aria-activedescendant={matches[active] ? `workflow-workspace-${matches[active].id}` : undefined} /></label>
      <ul id="workflow-workspaces" role="listbox" className={css.options} aria-label={tx('workflows:scope.choose-workspace')}>
        {matches.map((row, index) => <li key={row.id} id={`workflow-workspace-${row.id}`} role="option" aria-selected={row.id === selected} className={clsx(css.option, index === active && css.optionActive)}
          onMouseDown={event => event.preventDefault()} onClick={() => choose(row.id)}>
          <IconFolder size={14} /><span>{row.displayName}</span><small>{row.displayPath}</small>
        </li>)}
        {!matches.length && <li className={css.muted}>{tx('workflows:scope.no-workspace-match')}</li>}
      </ul>
    </div>}
  </div>;
}

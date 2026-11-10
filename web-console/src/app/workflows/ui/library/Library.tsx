import { presentationWarning } from '../../document/diagnostics';
import { useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import { useSelector } from '@xstate/react';
import clsx from 'clsx';
import type { WorkflowFileSummary } from '../../../../../../protocol/app-server/v77';
import { useTranslation } from '../../../../locale/react';
import { CardMenu } from '../../../../presentation/catalog/CardMenu';
import { TransferDialog, transferActions, type TransferIntent } from '../../../resources/shared/ResourceTransfer';
import { CatalogSearch, CatalogViewSwitch, type CatalogView } from '../../../../presentation/catalog/CatalogControls';
import css from '../../../../presentation/catalog/Catalog.module.css';
import { Button } from '../../../../presentation/primitives/Button';
import { Switch } from '../../../../presentation/primitives/Switch';
import { IconChevronDownOutline14, IconCopyOutline16, IconEditOutline16, IconPlusOutline16, IconSearchOutline16, IconTrashOutline16, IconWarningOutline16 } from '../../../../presentation/primitives/icons';
import type { WorkspaceAuthority } from '../../../../workspaces/authority';
import type { LibraryOperation, WorkflowLibraryActor, WorkflowLibrarySnapshot } from '../../machines/library';
import type { WorkflowSurfaceActor } from '../../machines/surface';
import { USER_SCOPE, type WorkflowScope } from '../../port';
import { IconFile, IconFolder } from '../icons';
import { WorkflowIcon } from '../WorkflowIcon';
import { FailureText } from '../notices';
import { LibraryDialog } from './dialogs';

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

/** The selected Workspace library and its independently authored Global section. */
export function WorkflowLibrary({ surface, authority }: { surface: WorkflowSurfaceActor; authority: WorkspaceAuthority }) {
  const tx = useTranslation();
  const library = useSelector(surface, snapshot => snapshot.context.library);
  const globalLibrary = useSelector(surface, snapshot => snapshot.context.globalLibrary);
  const globalState = useSelector(globalLibrary, snapshot => snapshot);
  const state = useSelector(library, snapshot => snapshot);
  const { scope, observation } = state.context;
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
  const workspace = scope.kind === 'workspace' ? workspaces.find(row => row.id === scope.workspaceId) : undefined;
  const scopeLabel = scope.kind === 'user' ? tx('workflows:scope.global-files') : workspace?.displayName ?? '';
  const showingWorkspace = tab === 'workspace';
  const revoked = showingWorkspace && scope.kind === 'workspace' && !!catalog && !workspace;
  useEffect(() => { if (showingWorkspace) globalLibrary.send({ type: 'ACTIVATE' }); }, [showingWorkspace, globalLibrary]);
  const [transferIntent, setTransferIntent] = useState<TransferIntent>();
  const transfer = observation?.port.transferResource;
  const workspaceDirectory = listing?.target.kind === 'workspace' ? listing.target.directory : undefined;
  const globalListing = globalState.context.observation?.listing;
  const transferAvailable = showingWorkspace && workspace && transfer && workspaceDirectory;
  const transferDisabled = !settled || !globalState.matches('ready') || !!listing?.diagnostic || !!globalListing?.diagnostic;
  const startTransfer = (from: WorkflowScope['kind'], file: WorkflowFileSummary, operation: TransferIntent['operation']) => {
    if (!transferAvailable || !file.id || transferDisabled) return;
    const [source, destination] = from === 'user' ? [globalListing, listing] : [listing, globalListing];
    const ids = (rows: typeof listing) => rows?.files.flatMap(row => row.id ? [row.id] : []) ?? [];
    setTransferIntent({
      operation, family: 'workflow', from, name: file.id, revision: file.revision,
      directory: workspaceDirectory, destinationLabel: from === 'user' ? workspace.displayName : tx('resources:scope.global'),
      taken: new Set([...ids(source).filter(id => id !== file.id), ...ids(destination)]),
      transfer,
    });
  };
  const onTransfer = transferAvailable ? startTransfer : undefined;
  const context = listing && !(showingWorkspace && !workspace) && <p className={css.context}>{scopeLabel}<span aria-hidden> · </span>{listing.files.length === 1 ? tx('workflows:library.count-one') : tx('workflows:library.count', { p0: listing.files.length })}</p>;

  return <section className={css.library} aria-labelledby="workflows-title">
    <header className={css.pageHead}>
      <div><h1 id="workflows-title">{tx('workflows:library.title')}</h1><p className={css.muted}>{tx('workflows:library.subtitle')}</p></div>
      <Button variant="primary" icon={<IconPlusOutline16 />} disabled={!settled || !listing || (showingWorkspace && (!workspace || revoked))}
        onClick={() => library.send({ type: 'CONFIRM', kind: 'create' })}>{tx('workflows:library.new')}</Button>
    </header>
    <div className={css.tabs} role="tablist" aria-label={tx('workflows:library.scopes')}>
      {(['workspace', 'global'] as const).map(name => <button key={name} type="button" role="tab" aria-selected={tab === name} className={clsx(css.tab, tab === name && css.tabActive)} onClick={() => chooseTab(name)}>
        {tx(name === 'workspace' ? 'workflows:scope.workspace' : 'workflows:scope.global')}
      </button>)}
    </div>
    <div className={clsx(css.controls, !showingWorkspace && css.globalControls)}>
      {showingWorkspace && <div className={css.controlsLeft}>
        <span className={css.subtle}>{tx('workflows:scope.choose-workspace')}</span>
        <WorkspaceChooser workspaces={workspaces} selected={workspace?.id} onSelect={id => select({ kind: 'workspace', workspaceId: id })} />
      </div>}
      {!showingWorkspace && context}
      <div className={css.controlsRight}>
        <CatalogSearch label={tx('workflows:library.search')} value={query} onChange={setQuery} />
        <CatalogViewSwitch view={view} onChange={chooseView} />
      </div>
    </div>
    {showingWorkspace && context}
    {showingWorkspace && catalog && !workspaces.length ? <p className={css.state}>{tx('workflows:library.no-workspaces')}</p>
      : <WorkflowFiles library={library} state={state} scopeLabel={scopeLabel} query={query} view={view} revoked={revoked}
        onTransfer={onTransfer} transferDisabled={transferDisabled} />}
    {showingWorkspace && <section className={css.scopeGroup} aria-label={tx('resources:scope.inherited')}>
      <h2 className={css.context}>{tx('resources:scope.inherited')}{globalListing && <> · {globalListing.files.length}</>}</h2>
      <WorkflowFiles library={globalLibrary} state={globalState} scopeLabel={tx('workflows:scope.global-files')} query={query} view={view}
        onTransfer={onTransfer} transferDisabled={transferDisabled} />
    </section>}
    {transferIntent && <TransferDialog intent={transferIntent} close={() => setTransferIntent(undefined)} refresh={() => {
      library.send({ type: 'REFRESH' });
      globalLibrary.send({ type: 'REFRESH' });
    }} />}
  </section>;
}

function WorkflowFiles({ library, state, scopeLabel, query, view, revoked, onTransfer, transferDisabled }: {
  library: WorkflowLibraryActor; state: WorkflowLibrarySnapshot; scopeLabel: string; query: string; view: CatalogView; revoked?: boolean;
  onTransfer?: (scope: WorkflowScope['kind'], file: WorkflowFileSummary, operation: TransferIntent['operation']) => void;
  transferDisabled: boolean;
}) {
  const tx = useTranslation();
  const { scope, observation, listFailure, operationFailure, confirmation } = state.context;
  const listing = observation?.listing;
  const settled = state.matches('ready');
  const modified = useModified();
  const operationName = (operation: LibraryOperation) => operation.kind === 'enable' ? tx(operation.enabled ? 'workflows:library.operation-enable' : 'workflows:library.operation-disable')
    : tx(`workflows:library.${operation.kind === 'create' ? 'new' : operation.kind}`);
  const needle = query.trim().toLowerCase();
  const rows = listing?.files ?? [];
  const files = needle ? rows.filter(file => [file.file_name, file.description ?? '', file.relative_path, file.display?.title ?? '', file.display?.description ?? ''].some(text => text.toLowerCase().includes(needle))) : rows;
  // The library machine binds a confirmation or an opened editor to the
  // accepted observation it starts from; the UI only names the file.
  const open = (file: WorkflowFileSummary) => library.send({ type: 'OPEN', file: file.file_name });
  const confirm = (kind: 'create' | 'edit' | 'copy' | 'delete', file?: WorkflowFileSummary) => library.send({ type: 'CONFIRM', kind, file: file?.file_name });
  // As a Dify draft, an unfinished workflow is still edited and copied;
  // only a file that cannot be read is not.
  const actions = (file: WorkflowFileSummary) => [
    { id: 'edit', label: tx('workflows:library.edit'), icon: <IconEditOutline16 />, disabled: !settled || !file.id || !file.revision || unreadable(file) },
    { id: 'copy', label: tx('workflows:library.copy'), icon: <IconCopyOutline16 />, disabled: !settled || !file.id || !file.revision || unreadable(file) },
    ...(onTransfer ? transferActions(tx, scope.kind, transferDisabled || !file.id || !file.revision || unreadable(file)) : []),
    { type: 'separator' as const, id: 'separator' },
    { id: 'delete', label: tx('workflows:library.delete'), icon: <IconTrashOutline16 />, danger: true, disabled: !settled || !file.id || !file.revision },
  ];
  // Enabled for Sessions: a Global file in every Workspace, a Workspace file
  // in its own. A file that fails its check is off and cannot be switched.
  const toggle = (file: WorkflowFileSummary) => {
    const name = file.display?.title ?? file.id ?? file.file_name, off = failing(file);
    const unconfigurable = !listing?.configuration_revision;
    return <Switch label={tx('workflows:library.enable', { p0: name })} checked={file.enabled && !off}
      disabled={!settled || !file.id || off || unconfigurable} className={css.enable}
      title={tx(off ? 'workflows:library.enable-failing' : unconfigurable ? 'workflows:library.enable-unconfigurable' : scope.kind === 'user' ? 'workflows:library.enable-global' : 'workflows:library.enable-workspace')}
      onChange={enabled => library.send({ type: 'ENABLE', file: file.file_name, enabled })} />;
  };
  const act = (file: WorkflowFileSummary, action: string) => {
    if (action === 'edit') confirm('edit', file);
    else if (action === 'copy') confirm('copy', file);
    else if (action === 'delete') confirm('delete', file);
    else onTransfer?.(scope.kind, file, action === 'copy-scope' ? 'copy' : 'move');
  };

  let body;
  if (revoked || listFailure?.kind === 'unavailable') body = <div className={css.state} role="alert"><p>{tx('workflows:library.unavailable')}</p><Button variant="outline" size="sm" onClick={() => library.send({ type: 'REFRESH' })}>{tx('workflows:library.retry')}</Button></div>;
  else if (state.matches('failed') && listFailure) body = <div className={css.state} role="alert"><p>{tx('workflows:library.read-failed', { p0: listFailure.kind === 'failed' ? listFailure.message : listFailure.kind })}</p><Button variant="outline" size="sm" onClick={() => library.send({ type: 'REFRESH' })}>{tx('workflows:library.retry')}</Button></div>;
  else if (!listing) body = <p className={css.state} role="status">{tx('workflows:library.loading')}</p>;
  else if (listing.diagnostic) body = <p className={css.state} role="alert">{tx('workflows:library.directory-invalid', { p0: listing.diagnostic.reason })}</p>;
  else if (!files.length) body = <p className={css.state}>{query.trim() ? tx('workflows:library.empty-search') : tx('workflows:library.empty')}</p>;
  else if (view === 'cards') body = <ul className={css.cards} aria-label={tx('workflows:library.title')}>
    {files.map(file => <li key={file.file_name} className={clsx(css.card, unreadable(file) && css.invalidCard)}>
      <div className={css.cardHead}>
        <button type="button" className={css.cardOpen} onClick={() => open(file)} disabled={!settled || !file.id || !file.revision} aria-label={file.display?.title ?? file.id ?? file.file_name}>
          <span className={css.fileTile}>{unreadable(file) ? <IconFile size={18} /> : <WorkflowIcon icon={file.display?.icon} />}</span>
          <span className={css.cardName}><span className={css.cardTitle}>{file.display?.title ?? file.id ?? file.file_name}</span>{file.display && <span className={css.cardId}>{file.id}</span>}</span>
        </button>
        {toggle(file)}
        <CardMenu label={tx('workflows:library.actions', { p0: file.file_name })} items={actions(file)} onSelect={action => act(file, action)} />
      </div>
      <FileFacts file={file} />
      <div className={css.cardFoot}><span className={css.path}><IconFile size={14} /><span>{file.relative_path}</span></span><span className={css.time}>{modified(file.modified_ms)}</span></div>
    </li>)}
  </ul>;
  else body = <table className={css.table}>
    <thead><tr><th>{tx('workflows:library.column-file')}</th><th>{tx('workflows:library.column-description')}</th><th className={css.optionalColumn}>{tx('workflows:library.column-location')}</th><th>{tx('workflows:library.column-modified')}</th><th>{tx('workflows:library.column-enabled')}</th><th aria-label={tx('workflows:library.actions', { p0: '' })} /></tr></thead>
    <tbody>{files.map(file => <tr key={file.file_name}>
      <td><button type="button" className={css.rowOpen} onClick={() => open(file)} disabled={!settled || !file.id || !file.revision}><WorkflowIcon icon={unreadable(file) ? undefined : file.display?.icon} size={15} />{file.display?.title ?? file.id ?? file.file_name}</button></td>
      <td><FileFacts file={file} compact /></td>
      <td className={css.optionalColumn}><code>{file.relative_path}</code></td><td>{modified(file.modified_ms)}</td><td>{toggle(file)}</td>
      <td><CardMenu label={tx('workflows:library.actions', { p0: file.file_name })} items={actions(file)} onSelect={action => act(file, action)} /></td>
    </tr>)}</tbody>
  </table>;

  return <>
    {operationFailure && <div className={css.banner} role="alert">
      {operationFailure.failure.kind === 'uncertain'
        ? tx('workflows:library.operation-uncertain', { p0: operationName(operationFailure.operation) })
        : <FailureText failure={operationFailure.failure} operation={operationName(operationFailure.operation)} />}
      {!confirmation && <Button size="sm" onClick={() => library.send({ type: 'DISMISS' })}>{tx('workflows:notice.dismiss')}</Button>}
    </div>}
    {body}
    {confirmation && <LibraryDialog key={confirmation.seq} confirmation={confirmation} scopeLabel={scopeLabel} library={library} />}
  </>;
}

/** A file the editor cannot open: unparseable, or not a Workflow at all.
 * A parsed file the check rejects is an unfinished workflow instead. */
const unreadable = (file: WorkflowFileSummary) => !!file.diagnostic && file.nodes == null;
/** A file the check rejects, unreadable or unfinished: never enabled. */
const failing = (file: WorkflowFileSummary) => !!file.diagnostic && !presentationWarning(file.diagnostic);

function FileFacts({ file, compact }: { file: WorkflowFileSummary; compact?: boolean }) {
  const tx = useTranslation();
  const diagnostic = file.diagnostic;
  if (diagnostic && unreadable(file)) return <p className={clsx(css.cardDescription, css.invalid, compact && css.compact)}>
    <IconWarningOutline16 /><span><strong>{tx('workflows:library.unreadable')}</strong> · {diagnostic.reason}{diagnostic.path !== '$' && <> <code>{diagnostic.path}</code></>}</span>
  </p>;
  const warning = presentationWarning(diagnostic);
  return <p className={clsx(css.cardDescription, compact && css.compact)}>
    {file.display?.description ?? file.description}
    {diagnostic && <span className={clsx(css.badge, css.badgeWarning)} title={`${diagnostic.reason}${diagnostic.path !== '$' ? ` · ${diagnostic.path}` : ''}`}>
      <IconWarningOutline16 />{tx(warning ? 'workflows:library.presentation-warning' : 'workflows:library.unchecked')}
    </span>}
    {file.shadows_global && <span className={css.badge} title={tx('workflows:library.shadows-global-detail')}>{tx('workflows:library.shadows-global')}</span>}
  </p>;
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

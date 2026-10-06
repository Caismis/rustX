/* Copyright (c) 2026 DeepSeek. MIT. Guide layout adapted from ui-sidebar-right GuideBody; see PROVENANCE.md. */
import { lazy, Suspense, useCallback, useEffect, useRef, useState } from 'react';
import type { ProductHostWorkspaces, WorkspaceAuthorityScope } from '../../workspaces/host';
import type { DesktopTarget } from '../../workspaces/desktop';
import type { WorkbenchRequest, WorkbenchResult, WorkbenchTerminal } from '../../workspaces/workbench';
import { useTranslation } from '../../locale/react';
import { Button } from '../../presentation/primitives/Button';
import { IconFolderClose16 } from '../../presentation/primitives/icons';
import css from '../../presentation/right-panel/Workbench.module.css';
const TerminalView = lazy(() => import('./WorkbenchTerminal'));
export function Workbench({ host, scope, target, visible, fullscreen, toggleFullscreen, closePanel }: {
  host: ProductHostWorkspaces; scope?: WorkspaceAuthorityScope; target?: DesktopTarget; visible: boolean; fullscreen: boolean; toggleFullscreen: () => void; closePanel: () => void;
}) {
  const tx = useTranslation();
  const [page, setPage] = useState<'start' | 'files' | string>('start');
  const [terminals, setTerminals] = useState<WorkbenchTerminal[]>([]), [shells, setShells] = useState<string[]>([]);
  const [shell, setShell] = useState(''), [error, setError] = useState(''), [busy, setBusy] = useState(false);
  const pendingCreate = useRef<{ id: string; shell: string } | null>(null);
  const lifetime = useRef<AbortController | null>(null);
  useEffect(() => { lifetime.current = new AbortController(); return () => lifetime.current?.abort(); }, []);
  const call = useCallback(async (request: WorkbenchRequest, signal?: AbortSignal) => {
    if (!host.workbench || !scope || !target) throw new Error(tx('artifacts:workbench.unavailable'));
    return host.workbench(scope, { target, request }, signal ?? lifetime.current?.signal);
  }, [host, scope?.authorityId, scope?.endpoint, target?.session_id, target?.active_node, tx]);
  useEffect(() => {
    if (!visible || !scope || !target || !host.workbench) return;
    const abort = new AbortController();
    void call({ kind: 'terminals' }, abort.signal).then(value => { if (abort.signal.aborted) return; setTerminals(value.terminals ?? []); setShells(value.shells ?? []); setShell(previous => previous || value.shells?.[0] || ''); }, cause => { if (!abort.signal.aborted) setError(String(cause)); });
    return () => abort.abort();
  }, [call, visible]);
  const available = !!host.workbench && !!scope && !!target;
  const create = async () => {
    if (busy || !shell) return;
    setBusy(true); setError(''); const pending = pendingCreate.current ??= { id: crypto.randomUUID(), shell }; const { id } = pending;
    try { const value = await call({ kind: 'create', ...pending }); if (!lifetime.current?.signal.aborted) { pendingCreate.current = null; setTerminals(value.terminals ?? []); setPage(id); } }
    catch (cause) { if (!lifetime.current?.signal.aborted) setError(String(cause)); }
    finally { if (!lifetime.current?.signal.aborted) setBusy(false); }
  };
  const close = async (id: string) => { setError(''); try { const value = await call({ kind: 'close', id }); if (!lifetime.current?.signal.aborted) { setTerminals(value.terminals ?? []); setPage('start'); } } catch (cause) { if (!lifetime.current?.signal.aborted) setError(String(cause)); } };
  return <div className={css.root} hidden={!visible} data-workbench onKeyDown={event => { if (page !== 'start' && page !== 'files') return; if (event.key === 'Escape' && page !== 'start' && !event.defaultPrevented) { event.preventDefault(); setPage('start'); } else if (event.key === 'Escape' && !event.defaultPrevented) { event.preventDefault(); if (fullscreen) toggleFullscreen(); else closePanel(); } }}>
    {page !== 'start' && <div className={css.toolbar}>
      <Button size="sm" onClick={() => setPage('start')}>{tx('artifacts:workbench.start')}</Button>
      <Button size="sm" disabled={!available} onClick={() => setPage('files')}>{tx('artifacts:workbench.files')}</Button>
    </div>}
    {terminals.length > 0 && <div className={css.tabs} role="tablist" aria-label={tx('artifacts:workbench.terminals')}>{terminals.map((terminal, index) => <div key={terminal.id}><button type="button" role="tab" aria-selected={page === terminal.id} onClick={() => setPage(terminal.id)}>{tx('artifacts:workbench.terminal')} {index + 1}</button><button type="button" aria-label={tx('artifacts:workbench.close-terminal', { p0: index + 1 })} onClick={() => void close(terminal.id)}>×</button></div>)}</div>}
    {error && <p role="alert" className={css.notice}>{error}</p>}
    {!available && <p className={css.notice}>{tx('artifacts:workbench.unavailable')}</p>}
    {page === 'start' ? <div className={css.guide}>
      <svg className={css.compass} width="56" height="56" viewBox="0 0 56 56" aria-hidden="true"><circle cx="28" cy="28" r="24" fill="none" stroke="currentColor" strokeWidth="4"/><path d="M38 18 32 32 18 38 24 24Z" fill="currentColor"/></svg>
      <button type="button" className={css.entry} disabled={!available} onClick={() => setPage('files')}><IconFolderClose16 size={26}/><span><strong>{tx('artifacts:workbench.files')}</strong><small>{tx('artifacts:workbench.files-description')}</small></span></button>
      <div className={css.entry}><button type="button" disabled={!available || busy || !shell} onClick={() => void create()}><span aria-hidden="true">❯_</span><span><strong>{tx('artifacts:workbench.new-terminal')}</strong><small>{tx('artifacts:workbench.terminal-description')}</small></span></button><select aria-label={tx('artifacts:workbench.shell')} value={shell} disabled={busy || !shells.length} onChange={e => setShell(e.target.value)}>{shells.map(item => <option key={item} value={item}>{item.split('/').at(-1)}</option>)}</select></div>
    </div> : page === 'files' ? visible && <WorkspaceFiles call={call}/> : visible && <Suspense fallback={<p>{tx('artifacts:workbench.loading')}</p>}><TerminalView key={page} id={page} call={call}/></Suspense>}
  </div>;
}
function WorkspaceFiles({ call }: { call: (request: WorkbenchRequest, signal?: AbortSignal) => Promise<WorkbenchResult> }) {
  const tx = useTranslation();
  const [path, setPath] = useState(''), [file, setFile] = useState<string>(), [revision, refresh] = useState(0);
  const [value, setValue] = useState<WorkbenchResult>(), [error, setError] = useState('');
  useEffect(() => { const abort = new AbortController(); setValue(undefined); setError(''); void call(file ? { kind: 'read', path: file } : { kind: 'files', path }, abort.signal).then(result => { if (!abort.signal.aborted) setValue(result); }, cause => { if (!abort.signal.aborted) setError(String(cause)); }); return () => abort.abort(); }, [path, file, revision, call]);
  return <section className={css.files} aria-label={tx('artifacts:workbench.files')}>
    <div className={css.toolbar}><Button size="sm" disabled={!path && !file} onClick={() => { if (file) setFile(undefined); else setPath(path.split('/').slice(0, -1).join('/')); }}>{tx('artifacts:workbench.back')}</Button><span className={css.path}>{file ?? (path || '/')}</span><Button size="sm" onClick={() => refresh(n => n + 1)}>{tx('artifacts:workbench.refresh')}</Button></div>
    {error ? <p role="alert">{error}</p> : !value ? <p>{tx('artifacts:workbench.loading')}</p> : file ? <pre className={css.source}>{value.text}</pre> : <div className={css.list}>{!value.entries?.length && <p>{tx('artifacts:workbench.empty')}</p>}{value.entries?.map(entry => <button type="button" key={entry.name} disabled={entry.link} title={entry.link ? tx('artifacts:workbench.symlink') : entry.name} onClick={() => { const next = [path, entry.name].filter(Boolean).join('/'); if (entry.directory) setPath(next); else setFile(next); }}><span aria-hidden="true">{entry.directory ? '▸' : '·'}</span>{entry.name}{entry.directory ? '/' : ''}</button>)}</div>}
  </section>;
}

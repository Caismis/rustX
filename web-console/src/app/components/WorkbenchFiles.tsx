/* Copyright (c) 2026 DeepSeek. MIT. Adapted from FilesBody and TextPreview; see PROVENANCE.md. */
import { DocumentBody } from './workbench-documents/DocumentBody';
import { viewersForPath, loadsBytes, type Viewer } from './workbench-documents/definitions';
import type { ZoomPreference } from './workbench-documents/zoom/types';
import type { PreviewViewState } from '../../presentation/right-panel/preview-view-state';
import { useCallback, useEffect, useLayoutEffect, useMemo, useReducer, useRef, useState } from 'react';
import type { WorkbenchRequest, WorkbenchResult } from '../../workspaces/workbench';
import { useTranslation } from '../../locale/react';
import { FileTypeIcon } from '../../presentation/primitives/FileTypeIcon';
import { PathLabel } from '../../presentation/primitives/PathLabel';
import { languageForPath } from '../../presentation/primitives/file-language';
import { CodeBlock } from '../../presentation/markdown/CodeBlock';
import { Menu } from '../../presentation/primitives/Menu';
import { IconFolderClose16, IconFolderOpenOutline16, IconRefreshOutline16, IconWrapLinesOutline16, IconCodeOutline16, IconChevronDownOutline14 } from '../../presentation/primitives/icons';
import css from '../../presentation/right-panel/WorkbenchFiles.module.css';
import type { DesktopApplication } from '../../workspaces/desktop';
export type WorkbenchCall = (request: WorkbenchRequest, signal?: AbortSignal) => Promise<WorkbenchResult>;
/** Only tree metadata and view preferences outlive a mounted body; source bytes do not. */
export interface FileTabView { expanded: Set<string>; levels: Map<string, WorkbenchResult>; cwd: string; revision: number; mode?: Viewer; zoom?: ZoomPreference; document?: PreviewViewState; top: number; left: number; wrap: boolean }
export const fileTabView = (): FileTabView => ({ expanded: new Set(), levels: new Map(), cwd: '', revision: 0, top: 0, left: 0, wrap: false });
const byName = new Intl.Collator(undefined, { numeric: true, sensitivity: 'base' });
function Reload({ reload }: { reload: () => void }) {
  const tx = useTranslation(); const label = tx('artifacts:workbench.refresh');
  return <button type="button" className={css.tool} aria-label={label} title={label} onClick={reload}><IconRefreshOutline16/></button>;
}
export function WorkspaceFiles({ call, view, onOpen }: { call: WorkbenchCall; view: FileTabView; onOpen: (path: string) => void }) {
  const tx = useTranslation(), [, redraw] = useReducer(n => n + 1, 0);
  const [revision, setRevision] = useState(view.revision), body = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => { const el = body.current!; el.scrollTop = view.top; return () => { view.top = el.scrollTop; }; }, [view]);
  const loaded = useCallback((result: WorkbenchResult) => { view.cwd = result.cwd ?? ''; redraw(); }, [view]);
  return <section className={css.root} aria-label={tx('artifacts:workbench.files')} data-workbench-files>
    <div className={css.header}><PathLabel path={view.cwd} className={css.path}/><Reload reload={() => { view.revision++; view.levels.clear(); setRevision(view.revision); }}/><OpenFileApp call={call} path="" directory/></div>
    <div className={css.body} ref={body}><Directory path="" {...{ call, view, revision, onOpen, loaded }}/></div>
  </section>;
}
function Directory({ path, call, view, revision, onOpen, loaded }: { path: string; call: WorkbenchCall; view: FileTabView; revision: number; onOpen: (path: string) => void; loaded: (result: WorkbenchResult) => void }) {
  const tx = useTranslation(), [, redraw] = useReducer(n => n + 1, 0);
  const [error, setError] = useState('');
  useEffect(() => {
    if (view.levels.has(path)) return;
    const abort = new AbortController(); setError('');
    void call({ kind: 'files', path }, abort.signal).then(result => {
      if (abort.signal.aborted || revision !== view.revision) return;
      view.levels.set(path, result); if (!path) loaded(result); redraw();
    }, cause => { if (!abort.signal.aborted && revision === view.revision) setError(String(cause)); });
    return () => abort.abort();
  }, [call, path, view, revision, loaded]);
  const value = view.levels.get(path);
  return <ul className={css.level}>
    {error ? <li className={css.note} role="alert">{error}</li> : !value ? <li className={css.note}>{tx('artifacts:workbench.loading')}</li> : !value.entries?.length ? <li className={css.note}>{tx('artifacts:workbench.empty')}</li> : [...value.entries].sort((a,b) => Number(b.directory) - Number(a.directory) || byName.compare(a.name,b.name)).map(entry => {
      const child = [path, entry.name].filter(Boolean).join('/'), expanded = view.expanded.has(child);
      return <li className={css.item} key={entry.name} data-files-path={child}>
        <button type="button" className={css.row} disabled={entry.link} title={entry.link ? tx('artifacts:workbench.symlink') : entry.name} aria-expanded={entry.directory ? expanded : undefined} onClick={() => {
          if (entry.directory) { if (expanded) view.expanded.delete(child); else view.expanded.add(child); redraw(); } else onOpen(child);
        }}>{entry.directory ? expanded ? <IconFolderOpenOutline16 className={css.icon}/> : <IconFolderClose16 className={css.icon}/> : <FileTypeIcon path={entry.name} size={16} className={css.fileIcon}/>}<span className={css.name}>{entry.name}</span></button>
        {entry.directory && expanded && <Directory path={child} {...{ call, view, revision, onOpen, loaded }}/ >}
      </li>;
    })}
  </ul>;
}
export function WorkspaceFile({ path, call, view, onOpen }: { path: string; call: WorkbenchCall; view: FileTabView; onOpen:(path:string)=>void }) {
  const tx = useTranslation(), scroll = useRef<HTMLDivElement>(null);
  const [value, setValue] = useState<WorkbenchResult>(), [error, setError] = useState(''), [revision, refresh] = useState(view.revision), [wrap, setWrap] = useState(view.wrap);
  const [mode, setMode] = useState<Viewer>(view.mode ?? viewersForPath(path)[0]), [menu, setMenu] = useState(false);
  useEffect(() => {
    const abort = new AbortController(); setError(''); setValue(undefined);
    void call({ kind: mode === 'office' ? 'office' : loadsBytes(mode) ? 'bytes' : 'read', path }, abort.signal).then(result => { if (!abort.signal.aborted && revision === view.revision) setValue(result); }, cause => { if (!abort.signal.aborted) setError(String(cause)); });
    return () => abort.abort();
  }, [path, call, revision, view, mode]);
  useLayoutEffect(() => { const el = scroll.current; if (!el) return; el.scrollTop = view.top; el.scrollLeft = view.left; return () => { view.top = el.scrollTop; view.left = el.scrollLeft; }; }, [view, !!value, mode]);
  const [lifetime] = useState(() => new AbortController());
  useEffect(() => () => lifetime.abort(), [lifetime]);
  const bytes = useMemo(() => value?.base64 ? Uint8Array.from(atob(value.base64), c => c.charCodeAt(0)) : new Uint8Array(), [value]);
  const fullPath = value?.cwd ? `${value.cwd.replace(/\/$/, '')}/${path}` : path;
  return <section className={css.root} data-workbench-file={path}>
    <div className={css.header}><PathLabel path={fullPath} className={css.path}/><Menu open={menu} onClose={() => setMenu(false)} align="end" dense autoFocus selectedId={mode} items={viewersForPath(path).map(id => ({ id, label: tx(`artifacts:workbench.${id}`) }))} onSelect={id => { view.mode = id as Viewer; setMode(view.mode); setMenu(false); }} anchor={<button type="button" className={`${css.tool} ${css.viewer}`} aria-label={tx('artifacts:workbench.viewer')} aria-haspopup="menu" aria-expanded={menu} onClick={() => setMenu(!menu)}>{tx(`artifacts:workbench.${mode}`)}</button>}/><button hidden={mode !== 'text' && mode !== 'code'} type="button" className={css.tool} aria-label={tx('artifacts:artifact-preview.wrap-lines')} aria-pressed={wrap} title={tx('artifacts:artifact-preview.wrap-lines')} onClick={() => { view.wrap = !wrap; setWrap(!wrap); }}><IconWrapLinesOutline16/></button><Reload reload={() => { view.revision++; refresh(view.revision); }}/><OpenFileApp call={call} path={path} directory={false}/></div>
    {error && <p className={css.note} role="alert">{error}</p>}
    {!value ? !error && <p className={css.note}>{tx('artifacts:workbench.loading')}</p> : mode !== 'text' && mode !== 'code' ? <div ref={scroll} className={css.document}><DocumentBody key={mode} call={call} onOpen={onOpen} mode={mode} text={value.text ?? ''} bytes={bytes} path={path} view={view} signal={lifetime.signal} retry={() => { view.revision++; refresh(view.revision); }}/></div> : mode === 'text' ? <div ref={scroll} className={css.plainText} data-wrap={wrap}><pre>{value.text}</pre></div> : <div className={css.renderer} data-wrap={wrap}><CodeBlock className={css.code} contentRef={scroll} code={value.text ?? ''} lang={languageForPath(path)} lineNumbers copyLabel={tx('common:search-block.copy')} copiedLabel={tx('common:search-block.copied')} toolbarLabel={tx('artifacts:workbench.code')}/></div>}
  </section>;
}
function OpenFileApp({ call, path, directory }: { call: WorkbenchCall; path: string; directory: boolean }) {
  const tx = useTranslation();
  const [apps, setApps] = useState<DesktopApplication[]>([]), [selected, setSelected] = useState<string>(), [menu, setMenu] = useState(false), [error, setError] = useState(''), [busy, setBusy] = useState(false);
  const live = useRef(true), pending = useRef(false);
  useEffect(() => { live.current = true; const abort = new AbortController(); void call({ kind: 'applications' }, abort.signal).then(result => {
    if (!abort.signal.aborted) setApps(result.applications?.available ? result.applications.applications.filter(app => app.id !== 'terminal') : []);
  }, cause => { if (!abort.signal.aborted) setError(String(cause)); }); return () => { live.current = false; abort.abort(); }; }, [call]);
  const app = apps.find(app => app.id === selected) ?? apps.find(app => app.id === (directory ? 'files' : 'code')) ?? apps[0];
  const launch = async (id = app?.id) => {
    if (!id || id === 'terminal' || pending.current) return;
    pending.current = true; setBusy(true); setError('');
    try { await call({ kind: 'open', path, directory, application: id }); }
    catch (cause) { if (live.current) setError(String(cause)); }
    finally { pending.current = false; if (live.current) setBusy(false); }
  };
  return <div className={css.openApp}>
    <Menu open={menu} onClose={() => setMenu(false)} align="end" dense autoFocus items={apps.map(a => ({ id: a.id, label: a.label }))} selectedId={app?.id} onSelect={id => { setSelected(id); setMenu(false); void launch(id as 'files' | 'code'); }} anchor={<div className={css.split}>
      <button type="button" disabled={!app || busy} aria-label={tx('agent:desktop.open')} title={app?.label ?? tx('agent:desktop.applications')} onClick={() => void launch()}>{app?.id === 'code' ? <IconCodeOutline16/> : <IconFolderOpenOutline16/>}</button><button type="button" disabled={!apps.length || busy} aria-label={tx('agent:desktop.choose')} aria-haspopup="menu" aria-expanded={menu} onClick={() => setMenu(!menu)}><IconChevronDownOutline14 size={10}/></button>
    </div>}/>{error && <span role="alert" className={css.launchError}>{error}</span>}
  </div>;
}

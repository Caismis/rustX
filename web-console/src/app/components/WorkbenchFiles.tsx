/* Copyright (c) 2026 DeepSeek. MIT. Adapted from FilesBody and TextPreview; see PROVENANCE.md. */
import { DocumentBody } from './workbench-documents/DocumentBody';
import { documentFailure } from './documents/errors';
import { viewersForPath, loadsBytes, type Viewer } from './workbench-documents/definitions';
import type { PreviewViewState } from '../../presentation/right-panel/preview-view-state';
import { useCallback, useEffect, useLayoutEffect, useReducer, useRef, useState, useSyncExternalStore } from 'react';
import { PreviewContentCache, PreviewContentOwner, type PreviewObservation } from '../../client/preview-content';
import { FILE_PREVIEW_LIMITS, isFileData } from '../../../shared/file-preview';
import { Button } from '../../presentation/primitives/Button';
import { useViewScroll } from '../../presentation/right-panel/use-view-scroll';
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
export type WorkbenchCall = ((request: WorkbenchRequest, signal?: AbortSignal) => Promise<WorkbenchResult>) & { observe?: (path: string, changed: () => void, signal: AbortSignal) => Promise<PreviewObservation> };
/** Logical tab state; only inert layout/view metadata is persisted. */
export interface FileTabView { line?: number; focusRevision?: number; expanded: Set<string>; levels: Map<string, WorkbenchResult>; cwd: string; revision: number; mode?: Viewer; document?: PreviewViewState; top: number; left: number; wrap: boolean; cache: PreviewContentCache; current(): boolean; preview?: { call: WorkbenchCall; path: string; owner: PreviewContentOwner; dispose(): void } }
export const fileTabView = (cache = new PreviewContentCache(), current = () => true): FileTabView => ({ expanded: new Set(), levels: new Map(), cwd: '', revision: 0, top: 0, left: 0, wrap: true, cache, current });
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
  const tx = useTranslation();
  const [wrap, setWrap] = useState(view.wrap);
  const [mode, setMode] = useState<Viewer>(view.mode ?? viewersForPath(path)[0]), [menu, setMenu] = useState(false);
  if (!view.preview || view.preview.path !== path) {
    view.preview?.dispose();
    const lifetime = new AbortController();
    const read = async (request: WorkbenchRequest, signal: AbortSignal) => {
      const value = await view.preview!.call(request, signal); lifetime.signal.throwIfAborted();
      if (!view.current()) throw new Error('obsolete');
      const result = value.preview;
      if (!result || !isFileData(result.data) || !result.version || result.data.byteLength > FILE_PREVIEW_LIMITS.convertedPdfBytes) throw new Error('malformed');
      view.cwd = value.cwd ?? ''; return result;
    };
    const owner = new PreviewContentOwner({ current: () => !lifetime.signal.aborted && view.current(), immutable: false,
      page: async (offset, signal) => {
        const result = await read({ kind: 'preview', path, read: { kind: 'text', offset, limit: FILE_PREVIEW_LIMITS.pageLines } }, signal);
        if (result.offset !== offset || !Number.isSafeInteger(result.lines) || result.lines! < 0 || result.lines! > FILE_PREVIEW_LIMITS.pageLines || !result.eof && !result.lines || result.data.byteLength > FILE_PREVIEW_LIMITS.pageBytes) throw new Error('malformed');
        return { text: new TextDecoder('utf-8', { fatal: true }).decode(result.data), version: result.version, offset, lines: result.lines!, eof: result.eof };
      }, bytes: async signal => {
        const result = await read({ kind: 'preview', path, read: { kind: 'bytes' } }, signal);
        if (result.offset !== 0 || !result.eof || result.data.byteLength !== result.bytes || result.data.byteLength > FILE_PREVIEW_LIMITS.fileBytes) throw new Error('malformed');
        return result;
      }, document: (_extension, signal) => read({ kind: 'office', path }, signal),
      probe: async signal => (await read({ kind: 'preview', path, read: { kind: 'bytes', range: { offset: 0, length: 1 } } }, signal)).version,
      observe: call.observe ? (signal, changed) => view.preview!.call.observe!(path, changed, signal) : undefined });
    view.preview = { call, path, owner, dispose: () => { lifetime.abort(); view.cache.release(owner); owner.dispose(); } };
  }
  view.preview.call = call;
  const owner = view.preview.owner, value = useSyncExternalStore(owner.subscribe, owner.getSnapshot), error = value.error;
  const failure = error === undefined ? undefined : documentFailure(error);
  const loaded = value.text !== undefined || value.data !== undefined;
  const reading = useViewScroll(view.top, view.left, (top, left) => { view.top = top; view.left = left; }, `${mode}:${loaded}:${value.loadedLines}`), scroll = reading.ref;
  useEffect(() => {
    view.cache.release(owner); owner.activate();
    return () => { owner.deactivate(); view.cache.retain(owner); };
  }, [owner, view]);
  useEffect(() => { if (mode) owner.ensure(mode === 'office' ? path.toLowerCase().endsWith('.docx') ? 'docx' : 'pptx' : loadsBytes(mode) ? 'bytes' : 'pages', Math.max(view.document?.textLoadedLines ?? 0, view.line ?? 0)); }, [owner, mode, path]);
  useLayoutEffect(() => { const el = scroll.current; if (!el || view.line === undefined || mode !== 'code' || !value.page?.eof && value.loadedLines < view.line) return; el.scrollTop = Math.max(0, view.line - 1) * parseFloat(getComputedStyle(el.querySelector('.line') ?? el).lineHeight); view.top = el.scrollTop; view.line = undefined; }, [view, loaded, mode, value.loadedLines]);
  useLayoutEffect(() => {
    const node = scroll.current; if (!node || mode !== 'code') return;
    const changed = () => {
      reading.onPosition(node);
      if (value.page && !value.page.eof && !value.loading && error === undefined && node.scrollHeight - node.scrollTop - node.clientHeight < 160) owner.more();
    };
    node.addEventListener('scroll', changed); return () => node.removeEventListener('scroll', changed);
  }, [mode, loaded, value.page, value.loading, error, owner]);
  useEffect(() => { if (value.text !== undefined) view.document = { ...view.document, textLoadedLines: value.loadedLines }; }, [view, value.text, value.loadedLines]);
  const [lifetime] = useState(() => new AbortController());
  useEffect(() => () => lifetime.abort(), [lifetime]);
  const bytes = value.data ?? new Uint8Array();
  const fullPath = view.cwd ? `${view.cwd.replace(/\/$/, '')}/${path}` : path;
  const more = value.page && !value.page.eof && <Button disabled={value.loading} onClick={() => owner.more()}>{tx(value.loading ? 'artifacts:document.loading' : 'artifacts:artifact-preview.load-more')}</Button>;
  return <section className={css.root} data-workbench-file={path}>
    <div className={css.header}><PathLabel path={fullPath} className={css.path}/>{viewersForPath(path).length > 1 && <Menu open={menu} onClose={() => setMenu(false)} align="end" dense autoFocus selectedId={mode} items={viewersForPath(path).map(id => ({ id, label: tx(`artifacts:workbench.${id}`) }))} onSelect={id => { view.mode = id as Viewer; setMode(view.mode); setMenu(false); }} anchor={<button type="button" className={`${css.tool} ${css.viewer}`} aria-label={tx('artifacts:workbench.viewer')} aria-haspopup="menu" aria-expanded={menu} onClick={() => setMenu(!menu)}>{tx(`artifacts:workbench.${mode}`)}</button>}/>}<button hidden={mode !== 'text' && mode !== 'code'} type="button" className={css.tool} aria-label={tx('artifacts:artifact-preview.wrap-lines')} aria-pressed={wrap} title={tx('artifacts:artifact-preview.wrap-lines')} onClick={() => { view.wrap = !wrap; setWrap(!wrap); }}><IconWrapLinesOutline16/></button><Reload reload={() => owner.refresh()}/><OpenFileApp call={call} path={path} directory={false}/></div>
    {failure !== undefined && <div className={css.note} role="alert"><p>{tx(`artifacts:document.${failure}`)}</p>{!['source_missing','source_unavailable','authorization_revoked','obsolete'].includes(failure) && !(failure === 'too_large' && value.text === undefined) && <Button onClick={() => owner.retry()}>{tx('artifacts:artifact-preview.retry-preview')}</Button>}</div>}
    {!owner.automaticRefresh && <p className={css.note} role="note">{tx('artifacts:document.manual-refresh')}</p>}
    {value.text === '' && value.page?.eof && <p className={css.note}>{tx('artifacts:document.empty')}</p>}
    {!mode ? <p className={css.note}>{tx('artifacts:artifact-preview.this-artifact-has-no-supported-inline-viewer')}</p> : !loaded ? error === undefined && <p className={css.note} role="status">{tx('artifacts:workbench.loading')}</p> : mode !== 'text' && mode !== 'code' ? <div ref={scroll} className={css.document} onScroll={reading.onScroll}><DocumentBody key={mode} call={call} onOpen={onOpen} mode={mode} text={value.text ?? ''} bytes={bytes} path={path} view={view} signal={lifetime.signal} retry={() => owner.retry()}/>{more}</div> : mode === 'code' ? <div className={css.renderer} data-wrap={wrap}><CodeBlock className={css.code} contentRef={scroll} code={value.text ?? ''} lang={languageForPath(path)} lineNumbers copyLabel={tx('common:search-block.copy')} copiedLabel={tx('common:search-block.copied')} toolbarLabel={tx('artifacts:workbench.code')}/>{more}</div> : <div ref={scroll} className={css.plainText} data-wrap={wrap} onScroll={event => {
      const node = event.currentTarget; reading.onScroll(event);
      if (value.page && !value.page.eof && !value.loading && error === undefined && node.scrollHeight - node.scrollTop - node.clientHeight < 160) owner.more();
    }}><pre>{value.text}</pre>{more}</div>}
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

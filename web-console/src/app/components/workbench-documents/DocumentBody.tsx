/* Copyright (c) 2026 DeepSeek. MIT. MarkdownBody and ImageBody adapted; see PROVENANCE.md. */
import { lazy, Suspense, useEffect, useMemo, useState } from 'react';
import { MarkdownText } from '../../../presentation/markdown/MarkdownText';
import { useLocale, useTranslation } from '../../../locale/react';
import { splitFrontmatter } from './frontmatter';
import { FrontmatterFields } from './frontmatter-fields';
import { ZoomViewport, zoomSurfaceClass } from './zoom/ZoomViewport';
import { FIT_WIDTH } from './zoom/types';
import { excelFormat } from './excel/format';
import { en, zh } from '../../../locale/dictionaries/workbench-excel';
import { createBasicHtmlDocument } from './basic-document';
import { DocumentResources } from '../../../presentation/markdown/DocumentResources';
import { relativeDocumentPath, WorkspaceImage } from './resources';
import type { FileTabView, WorkbenchCall } from '../WorkbenchFiles';
import type { Viewer } from './definitions';
import css from './MarkdownBody.module.css';
const Excel = lazy(() => import('./excel/excel').then(m => ({ default: m.ExcelBody })));
const Pdf = lazy(() => import('../documents/PdfPreview'));
const limits = { maxBytes: 16 * 1024 * 1024, maxCells: 200000, timeoutMs: 15000 };
export function DocumentBody({ mode, text, bytes, path, view, signal, retry, call, onOpen }: {mode: Viewer;text: string;bytes: Uint8Array<ArrayBuffer>;path: string;view: FileTabView;signal: AbortSignal;retry: () => void; call:WorkbenchCall;onOpen:(path:string)=>void}) {
  const tx = useTranslation(), locale = useLocale();
  const [state,setState] = useState(view.document ?? {});
  const change = (patch: typeof state) => { view.document = {...view.document,...patch}; setState(view.document); };
  const source = useMemo(() => mode === 'html' ? createBasicHtmlDocument(new TextEncoder().encode(text)) : '', [mode,text]);
  const split = useMemo(() => splitFrontmatter(text), [text]);
  const loading = <p role="status">{tx('artifacts:workbench.loading')}</p>;
  if (mode === 'markdown') return <DocumentResources.Provider value={{image:(url,alt)=>{const resolved=relativeDocumentPath(path,url);return resolved?<WorkspaceImage key={resolved} path={resolved} alt={alt} call={call}/>:<span>{alt}</span>;},link:(url,children)=>{const resolved=relativeDocumentPath(path,url);return resolved?<a href={url} onClick={event=>{event.preventDefault();onOpen(resolved);}}>{children}</a>:undefined;}}}><div className={css.document} data-document-markdown>
    {split && <div className={css.frontmatter} data-document-frontmatter><FrontmatterFields source={split.source} fallback={<pre className={css.frontmatterSource}>{split.source}</pre>}/></div>}
    <MarkdownText text={split?.body ?? text} streaming={false}/>
  </div></DocumentResources.Provider>;
  if (mode === 'html') return <iframe style={{border:0,width:'100%',height:'100%'}} title="HTML" sandbox="" referrerPolicy="no-referrer" srcDoc={source}/>;
  if (mode === 'image') return <ImageBody bytes={bytes} path={path} view={view} signal={signal}/>;
  if (mode === 'pdf' || mode === 'office') return <Suspense fallback={loading}><Pdf bytes={bytes} signal={signal} retry={retry} viewState={state} onViewStateChange={change}/></Suspense>;
  if (mode === 'excel') {
    const dict = locale.active === 'zh' ? zh : en;
    return <Suspense fallback={loading}><Excel content={{kind:'bytes',data:bytes}} format={excelFormat(path)} limits={limits} loading={loading} t={(key,params) => Object.entries(params ?? {}).reduce((value,[name,replacement])=>value.replaceAll(`{${name}}`,String(replacement)),dict[key])}/></Suspense>;
  }
  return null;
}
function ImageBody({bytes,path,view,signal}: {bytes:Uint8Array<ArrayBuffer>;path:string;view:FileTabView;signal:AbortSignal}) {
  const tx = useTranslation(), [url,setUrl] = useState(''), [width,setWidth] = useState<number>(), [zoom,setZoom] = useState(view.zoom ?? FIT_WIDTH);
  useEffect(()=>{ const mime = path.toLowerCase().endsWith('.svg') ? 'image/svg+xml' : `image/${path.split('.').at(-1)}`; const next = URL.createObjectURL(new Blob([bytes],{type:mime}));setUrl(next);return ()=>URL.revokeObjectURL(next); },[bytes,path]);
  return <ZoomViewport preference={zoom} intrinsicWidth={width} signal={signal} scrollportRef={()=>{}} onPreference={preference=>{view.zoom=preference;setZoom(preference);}} labels={{controls:tx('artifacts:document.zoom'),menu:tx('artifacts:document.zoom'),out:tx('artifacts:workbench.zoom-out'),into:tx('artifacts:workbench.zoom-in'),fitWidth:tx('artifacts:workbench.fit-width'),value:percent=>`${percent}%`}}>
    {url && <img src={url} alt={path} className={zoomSurfaceClass} data-document-zoom-surface style={{'--document-zoom-width': `${width ?? 1}px` } as import('react').CSSProperties} onLoad={event=>setWidth(event.currentTarget.naturalWidth)}/>}
  </ZoomViewport>;
}

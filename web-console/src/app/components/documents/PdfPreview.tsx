/* Copyright (c) 2026 DeepSeek. MIT. PDF page presentation adapted; see PROVENANCE.md. */
import { memo, useCallback, useEffect, useRef, useState, type CSSProperties } from 'react';
import { PdfDocumentOwner, type PdfPageSize } from '../../../client/pdf-document';
import { PDF_LIMITS } from '../../../client/pdf-limits';
import { useTranslation } from '../../../locale/react';
import { documentFailure } from './errors';
import type { PreviewViewStateProps } from '../../../presentation/right-panel/preview-view-state';
import { ZoomViewport, zoomSurfaceClass } from '../workbench-documents/zoom/ZoomViewport';
import { FIT_WIDTH, type ZoomPreference } from '../workbench-documents/zoom/types';
import css from './PdfPreview.module.css';

type Loaded = { bytes: Uint8Array<ArrayBuffer>; owner: PdfDocumentOwner; sizes: readonly PdfPageSize[] };
export default function PdfPreview(props: PreviewViewStateProps & { bytes: Uint8Array<ArrayBuffer>; signal: AbortSignal; retry: () => void }) {
  const tx = useTranslation(), [loaded, setLoaded] = useState<Loaded>(), [error, setError] = useState('');
  const [renderZoom, setRenderZoom] = useState(() => props.viewState.pdfZoom?.kind === 'fixed' ? props.viewState.pdfZoom.scale : 1);
  const current = useRef(props); current.current = props;
  const scroll = useRef<HTMLElement | null>(null);
  const preference = props.viewState.pdfZoom ?? FIT_WIDTH;
  const saveScroll = useCallback(() => {
    const node = scroll.current;
    if (node && !current.current.signal.aborted) current.current.onViewStateChange({ pdfScrollTop: node.scrollTop, pdfScrollLeft: node.scrollLeft });
  }, []);
  const attachScroll = useCallback((node: HTMLElement | null) => {
    scroll.current?.removeEventListener('scroll', saveScroll);
    scroll.current = node;
    if (!node) return;
    node.dataset.previewScroll = 'pdf';
    node.scrollTop = current.current.viewState.pdfScrollTop ?? 0;
    node.scrollLeft = current.current.viewState.pdfScrollLeft ?? 0;
    node.addEventListener('scroll', saveScroll, { passive: true });
  }, [saveScroll]);
  const setPreference = useCallback((pdfZoom: ZoomPreference) => current.current.onViewStateChange({ pdfZoom }), []);
  const failed = useCallback((code: string) => setError(code), []);
  useEffect(() => {
    const { bytes, signal } = props;
    if (signal.aborted) return;
    let live = true, owner: PdfDocumentOwner | undefined;
    const fail = (code: string) => { clearTimeout(timer); if (live) { setError(code); owner?.dispose(); } };
    const timer = setTimeout(() => fail('parser_timeout'), PDF_LIMITS.loadWatchdogMs);
    const retire = () => { live = false; clearTimeout(timer); owner?.dispose(); };
    signal.addEventListener('abort', retire, { once: true });
    setLoaded(undefined); setError('');
    try {
      owner = new PdfDocumentOwner(bytes, fail);
      const owned = owner;
      void owned.pageSizes.then(sizes => { clearTimeout(timer); if (live) setLoaded({ bytes, owner: owned, sizes }); }, cause => fail(documentFailure(cause)));
    } catch (cause) { fail(documentFailure(cause)); }
    return () => { signal.removeEventListener('abort', retire); retire(); };
  }, [props.bytes, props.signal]);
  if (error) return <div role="alert" className={css.status}><span>{tx(`artifacts:document.${documentFailure(new Error(error))}`)}</span><button onClick={props.retry}>{tx('artifacts:artifact-preview.retry-preview')}</button></div>;
  if (!loaded || loaded.bytes !== props.bytes) return <p className={css.status} role="status">{tx('artifacts:document.loading')}</p>;
  return <ZoomViewport preference={preference} intrinsicWidth={loaded.sizes[0]?.width} horizontalInset={24}
    labels={{ controls: tx('artifacts:document.zoom'), menu: tx('artifacts:document.zoom'), out: tx('artifacts:workbench.zoom-out'), into: tx('artifacts:workbench.zoom-in'), fitWidth: tx('artifacts:workbench.fit-width'), value: percent => `${percent}%` }}
    signal={props.signal} scrollportRef={attachScroll} onPreference={setPreference} onRenderZoom={setRenderZoom}>
    <section className={css.body} data-pdf-preview>{loaded.sizes.map((size, index) => <PdfPage key={index} owner={loaded.owner} page={index + 1} size={size} zoom={renderZoom} root={scroll} signal={props.signal} failed={failed}/>)}</section>
  </ZoomViewport>;
}

const PdfPage = memo(function PdfPage({ owner, page, size, zoom, root, signal, failed }: {
  owner: PdfDocumentOwner; page: number; size: PdfPageSize; zoom: number; root: React.RefObject<HTMLElement | null>; signal: AbortSignal; failed: (code: string) => void;
}) {
  const tx = useTranslation(), host = useRef<HTMLDivElement>(null), canvas = useRef<HTMLCanvasElement>(null), text = useRef<HTMLDivElement>(null);
  const [visible, setVisible] = useState(page === 1), [ready, setReady] = useState(false), [error, setError] = useState(''), [attempt, retry] = useState(0);
  useEffect(() => {
    const node = host.current!;
    if (typeof IntersectionObserver === 'undefined') { setVisible(true); return; }
    const observer = new IntersectionObserver(entries => setVisible(entries.some(entry => entry.isIntersecting)), { root: root.current, rootMargin: '100% 0px' });
    observer.observe(node);
    return () => observer.disconnect();
  }, [root]);
  useEffect(() => {
    const node = text.current!;
    const resize = () => node.style.setProperty('--pdf-text-scale', String(node.getBoundingClientRect().width / size.width));
    resize();
    const observer = new ResizeObserver(resize); observer.observe(node);
    return () => observer.disconnect();
  }, [size.width]);
  useEffect(() => {
    const bitmap = canvas.current!, layer = text.current!;
    if (!visible || signal.aborted) { owner.release(bitmap); setReady(false); return; }
    const abort = new AbortController();
    let timer: ReturnType<typeof setTimeout>;
    const retire = () => { clearTimeout(timer); abort.abort(); owner.release(bitmap); };
    signal.addEventListener('abort', retire, { once: true });
    setError('');
    timer = setTimeout(() => { if (!abort.signal.aborted) { abort.abort(); owner.dispose(); failed('parser_timeout'); } }, PDF_LIMITS.renderWatchdogMs);
    void owner.render(page, zoom, bitmap, layer, abort.signal).then(() => { if (!abort.signal.aborted) setReady(true); }, cause => { if (!abort.signal.aborted) setError(documentFailure(cause)); }).finally(() => clearTimeout(timer));
    return () => { clearTimeout(timer); signal.removeEventListener('abort', retire); abort.abort(); };
  }, [owner, page, size, visible, zoom, signal, attempt, failed]);
  useEffect(() => () => { owner.release(canvas.current!); }, [owner]);
  return <div ref={host} className={css.page} data-pdf-page={page}>
    <div className={`${css.surface} ${zoomSurfaceClass}`} data-document-zoom-surface style={{ '--document-zoom-width': `${size.width}px`, aspectRatio: `${size.width} / ${size.height}` } as CSSProperties}>
      {!ready && !error && <div className={css.placeholder} role={visible ? 'status' : undefined} aria-label={visible ? tx('artifacts:document.loading') : undefined}/>}
      <canvas ref={canvas} className={css.canvas} role="img" aria-label={`${tx('artifacts:document.page')} ${page}`}/>
      <div ref={text} className={css.text} data-pdf-text/>
    </div>
    {error && <div role="alert" className={css.status}><span>{tx(`artifacts:document.${documentFailure(new Error(error))}`)}</span><button onClick={() => retry(n => n + 1)}>{tx('artifacts:artifact-preview.retry-preview')}</button></div>}
  </div>;
});

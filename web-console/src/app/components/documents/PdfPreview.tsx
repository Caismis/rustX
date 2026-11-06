/* Copyright (c) 2026 DeepSeek. MIT. Adapted from Harness pdf/pdf.tsx; native resource ownership retained. See PROVENANCE.md. */
import { memo, useCallback, useEffect, useLayoutEffect, useRef, useState, type CSSProperties } from 'react';
import { PdfDocumentOwner, type PdfPageSize } from '../../../client/pdf-document';
import { PDF_LIMITS } from '../../../client/pdf-limits';
import { useTranslation } from '../../../locale/react';
import { documentFailure } from './errors';
import type { PreviewViewStateProps } from '../../../presentation/right-panel/preview-view-state';
import { useViewScroll } from '../../../presentation/right-panel/use-view-scroll';
import { ZoomViewport, zoomSurfaceClass } from '../workbench-documents/zoom/ZoomViewport';
import { FIT_WIDTH, type ZoomPreference } from '../workbench-documents/zoom/types';
import css from './PdfPreview.module.css';

type Loaded = { bytes: Uint8Array<ArrayBuffer>; owner: PdfDocumentOwner; pages: number };
export default function PdfPreview(props: PreviewViewStateProps & { bytes: Uint8Array<ArrayBuffer>; signal: AbortSignal; retry: () => void; maxBytes?: number }) {
  const tx = useTranslation(), [loaded, setLoaded] = useState<Loaded>(), [error, setError] = useState('');
  const [pageWidth, setPageWidth] = useState<number>();
  const [renderZoom, setRenderZoom] = useState(() => props.viewState.pdfZoom?.kind === 'fixed' ? props.viewState.pdfZoom.scale : 1);
  const current = useRef(props); current.current = props;
  const preference = props.viewState.pdfZoom ?? FIT_WIDTH;
  const reading = useViewScroll<HTMLElement>(props.viewState.pdfScrollTop ?? 0, props.viewState.pdfScrollLeft ?? 0,
    (pdfScrollTop, pdfScrollLeft) => { if (!current.current.signal.aborted) current.current.onViewStateChange({ pdfScrollTop, pdfScrollLeft }); }, loaded);
  const latestReading = useRef(reading); latestReading.current = reading;
  const saveScroll = useCallback(() => {
    const node = latestReading.current.ref.current;
    if (node) latestReading.current.onPosition(node);
  }, []);
  const attachScroll = useCallback((node: HTMLElement | null) => {
    const held = latestReading.current;
    held.ref.current?.removeEventListener('scroll', saveScroll);
    held.ref.current = node;
    if (!node) return;
    node.dataset.previewScroll = 'pdf'; held.restore();
    node.addEventListener('scroll', saveScroll, { passive: true });
  }, [saveScroll]);
  const setPreference = useCallback((pdfZoom: ZoomPreference) => current.current.onViewStateChange({ pdfZoom }), []);
  useEffect(() => {
    const { bytes, signal } = props;
    if (signal.aborted) return;
    let live = true, owner: PdfDocumentOwner | undefined;
    const fail = (code: string) => { clearTimeout(timer); if (live) { setError(code); owner?.dispose(); } };
    const timer = setTimeout(() => fail('parser_timeout'), PDF_LIMITS.loadWatchdogMs);
    const retire = () => { live = false; clearTimeout(timer); owner?.dispose(); };
    signal.addEventListener('abort', retire, { once: true });
    setLoaded(undefined); setError(''); setPageWidth(undefined);
    try {
      owner = new PdfDocumentOwner(bytes, fail, props.maxBytes);
      const owned = owner;
      // As in Harness, opening the document exposes its page count immediately.
      // Only viewport-local pages enter the renderer; there is no all-pages scan.
      void owned.document.then(document => { clearTimeout(timer); if (live) setLoaded({ bytes, owner: owned, pages: document.numPages }); }, cause => fail(documentFailure(cause)));
    } catch (cause) { fail(documentFailure(cause)); }
    return () => { signal.removeEventListener('abort', retire); retire(); };
  }, [props.bytes, props.signal, props.maxBytes]);
  if (error) return <div role="alert" className={css.status}><span>{tx(`artifacts:document.${documentFailure(new Error(error))}`)}</span><button onClick={props.retry}>{tx('artifacts:artifact-preview.retry-preview')}</button></div>;
  if (!loaded || loaded.bytes !== props.bytes) return <p className={css.status} role="status">{tx('artifacts:document.loading')}</p>;
  return <ZoomViewport preference={preference} intrinsicWidth={pageWidth} horizontalInset={24}
    labels={{ controls: tx('artifacts:document.zoom'), menu: tx('artifacts:document.zoom'), out: tx('artifacts:workbench.zoom-out'), into: tx('artifacts:workbench.zoom-in'), fitWidth: tx('artifacts:workbench.fit-width'), value: percent => `${percent}%` }}
    signal={props.signal} scrollportRef={attachScroll} onPreference={setPreference} onRenderZoom={setRenderZoom}>
    <section className={css.body} data-pdf-preview>{Array.from({ length: loaded.pages }, (_, index) => <PdfPage key={index} owner={loaded.owner} page={index + 1} zoom={renderZoom} root={reading.ref} signal={props.signal}
      onWidth={index === 0 ? setPageWidth : undefined} onGeometry={reading.restore}/>)}</section>
  </ZoomViewport>;
}

const PdfPage = memo(function PdfPage({ owner, page, zoom, root, signal, onWidth, onGeometry }: {
  owner: PdfDocumentOwner; page: number; zoom: number; root: React.RefObject<HTMLElement | null>; signal: AbortSignal;
  onWidth?: (width: number) => void; onGeometry: () => void;
}) {
  const tx = useTranslation(), host = useRef<HTMLDivElement>(null), canvas = useRef<HTMLCanvasElement>(null), text = useRef<HTMLDivElement>(null);
  const [visible, setVisible] = useState(page === 1), [ready, setReady] = useState(false), [error, setError] = useState(''), [attempt, retry] = useState(0);
  const [size, setSize] = useState<PdfPageSize>();
  useEffect(() => {
    const node = host.current!;
    if (typeof IntersectionObserver === 'undefined') { setVisible(true); return; }
    const observer = new IntersectionObserver(entries => setVisible(entries.some(entry => entry.isIntersecting)), { root: root.current, rootMargin: '100% 0px' });
    observer.observe(node);
    return () => observer.disconnect();
  }, [root]);
  useLayoutEffect(() => {
    if (!size) return;
    const node = text.current!;
    const resize = () => node.style.setProperty('--pdf-text-scale', String(node.getBoundingClientRect().width / size.width));
    resize(); onGeometry();
    const observer = new ResizeObserver(resize); observer.observe(node);
    return () => observer.disconnect();
  }, [size, onGeometry]);
  useEffect(() => {
    const bitmap = canvas.current!, layer = text.current!;
    if (!visible || signal.aborted) { owner.release(bitmap); setReady(false); return; }
    const abort = new AbortController();
    const retire = () => { abort.abort(); owner.release(bitmap); };
    signal.addEventListener('abort', retire, { once: true });
    setError('');
    void owner.render(page, zoom, bitmap, layer, abort.signal).then(size => {
      if (!abort.signal.aborted) { setSize(previous => previous?.width === size.width && previous.height === size.height ? previous : size); onWidth?.(size.width); setReady(true); }
    }, cause => { if (!abort.signal.aborted) setError(documentFailure(cause)); });
    return () => { signal.removeEventListener('abort', retire); abort.abort(); };
  }, [owner, page, visible, zoom, signal, attempt, onWidth]);
  useEffect(() => () => { owner.release(canvas.current!); }, [owner]);
  return <div ref={host} className={css.page} data-pdf-page={page}>
    {!size && !error && <div className={css.unmeasured} role={visible ? 'status' : undefined} aria-label={visible ? tx('artifacts:document.loading') : undefined}/>}
    <div className={`${css.surface} ${zoomSurfaceClass}`} data-document-zoom-surface hidden={!size} style={size ? { '--document-zoom-width': `${size.width}px`, aspectRatio: `${size.width} / ${size.height}` } as CSSProperties : undefined}>
      {!ready && !error && <div className={css.placeholder} role={visible ? 'status' : undefined} aria-label={visible ? tx('artifacts:document.loading') : undefined}/>}
      <canvas ref={canvas} className={css.canvas} role="img" aria-label={`${tx('artifacts:document.page')} ${page}`}/>
      <div ref={text} className={css.text} data-pdf-text/>
    </div>
    {error && <div role="alert" className={css.status}><span>{tx(`artifacts:document.${documentFailure(new Error(error))}`)}</span><button onClick={() => retry(n => n + 1)}>{tx('artifacts:artifact-preview.retry-preview')}</button></div>}
  </div>;
});

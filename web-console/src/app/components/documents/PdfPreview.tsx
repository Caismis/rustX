import { useEffect, useRef, useState } from 'react';
import { PdfDocumentOwner } from '../../../client/pdf-document';
import { PDF_LIMITS } from '../../../client/pdf-limits';
import { useTranslation } from '../../../locale/react';
import { documentFailure } from './errors';
import './pdf-text.css';
import type { PreviewViewStateProps } from '../../../presentation/right-panel/preview-view-state';
import { useViewScroll } from '../../../presentation/right-panel/use-view-scroll';

export default function PdfPreview({ bytes, signal, retry, viewState, onViewStateChange }: PreviewViewStateProps & { bytes: Uint8Array<ArrayBuffer>; signal: AbortSignal; retry: () => void }) {
  const tx = useTranslation(), [owner, setOwner] = useState<PdfDocumentOwner>(), [pages, setPages] = useState(0);
  const [error, setError] = useState(''), [loading, setLoading] = useState(true);
  const number = Math.min(Math.max(1, viewState.pdfPage ?? 1), pages || PDF_LIMITS.pdfPages);
  const zoom = [0.5, 1, 1.5, 2].includes(viewState.pdfZoom ?? 1) ? viewState.pdfZoom ?? 1 : 1;
  const canvas = useRef<HTMLCanvasElement>(null), text = useRef<HTMLDivElement>(null);
  const scroll = useViewScroll(viewState.pdfScrollTop ?? 0, viewState.pdfScrollLeft ?? 0,
    (pdfScrollTop, pdfScrollLeft) => { if (!signal.aborted) onViewStateChange({ pdfScrollTop, pdfScrollLeft }); }, bytes);
  const restoreScroll = scroll.restore;
  const selectPage = (pdfPage: number) => onViewStateChange({ pdfPage, pdfScrollTop: 0, pdfScrollLeft: 0 });
  useEffect(() => {
    if (signal.aborted) return;
    let live = true, owned: PdfDocumentOwner | undefined;
    const fail = (code: string) => { clearTimeout(timer); if (live) { setError(code); setLoading(false); owned?.dispose(); } };
    const timer = setTimeout(() => fail('parser_timeout'), PDF_LIMITS.loadWatchdogMs);
    const retire = () => { live = false; clearTimeout(timer); owned?.dispose(); };
    signal.addEventListener('abort', retire, { once: true });
    setLoading(true); setError(''); setPages(0); setOwner(undefined);
    try {
      owned = new PdfDocumentOwner(bytes, fail); setOwner(owned);
      void owned.document.then(doc => { clearTimeout(timer); if (live) setPages(doc.numPages); }, cause => { clearTimeout(timer); fail(documentFailure(cause)); });
    } catch (cause) { clearTimeout(timer); fail(documentFailure(cause)); }
    return () => { signal.removeEventListener('abort', retire); retire(); };
  }, [bytes, signal]);
  useEffect(() => {
    if (signal.aborted || !owner || !pages || !canvas.current || !text.current) return;
    const abort = new AbortController(), rendered = canvas.current, layer = text.current;
    setLoading(true);
    const timer = setTimeout(() => { if (!abort.signal.aborted) { setError('parser_timeout'); setLoading(false); abort.abort(); owner.dispose(); } }, PDF_LIMITS.renderWatchdogMs);
    const retire = () => { clearTimeout(timer); abort.abort(); rendered.width = 0; rendered.height = 0; layer.replaceChildren(); };
    signal.addEventListener('abort', retire, { once: true });
    void owner.render(number, zoom, rendered, layer, abort.signal).then(() => {
      if (!abort.signal.aborted) { restoreScroll(); setLoading(false); }
    }, cause => { if (!abort.signal.aborted) { setError(documentFailure(cause)); setLoading(false); owner.dispose(); } }).finally(() => clearTimeout(timer));
    return () => { signal.removeEventListener('abort', retire); retire(); };
  }, [owner, pages, number, zoom, restoreScroll, signal]);
  return <div className="document-view" ref={scroll.ref} onScroll={scroll.onScroll} data-preview-scroll="pdf"><div className="document-toolbar">
    <button disabled={number <= 1 || !!error || loading} onClick={() => selectPage(number - 1)}>{tx('artifacts:document.previous')}</button>
    <span>{tx('artifacts:document.page')} {number} / {pages}</span>
    <button disabled={number >= pages || !!error || loading} onClick={() => selectPage(number + 1)}>{tx('artifacts:document.next')}</button>
    <label>{tx('artifacts:document.zoom')}<select aria-label={tx('artifacts:document.zoom')} disabled={!!error || loading} value={zoom} onChange={event => onViewStateChange({ pdfZoom: +event.target.value, pdfScrollTop: 0, pdfScrollLeft: 0 })}>
      {[0.5, 1, 1.5, 2].map(value => <option key={value} value={value}>{value * 100}%</option>)}
    </select></label>
  </div>{loading && <p role="status">{tx('artifacts:document.loading')}</p>}
    {error && <div role="alert"><p>{tx(`artifacts:document.${documentFailure(new Error(error))}`)}</p><button onClick={retry}>{tx('artifacts:artifact-preview.retry-preview')}</button></div>}
    <div className="document-page" key={`${number}:${zoom}`}><canvas ref={canvas} aria-label={tx('artifacts:document.pdf')} /><div ref={text} className="textLayer" /></div>
  </div>;
}

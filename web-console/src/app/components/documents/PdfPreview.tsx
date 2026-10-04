import { useEffect, useRef, useState } from 'react';
import { PdfDocumentOwner } from '../../../client/pdf-document';
import { DOCUMENT_LIMITS } from '../../../client/document-types';
import { useTranslation } from '../../../locale/react';
import { documentFailure } from './errors';
import './pdf-text.css';

export default function PdfPreview({ bytes, retry }: { bytes: Uint8Array<ArrayBuffer>; retry: () => void }) {
  const tx = useTranslation(), [owner, setOwner] = useState<PdfDocumentOwner>(), [pages, setPages] = useState(0);
  const [number, setNumber] = useState(1), [zoom, setZoom] = useState(1), [error, setError] = useState(''), [loading, setLoading] = useState(true);
  const canvas = useRef<HTMLCanvasElement>(null), text = useRef<HTMLDivElement>(null);
  useEffect(() => {
    let live = true, owned: PdfDocumentOwner | undefined;
    const fail = (code: string) => { clearTimeout(timer); if (live) { setError(code); setLoading(false); owned?.dispose(); } };
    const timer = setTimeout(() => fail('parser_timeout'), DOCUMENT_LIMITS.timeout);
    setLoading(true); setError(''); setNumber(1); setZoom(1); setPages(0);
    try {
      owned = new PdfDocumentOwner(bytes, fail); setOwner(owned);
      void owned.document.then(doc => { clearTimeout(timer); if (live) setPages(doc.numPages); }, cause => { clearTimeout(timer); fail(documentFailure(cause)); });
    } catch (cause) { clearTimeout(timer); fail(documentFailure(cause)); }
    return () => { live = false; clearTimeout(timer); owned?.dispose(); };
  }, [bytes]);
  useEffect(() => {
    if (!owner || !pages || !canvas.current || !text.current) return;
    const abort = new AbortController(), rendered = canvas.current, layer = text.current;
    setLoading(true);
    const timer = setTimeout(() => { if (!abort.signal.aborted) { setError('parser_timeout'); setLoading(false); abort.abort(); owner.dispose(); } }, DOCUMENT_LIMITS.timeout);
    void owner.render(number, zoom, rendered, layer, abort.signal).then(() => {
      if (!abort.signal.aborted) setLoading(false);
    }, cause => { if (!abort.signal.aborted) { setError(documentFailure(cause)); setLoading(false); owner.dispose(); } }).finally(() => clearTimeout(timer));
    return () => { clearTimeout(timer); abort.abort(); rendered.width = 0; rendered.height = 0; layer.replaceChildren(); };
  }, [owner, pages, number, zoom]);
  return <div className="document-view"><div className="document-toolbar">
    <button disabled={number <= 1 || !!error || loading} onClick={() => setNumber(number - 1)}>{tx('artifacts:document.previous')}</button>
    <span>{tx('artifacts:document.page')} {number} / {pages}</span>
    <button disabled={number >= pages || !!error || loading} onClick={() => setNumber(number + 1)}>{tx('artifacts:document.next')}</button>
    <label>{tx('artifacts:document.zoom')}<select aria-label={tx('artifacts:document.zoom')} disabled={!!error || loading} value={zoom} onChange={event => setZoom(+event.target.value)}>
      {[0.5, 1, 1.5, 2].map(value => <option key={value} value={value}>{value * 100}%</option>)}
    </select></label>
  </div>{loading && <p role="status">{tx('artifacts:document.loading')}</p>}
    {error && <div role="alert"><p>{tx(`artifacts:document.${documentFailure(new Error(error))}`)}</p><button onClick={retry}>{tx('artifacts:artifact-preview.retry-preview')}</button></div>}
    <div className="document-page" key={`${number}:${zoom}`}><canvas ref={canvas} aria-label={tx('artifacts:document.pdf')} /><div ref={text} className="textLayer" /></div>
  </div>;
}

import { lazy, Suspense, useEffect, useState } from 'react';
import type { FilePreviewLease } from '../../../client/session-files';
import { DERIVED_PDF_MAX_BYTES, type DocumentKind, type DerivedDocument } from '../../../../shared/documents.ts';
import { useTranslation } from '../../../locale/react';
import { HtmlPreview } from './HtmlPreview';
import { WorkbookPreview } from './WorkbookPreview';
import { documentFailure } from './errors';
import './documents.css';
import type { PreviewViewStateProps } from '../../../presentation/right-panel/preview-view-state';
const PdfPreview = lazy(() => import('./PdfPreview'));

export function DocumentPreview({ kind, bytes, resources, retry, viewState, onViewStateChange }: PreviewViewStateProps & { kind: DocumentKind; bytes: Uint8Array<ArrayBuffer>; resources: FilePreviewLease; retry: () => void }) {
  const tx = useTranslation(), [derived, setDerived] = useState<DerivedDocument>(), [pdf, setPdf] = useState<Uint8Array<ArrayBuffer>>(), [error, setError] = useState('');
  useEffect(() => {
    const abort = new AbortController(); setDerived(undefined); setPdf(undefined); setError('');
    if (kind === 'pdf' || kind === 'html') return () => abort.abort();
    void resources.derive(kind, bytes, abort.signal).then(result => {
      if (abort.signal.aborted || !resources.current()) return;
      if (result.kind === 'pdf') {
        if (result.data.length > Math.ceil(DERIVED_PDF_MAX_BYTES / 3) * 4) throw new Error('too_large');
        const decoded = atob(result.data);
        if (decoded.length > DERIVED_PDF_MAX_BYTES) throw new Error('too_large');
        setPdf(Uint8Array.from(decoded, c => c.charCodeAt(0)));
      } else setDerived(result);
    }).catch(cause => { if (!abort.signal.aborted && resources.current()) setError(documentFailure(cause)); });
    return () => abort.abort();
  }, [bytes, kind, resources]);
  if (error) return <div role="alert"><p>{tx(`artifacts:document.${documentFailure(new Error(error))}`)}</p><button onClick={retry}>{tx('artifacts:artifact-preview.retry-preview')}</button></div>;
  if (kind === 'html') {
    try { new TextDecoder('utf-8', { fatal: true }).decode(bytes); }
    catch { return <p role="alert">{tx('artifacts:document.malformed')}</p>; }
    return <HtmlPreview bytes={bytes} viewState={viewState} onViewStateChange={onViewStateChange} />;
  }
  const loading = <p role="status">{tx('artifacts:document.loading')}</p>;
  if (kind === 'pdf' || pdf) return <>{pdf && <p>{tx('artifacts:document.fidelity')}</p>}<Suspense fallback={loading}><PdfPreview bytes={pdf ?? bytes} signal={resources.signal} retry={retry} viewState={viewState} onViewStateChange={onViewStateChange} /></Suspense></>;
  if (derived?.kind === 'xlsx') return <WorkbookPreview workbook={derived} viewState={viewState} onViewStateChange={onViewStateChange} />;
  return loading;
}

import { lazy, Suspense, useEffect, useState } from 'react';
import type { FilePreviewLease } from '../../../client/session-files';
import { DERIVED_PDF_MAX_BYTES, type DocumentKind } from '../../../../shared/documents.ts';
import { useTranslation } from '../../../locale/react';
import { HtmlPreview } from './HtmlPreview';
import { SpreadsheetPreview } from './SpreadsheetPreview';
import { documentFailure } from './errors';
import './documents.css';
import type { PreviewViewStateProps } from '../../../presentation/right-panel/preview-view-state';
import { isFileData } from '../../../../shared/file-preview.ts';
const PdfPreview = lazy(() => import('./PdfPreview'));

export function DocumentPreview({ kind, bytes, resources, retry, viewState, onViewStateChange }: PreviewViewStateProps & { kind: DocumentKind; bytes?: Uint8Array<ArrayBuffer>; resources: FilePreviewLease; retry: () => void }) {
  const tx = useTranslation(), [pdf, setPdf] = useState<Uint8Array<ArrayBuffer>>(), [error, setError] = useState('');
  useEffect(() => {
    const abort = new AbortController(); setPdf(undefined); setError('');
    if (kind === 'pdf' || kind === 'html' || kind === 'xlsx') return () => abort.abort();
    void resources.derive(kind, abort.signal).then(result => {
      if (abort.signal.aborted || !resources.current()) return;
      if (result.kind === 'pdf') {
        if (!isFileData(result.data) || result.data.byteLength > DERIVED_PDF_MAX_BYTES) throw new Error('too_large');
        setPdf(result.data);
      } else throw new Error('malformed');
    }).catch(cause => { if (!abort.signal.aborted && resources.current()) setError(documentFailure(cause)); });
    return () => abort.abort();
  }, [bytes, kind, resources]);
  if (error) return <div role="alert"><p>{tx(`artifacts:document.${documentFailure(new Error(error))}`)}</p><button onClick={retry}>{tx('artifacts:artifact-preview.retry-preview')}</button></div>;
  if (kind === 'html' && bytes) {
    try { new TextDecoder('utf-8', { fatal: true }).decode(bytes); }
    catch { return <p role="alert">{tx('artifacts:document.malformed')}</p>; }
    return <HtmlPreview bytes={bytes} viewState={viewState} onViewStateChange={onViewStateChange} />;
  }
  const loading = <p role="status">{tx('artifacts:document.loading')}</p>;
  if ((kind === 'pdf' && bytes) || pdf) return <Suspense fallback={loading}><PdfPreview bytes={(pdf ?? bytes)!} maxBytes={pdf ? DERIVED_PDF_MAX_BYTES : undefined} signal={resources.signal} retry={retry} viewState={viewState} onViewStateChange={onViewStateChange} /></Suspense>;
  if (kind === 'xlsx' && bytes) return <SpreadsheetPreview bytes={bytes} signal={resources.signal} viewState={viewState} onViewStateChange={onViewStateChange}/>;
  return loading;
}

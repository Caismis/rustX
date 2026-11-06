import { lazy, Suspense } from 'react';
import type { FilePreviewLease } from '../../../client/session-files';
import { DERIVED_PDF_MAX_BYTES, type DocumentKind } from '../../../../shared/documents.ts';
import { useTranslation } from '../../../locale/react';
import { HtmlPreview } from './HtmlPreview';
import { SpreadsheetPreview } from './SpreadsheetPreview';
import './documents.css';
import type { PreviewViewStateProps } from '../../../presentation/right-panel/preview-view-state';
const PdfPreview = lazy(() => import('./PdfPreview'));

export function DocumentPreview({ kind, bytes, derived = false, resources, retry, viewState, onViewStateChange }: PreviewViewStateProps & { kind: Exclude<DocumentKind, 'docx' | 'pptx'>; bytes: Uint8Array<ArrayBuffer>; derived?: boolean; resources: FilePreviewLease; retry: () => void }) {
  const tx = useTranslation();
  if (kind === 'html' && bytes) {
    try { new TextDecoder('utf-8', { fatal: true }).decode(bytes); }
    catch { return <p role="alert">{tx('artifacts:document.malformed')}</p>; }
    return <HtmlPreview bytes={bytes} viewState={viewState} onViewStateChange={onViewStateChange} />;
  }
  const loading = <p role="status">{tx('artifacts:document.loading')}</p>;
  if (kind === 'pdf') return <Suspense fallback={loading}><PdfPreview bytes={bytes} maxBytes={derived ? DERIVED_PDF_MAX_BYTES : undefined} signal={resources.signal} retry={retry} viewState={viewState} onViewStateChange={onViewStateChange} /></Suspense>;
  if (kind === 'xlsx' || kind === 'csv' || kind === 'tsv') return <SpreadsheetPreview format={kind} bytes={bytes} signal={resources.signal} viewState={viewState} onViewStateChange={onViewStateChange}/>;
  return loading;
}

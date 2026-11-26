import { LoadingIndicator } from '../../../presentation/primitives/LoadingIndicator';
import { lazy, Suspense } from 'react';
import type { FilePreviewLease } from '../../../client/session-files';
import { DERIVED_PDF_MAX_BYTES, type DocumentKind } from '../../../../shared/documents.ts';
import { useTranslation } from '../../../locale/react';
import { BasicHtmlFrame } from '../workbench-documents/html/HtmlBody';
import { SpreadsheetPreview } from './SpreadsheetPreview';
import type { PreviewViewStateProps } from '../../../presentation/right-panel/preview-view-state';
const PdfPreview = lazy(() => import('./PdfPreview'));

export function DocumentPreview({ kind, bytes, derived = false, resources, retry, viewState, onViewStateChange }: PreviewViewStateProps & { kind: Exclude<DocumentKind, 'docx' | 'pptx'>; bytes: Uint8Array<ArrayBuffer>; derived?: boolean; resources: FilePreviewLease; retry: () => void }) {
  const tx = useTranslation();
  if (kind === 'html') return <BasicHtmlFrame data={bytes}/>;
  const loading = <LoadingIndicator label={tx('artifacts:document.loading')}/>;
  if (kind === 'pdf') return <Suspense fallback={loading}><PdfPreview bytes={bytes} maxBytes={derived ? DERIVED_PDF_MAX_BYTES : undefined} signal={resources.signal} retry={retry} viewState={viewState} onViewStateChange={onViewStateChange} /></Suspense>;
  if (kind === 'xlsx' || kind === 'csv' || kind === 'tsv') return <SpreadsheetPreview format={kind} bytes={bytes} signal={resources.signal} viewState={viewState} onViewStateChange={onViewStateChange}/>;
  return loading;
}

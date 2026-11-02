import { lazy, Suspense } from 'react';
import { useLocale } from '../../../locale/react';
import { en, zh } from '../../../locale/dictionaries/geometry';
import type { PreviewViewStateProps } from '../../../presentation/right-panel/preview-view-state';

const GeometryPreview = lazy(() => import('./geometry/GeometryPreview'));
export function GeometryDocument(props: PreviewViewStateProps & { bytes: Uint8Array<ArrayBuffer>; name: string; copyId: string; signal: AbortSignal; current: () => boolean }) {
  const locale = useLocale();
  return <Suspense fallback={<p role="status">{(locale.active === 'zh' ? zh : en).loading}</p>}><GeometryPreview {...props}/></Suspense>;
}

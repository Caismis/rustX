import { lazy, Suspense } from 'react';
import { useLocale } from '../../../locale/react';
import { en, zh } from '../../../locale/dictionaries/geometry';
import type { GeometryPreviewProps } from './geometry/GeometryPreview';

const GeometryPreview = lazy(() => import('./geometry/GeometryPreview'));
export function GeometryDocument(props: GeometryPreviewProps) {
  const locale = useLocale();
  return <Suspense fallback={<p role="status">{(locale.active === 'zh' ? zh : en).loading}</p>}><GeometryPreview {...props}/></Suspense>;
}

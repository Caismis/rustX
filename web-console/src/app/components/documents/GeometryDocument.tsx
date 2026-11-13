import { useEffect, useState } from 'react';
import { useLocale } from '../../../locale/react';
import { en, zh } from '../../../locale/dictionaries/geometry';
import type { GeometryPreviewProps } from './geometry/GeometryPreview';
import { geometryFormat } from './geometry/format';
import { GeometryLoading } from './GeometryLoading';
import css from './GeometryDocument.module.css';

type Viewer = typeof import('./geometry/GeometryPreview').default;
let loaded: Viewer | undefined;
const loadGeometry = () => import('./geometry/GeometryPreview').then(module => (loaded = module.default));
// Explicit visible geometry demand only; overlap module loading with the already
// authorized source read, without opening a file or retaining a scene/Worker.
export function preloadGeometry(name: string) {
  const format = geometryFormat(name);
  void loadGeometry().catch(() => {});
  if (format === 'glb') void import('three/addons/loaders/GLTFLoader.js').catch(() => {});
  else if (format !== 'stl') void import('./geometry/cad-code').then(module => module.cadCode()).catch(() => {});
}
export type GeometryDocumentProps = Omit<GeometryPreviewProps, 'bytes'> & { bytes?: Uint8Array<ArrayBuffer> };
export function GeometryDocument(props: GeometryDocumentProps) {
  const locale = useLocale(), [Viewer, setViewer] = useState(() => loaded), [failed, setFailed] = useState(false);
  useEffect(() => {
    if (Viewer || props.signal.aborted) return;
    setFailed(false);
    let active = true;
    void loadGeometry().then(component => { if (active && !props.signal.aborted) setViewer(() => component); }, () => { if (active && !props.signal.aborted) setFailed(true); });
    return () => { active = false; };
  }, [Viewer, props.signal, props.copyId]);
  const t = locale.active === 'zh' ? zh : en;
  // Suspense's fallback throttle delayed resolved code by ~300 ms in production.
  // The explicit module state starts the admitted import as soon as code arrives.
  const { bytes, ...view } = props;
  return <div className={css.root}>
    {Viewer && bytes ? <Viewer {...view} bytes={bytes}/> : <p role={failed ? 'alert' : 'status'}>{failed ? t.invalid : <GeometryLoading stage="loading"/>}</p>}
  </div>;
}

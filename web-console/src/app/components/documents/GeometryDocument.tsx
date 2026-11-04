import { useEffect, useState } from 'react';
import { useLocale } from '../../../locale/react';
import { en, zh } from '../../../locale/dictionaries/geometry';
import type { GeometryPreviewProps } from './geometry/GeometryPreview';
import { geometryFormat } from './geometry/format';
import { GeometryLoading } from './GeometryLoading';

type Viewer = typeof import('./geometry/GeometryPreview').default;
let loaded: Viewer | undefined;
const loadGeometry = () => import('./geometry/GeometryPreview').then(module => (loaded = module.default));
// Explicit visible geometry demand only; overlap module loading with the already
// authorized source read, without opening a file or retaining a scene/Worker.
export function preloadGeometry(name: string) {
  void loadGeometry().catch(() => {});
  const format = geometryFormat(name);
  if (format === 'glb') void import('three/addons/loaders/GLTFLoader.js').catch(() => {});
  else if (format !== 'stl') void import('./geometry/cad-code').then(module => module.cadCode()).catch(() => {});
}
export function GeometryDocument(props: GeometryPreviewProps) {
  const locale = useLocale(), [Viewer, setViewer] = useState(() => loaded), [failed, setFailed] = useState(false);
  useEffect(() => {
    if (Viewer) return;
    let active = true;
    void loadGeometry().then(component => { if (active && !props.signal.aborted) setViewer(() => component); }, () => { if (active && !props.signal.aborted) setFailed(true); });
    return () => { active = false; };
  }, [Viewer, props.signal]);
  const t = locale.active === 'zh' ? zh : en;
  // Suspense's fallback throttle delayed resolved code by ~300 ms in production.
  // The explicit module state starts the admitted import as soon as code arrives.
  return Viewer ? <Viewer {...props}/> : <p role={failed ? 'alert' : 'status'}>{failed ? t.invalid : <GeometryLoading stage="loading"/>}</p>;
}

import { useEffect, useRef, useState } from 'react';
import { useLocale } from '../../../locale/react';
import { en, zh } from '../../../locale/dictionaries/geometry';
import type { GeometryPreviewProps } from './geometry/GeometryPreview';
import { geometryFormat } from './geometry/format';
import { GeometryLoading } from './GeometryLoading';
import { GeometryRoughPreview } from './GeometryRoughPreview';
import type { GeometryRoughPose, GeometryRoughPreview as RoughData } from '../../../client/geometry-rough';
import css from './GeometryDocument.module.css';

type Viewer = typeof import('./geometry/GeometryPreview').default;
let loaded: Viewer | undefined;
const loadGeometry = () => import('./geometry/GeometryPreview').then(module => (loaded = module.default));
// Explicit visible geometry demand only; overlap module loading with the already
// authorized source read, without opening a file or retaining a scene/Worker.
export function preloadGeometry(name: string) {
  const format = geometryFormat(name);
  // STEP's source-bound rough picture must win the first read/paint. Loading
  // and compiling the large CAD runtime here competes with its tiny probes.
  // The full viewer requests that code once the complete source is available.
  if (format === 'step') return;
  void loadGeometry().catch(() => {});
  if (format === 'glb') void import('three/addons/loaders/GLTFLoader.js').catch(() => {});
  else if (format !== 'stl') void import('./geometry/cad-code').then(module => module.cadCode()).catch(() => {});
}
export type GeometryDocumentProps = Omit<GeometryPreviewProps,'bytes'> & { bytes?:Uint8Array<ArrayBuffer>; rough?:RoughData };
export function GeometryDocument(props: GeometryDocumentProps) {
  const locale = useLocale(), [Viewer, setViewer] = useState(() => loaded), [failed, setFailed] = useState(false);
  const [fineReady,setFineReady]=useState(false),[fineFailed,setFineFailed]=useState(false),pose=useRef<GeometryRoughPose|undefined>(undefined);
  const [refineCopy,setRefineCopy]=useState<string>(),dragging=useRef(false),quiet=useRef<ReturnType<typeof setTimeout>>(undefined);
  const allowRefinement=!props.rough||refineCopy===props.copyId;
  const interaction=(active:boolean)=>{
    dragging.current=active;clearTimeout(quiet.current);
    if(!active&&props.bytes&&!allowRefinement)quiet.current=setTimeout(()=>{
      if(!props.signal.aborted&&props.current())setRefineCopy(props.copyId);
    },120);
  };
  useEffect(()=>{setFineReady(false);setFineFailed(false);dragging.current=false;},[props.copyId]);
  useEffect(()=>{
    if(props.bytes&&props.rough&&!allowRefinement)interaction(dragging.current);
    return()=>clearTimeout(quiet.current);
  },[props.bytes,props.copyId,props.rough?.version,allowRefinement]);
  useEffect(() => {
    if (Viewer || !allowRefinement || props.signal.aborted || !props.bytes && geometryFormat(props.name)==='step') return;
    let active = true;
    void loadGeometry().then(component => { if (active && !props.signal.aborted) setViewer(() => component); }, () => { if (active && !props.signal.aborted) setFailed(true); });
    return () => { active = false; };
  }, [Viewer, props.signal, props.bytes,allowRefinement]);
  const t = locale.active === 'zh' ? zh : en;
  // Suspense's fallback throttle delayed resolved code by ~300 ms in production.
  // The explicit module state starts the admitted import as soon as code arrives.
  const {rough,bytes,...view}=props;
  return <div className={css.root}>
    {Viewer && bytes && allowRefinement ? <Viewer {...view} bytes={bytes} initialPose={()=>pose.current} onDisplay={()=>setFineReady(true)} onFailure={()=>setFineFailed(true)}/> : <p role={failed?'alert':'status'}>{failed?t.invalid:<GeometryLoading stage="loading"/>}</p>}
    {rough && !fineReady && !fineFailed && !failed && <GeometryRoughPreview preview={rough} copyId={props.copyId} current={props.current} signal={props.signal} pose={pose} interaction={interaction} labels={t}/>}
  </div>;
}

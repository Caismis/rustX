import { useEffect, useRef, useState } from 'react';
import { Box3, type Object3D } from 'three';
import { useLocale } from '../../../../locale/react';
import { en, zh } from '../../../../locale/dictionaries/geometry';
import type { PreviewViewStateProps } from '../../../../presentation/right-panel/preview-view-state';
import { geometryFormat } from './format';
import { importGlb } from './glb';
import { parseGeometry } from './parse';
import { disposeScene, modelScene, validateScene } from './scene-resources';
import { GeometryError, type GeometryFailure } from './limits';
import { mountGeometry, type Part, type StandardView } from './viewport';
import { Button } from '../../../../presentation/primitives/Button';
import { Menu } from '../../../../presentation/primitives/Menu';
import { IconChevronDownOutline14 } from '../../../../presentation/primitives/icons';
import zoomCss from '../../workbench-documents/zoom/ZoomControls.module.css';
import css from './GeometryPreview.module.css';

export default function GeometryPreview({ bytes, name, copyId, signal, current, viewState, onViewStateChange }: PreviewViewStateProps & { bytes: Uint8Array<ArrayBuffer>; name: string; copyId: string; signal: AbortSignal; current: () => boolean }) {
  const locale = useLocale(), t = locale.active === 'zh' ? zh : en;
  const container = useRef<HTMLDivElement>(null), active = useRef<ReturnType<typeof mountGeometry>>(undefined);
  const latest = useRef({ viewState, onViewStateChange, t }); latest.current = { viewState, onViewStateChange, t };
  const [state, setState] = useState<{ parts?: Part[]; dimensions?: number[]; error?: GeometryFailure; selected?: string; hidden?: string[] }>({});
  const [menu, setMenu] = useState(false);
  const format = geometryFormat(name);
  useEffect(() => {
    const controller = new AbortController(); let roots: Object3D[] = [], mounted: ReturnType<typeof mountGeometry> | undefined, release: (() => void) | undefined;
    // Capture this copy's predicate, rather than adopting a later render's
    // permission while an earlier import is still completing.
    const ownsCopy = current;
    const live = () => !controller.signal.aborted && !signal.aborted && ownsCopy();
    const retire = () => { controller.abort(); mounted?.dispose(); if (active.current === mounted) active.current = undefined; mounted = undefined; if (release) { release(); release = undefined; } else disposeScene(roots); roots = []; };
    signal.addEventListener('abort', retire, { once: true }); setState({});
    if (!live()) { retire(); return; }
    void (async () => {
      let object: Object3D, bounds: Box3;
      if (format === 'glb') { const loaded = await importGlb(bytes, controller.signal); roots = loaded.scenes; release = loaded.dispose; object = loaded.scene; bounds = validateScene([object]); }
      else { const model = await parseGeometry(bytes, format, controller.signal); if (!live()) return; object = modelScene(model); roots = [object]; bounds = validateScene(roots); }
      if (!live()) { retire(); return; }
      const props = latest.current, saved = props.viewState.geometry?.copyId === copyId ? props.viewState.geometry : undefined;
      mounted = mountGeometry(container.current!, object, bounds, saved, props.t, patch => {
        if (!live()) return;
        latest.current.onViewStateChange({ geometry: { copyId, ...patch } });
        setState(value => ({ ...value, selected: patch.selected, hidden: patch.hidden }));
      }, () => { const publish = live(); retire(); if (publish) setState({ error: 'webgl' }); });
      active.current = mounted;
      setState(value => ({ ...value, parts: mounted!.parts, dimensions: [bounds.max.x - bounds.min.x, bounds.max.y - bounds.min.y, bounds.max.z - bounds.min.z] }));
    })().catch(error => {
      const publish = live(); retire();
      if (publish) setState({ error: error instanceof GeometryError ? error.code : 'invalid' });
    });
    return () => { signal.removeEventListener('abort', retire); retire(); };
  }, [bytes, format, copyId, signal]);
  // Names/labels do not own the renderer lifetime.
  useEffect(() => { const canvas = container.current?.querySelector('canvas'); canvas?.setAttribute('aria-label', t.canvas); canvas?.setAttribute('aria-description', t.axes); }, [t]);
  const views: StandardView[] = ['isometric', 'front', 'back', 'left', 'right', 'top', 'bottom'];
  const number = new Intl.NumberFormat(locale.active, { maximumSignificantDigits: 6 });
  return <section className={css.root} data-geometry-format={format}>
    {state.error ? <p className={css.message} role="alert">{t[state.error]}</p> : <>
      <div className={css.viewport} ref={container}>
        {!state.parts && <p className={css.loading} role="status">{t.loading}</p>}
        <div className={`${zoomCss.panel} ${zoomCss.visible}`} role="toolbar" aria-label={t.controls}>
          <Button size="sm" disabled={!state.parts} onClick={() => active.current?.fit()}>{t.fit}</Button>
          <Menu open={menu} onClose={() => setMenu(false)} align="start" dense autoFocus
            items={views.map(view => ({ id: view, label: t[view] }))} onSelect={view => { active.current?.fit(view as StandardView); setMenu(false); }}
            anchor={<Button size="sm" className={zoomCss.percent} disabled={!state.parts} aria-label={t.view} aria-haspopup="menu" aria-expanded={menu} onClick={() => setMenu(value => !value)}>{t.view}<IconChevronDownOutline14 size={12}/></Button>}/>
        </div>
      </div>
      {state.dimensions && <p className={css.metadata}>{format.toUpperCase()} · {t.bounds}: {state.dimensions.map(value => number.format(value)).join(' × ')} · {format === 'glb' ? t.meter : format === 'step' || format === 'iges' ? t.millimeter : t.unknown}</p>}
      {state.parts && <div className={css.partsPanel}><p className={css.heading}>{t.parts}</p><ul className={css.parts} aria-label={t.parts}>
        {state.parts.map((part, index) => <li key={part.id} style={{ paddingInlineStart: `${Math.min(part.depth, 8) * 8}px` }}>
          <input type="checkbox" aria-label={`${t.visible}: ${part.name || `${t.part} ${index + 1}`}`} checked={!state.hidden?.includes(part.id)} onChange={event => active.current?.visibility(part.id, event.target.checked)}/>
          <Button size="sm" className={css.partButton} aria-pressed={state.selected === part.id} onClick={() => active.current?.select(state.selected === part.id ? undefined : part.id)}>{part.name || `${t.part} ${index + 1}`}</Button>
        </li>)}
      </ul></div>}
    </>}
  </section>;
}

import { useEffect, useId, useMemo, useRef, useState } from 'react';
import { Box3, type Object3D } from 'three';
import { useLocale } from '../../../../locale/react';
import { en, zh } from '../../../../locale/dictionaries/geometry';
import type { PreviewViewState, PreviewViewStateProps } from '../../../../presentation/right-panel/preview-view-state';
import { geometryFormat } from './format';
import { importGlb } from './glb';
import { parseGeometry } from './parse';
import { disposeScene, modelScene } from './scene-resources';
import { GeometryError, type GeometryFailure } from './limits';
import type { GeometryImportStage } from './stages';
import { GeometryLoading } from '../GeometryLoading';
import { mountGeometry, type Part, type Projection, type StandardView } from './viewport';
import type { Inspection } from './structure';
import type { OutlineStatus } from './outlines';
import { GeometryViewGlyph } from './GeometryViewGlyph';
import { PartsTree } from './PartsTree';
import { Button } from '../../../../presentation/primitives/Button';
import { Menu } from '../../../../presentation/primitives/Menu';
import { Tooltip } from '../../../../presentation/primitives/Tooltip';
import { IconChecklistOutline14, IconCloseOutline16 } from '../../../../presentation/primitives/icons';
import css from './GeometryPreview.module.css';

export interface GeometryPreviewProps extends PreviewViewStateProps { bytes: Uint8Array<ArrayBuffer>; name: string; copyId: string; signal: AbortSignal; current: () => boolean; onGeometryRetire: (geometry: NonNullable<PreviewViewState['geometry']>) => void }
export default function GeometryPreview({ bytes, name, copyId, signal, current, viewState, onViewStateChange, onGeometryRetire }: GeometryPreviewProps) {
  const locale = useLocale(), t = locale.active === 'zh' ? zh : en;
  const container = useRef<HTMLDivElement>(null), active = useRef<ReturnType<typeof mountGeometry>>(undefined), outlineAction = useRef(0);
  const latest = useRef({ viewState, onViewStateChange, t }); latest.current = { viewState, onViewStateChange, t };
  const [state, setState] = useState<{ stage?: GeometryImportStage; parts?: Part[]; dimensions?: number[]; error?: GeometryFailure; selected?: string; hidden?: string[]; isolation?: string; projection?: Projection; inspection?: Inspection; outlineStatus?: OutlineStatus | 'loading' }>({});
  const [menu, setMenu] = useState(false);
  const [partsOpen, setPartsOpen] = useState(false), partsId = useId();
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set());
  const format = geometryFormat(name);
  useEffect(() => {
    const controller = new AbortController(); let roots: Object3D[] = [], mounted: ReturnType<typeof mountGeometry> | undefined, release: (() => void) | undefined;
    let selection: { selected?: string; hidden?: string[]; isolation?: string; projection: Projection } = { projection: 'perspective' };
    // Capture this copy's predicate, rather than adopting a later render's
    // permission while an earlier import is still completing.
    const ownsCopy = current;
    const live = () => !controller.signal.aborted && !signal.aborted && ownsCopy();
    const stage = (stage: GeometryImportStage) => { if (live()) setState(value => value.stage === stage ? value : { ...value, stage }); };
    const retire = () => {
      if (mounted && active.current === mounted) outlineAction.current++;
      // The visible-body signal may already be aborted. Only the owner can
      // authorize saving inert metadata for this exact retained content copy.
      mounted?.flush(patch => onGeometryRetire({ copyId, ...patch }));
      controller.abort(); mounted?.dispose(); if (active.current === mounted) active.current = undefined; mounted = undefined;
      if (release) { release(); release = undefined; } else disposeScene(roots); roots = [];
    };
    signal.addEventListener('abort', retire, { once: true }); setState({}); setPartsOpen(false); setExpanded(new Set()); setMenu(false);
    if (!live()) { retire(); return; }
    void (async () => {
      let object: Object3D, bounds: Box3, displayStart: number;
      if (format === 'glb') { stage('importing'); const loaded = await importGlb(bytes, controller.signal); roots = loaded.scenes; release = loaded.dispose; object = loaded.scene; bounds = loaded.bounds; displayStart = performance.now(); }
      else { const model = await parseGeometry(bytes, format, controller.signal, stage); if (!live()) return; stage('preparing'); displayStart = performance.now(); const admitted = modelScene(model); object = admitted.scene; bounds = admitted.bounds; roots = [object]; }
      if (!live()) { retire(); return; }
      stage('preparing');
      const props = latest.current, saved = props.viewState.geometry?.copyId === copyId ? props.viewState.geometry : undefined;
      mounted = mountGeometry(container.current!, object, bounds, saved, props.t, patch => {
        if (!live()) return;
        latest.current.onViewStateChange({ geometry: { copyId, ...patch } });
        const projection = patch.camera?.projection ?? selection.projection;
        if (selection.selected !== patch.selected || !sameHidden(selection.hidden, patch.hidden) || selection.isolation !== patch.isolation?.target || selection.projection !== projection) {
          selection = { selected: patch.selected, hidden: patch.hidden, isolation: patch.isolation?.target, projection };
          setState(value => ({ ...value, ...selection, inspection: mounted?.inspect() }));
        }
      }, () => { const publish = live(); retire(); if (publish) setState({ error: 'webgl' }); }, { generated: format !== 'glb' });
      // Keep only the latest measurement per supported format, rather than an
      // unbounded browser-lifetime history of preview openings.
      const measure = `geometry:display-prepare:${format}`;
      performance.clearMeasures(measure); performance.measure(measure, { start: displayStart });
      active.current = mounted;
      setState(value => ({ ...value, parts: mounted!.parts, projection: mounted!.projection, inspection: mounted!.inspect(), dimensions: [bounds.max.x - bounds.min.x, bounds.max.y - bounds.min.y, bounds.max.z - bounds.min.z] }));
      if (saved?.outlines !== false) requestOutlines(true);
    })().catch(error => {
      const publish = live(); retire();
      if (publish) setState({ error: error instanceof GeometryError ? error.code : 'invalid' });
    });
    return () => { signal.removeEventListener('abort', retire); retire(); };
  }, [bytes, format, copyId, signal]);
  // Names/labels do not own the renderer lifetime.
  useEffect(() => { active.current?.relabel(t); }, [t]);
  const views: StandardView[] = ['isometric', 'front', 'back', 'left', 'right', 'top', 'bottom'];
  const number = useMemo(() => new Intl.NumberFormat(locale.active, { maximumSignificantDigits: 6 }), [locale.active]);
  const dimensions = useMemo(() => state.dimensions?.map(value => number.format(value)).join(' × '), [state.dimensions, number]);
  const units = format === 'glb' ? t.meter : format === 'step' || format === 'iges' ? t.millimeter : t.unknown;
  const partCount = useMemo(() => state.parts?.filter(part => part.kind === 'mesh').length, [state.parts]);
  const closeParts = () => { setPartsOpen(false); container.current?.querySelector<HTMLButtonElement>('[data-geometry-parts-toggle]')?.focus(); };
  function requestOutlines(enabled: boolean) {
    const owner = active.current; if (!owner) return;
    const action = ++outlineAction.current; setState(value => ({ ...value, outlineStatus: enabled ? 'loading' : 'off' }));
    void owner.setOutlines(enabled).catch((): OutlineStatus => 'failed').then(status => {
      if (active.current === owner && outlineAction.current === action) setState(value => ({ ...value, outlineStatus: status }));
    });
  }
  return <section className={css.root} data-geometry-format={format} aria-label={t.preview} onKeyDown={event => {
    if (!partsOpen || event.key !== 'Escape' || event.defaultPrevented || event.nativeEvent.isComposing || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
    event.preventDefault(); event.stopPropagation(); closeParts();
  }}>
    <div className={css.viewport} ref={container}>
      {state.error ? <div className={css.message} role="alert"><ModelGlyph/><p>{t[state.error]}</p></div>
        : !state.parts && <div className={css.loading} role="status" aria-live="polite"><span className={css.loader} aria-hidden="true"/><GeometryLoading stage={state.stage ?? 'loading'}/></div>}
      {!state.error && <>
        <div className={css.rail} role="toolbar" aria-label={t.controls} aria-orientation="vertical">
          <Tooltip label={t.fit} side="right" delayMs={500}><button type="button" className={css.control} disabled={!state.parts} aria-label={t.fit} onClick={() => active.current?.fit()}><FitGlyph/></button></Tooltip>
          <Menu open={menu} onClose={() => setMenu(false)} side="right" align="start" dense autoFocus selectedId={state.projection ?? 'perspective'}
            items={[{ id: 'reset', label: t.reset, icon: <GeometryViewGlyph view="isometric"/> }, { id: 'directions', type: 'separator' }, ...views.map(view => ({ id: view, label: t[view], icon: <GeometryViewGlyph view={view}/> })), { id: 'projection-divider', type: 'separator' }, { id: 'projection-label', type: 'label', text: t.projection }, { id: 'perspective', label: t.perspective }, { id: 'orthographic', label: t.orthographic }]} onSelect={view => { if (view === 'reset') active.current?.reset(); else if (view === 'perspective' || view === 'orthographic') active.current?.setProjection(view); else active.current?.fit(view as StandardView); setMenu(false); }}
            anchor={<Tooltip label={t.view} side="right" delayMs={500}><button type="button" className={`${css.control} ${menu ? css.controlActive : ''}`} disabled={!state.parts} aria-label={t.view} aria-haspopup="menu" aria-expanded={menu} onClick={() => setMenu(value => !value)}><ModelGlyph/></button></Tooltip>}/>
          <span className={css.divider} aria-hidden="true"/>
          <Tooltip label={t.outlineHelp} side="right" delayMs={500}><button type="button" className={`${css.control} ${state.outlineStatus === 'on' ? css.controlActive : ''}`} disabled={!state.parts} aria-label={t.outlines} aria-pressed={state.outlineStatus === 'on'} aria-busy={state.outlineStatus === 'loading'} onClick={() => requestOutlines(state.outlineStatus !== 'on' && state.outlineStatus !== 'loading')}><OutlineGlyph/></button></Tooltip>
          <Tooltip label={t.parts} side="right" delayMs={500}><button type="button" className={`${css.control} ${partsOpen ? css.controlActive : ''}`} disabled={!state.parts} aria-label={t.parts} aria-expanded={partsOpen} aria-controls={partsId} data-geometry-parts-toggle onClick={() => setPartsOpen(value => !value)}><IconChecklistOutline14 size={16}/></button></Tooltip>
        </div>
        {state.outlineStatus === 'loading' || state.outlineStatus === 'limit' || state.outlineStatus === 'failed' ? <p className={css.outlineStatus} role="status">{state.outlineStatus === 'loading' ? t.outlineLoading : state.outlineStatus === 'limit' ? t.outlineLimit : t.outlineFailed}</p> : null}
        {state.parts && partsOpen && <div className={css.partsPanel} id={partsId}>
          <header className={css.partsHeader}><span>{t.parts}<span className={css.count}>{number.format(partCount!)}</span></span><Button size="sm" className={css.panelClose} aria-label={t.closeParts} onClick={closeParts}><IconCloseOutline16/></Button></header>
          <div className={css.structureActions}>
            <Button size="sm" disabled={!state.selected} onClick={() => active.current?.isolate()}>{t.isolate}</Button>
            {state.isolation && <Button size="sm" onClick={() => active.current?.restore()}>{t.restore}</Button>}
            <Button size="sm" onClick={() => active.current?.showAll()}>{t.showAll}</Button>
            <Button size="sm" disabled={!state.selected || !state.inspection?.visibleTriangles} onClick={() => active.current?.focus()}>{t.focus}</Button>
          </div>
          <PartsTree key={copyId} parts={state.parts} selected={state.selected} hidden={state.hidden} expanded={expanded} onExpandedChange={setExpanded} onSelect={id => active.current?.select(id)} onVisibility={(id, visible) => active.current?.visibility(id, visible)} labels={t}/>
          {state.inspection && <section className={css.properties} aria-label={t.properties}>
            <strong title={state.inspection.name}>{state.selected ? state.inspection.name || `${t.part} ${(state.parts.findIndex(part => part.id === state.selected) + 1)}` : t.model}</strong>
            <span>{t.mesh} · {units}</span>
            <dl><dt>{t.triangles}</dt><dd>{number.format(state.inspection.triangles)}</dd><dt>{t.renderedTriangles}</dt><dd>{number.format(state.inspection.visibleTriangles)}</dd></dl>
            {!!state.inspection.min.length && <><span>{t.worldBounds}</span><dl>
              <dt>{t.minimum}</dt><dd>{state.inspection.min.map(value => number.format(value)).join(', ')}</dd>
              <dt>{t.maximum}</dt><dd>{state.inspection.max.map(value => number.format(value)).join(', ')}</dd>
              <dt>{t.bounds}</dt><dd>{state.inspection.dimensions.map(value => number.format(value)).join(' × ')}</dd>
            </dl></>}
          </section>}
        </div>}
      </>}
    </div>
    <footer className={css.footer}>
      {dimensions && <p className={css.metadata} title={`${t.bounds}: ${dimensions} · ${units}`}>{dimensions}<span className={css.units}>{units}</span></p>}
      <span className={css.format}>{format.toUpperCase()}</span>
    </footer>
  </section>;
}

function sameHidden(left: string[] | undefined, right: string[] | undefined) { return (left?.length ?? 0) === (right?.length ?? 0) && (left ?? []).every((id, index) => id === right?.[index]); }

function ModelGlyph() {
  return <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.2" strokeLinejoin="round" aria-hidden="true"><path d="m8 1.8 5.5 3.1v6.2L8 14.2l-5.5-3.1V4.9L8 1.8Z"/><path d="m2.5 4.9 5.5 3.2 5.5-3.2M8 8.1v6.1"/></svg>;
}
function FitGlyph() {
  return <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M2.5 6V2.5H6m4 0h3.5V6M2.5 10v3.5H6m7.5-3.5v3.5H10"/></svg>;
}
function OutlineGlyph() {
  return <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinejoin="round" aria-hidden="true"><path d="m8 1.5 6 3.3v6.4L8 14.5l-6-3.3V4.8L8 1.5Z"/><path d="m4.5 6 3.5 2 3.5-2M8 8v4" strokeWidth=".7"/></svg>;
}

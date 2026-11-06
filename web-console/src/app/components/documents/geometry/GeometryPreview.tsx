import { useLayoutEffect, useEffect, useId, useMemo, useRef, useState } from 'react';
import type { Box3, Object3D } from 'three';
import { useLocale } from '../../../../locale/react';
import { en, zh } from '../../../../locale/dictionaries/geometry';
import type { PreviewViewState, PreviewViewStateProps } from '../../../../presentation/right-panel/preview-view-state';
import { geometryFormat } from './format';
import { importGlb } from './glb';
import { parseGeometry } from './parse';
import { disposeScene } from './scene-resources';
import { GeometryError, type GeometryFailure } from './limits';
import type { GeometryImportStage } from './stages';
import { GeometryLoading } from '../GeometryLoading';
import { mountGeometry, prepareGeometryGraphics, type Part, type Projection, type StandardView, type NavigationMode } from './viewport';
import type { Inspection } from './structure';
import type { OutlineStatus } from './outlines';
import { GeometryCubeGlyph, GeometryViewGlyph } from './GeometryViewGlyph';
import { cameraFromPose, progressivePresentation } from './progressive';
import { PartsTree } from './PartsTree';
import { Button } from '../../../../presentation/primitives/Button';
import { Menu } from '../../../../presentation/primitives/Menu';
import { Tooltip } from '../../../../presentation/primitives/Tooltip';
import { IconChecklistOutline14, IconCloseOutline16, IconRefreshOutline16 } from '../../../../presentation/primitives/icons';
import css from './GeometryPreview.module.css';

export interface GeometryPreviewProps extends PreviewViewStateProps { visible?: boolean; bytes: Uint8Array<ArrayBuffer>; name: string; copyId: string; signal: AbortSignal; current: () => boolean; takeSource?: () => Uint8Array<ArrayBuffer> | undefined; releaseSource?: () => void; onGeometryRetire: (geometry: NonNullable<PreviewViewState['geometry']>) => void }
export default function GeometryPreview({ bytes, name, copyId, signal, current, takeSource, releaseSource, viewState, onViewStateChange, onGeometryRetire, visible = true }: GeometryPreviewProps) {
  const locale = useLocale(), t = locale.active === 'zh' ? zh : en;
  const container = useRef<HTMLDivElement>(null), active = useRef<ReturnType<typeof mountGeometry>>(undefined), outlineAction = useRef(0);
  const latest = useRef({ viewState, onViewStateChange, t, visible }); latest.current = { viewState, onViewStateChange, t, visible };
  const presentation = useRef<((visible: boolean) => void) | undefined>(undefined), deferredOutlines = useRef<boolean | undefined>(undefined), pendingOutlines = useRef<number | undefined>(undefined);
  useLayoutEffect(() => {
    presentation.current?.(visible);
    if (!visible) setMenu(false);
    if (visible && deferredOutlines.current !== undefined && pendingOutlines.current === undefined) requestOutlines(deferredOutlines.current);
  }, [visible]);
  const [state, setState] = useState<{ stage?: GeometryImportStage; preview?: boolean; parts?: Part[]; dimensions?: number[]; error?: GeometryFailure; selected?: string; hidden?: string[]; isolation?: string; projection?: Projection; navigationMode?: NavigationMode; inspection?: Inspection; outlineStatus?: OutlineStatus | 'loading' }>({});
  const [menu, setMenu] = useState(false);
  const [partsOpen, setPartsOpen] = useState(false), partsId = useId();
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set());
  const format = geometryFormat(name);
  useEffect(() => {
    const controller = new AbortController(); let roots: Object3D[] = [], mounted: ReturnType<typeof mountGeometry> | undefined;
    let graphics: ReturnType<typeof prepareGeometryGraphics> | undefined, release: (() => void) | undefined;
    let progressive: ReturnType<typeof progressivePresentation> | undefined, provisional: ReturnType<typeof mountGeometry> | undefined;
    let selection: { selected?: string; hidden?: string[]; isolation?: string; projection: Projection; navigationMode: NavigationMode } = { projection: 'orthographic', navigationMode: 'orbit' };
    // Capture this copy's predicate, rather than adopting a later render's
    // permission while an earlier import is still completing.
    const ownsCopy = current;
    const live = () => !controller.signal.aborted && !signal.aborted && ownsCopy();
    const stage = (stage: GeometryImportStage) => { if (live()) setState(value => value.stage === stage ? value : { ...value, stage }); };
    const present = (shown: boolean) => { progressive?.setVisible(shown); mounted?.setVisible(shown); };
    presentation.current = present;
    const retire = () => {
      if (presentation.current === present) { presentation.current = undefined; deferredOutlines.current = undefined; pendingOutlines.current = undefined; }
      if (mounted && active.current === mounted) outlineAction.current++;
      // The visible-body signal may already be aborted. Only the owner can
      // authorize saving inert metadata for this exact retained content copy.
      mounted?.flush(patch => onGeometryRetire({ copyId, ...patch }));
      controller.abort(); mounted?.dispose(); progressive?.dispose(); progressive = undefined; graphics?.dispose(); graphics = undefined;
      if (active.current === mounted || active.current === provisional) active.current = undefined; mounted = provisional = undefined;
      if (release) { release(); release = undefined; } else disposeScene(roots); roots = [];
    };
    signal.addEventListener('abort', retire, { once: true }); setState({}); setPartsOpen(false); setExpanded(new Set()); setMenu(false);
    if (!live()) { retire(); return; }
    const failed = (error: unknown) => {
      const publish = live(); retire();
      if (publish) setState({ error: error instanceof GeometryError ? error.code : 'invalid' });
    };
    void (async () => {
      let object: Object3D, bounds: Box3, displayStart: number, prepared = Promise.resolve(), startPreview: (() => void) | undefined;
      const graphicsLost = () => { const publish = live(); retire(); if (publish) setState({ error: 'webgl' }); };
      if (format === 'glb') stage('importing');
      const pending = format === 'glb' ? importGlb(bytes, controller.signal)
        : parseGeometry(bytes, format, controller.signal, stage, () => live() ? takeSource?.() : undefined, format === 'stl' ? undefined : lod => {
          if (live() && progressive) progressive.add(lod); else lod.dispose();
        });
      void pending.catch(() => {});
      try { graphics = prepareGeometryGraphics(format !== 'glb', graphicsLost, { visible: latest.current.visible }); }
      catch (error) { void pending.then(value => value.dispose(), () => {}); throw error; }
      if (format !== 'glb' && format !== 'stl') {
        const props = latest.current;
        progressive = progressivePresentation({ visible: latest.current.visible, container: container.current!, graphics, current: live, copyId, labels: () => latest.current.t,
          saved: props.viewState.geometry?.copyId === copyId ? props.viewState.geometry : undefined,
          mounted: view => { provisional = view; active.current = view ?? mounted; if (view) setState(value => ({ ...value, navigationMode: view.navigationMode })); },
          displayed: () => { if (live()) setState(value => ({ ...value, preview: true })); }, failed,
        });
      }
      const loaded = await pending; object = loaded.scene; bounds = loaded.bounds; release = loaded.dispose;
      if ('scenes' in loaded) { roots = loaded.scenes; displayStart = performance.now(); }
      else {
        roots = [object]; displayStart = loaded.prepareStart; prepared = loaded.ready;
        if (loaded.pendingPreview) startPreview = () => { if (live()) loaded.startPreview(); };
      }
      if (!live()) { retire(); return; }
      const lodSeed = progressive ? await progressive.complete() : undefined;
      if (!live()) { retire(); return; }
      releaseSource?.();
      stage('preparing');
      const props = latest.current;let saved = props.viewState.geometry?.copyId === copyId ? props.viewState.geometry : undefined;
      if (lodSeed?.navigationMode) saved = { ...saved, copyId, navigationMode: lodSeed.navigationMode };
      if (lodSeed?.pose) saved = { ...saved, copyId, camera: cameraFromPose(lodSeed.pose, bounds, lodSeed.viewportSize, lodSeed.projection) };
      mounted = mountGeometry(container.current!, object, bounds, saved, props.t, patch => {
        if (!live()) return;
        latest.current.onViewStateChange({ geometry: { copyId, ...patch } });
        const projection = patch.camera?.projection ?? selection.projection, navigationMode = patch.navigationMode === 'pan' ? 'pan' : 'orbit';
        if (selection.selected !== patch.selected || !sameHidden(selection.hidden, patch.hidden) || selection.isolation !== patch.isolation?.target || selection.projection !== projection || selection.navigationMode !== navigationMode) {
          selection = { selected: patch.selected, hidden: patch.hidden, isolation: patch.isolation?.target, projection, navigationMode };
          setState(value => ({ ...value, ...selection, inspection: mounted?.inspect() }));
        }
      }, graphicsLost, { visible: latest.current.visible, generated: format !== 'glb', pendingPreview: !!startPreview, onFirstGpuReady:()=>{if(live())startPreview?.();}, graphics });
      const ownerDocument = container.current!.ownerDocument;
      if (latest.current.visible && lodSeed?.focusedAxis && ownerDocument.activeElement === ownerDocument.body) mounted.focusAxis(lodSeed.focusedAxis);
      progressive?.dispose(); progressive = undefined;
      // Keep only the latest measurement per supported format, rather than an
      // unbounded browser-lifetime history of preview openings.
      const measure = `geometry:display-prepare:${format}`;
      performance.clearMeasures(measure); performance.measure(measure, { start: displayStart });
      active.current = mounted;
      setState(value => ({ ...value, preview: false, parts: mounted!.parts, projection: mounted!.projection, navigationMode: mounted!.navigationMode, inspection: mounted!.inspect(), dimensions: [bounds.max.x - bounds.min.x, bounds.max.y - bounds.min.y, bounds.max.z - bounds.min.z] }));
      if (saved?.outlines !== false) requestOutlines(true);
      void prepared.then(() => { if (!live() || !mounted) return; mounted.refreshPreview(); });
    })().catch(failed);
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
    const action = ++outlineAction.current;
    deferredOutlines.current = enabled; pendingOutlines.current = undefined;
    setState(value => ({ ...value, outlineStatus: enabled ? 'loading' : 'off' }));
    if (!latest.current.visible) return;
    const owner = active.current; if (!owner) return;
    pendingOutlines.current = action;
    void owner.setOutlines(enabled).catch((): OutlineStatus => 'failed').then(status => {
      if (active.current !== owner || outlineAction.current !== action) return;
      pendingOutlines.current = undefined;
      // A visible request can suspend while waiting for surface preparation.
      // Keep its demand until it finishes, and avoid restarting running work
      // when the body is shown before that wait has settled.
      deferredOutlines.current = enabled && status === 'off' ? true : undefined;
      if (deferredOutlines.current) { if (latest.current.visible) requestOutlines(true); return; }
      setState(value => ({ ...value, outlineStatus: status }));
    });
  }
  function changeNavigationMode(mode: NavigationMode) {
    const owner = active.current; if (!owner) return;
    owner.setNavigationMode(mode);
    setState(value => value.navigationMode === owner.navigationMode ? value : { ...value, navigationMode: owner.navigationMode });
  }
  return <section className={css.root} data-geometry-format={format} aria-label={t.preview} onKeyDown={event => {
    if (!partsOpen || event.key !== 'Escape' || event.defaultPrevented || event.nativeEvent.isComposing || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
    event.preventDefault(); event.stopPropagation(); closeParts();
  }}>
    <div className={css.viewport} ref={container}>
      {state.error ? <div className={css.message} role="alert"><GeometryCubeGlyph/><p>{t[state.error]}</p></div>
        : !state.parts && (state.preview ? <p className={css.previewStatus} role="status" aria-live="polite">{t.refining}</p> : <div className={css.loading} role="status" aria-live="polite"><span className={css.loader} aria-hidden="true"/><GeometryLoading stage={state.stage ?? 'loading'}/></div>)}
      {!state.error && <>
        <div className={css.rail} role="toolbar" aria-label={t.controls} aria-orientation="vertical">
          <Tooltip label={t.pan} side="right" delayMs={500}><button type="button" className={`${css.control} ${state.navigationMode === 'pan' ? css.controlActive : ''}`} disabled={!state.navigationMode} aria-label={t.pan} aria-pressed={state.navigationMode === 'pan'} onClick={() => changeNavigationMode(state.navigationMode === 'pan' ? 'orbit' : 'pan')}><PanGlyph/></button></Tooltip>
          <span className={css.divider} aria-hidden="true"/>
          <Tooltip label={t.fit} side="right" delayMs={500}><button type="button" className={css.control} disabled={!state.parts} aria-label={t.fit} onClick={() => active.current?.fit()}><FitGlyph/></button></Tooltip>
          <Menu open={menu} onClose={() => setMenu(false)} side="right" align="start" dense autoFocus selectedId={state.projection ?? 'orthographic'}
            items={[{ id: 'reset', label: t.reset, icon: <IconRefreshOutline16/> }, { id: 'isometric', label: t.isometric, icon: <GeometryCubeGlyph/> }, { id: 'directions', type: 'separator' }, ...views.filter(view => view !== 'isometric').map(view => ({ id: view, label: t[view], icon: <GeometryViewGlyph view={view}/> })), { id: 'projection-divider', type: 'separator' }, { id: 'projection-label', type: 'label', text: t.projection }, { id: 'perspective', label: t.perspective }, { id: 'orthographic', label: t.orthographic }]} onSelect={view => { if (view === 'reset') active.current?.reset(); else if (view === 'perspective' || view === 'orthographic') active.current?.setProjection(view); else active.current?.fit(view as StandardView); setMenu(false); }}
            anchor={<Tooltip label={t.view} side="right" delayMs={500}><button type="button" className={`${css.control} ${menu ? css.controlActive : ''}`} disabled={!state.parts} aria-label={t.view} aria-haspopup="menu" aria-expanded={menu} onClick={() => setMenu(value => !value)}><GeometryCubeGlyph/></button></Tooltip>}/>
          <span className={css.divider} aria-hidden="true"/>
          <Tooltip label={t.outlines} side="right" delayMs={500}><button type="button" className={`${css.control} ${state.outlineStatus === 'on' ? css.controlActive : ''}`} disabled={!state.parts} aria-label={t.outlines} aria-pressed={state.outlineStatus === 'on'} aria-busy={state.outlineStatus === 'loading'} onClick={() => requestOutlines(state.outlineStatus !== 'on' && state.outlineStatus !== 'loading')}><OutlineGlyph/></button></Tooltip>
          <Tooltip label={t.parts} side="right" delayMs={500}><button type="button" className={`${css.control} ${partsOpen ? css.controlActive : ''}`} disabled={!state.parts} aria-label={t.parts} aria-expanded={partsOpen} aria-controls={partsId} data-geometry-parts-toggle onClick={() => setPartsOpen(value => !value)}><IconChecklistOutline14 size={16}/></button></Tooltip>
        </div>
        {state.outlineStatus === 'limit' || state.outlineStatus === 'failed' ? <p className={css.outlineStatus} role="status">{state.outlineStatus === 'limit' ? t.outlineLimit : t.outlineFailed}</p> : null}
        {state.parts && partsOpen && <div className={css.partsPanel} id={partsId}>
          <header className={css.partsHeader}><span>{t.parts}<span className={css.count}>{number.format(partCount!)}</span></span><Button size="sm" className={css.panelClose} aria-label={t.closeParts} onClick={closeParts}><IconCloseOutline16/></Button></header>
          <div className={css.structureActions}>
            <Button size="sm" className={state.isolation ? css.controlActive : undefined} disabled={!state.selected && !state.isolation} aria-pressed={!!state.isolation} onClick={() => state.isolation && (!state.selected || state.selected === state.isolation) ? active.current?.restore() : active.current?.isolate()}>{t.isolate}</Button>
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

function FitGlyph() {
  return <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M2.5 6V2.5H6m4 0h3.5V6M2.5 10v3.5H6m7.5-3.5v3.5H10"/></svg>;
}
function OutlineGlyph() {
  return <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M2.5 5V2.5H5M11 2.5h2.5V5M13.5 11v2.5H11M5 13.5H2.5V11M7 2.5h2M13.5 7v2M9 13.5H7M2.5 9V7"/></svg>;
}

function PanGlyph() {
  return <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M8 1.5v13M1.5 8h13M5.5 4 8 1.5 10.5 4M5.5 12 8 14.5 10.5 12M4 5.5 1.5 8 4 10.5M12 5.5 14.5 8 12 10.5"/></svg>;
}

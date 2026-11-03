import { useEffect, useId, useMemo, useRef, useState, type ReactNode } from 'react';
import { Box3, type Object3D } from 'three';
import { useLocale } from '../../../../locale/react';
import { en, zh } from '../../../../locale/dictionaries/geometry';
import type { PreviewViewStateProps } from '../../../../presentation/right-panel/preview-view-state';
import { geometryFormat } from './format';
import { importGlb } from './glb';
import { parseGeometry } from './parse';
import { disposeScene, modelScene } from './scene-resources';
import { GeometryError, type GeometryFailure } from './limits';
import { mountGeometry, type Part, type StandardView } from './viewport';
import { Button } from '../../../../presentation/primitives/Button';
import { Menu } from '../../../../presentation/primitives/Menu';
import { Tooltip } from '../../../../presentation/primitives/Tooltip';
import { IconChevronRightOutline14, IconChecklistOutline14, IconCloseOutline16, IconFolderClose16 } from '../../../../presentation/primitives/icons';
import css from './GeometryPreview.module.css';

export default function GeometryPreview({ bytes, name, copyId, signal, current, viewState, onViewStateChange }: PreviewViewStateProps & { bytes: Uint8Array<ArrayBuffer>; name: string; copyId: string; signal: AbortSignal; current: () => boolean }) {
  const locale = useLocale(), t = locale.active === 'zh' ? zh : en;
  const container = useRef<HTMLDivElement>(null), active = useRef<ReturnType<typeof mountGeometry>>(undefined);
  const latest = useRef({ viewState, onViewStateChange, t }); latest.current = { viewState, onViewStateChange, t };
  const [state, setState] = useState<{ parts?: Part[]; dimensions?: number[]; error?: GeometryFailure; selected?: string; hidden?: string[] }>({});
  const [menu, setMenu] = useState(false);
  const [partsOpen, setPartsOpen] = useState(false), partsId = useId();
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set());
  const tree = useMemo(() => partTree(state.parts ?? []), [state.parts]);
  const format = geometryFormat(name);
  useEffect(() => {
    const controller = new AbortController(); let roots: Object3D[] = [], mounted: ReturnType<typeof mountGeometry> | undefined, release: (() => void) | undefined;
    // Capture this copy's predicate, rather than adopting a later render's
    // permission while an earlier import is still completing.
    const ownsCopy = current;
    const live = () => !controller.signal.aborted && !signal.aborted && ownsCopy();
    const retire = () => { controller.abort(); mounted?.dispose(); if (active.current === mounted) active.current = undefined; mounted = undefined; if (release) { release(); release = undefined; } else disposeScene(roots); roots = []; };
    signal.addEventListener('abort', retire, { once: true }); setState({}); setPartsOpen(false); setExpanded(new Set()); setMenu(false);
    if (!live()) { retire(); return; }
    void (async () => {
      let object: Object3D, bounds: Box3;
      if (format === 'glb') { const loaded = await importGlb(bytes, controller.signal); roots = loaded.scenes; release = loaded.dispose; object = loaded.scene; bounds = loaded.bounds; }
      else { const model = await parseGeometry(bytes, format, controller.signal); if (!live()) return; const admitted = modelScene(model); object = admitted.scene; bounds = admitted.bounds; roots = [object]; }
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
  const dimensions = state.dimensions?.map(value => number.format(value)).join(' × ');
  const units = format === 'glb' ? t.meter : format === 'step' || format === 'iges' ? t.millimeter : t.unknown;
  const partCount = state.parts?.filter(part => part.kind === 'mesh').length;
  const closeParts = () => { setPartsOpen(false); container.current?.querySelector<HTMLButtonElement>('[data-geometry-parts-toggle]')?.focus(); };
  const renderPart = ({ part, ordinal, children }: PartBranch): ReactNode => {
    const name = part.name || `${t.part} ${ordinal}`, visible = !state.hidden?.includes(part.id), open = expanded.has(part.id);
    return <li key={part.id}>
      <div className={css.partRow} data-hidden={!visible || undefined}>
        {children.length ? <Button size="sm" className={css.disclosure} aria-label={`${open ? t.collapse : t.expand}: ${name}`} aria-expanded={open} onClick={() => setExpanded(value => { const next = new Set(value); if (open) next.delete(part.id); else next.add(part.id); return next; })}><IconChevronRightOutline14 className={open ? css.expanded : undefined}/></Button> : <span className={css.branchSpace}/>}
        <Button size="sm" className={css.partButton} title={name} icon={part.kind === 'group' ? <IconFolderClose16/> : <ModelGlyph/>} aria-pressed={state.selected === part.id} onClick={() => active.current?.select(state.selected === part.id ? undefined : part.id)}><span>{name}</span></Button>
        <Tooltip label={`${visible ? t.hide : t.show}: ${name}`} side="top" delayMs={500}><span className={css.visibilityAnchor}><Button size="sm" className={css.visibility} role="checkbox" aria-label={`${t.visible}: ${name}`} aria-checked={visible} onClick={() => active.current?.visibility(part.id, !visible)}><VisibilityGlyph visible={visible}/></Button></span></Tooltip>
      </div>
      {children.length > 0 && open && <ul>{children.map(renderPart)}</ul>}
    </li>;
  };
  return <section className={css.root} data-geometry-format={format} aria-label={t.preview} onKeyDown={event => {
    if (!partsOpen || event.key !== 'Escape' || event.defaultPrevented || event.nativeEvent.isComposing || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
    event.preventDefault(); event.stopPropagation(); closeParts();
  }}>
    <div className={css.viewport} ref={container}>
      {state.error ? <div className={css.message} role="alert"><ModelGlyph/><p>{t[state.error]}</p></div>
        : !state.parts && <div className={css.loading} role="status"><span className={css.loader} aria-hidden="true"/>{t.loading}</div>}
      {!state.error && <>
        <div className={css.rail} role="toolbar" aria-label={t.controls} aria-orientation="vertical">
          <Tooltip label={t.fit} side="right" delayMs={500}><button type="button" className={css.control} disabled={!state.parts} aria-label={t.fit} onClick={() => active.current?.fit()}><FitGlyph/></button></Tooltip>
          <Menu open={menu} onClose={() => setMenu(false)} side="right" align="start" dense autoFocus
            items={views.map(view => ({ id: view, label: t[view] }))} onSelect={view => { active.current?.fit(view as StandardView); setMenu(false); }}
            anchor={<Tooltip label={t.view} side="right" delayMs={500}><button type="button" className={`${css.control} ${menu ? css.controlActive : ''}`} disabled={!state.parts} aria-label={t.view} aria-haspopup="menu" aria-expanded={menu} onClick={() => setMenu(value => !value)}><ModelGlyph/></button></Tooltip>}/>
          <span className={css.divider} aria-hidden="true"/>
          <Tooltip label={t.parts} side="right" delayMs={500}><button type="button" className={`${css.control} ${partsOpen ? css.controlActive : ''}`} disabled={!state.parts} aria-label={t.parts} aria-expanded={partsOpen} aria-controls={partsId} data-geometry-parts-toggle onClick={() => setPartsOpen(value => !value)}><IconChecklistOutline14 size={16}/></button></Tooltip>
        </div>
        {state.parts && partsOpen && <div className={css.partsPanel} id={partsId}>
          <header className={css.partsHeader}><span>{t.parts}<span className={css.count}>{number.format(partCount!)}</span></span><Button size="sm" className={css.panelClose} aria-label={t.closeParts} onClick={closeParts}><IconCloseOutline16/></Button></header>
          <ul className={css.parts} aria-label={t.parts}>{tree.map(renderPart)}</ul>
        </div>}
      </>}
    </div>
    <footer className={css.footer}>
      {dimensions && <p className={css.metadata} title={`${t.bounds}: ${dimensions} · ${units}`}>{dimensions}<span className={css.units}>{units}</span></p>}
      <span className={css.format}>{format.toUpperCase()}</span>
    </footer>
  </section>;
}

interface PartBranch { part: Part; ordinal: number; children: PartBranch[] }
function partTree(parts: Part[]): PartBranch[] {
  const roots: PartBranch[] = [], parents: PartBranch[] = [];
  parts.forEach((part, index) => {
    while (parents.length && parents.at(-1)!.part.depth >= part.depth) parents.pop();
    const branch = { part, ordinal: index + 1, children: [] };
    (parents.at(-1)?.children ?? roots).push(branch); parents.push(branch);
  });
  return roots;
}

function ModelGlyph() {
  return <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.2" strokeLinejoin="round" aria-hidden="true"><path d="m8 1.8 5.5 3.1v6.2L8 14.2l-5.5-3.1V4.9L8 1.8Z"/><path d="m2.5 4.9 5.5 3.2 5.5-3.2M8 8.1v6.1"/></svg>;
}
function FitGlyph() {
  return <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M2.5 6V2.5H6m4 0h3.5V6M2.5 10v3.5H6m7.5-3.5v3.5H10"/></svg>;
}
function VisibilityGlyph({ visible }: { visible: boolean }) {
  return <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M1.5 8S4 3.7 8 3.7 14.5 8 14.5 8s-2.5 4.3-6.5 4.3S1.5 8 1.5 8Z"/><circle cx="8" cy="8" r="1.8"/>{!visible && <path d="m2 2 12 12"/>}</svg>;
}

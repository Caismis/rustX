import { lazy, Suspense, useEffect, useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import { useTranslation } from '../../locale/react';
import css from './ExpressGeneration.module.css';

const ExpressBlueprint = lazy(() => import('./ExpressBlueprint'));

function PendingSurface({ draft, live }: { draft?: string; live: boolean }) {
  const tx = useTranslation();
  const placeholder = <div className={css.emptyFrame}/>;
  return <div className={css.pending} data-a2ui-generation={live ? 'draft' : undefined}>
    <div className={css.status} role="status">{tx('interactions:express.preparing')}</div>
    <div className={css.preview} aria-hidden="true">
      {draft?.trim()
        ? <Suspense fallback={placeholder}><ExpressBlueprint source={draft}/></Suspense>
        : placeholder}
      {live && <span className={css.previewScan}/>}
    </div>
  </div>;
}

type Box = { x: number; y: number; w: number; h: number };
type Drawing = { width: number; height: number; frame: Box; boxes: Box[]; lines: Box[] };
const boxSelector = '[data-a2-type] > *, [data-slot="card"], figure, img, svg[role="img"], table, button, input';
const textSelector = 'h1,h2,h3,h4,h5,h6,p,div,span,td,th,button,label';

function measure(surface: Element, host: Element): Drawing {
  const bounds = host.getBoundingClientRect();
  const frame = local(surface.getBoundingClientRect(), bounds);
  const boxes: Box[] = [];
  const lines: Box[] = [];
  const seen = new Set<string>();
  for (const element of Array.from(surface.querySelectorAll(boxSelector)).slice(0, 96)) {
    const box = local(element.getBoundingClientRect(), bounds);
    if (box.y < frame.y || box.y + box.h > frame.y + frame.h + 1 || box.h < 24 || box.w < 32 ||
      box.w >= frame.w && box.h >= frame.h - 2) continue;
    const key = [box.x, box.y, box.w, box.h].join(',');
    if (seen.has(key)) continue;
    seen.add(key);
    boxes.push(box);
    if (boxes.length === 36) break;
  }
  const range = document.createRange();
  for (const element of Array.from(surface.querySelectorAll(textSelector)).slice(0, 220)) {
    for (const node of element.childNodes) {
      if (node.nodeType !== Node.TEXT_NODE || !node.textContent?.trim()) continue;
      range.selectNodeContents(node);
      for (const rect of range.getClientRects()) {
        const line = local(rect, bounds);
        if (line.w < 5 || line.y < frame.y || line.y + line.h > frame.y + frame.h + 1) continue;
        lines.push(line);
        if (lines.length === 88) return { width: bounds.width, height: bounds.height, frame, boxes, lines };
      }
    }
  }
  return { width: bounds.width, height: bounds.height, frame, boxes, lines };
}

function local(rect: DOMRect, bounds: DOMRect): Box {
  return { x: Math.round(rect.x - bounds.x), y: Math.round(rect.y - bounds.y), w: Math.round(rect.width), h: Math.round(rect.height) };
}

function Wireframe({ drawing }: { drawing: Drawing }) {
  const { width, height, frame, boxes, lines } = drawing;
  const corners = [[frame.x, frame.y], [frame.x + frame.w, frame.y],
    [frame.x, frame.y + frame.h], [frame.x + frame.w, frame.y + frame.h]];
  return <div className={css.overlay} aria-hidden="true">
    <svg className={css.wire} viewBox={`0 0 ${width} ${height}`} focusable="false">
      <g className={css.guides}>
        <path d={`M0 ${frame.y}H${width} M0 ${frame.y + frame.h}H${width} M${frame.x} 0V${height} M${frame.x + frame.w} 0V${height}`}/>
      </g>
      <g className={css.edges}>
        <rect x={frame.x + 1} y={frame.y + 1} width={Math.max(0, frame.w - 2)} height={Math.max(0, frame.h - 2)} rx="14"/>
        {boxes.map(({ x, y, w, h }, i) => <rect key={i} x={x} y={y} width={w} height={h} rx="9"/>)}
        {lines.map(({ x, y, w, h }, i) => <path key={i} d={`M${x} ${y + h * .58}h${w}`}/>)}
      </g>
      <g className={css.corners}>
        {corners.map(([x, y], i) => <rect key={i} x={Math.min(Math.max(0, width - 5), Math.max(0, x - 2))} y={Math.min(Math.max(0, height - 5), Math.max(0, y - 2))} width="5" height="5" rx="1"/>)}
      </g>
    </svg>
    <span className={css.scan}/>
  </div>;
}

function Reveal({ animate, children }: { animate: boolean; children: ReactNode }) {
  const result = useRef<HTMLDivElement>(null);
  const [armed] = useState(animate);
  const [drawing, setDrawing] = useState<Drawing>();
  const [finished, setFinished] = useState(false);
  const revealing = !!drawing && !finished;

  useLayoutEffect(() => {
    const host = result.current;
    const surface = host?.querySelector('[data-a2ui-express]');
    if (!armed || finished || !host || !surface || window.matchMedia?.('(prefers-reduced-motion: reduce)').matches) return;
    let lastW = 0;
    let lastH = 0;
    let raf = 0;
    const update = () => {
      const w = host.clientWidth;
      const h = host.clientHeight;
      if (!w || !h || (w === lastW && h === lastH)) return;
      lastW = w;
      lastH = h;
      setDrawing(measure(surface, host));
    };
    update();
    if (typeof ResizeObserver === 'undefined') return;
    const resize = new ResizeObserver(() => {
      cancelAnimationFrame(raf);
      raf = requestAnimationFrame(update);
    });
    resize.observe(surface);
    return () => {
      cancelAnimationFrame(raf);
      resize.disconnect();
    };
  }, [armed, finished]);

  useEffect(() => {
    if (!revealing) return;
    const timeout = window.setTimeout(() => setFinished(true), 1800);
    return () => clearTimeout(timeout);
  }, [revealing]);

  return <div className={css.reveal} data-a2ui-generation={revealing ? 'reveal' : undefined}>
    <div ref={result} className={css.result} inert={revealing}
      onAnimationEnd={event => { if (revealing && event.currentTarget === event.target) setFinished(true); }}>
      {children}
    </div>
    {revealing && <Wireframe drawing={drawing}/>}
  </div>;
}

export function ExpressGeneration({ live, draft, contentKey, children }: {
  live: boolean; draft?: string; contentKey?: string; children: ReactNode;
}) {
  const [state, setState] = useState(() => ({
    pendingLive: draft !== undefined,
    activeKey: contentKey,
    animate: live,
  }));
  if (draft !== undefined) {
    if (!state.pendingLive || state.activeKey !== undefined) {
      setState({ pendingLive: true, activeKey: undefined, animate: true });
    }
    return <PendingSurface draft={draft} live/>;
  }
  if (contentKey !== state.activeKey) {
    setState({ pendingLive: false, activeKey: contentKey, animate: live || state.pendingLive });
  }
  const animate = contentKey === state.activeKey ? state.animate : live || state.pendingLive;
  return <Suspense fallback={<PendingSurface live={animate}/>}><Reveal key={contentKey} animate={animate}>{children}</Reveal></Suspense>;
}

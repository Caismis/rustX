/* Copyright (c) 2026 DeepSeek. MIT. See PROVENANCE.md. */
import { useCallback, useEffect, useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import { computeColumns, SIDEBAR_DEFAULT, SIDEBAR_AUTO_COLLAPSE, SIDEBAR_MIN, SIDEBAR_MAX, PRIMARY_RAIL_WIDTH, clampWidth } from './columns';
import css from './AppFrame.module.css';
export interface SidebarGeometry { overlay: boolean; collapsed: boolean; width: number; toggleSidebar: () => void; dismissSidebar: () => void }
function DragHandle(props: { side: 'sidebar' | 'rightbar'; left: number; onStart: () => void; onDrag: (dx: number) => void; onEnd: () => void }) {
  const [dragging, setDragging] = useState(false)
  const origin = useRef(0)
  const latest = useRef(0)
  const frame = useRef<number | null>(null)
  const capture = useRef<{ element: HTMLDivElement; id: number } | null>(null)
  const callbacks = useRef({ onStart: props.onStart, onDrag: props.onDrag, onEnd: props.onEnd })
  callbacks.current = { onStart: props.onStart, onDrag: props.onDrag, onEnd: props.onEnd }

  const endDrag = useCallback(() => {
    const active = capture.current
    if (active === null) return
    capture.current = null
    if (frame.current !== null) { cancelAnimationFrame(frame.current); frame.current = null }
    if (active.element.hasPointerCapture(active.id)) active.element.releasePointerCapture(active.id)
    setDragging(false)
    callbacks.current.onEnd()
  }, [])
  useEffect(() => endDrag, [endDrag])

  const onPointerDown = useCallback((e: React.PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0 || capture.current !== null) return
    e.preventDefault()
    e.currentTarget.setPointerCapture(e.pointerId)
    capture.current = { element: e.currentTarget, id: e.pointerId }
    origin.current = e.clientX
    latest.current = e.clientX
    callbacks.current.onStart()
    setDragging(true)
  }, [])
  const onPointerMove = useCallback((e: React.PointerEvent<HTMLDivElement>) => {
    if (capture.current?.id !== e.pointerId) return
    latest.current = e.clientX
    frame.current ??= requestAnimationFrame(() => {
      frame.current = null
      callbacks.current.onDrag(latest.current - origin.current)
    })
  }, [])
  const onPointerUp = useCallback((e: React.PointerEvent<HTMLDivElement>) => {
    if (capture.current?.id !== e.pointerId) return
    callbacks.current.onDrag(e.clientX - origin.current)
    endDrag()
  }, [endDrag])
  const onPointerCancel = useCallback((e: React.PointerEvent<HTMLDivElement>) => {
    if (capture.current?.id === e.pointerId) endDrag()
  }, [endDrag])

  return (
    <div
      className={css.handle}
      style={{ left: props.left }}
      data-side={props.side}
      data-dragging={dragging || undefined}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerCancel}
      onLostPointerCapture={onPointerCancel}
    />
  )
}


export interface SidebarPreference { width: number; collapsed: boolean }

/** Permanent primary rail, optional Home sidebar, content and retained workbench. */
export function AppFrame({ primary, home = true, initialSidebarPreference, onSidebarPreferenceChange, sidebar, children, rightPanel, rightOpen = false, overlay, dismissSidebarLabel, onSidebarDismiss, sidebarPublication }: {
  primary: (geometry: { sidebarExpanded: boolean; expandSidebar: () => void }) => ReactNode;
  home?: boolean; dismissSidebarLabel: string;
  initialSidebarPreference?: Partial<SidebarPreference>;
  onSidebarPreferenceChange?: (value: SidebarPreference) => void;
  sidebarPublication?: { sidebar: boolean };
  /** User/breakpoint retirement of an overlay gesture; never native lifecycle work. */
  onSidebarDismiss?: () => void;
  sidebar: (geometry: SidebarGeometry) => ReactNode; children: ReactNode;
  rightPanel?: (geometry: { width: number; viewportWidth: number; canShow: boolean }) => ReactNode;
  rightOpen?: boolean; overlay?: ReactNode;
}) {
  const frameRef = useRef<HTMLDivElement>(null);
  const primaryRef = useRef<HTMLDivElement>(null);
  const sidebarRef = useRef<HTMLDivElement>(null);
  const [viewport, setViewport] = useState(() => window.innerWidth);
  const [preference, setPreference] = useState<SidebarPreference>(() => ({
    width: typeof initialSidebarPreference?.width === 'number' && Number.isFinite(initialSidebarPreference.width) ? clampWidth(initialSidebarPreference.width, SIDEBAR_MIN, SIDEBAR_MAX) : SIDEBAR_DEFAULT,
    collapsed: initialSidebarPreference?.collapsed === true,
  }));
  const [narrowExpanded, setNarrowExpanded] = useState(false);
  const [rightWidth, setRightWidth] = useState<number>();
  const [dragging, setDragging] = useState(false);
  useEffect(() => { onSidebarPreferenceChange?.(preference); }, [preference, onSidebarPreferenceChange]);
  useLayoutEffect(() => {
    const element = frameRef.current!;
    const measure = () => { const width = element.getBoundingClientRect().width; if (width > 0) setViewport(width); };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  const narrow = viewport < SIDEBAR_AUTO_COLLAPSE;
  const [previousNarrow, setPreviousNarrow] = useState(narrow);
  if (previousNarrow !== narrow) { setPreviousNarrow(narrow); setNarrowExpanded(false); }
  const collapsed = narrow ? !narrowExpanded : preference.collapsed;
  const expanded = home && !collapsed;
  const floatingSidebar = expanded && narrow;
  const normal = computeColumns(viewport, expanded && !narrow ? preference.width : 0, rightWidth ?? viewport * .45);
  const cols = computeColumns(viewport, expanded && !narrow ? preference.width : 0, home && rightOpen ? rightWidth ?? viewport * .45 : 0);
  const visibleSidebarWidth = narrow ? Math.min(preference.width, viewport - PRIMARY_RAIL_WIDTH) : preference.width;
  const base = useRef(0);
  const wasFloating = useRef(false);
  useLayoutEffect(() => {
    if (wasFloating.current && !narrow) onSidebarDismiss?.();
    if (floatingSidebar) sidebarRef.current?.querySelector<HTMLElement>('[data-sidebar-toggle]')?.focus();
    else if (wasFloating.current && home) primaryRef.current?.querySelector<HTMLElement>('[data-destination="home"]')?.focus();
    wasFloating.current = floatingSidebar;
  }, [floatingSidebar, narrow, home, onSidebarDismiss]);
  // Page switches and successful publication close only the overlay geometry.
  useLayoutEffect(() => { if (!home || sidebarPublication?.sidebar) setNarrowExpanded(false); }, [home, sidebarPublication]);
  const dismissSidebar = () => setNarrowExpanded(false);
  const userDismissSidebar = () => { onSidebarDismiss?.(); dismissSidebar(); };
  const expandSidebar = () => { if (narrow) setNarrowExpanded(true); else setPreference(value => ({ ...value, collapsed: false })); };
  const toggleSidebar = () => {
    if (narrow) { if (floatingSidebar) userDismissSidebar(); else expandSidebar(); }
    else {
      if (!collapsed) primaryRef.current?.querySelector<HTMLElement>('[data-destination="home"]')?.focus({ preventScroll: true });
      setPreference(value => ({ ...value, collapsed: !value.collapsed }));
    }
  };
  return <div ref={frameRef} className={css.frame} data-harness-frame data-primary-page={home ? 'home' : 'page'}
    data-sidebar-overlay={floatingSidebar || undefined}
    onKeyDown={event => { if (floatingSidebar && event.key === 'Escape' && !event.defaultPrevented) { event.preventDefault(); userDismissSidebar(); } }}
    data-sidebar-collapsed={!expanded || undefined} data-rightbar-collapsed={cols.rightbar === 0 || undefined}
    data-dragging={dragging || undefined} style={{ gridTemplateColumns: `${cols.primary}px ${cols.sidebar}px minmax(0, 1fr) ${cols.rightbar}px` }}>
    <div ref={primaryRef} className={css.primaryCol}>{primary({ sidebarExpanded: expanded, expandSidebar })}</div>
    <div ref={sidebarRef} id="home-sidebar" className={css.sidebarCol} hidden={!expanded} style={{ width: visibleSidebarWidth }}>{sidebar({ overlay: floatingSidebar, collapsed: !expanded, width: visibleSidebarWidth, toggleSidebar, dismissSidebar })}</div>
    <main className={css.centerCol} inert={floatingSidebar}>{children}</main>
    <div className={css.rightbarCol} data-rightbar-col hidden={!home} inert={floatingSidebar || !home}>{rightPanel?.({ width: normal.rightbar, viewportWidth: viewport, canShow: normal.rightbar > 0 })}</div>
    {floatingSidebar && <button type="button" className={css.sidebarBackdrop} tabIndex={-1} aria-label={dismissSidebarLabel} onClick={userDismissSidebar} />}
    <div className={css.overlayLayer} data-shell-overlay>{overlay}</div>
    {expanded && !narrow && <DragHandle side="sidebar" left={cols.primary + cols.sidebar} onStart={() => { base.current = cols.sidebar; setDragging(true); }}
      onDrag={dx => setPreference(value => ({ ...value, width: clampWidth(base.current + dx, SIDEBAR_MIN, SIDEBAR_MAX) }))} onEnd={() => setDragging(false)} />}
    {home && rightOpen && normal.rightbar > 0 && <DragHandle side="rightbar" left={viewport - normal.rightbar} onStart={() => { base.current = normal.rightbar; setDragging(true); }}
      onDrag={dx => setRightWidth(base.current - dx)} onEnd={() => setDragging(false)} />}
  </div>;
}

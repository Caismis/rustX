/* Copyright (c) 2026 DeepSeek. MIT. See PROVENANCE.md. */
import { useCallback, useEffect, useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import { computeColumns, SIDEBAR_DEFAULT, SIDEBAR_AUTO_COLLAPSE, SIDEBAR_MIN, SIDEBAR_MAX, PRIMARY_RAIL_WIDTH, clampWidth } from './columns';
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
      className="absolute top-0 bottom-0 w-[8px] -ml-[4px] cursor-col-resize z-[11] touch-none transition-[left] duration-[var(--ds-transition-duration-slow)] ease-[var(--ds-ease-in-out)] motion-reduce:transition-none group-data-[dragging]/frame:transition-none group-data-[rightbar-fullscreen]/frame:transition-none group-data-[rightbar-instant]/frame:transition-none [[data-windows-titlebar]_&]:top-[var(--dsh-windows-titlebar-height)]"
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
  return <div ref={frameRef} className="group/frame relative grid h-full overflow-clip box-border grid-rows-[minmax(0,1fr)] bg-[var(--dsw-alias-bg-base)] [--dsh-primary-rail-width:56px] transition-[grid-template-columns] duration-[var(--ds-transition-duration-slow)] ease-[var(--ds-ease-in-out)] data-[dragging]:transition-none motion-reduce:transition-none data-[rightbar-fullscreen]:transition-none data-[rightbar-instant]:transition-none [[data-windows-titlebar]_&]:[--dsh-windows-content-radius:16px] [[data-windows-titlebar]_&]:pt-[var(--dsh-windows-titlebar-height)] [[data-windows-titlebar]_&]:bg-[var(--dsw-specific-sidebar-fill)] [[data-windows-titlebar]_&]:before:content-[''] [[data-windows-titlebar]_&]:before:absolute [[data-windows-titlebar]_&]:before:inset-x-0 [[data-windows-titlebar]_&]:before:top-0 [[data-windows-titlebar]_&]:before:h-[var(--dsh-windows-titlebar-height)] [[data-windows-titlebar]_&]:before:bg-[var(--dsw-specific-sidebar-fill)] [[data-windows-titlebar]_&]:before:[-webkit-app-region:drag] [[data-platform='darwin']_&]:bg-transparent" data-harness-frame data-primary-page={home ? 'home' : 'page'}
    data-sidebar-overlay={floatingSidebar || undefined}
    onKeyDown={event => { if (floatingSidebar && event.key === 'Escape' && !event.defaultPrevented) { event.preventDefault(); userDismissSidebar(); } }}
    data-sidebar-collapsed={!expanded || undefined} data-rightbar-collapsed={cols.rightbar === 0 || undefined}
    data-dragging={dragging || undefined} style={{ gridTemplateColumns: `${cols.primary}px ${cols.sidebar}px minmax(0, 1fr) ${cols.rightbar}px` }}>
    <div ref={primaryRef} className="[grid-column:1] [grid-row:1] min-w-0 z-[90] [border-right:0.5px_solid_var(--dsw-alias-border-l3)]">{primary({ sidebarExpanded: expanded, expandSidebar })}</div>
    <div ref={sidebarRef} id="home-sidebar" className="[grid-column:2] [grid-row:1] min-w-0 overflow-hidden bg-[var(--dsw-specific-sidebar-fill)] [border-right:0.5px_solid_var(--dsw-alias-border-l3)] [&[hidden]]:hidden group-data-[sidebar-overlay]/frame:absolute group-data-[sidebar-overlay]/frame:[grid-area:1/1/-1/-1] group-data-[sidebar-overlay]/frame:inset-y-0 group-data-[sidebar-overlay]/frame:right-auto group-data-[sidebar-overlay]/frame:left-[var(--dsh-primary-rail-width)] group-data-[sidebar-overlay]/frame:z-[85] group-data-[sidebar-overlay]/frame:shadow-[var(--dsw-elevation-prominent)] [[data-windows-titlebar]_&]:border-r-0 [[data-platform='darwin']_&]:bg-[color-mix(in_srgb,var(--dsw-specific-sidebar-fill)_60%,transparent)] [[data-platform='darwin']_&]:border-r-0" hidden={!expanded} style={{ width: visibleSidebarWidth }}>{sidebar({ overlay: floatingSidebar, collapsed: !expanded, width: visibleSidebarWidth, toggleSidebar, dismissSidebar })}</div>
    <main className="[grid-column:3] bg-[var(--dsw-alias-bg-base)] min-w-0 flex flex-col overflow-hidden [[data-windows-titlebar]_&]:bg-[var(--dsw-alias-bg-base)] [[data-windows-titlebar]_&]:rounded-tl-[var(--dsh-windows-content-radius)] [[data-windows-titlebar]_&]:[corner-shape:round] [[data-platform='darwin']_&]:[border-left:0.5px_solid_var(--dsw-alias-border-l3)]" inert={floatingSidebar}>{children}</main>
    <div className="[grid-column:4] static min-w-0 overflow-visible [&[hidden]]:hidden" data-rightbar-col hidden={!home} inert={floatingSidebar || !home}>{rightPanel?.({ width: normal.rightbar, viewportWidth: viewport, canShow: normal.rightbar > 0 })}</div>
    {floatingSidebar && <button type="button" className="absolute inset-y-0 right-0 left-[var(--dsh-primary-rail-width)] z-[84] border-0 p-0 bg-[var(--dsw-alias-bg-mask-1)] [[data-windows-titlebar]_&]:top-[var(--dsh-windows-titlebar-height)]" tabIndex={-1} aria-label={dismissSidebarLabel} onClick={userDismissSidebar} />}
    <div className="absolute inset-0 z-20 pointer-events-none [&>*]:pointer-events-auto" data-shell-overlay>{overlay}</div>
    {expanded && !narrow && <DragHandle side="sidebar" left={cols.primary + cols.sidebar} onStart={() => { base.current = cols.sidebar; setDragging(true); }}
      onDrag={dx => setPreference(value => ({ ...value, width: clampWidth(base.current + dx, SIDEBAR_MIN, SIDEBAR_MAX) }))} onEnd={() => setDragging(false)} />}
    {home && rightOpen && normal.rightbar > 0 && <DragHandle side="rightbar" left={viewport - normal.rightbar} onStart={() => { base.current = normal.rightbar; setDragging(true); }}
      onDrag={dx => setRightWidth(base.current - dx)} onEnd={() => setDragging(false)} />}
  </div>;
}

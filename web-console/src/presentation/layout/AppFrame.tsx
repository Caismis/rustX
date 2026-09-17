/* Copyright (c) 2026 DeepSeek. MIT. See PROVENANCE.md. */
import { useCallback, useEffect, useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import { computeColumns, SIDEBAR_DEFAULT, SIDEBAR_AUTO_COLLAPSE, SIDEBAR_MIN, SIDEBAR_MAX, clampWidth } from './columns';
import css from './AppFrame.module.css';
export interface SidebarGeometry { collapsed: boolean; width: number; toggleSidebar: () => void }
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


/** Harness measured three-column shell; every state here is disposable geometry. */
export function AppFrame({ sidebar, children, rightPanel, rightOpen = false, overlay }: {
  sidebar: (geometry: SidebarGeometry) => ReactNode; children: ReactNode;
  rightPanel?: (geometry: { width: number; viewportWidth: number; canShow: boolean }) => ReactNode;
  rightOpen?: boolean; overlay?: ReactNode;
}) {
  const frameRef = useRef<HTMLDivElement>(null);
  const [viewport, setViewport] = useState(() => window.innerWidth);
  const [sidebarWidth, setSidebarWidth] = useState(SIDEBAR_DEFAULT);
  const [narrowExpanded, setNarrowExpanded] = useState(false);
  const [rightWidth, setRightWidth] = useState<number>();
  const [dragging, setDragging] = useState(false);
  useLayoutEffect(() => {
    const element = frameRef.current!;
    let frame: number | undefined;
    const measure = () => { const width = element.getBoundingClientRect().width; if (width > 0) setViewport(width); };
    measure();
    const observer = new ResizeObserver(() => { frame ??= requestAnimationFrame(() => { frame = undefined; measure(); }); });
    observer.observe(element);
    return () => { observer.disconnect(); if (frame !== undefined) cancelAnimationFrame(frame); };
  }, []);
  const narrow = viewport < SIDEBAR_AUTO_COLLAPSE;
  const collapsed = narrow ? !narrowExpanded : sidebarWidth === 0;
  const preference = collapsed ? 0 : sidebarWidth || SIDEBAR_DEFAULT;
  const normal = computeColumns(viewport, preference, rightWidth ?? viewport * .45);
  const cols = computeColumns(viewport, preference, rightOpen ? rightWidth ?? viewport * .45 : 0);
  const base = useRef(0);
  const toggleSidebar = () => { if (narrow) setNarrowExpanded(value => !value); else setSidebarWidth(value => value === 0 ? SIDEBAR_DEFAULT : 0); };
  return <div ref={frameRef} className={css.frame} data-harness-frame
    data-sidebar-collapsed={collapsed || undefined} data-rightbar-collapsed={cols.rightbar === 0 || undefined}
    data-dragging={dragging || undefined} style={{ gridTemplateColumns: `${cols.sidebar}px minmax(0, 1fr) ${cols.rightbar}px` }}>
    <div className={css.sidebarCol}>{sidebar({ collapsed, width: cols.sidebar, toggleSidebar })}</div>
    <main className={css.centerCol}>{children}</main>
    <div className={css.rightbarCol} data-rightbar-col>{rightPanel?.({ width: normal.rightbar, viewportWidth: viewport, canShow: normal.rightbar > 0 })}</div>
    <div className={css.overlayLayer} data-shell-overlay>{overlay}</div>
    {!collapsed && <DragHandle side="sidebar" left={cols.sidebar} onStart={() => { base.current = cols.sidebar; setDragging(true); }}
      onDrag={dx => setSidebarWidth(clampWidth(base.current + dx, SIDEBAR_MIN, SIDEBAR_MAX))} onEnd={() => setDragging(false)} />}
    {rightOpen && normal.rightbar > 0 && <DragHandle side="rightbar" left={viewport - normal.rightbar} onStart={() => { base.current = normal.rightbar; setDragging(true); }}
      onDrag={dx => setRightWidth(base.current - dx)} onEnd={() => setDragging(false)} />}
  </div>;
}

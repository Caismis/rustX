/* Copyright (c) 2026 DeepSeek. MIT. Adapted from FileNavigator and NavigatorToggle; see PROVENANCE.md. */
import { useLayoutEffect, useReducer, useRef, useState, type CSSProperties } from 'react';
import { useTranslation } from '../../locale/react';
import { Tooltip } from '../../presentation/primitives/Tooltip';
import { IconFolderClose16, IconFolderOpenOutline16 } from '../../presentation/primitives/icons';
import { WorkspaceFile, WorkspaceFiles, type FileTabView, type WorkbenchCall } from './WorkbenchFiles';
import css from './WorkbenchNavigator.module.css';
import fileCss from '../../presentation/right-panel/WorkbenchFiles.module.css';
export interface WorkbenchNavigatorState { open: boolean; width: number; view: FileTabView }
/** The same pane keeps its tree, width and reading position across file replacement. */
export function WorkbenchFileNavigator({ navigator, path, call, view, onOpen, onNavigate }: {
  navigator: WorkbenchNavigatorState; path: string; call: WorkbenchCall; view: FileTabView;
  onOpen(path: string): void; onNavigate(path: string): void;
}) {
  const tx = useTranslation(), [, redraw] = useReducer(n => n + 1, 0);
  const root = useRef<HTMLDivElement>(null), toggle = useRef<HTMLButtonElement>(null);
  const drag = useRef<{ x: number; width: number }>(undefined), [dragging, setDragging] = useState(false);
  const [available, setAvailable] = useState<number>();
  useLayoutEffect(() => {
    const node = root.current!;
    const observer = new ResizeObserver(() => setAvailable(node.clientWidth));
    setAvailable(node.clientWidth); observer.observe(node); return () => observer.disconnect();
  }, []);
  const mode = available === undefined ? 'overlay' : available >= 560 ? 'push' : available >= 400 ? 'overlay' : 'fill';
  const maximum = available === undefined ? navigator.width : Math.max(0, available - (available >= 560 ? 336 : 120));
  const minimum = Math.min(180, maximum), width = Math.min(Math.max(minimum, navigator.width), maximum);
  const resize = (next: number) => { navigator.width = Math.min(Math.max(next, minimum), maximum); redraw(); };
  const label = tx(navigator.open ? 'artifacts:workbench.navigator-hide' : 'artifacts:workbench.navigator-show');
  const close = () => { navigator.open = false; redraw(); toggle.current?.focus({ preventScroll: true }); };
  return <div className={css.root} ref={root}>
    <WorkspaceFile {...{ path, call, view, onOpen }} tools={<Tooltip label={label} side="bottom" delayMs={500}>
      <button ref={toggle} type="button" className={fileCss.tool} aria-label={label} aria-expanded={navigator.open} data-files-navigator-toggle onClick={() => { navigator.open = !navigator.open; redraw(); }}>{navigator.open ? <IconFolderOpenOutline16/> : <IconFolderClose16/>}</button>
    </Tooltip>} navigator={navigator.open && <aside className={`${css.navigator} ${css[mode]} ${dragging ? css.dragging : ''}`} style={{ '--files-navigator-width': `${width}px` } as CSSProperties} aria-label={tx('artifacts:workbench.files')} data-files-navigator data-files-navigator-mode={mode} onKeyDown={event => { if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); close(); } }}>
      {mode !== 'fill' && <div className={css.separator} role="separator" aria-orientation="vertical" aria-label={tx('artifacts:workbench.navigator-resize')} aria-valuemin={minimum} aria-valuemax={maximum} aria-valuenow={width} tabIndex={0}
        onKeyDown={event => { const next = event.key === 'ArrowLeft' ? width + 20 : event.key === 'ArrowRight' ? width - 20 : event.key === 'Home' ? minimum : event.key === 'End' ? maximum : undefined; if (next !== undefined) { event.preventDefault(); resize(next); } }}
        onPointerDown={event => { if (event.button !== 0) return; event.preventDefault(); event.currentTarget.setPointerCapture(event.pointerId); drag.current = { x: event.clientX, width }; setDragging(true); }}
        onPointerMove={event => { if (drag.current) resize(drag.current.width + drag.current.x - event.clientX); }}
        onPointerUp={event => event.currentTarget.releasePointerCapture(event.pointerId)} onLostPointerCapture={() => { drag.current = undefined; setDragging(false); }}/ >}
      <WorkspaceFiles call={call} view={navigator.view} embedded current={path} onOpen={next => {
        if (mode === 'fill' || matchMedia('(width < 768px)').matches) navigator.open = false;
        onNavigate(next); redraw();
      }}/>
    </aside>}/>
  </div>;
}

/* Copyright (c) 2026 DeepSeek. MIT. SidebarRight panel seat; see PROVENANCE.md. */
import type { ReactNode } from 'react';
import { Button } from '../primitives/Button';
import dockCss from './SidebarRight.module.css';
import inspectorCss from './InspectorPanel.module.css';
export function RightPanel({ open, close, width, canShow, title, closeLabel, fullscreen = false, actions, headerless = false, children }: {
  headerless?: boolean; actions?: ReactNode; fullscreen?: boolean; open: boolean; close: () => void; width: number; canShow: boolean; title: string; closeLabel: string; children: ReactNode;
}) {
  const css = headerless ? dockCss : inspectorCss;
  return <aside className={css.panel} aria-label={title} inert={!open && !headerless} data-sidebar-right-open={open || undefined}
    data-sidebar-right-panel={fullscreen || !canShow ? 'fullscreen' : 'normal'} style={{ width: !fullscreen && canShow ? width : '100%', '--dsh-sidebar-width': `${width}px` } as import('react').CSSProperties}>
    {!headerless && <header className={css.extensionHeader}><strong>{title}</strong><div style={{ display: 'flex', gap: 8 }}>{actions}<Button onClick={close} aria-label={closeLabel}>×</Button></div></header>}
    <div className={css.extensionBody}>{children}</div>
  </aside>;
}

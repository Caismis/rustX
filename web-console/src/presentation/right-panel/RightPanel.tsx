/* Copyright (c) 2026 DeepSeek. MIT. SidebarRight panel seat; see PROVENANCE.md. */
import type { ReactNode } from 'react';
import dockCss from './SidebarRight.module.css';
import previewCss from './PreviewPanel.module.css';
export function RightPanel({ open, width, canShow, title, fullscreen = false, docked = false, children }: {
  docked?: boolean; fullscreen?: boolean; open: boolean; width: number; canShow: boolean; title: string; children: ReactNode;
}) {
  const css = docked ? dockCss : previewCss;
  return <aside className={css.panel} role={open ? 'complementary' : 'presentation'} aria-label={open ? title : undefined} inert={!open && !docked} data-sidebar-right-open={open || undefined}
    data-sidebar-right-panel={fullscreen || !canShow ? 'fullscreen' : 'normal'} style={{ width: !fullscreen && canShow ? width : '100%', '--dsh-sidebar-width': `${width}px` } as import('react').CSSProperties}>
    <div className={css.extensionBody}>{children}</div>
  </aside>;
}

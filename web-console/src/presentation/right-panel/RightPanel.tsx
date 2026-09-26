/* Copyright (c) 2026 DeepSeek. MIT. SidebarRight panel seat; see PROVENANCE.md. */
import type { ReactNode } from 'react';
import { Button } from '../primitives/Button';
import css from './SidebarRight.module.css';
export function RightPanel({ open, close, width, canShow, title, closeLabel, children }: {
  open: boolean; close: () => void; width: number; canShow: boolean; title: string; closeLabel: string; children: ReactNode;
}) {
  return <aside className={css.panel} aria-label={title} inert={!open} data-sidebar-right-open={open || undefined}
    data-sidebar-right-panel={!canShow ? 'fullscreen' : 'normal'} style={{ width: canShow ? width : '100%' }}>
    <header className={css.extensionHeader}><strong>{title}</strong><Button onClick={close} aria-label={closeLabel}>×</Button></header>
    <div className={css.extensionBody}>{children}</div>
  </aside>;
}

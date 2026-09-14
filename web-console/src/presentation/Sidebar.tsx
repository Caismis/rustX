// Extracted from DeepSeek Harness ui-sidebar/SidebarRoot; see PROVENANCE.md.
import type { ReactNode } from 'react';
import css from './Sidebar.module.css';
export function Sidebar({ children, footer }: { children: ReactNode; footer: ReactNode }) {
  return <div className={css.root}>
    <div className={css.logoRow}><span className="wordmark">rust<span>X</span></span><span className="eyebrow">DEVELOPER CONSOLE</span></div>
    <div className={css.regionArea}>{children}</div>
    <div className={css.footArea}>{footer}</div>
  </div>;
}

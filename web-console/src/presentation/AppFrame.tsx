// Extracted from DeepSeek Harness ui-layout/AppFrame; see PROVENANCE.md.
// The three-column presentation remains. React props replace the Host slot graph.
import type { ReactNode } from 'react';
import css from './AppFrame.module.css';
export function AppFrame({ sidebar, children, inspector }: { sidebar: ReactNode; children: ReactNode; inspector: ReactNode }) {
  return <div className={css.frame}>
    <aside className={css.sidebarCol}>{sidebar}</aside>
    <main className={css.centerCol}>{children}</main>
    <aside className={css.rightbarCol} aria-label="Developer inspector">{inspector}</aside>
  </div>;
}

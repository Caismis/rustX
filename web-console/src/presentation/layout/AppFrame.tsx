/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
// Copyright (c) 2026 DeepSeek. MIT. Adapted AppFrame; see PROVENANCE.md.
import type { ReactNode } from 'react';
import css from './AppFrame.module.css';
/** Geometry only. Occupants own navigation, runtime bindings and actions. */
export function AppFrame({ navigation, children, dock, dockLabel = 'Inspector' }: {
  navigation?: ReactNode; children: ReactNode; dock?: ReactNode; dockLabel?: string;
}) {
  return <div className={css.frame} data-navigation={!!navigation} data-dock={!!dock}>
    {navigation && <aside className={css.sidebarCol} aria-label="Navigation">{navigation}</aside>}
    <main className={css.centerCol}>{children}</main>
    {dock && <aside className={css.rightbarCol} aria-label={dockLabel}>{dock}</aside>}
  </div>;
}

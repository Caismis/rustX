import type { ReactNode } from 'react';
import css from './SettingsContent.module.css';

export function Badge({ children, tone }: { children: ReactNode; tone?: 'error' | 'success' }) {
  return <span className={css.badge} data-tone={tone}>{children}</span>;
}

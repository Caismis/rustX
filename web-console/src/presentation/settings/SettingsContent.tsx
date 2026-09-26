import { useTranslation } from '../../locale/react';
import type { ReactNode } from 'react';
import css from './SettingsContent.module.css';

/** Product-neutral seats using the Harness Settings card and row vocabulary. */
export function SettingsCard({ title, meta, actions, children }: { title: string; meta?: ReactNode; actions?: ReactNode; children?: ReactNode }) {
  return <article className={css.card}><div className={css.rowHead}><strong>{title}</strong>{meta}{actions}</div>{children}</article>;
}
export function Badge({ children, tone }: { children: ReactNode; tone?: 'error' | 'success' }) {
  return <span className={css.badge} data-tone={tone}>{children}</span>;
}
export function Facts({ rows }: { rows: readonly (readonly [string, ReactNode])[] }) {
  const tx = useTranslation();
  return <dl>{rows.map(([label, value], index) => <div key={index}><dt>{label}</dt><dd>{value ?? tx('settings:settings-content.not-specified')}</dd></div>)}</dl>;
}

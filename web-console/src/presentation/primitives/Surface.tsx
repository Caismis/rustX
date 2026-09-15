import type { HTMLAttributes, ReactNode } from 'react';
import clsx from 'clsx';
import { StateDot } from './StateDot';
import css from './Surface.module.css';
/** Shared passive card and feedback presentation for content and forms. */
export function Card({ className, ...props }: HTMLAttributes<HTMLDivElement>) {
  return <div className={clsx(css.card, className)} {...props} />;
}
export function Feedback({ kind, title, children }: { kind: 'loading' | 'empty' | 'error'; title: string; children?: ReactNode }) {
  return <Card role={kind === 'error' ? 'alert' : 'status'}>
    <div className={css.heading}>{kind !== 'empty' && <StateDot state={kind === 'loading' ? 'ongoing' : 'error'} />}<strong>{title}</strong></div>
    {children}
  </Card>;
}

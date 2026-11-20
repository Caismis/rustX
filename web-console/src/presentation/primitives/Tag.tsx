import type { ReactNode } from 'react';
import clsx from 'clsx';
import css from './Tag.module.css';

export type TagTone =
  | 'outline'
  | 'solid'
  | 'neutral'
  | 'quiet'
  | 'success'
  | 'info'
  | 'warning'
  | 'danger';

export function Tag({ tone = 'outline', className, children }: {
  tone?: TagTone;
  className?: string | undefined;
  children?: ReactNode;
}) {
  return <span className={clsx(css.tag, className)} data-tone={tone}>{children}</span>;
}

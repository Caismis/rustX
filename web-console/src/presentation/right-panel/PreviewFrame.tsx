/* Copyright (c) 2026 DeepSeek. MIT. Adapted from Harness PreviewFrame; see PROVENANCE.md. */
import type { ReactNode } from 'react';
import { PathLabel } from '../primitives/PathLabel';
import css from './PreviewFrame.module.css';
/** Every loading/ready/error state retains the same path header and body seat. */
export function PreviewFrame({ path, tools, banner, navigator, children, label, ...data }: {
  path: string; tools?: ReactNode; banner?: ReactNode; navigator?: ReactNode; children: ReactNode; label?: string;
  [attribute: `data-${string}`]: string | undefined;
}) {
  return <section className={css.preview} aria-label={label} {...data}>
    {banner}
    <header className={css.header} data-preview-header><PathLabel path={path} className={css.path}/>{tools}</header>
    <div className={css.content} data-textpreview-content><div className={css.main}>{children}</div>{navigator}</div>
  </section>;
}

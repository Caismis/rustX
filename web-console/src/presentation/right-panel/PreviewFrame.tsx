/* Copyright (c) 2026 DeepSeek. MIT. Adapted from Harness PreviewFrame; see PROVENANCE.md. */
import type { ReactNode } from 'react';
import { PathLabel } from '../primitives/PathLabel';
/** Every loading/ready/error state retains the same path header and body seat. */
export function PreviewFrame({ path, tools, banner, navigator, children, label, ...data }: {
  path: string; tools?: ReactNode; banner?: ReactNode; navigator?: ReactNode; children: ReactNode; label?: string;
  [attribute: `data-${string}`]: string | undefined;
}) {
  return <section className="flex flex-1 flex-col min-w-0 h-full min-h-0" aria-label={label} {...data}>
    {banner}
    <header className="flex flex-none gap-[4px] items-center box-border h-[38px] pl-[16px] pr-[6px] py-0 [border-bottom:0.5px_solid_var(--dsw-alias-border-l3)]" data-preview-header><PathLabel path={path} className="mr-[12px]"/>{tools}</header>
    <div className="relative isolate flex flex-1 min-w-0 min-h-0" data-textpreview-content><div className="isolate flex flex-col flex-1 min-w-0 min-h-0">{children}</div>{navigator}</div>
  </section>;
}

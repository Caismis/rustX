/* Copyright (c) 2026 DeepSeek. MIT. Source port; see PROVENANCE.md. */
/** Shared indeterminate loading feedback for document reads and rendering. */
import type { ReactNode } from 'react'
import clsx from 'clsx'
import { StateDot } from './StateDot'

/**
 * @param props - localized status label and compact inline placement for additional pages.
 * @returns a centered document loading status or an accessible inline spinner.
 */
export function LoadingIndicator({ label, inline = false }: {
  label: ReactNode
  inline?: boolean
}): ReactNode {
  return <span className={clsx('flex flex-col gap-[12px] items-center justify-center box-border w-full h-full min-h-0 p-[24px] [color:var(--dsw-alias-label-secondary)] [font-family:var(--dsw-font,sans-serif)] text-[var(--dsh-content-font-size,14px)] leading-[1.5] text-center whitespace-normal', inline && 'inline-flex flex-row gap-[8px] w-auto h-auto p-0 align-middle')} role="status" aria-label={typeof label === 'string' ? label : undefined} data-document-loading>
    <StateDot state="ongoing" size={inline ? 14 : 28} />
    {!inline && <span>{label}</span>}
  </span>
}

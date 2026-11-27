/* Copyright (c) 2026 DeepSeek. MIT. Source port; see PROVENANCE.md. */
/** Shared shortcut keycaps; callers supply the effective platform presentation. */
import clsx from 'clsx'
import { Kbd } from '@/components/ui/kbd'

/**
 * Render one command's keycaps without owning binding defaults or localized copy.
 * @param props - effective key labels, presentation variant and optional interaction styling.
 * @returns coss ui keycaps with plus-separated combinations grouped together.
 */
export function ShortcutKeys({ keys, variant = 'plain', className }: { keys: readonly string[]; variant?: 'plain' | 'tooltip'; className?: string | undefined }) {
 return (
 <span className={clsx('inline-flex flex-none items-center gap-[3px] [color:var(--dsw-alias-label-tertiary)] text-[12px] leading-[16px] whitespace-nowrap', variant === 'tooltip' && '[color:inherit] text-[11px] leading-[14px] gap-[2px] [&_kbd]:min-w-[16px] [&_kbd]:h-[16px] [&_kbd]:px-[2px] [&_kbd]:py-0 [&_kbd]:rounded-[4px] [&_kbd]:border-0 [&_kbd]:bg-[var(--dsw-alias-tooltip-key-bg)]', variant === 'tooltip' && keys.includes('+') && 'box-border h-[16px] px-[4px] py-0 rounded-[4px] bg-[var(--dsw-alias-tooltip-key-bg)] [&_kbd]:min-w-0! [&_kbd]:h-auto! [&_kbd]:p-0! [&_kbd]:border-0! [&_kbd]:bg-transparent!', className)}>
 {keys.map((key, index) => key === '+'
 ? <kbd key={index} className="">{key}</kbd>
 : <Kbd key={index}>{key}</Kbd>)}
 </span>
 )
}

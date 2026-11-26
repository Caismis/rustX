/* Copyright (c) 2026 DeepSeek. MIT. Source port; see PROVENANCE.md. */
/** Shared shortcut keycaps; callers supply the effective platform presentation. */
import clsx from 'clsx'
import { Kbd } from '@/components/ui/kbd'
import css from './ShortcutKeys.module.css'

/**
 * Render one command's keycaps without owning binding defaults or localized copy.
 * @param props - effective key labels, presentation variant and optional interaction styling.
 * @returns coss ui keycaps with plus-separated combinations grouped together.
 */
export function ShortcutKeys({ keys, variant = 'plain', className }: { keys: readonly string[]; variant?: 'plain' | 'tooltip'; className?: string | undefined }) {
  return (
    <span className={clsx(css.keys, variant === 'tooltip' && css.tooltip, variant === 'tooltip' && keys.includes('+') && css.joined, className)}>
      {keys.map((key, index) => key === '+'
        ? <kbd key={index} className={css.separator}>{key}</kbd>
        : <Kbd key={index}>{key}</Kbd>)}
    </span>
  )
}

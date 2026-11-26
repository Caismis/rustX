/* Copyright (c) 2026 DeepSeek. MIT. See PROVENANCE.md. */
// Pill: compact coss ui chip/control, selectable when given `onClick` and a
// static badge otherwise.

import type { ButtonHTMLAttributes, ReactNode } from 'react'
import { Badge } from '@/components/ui/badge'
import { Button as CossButton } from '@/components/ui/button'

/**
 * Render a pill chip. Interactive when onClick is supplied (renders a button);
 * otherwise a static badge.
 * @param props.active - selected/active visual state.
 * @returns pill element.
 */
export function Pill({ active = false, className, children, onClick, ...rest }: {
  active?: boolean
  className?: string | undefined
  children?: ReactNode
} & ButtonHTMLAttributes<HTMLButtonElement>) {
  if (!onClick) {
    return <Badge variant={active ? 'default' : 'secondary'} size="lg" className={className}>{children}</Badge>
  }
  return (
    <CossButton
      type="button"
      variant={active ? 'default' : 'secondary'}
      size="xs"
      className={className}
      onClick={onClick}
      {...rest}
    >
      {children}
    </CossButton>
  )
}

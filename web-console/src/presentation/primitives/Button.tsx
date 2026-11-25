/* Copyright (c) 2026 DeepSeek. MIT. See PROVENANCE.md. */
// Button: coss ui button atom behind the rustX variant vocabulary. Keeping
// the vocabulary here prevents the external component API from leaking through
// the application while the visual implementation is coss-owned.

import { forwardRef, type ButtonHTMLAttributes, type ReactNode } from 'react'
import { Button as CossButton } from '@/components/ui/button'

/** Visual variant, backed by the corresponding coss ui variant. */
export type ButtonVariant = 'primary' | 'ghost' | 'outline' | 'toolbar' | 'danger' | 'danger-outline'

const VARIANT_MAP = {
  primary: 'default',
  ghost: 'ghost',
  outline: 'outline',
  toolbar: 'secondary',
  danger: 'destructive',
  'danger-outline': 'destructive-outline',
} as const

/**
 * Render a button.
 * @param props.variant - visual family (default 'ghost'). 'danger' is a
 * destructive action (e.g. confirming a deletion).
 * @param props.size - 'md' standard control or 'sm' compact control.
 * @param props.icon - optional leading icon node.
 * @returns the button element; native button attributes pass through.
 */
export const Button = forwardRef<HTMLButtonElement, {
  variant?: ButtonVariant
  size?: 'md' | 'sm'
  icon?: ReactNode
  className?: string | undefined
  children?: ReactNode
} & ButtonHTMLAttributes<HTMLButtonElement>>(function Button({ variant = 'ghost', size = 'md', icon, className, children, ...rest }, ref) {
  return (
    <CossButton
      ref={ref}
      variant={VARIANT_MAP[variant]}
      size={size === 'sm' ? 'sm' : 'default'}
      className={className}
      {...rest}
    >
      {icon}
      {children}
    </CossButton>
  )
})

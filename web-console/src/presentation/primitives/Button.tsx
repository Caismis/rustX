/* Copyright (c) 2026 DeepSeek. MIT. See PROVENANCE.md. */
// Button: coss ui button atom behind the rustX variant vocabulary. Keeping
// the vocabulary here prevents the external component API from leaking through
// the application while the visual implementation is coss-owned.

import { forwardRef, type ButtonHTMLAttributes, type ReactNode } from 'react'
import clsx from 'clsx'
import { Button as CossButton } from '@/components/ui/button'

/** rustX design geometry (figma): md 36px/14px pad/r18, sm 28px/10px pad/r14.
 * coss sizes are smaller and square-er; the important modifiers win against
 * coss responsive (`sm:`) utilities regardless of stylesheet order. */
const SIZE_GEOMETRY = {
  md: 'h-9! rounded-[18px]! px-[14px]! text-[14px]! leading-[22px]! gap-[4px]!',
  sm: 'h-7! rounded-[14px]! px-[10px]! text-[12px]! leading-[18px]! gap-[4px]!',
} as const

/** rustX design fills (figma): primary is the dark capsule, not coss blue. */
const VARIANT_COLOR = {
  primary: 'border-transparent! shadow-none! bg-(--dsw-alias-button-primary-fill)! text-(--dsw-alias-label-primary-foreground)! hover:bg-(--dsw-alias-button-primary-hover)!',
  ghost: 'border-transparent! shadow-none! hover:bg-(--dsw-alias-interactive-bg-hover)! active:bg-(--dsw-alias-interactive-bg-active)!',
  outline: 'shadow-none! bg-transparent! [border:0.5px_solid_var(--dsw-alias-border-l3)]! hover:bg-(--dsw-alias-interactive-bg-hover)!',
  toolbar: 'border-transparent! shadow-none! bg-(--dsw-alias-button-tool-bar-fill)! hover:bg-(--dsw-alias-button-tool-bar-hover)!',
  danger: '',
  'danger-outline': '',
} as const

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
      className={clsx(SIZE_GEOMETRY[size], VARIANT_COLOR[variant], className)}
      {...rest}
    >
      {icon}
      {children}
    </CossButton>
  )
})

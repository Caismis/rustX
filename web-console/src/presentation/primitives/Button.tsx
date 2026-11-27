/* Copyright (c) 2026 DeepSeek. MIT. See PROVENANCE.md. */
// Button: coss ui button atom behind the rustX variant vocabulary. Keeping
// the vocabulary here prevents the external component API from leaking through
// the application while the visual implementation is coss-owned.

import { forwardRef, type ButtonHTMLAttributes, type ReactNode } from 'react'
import clsx from 'clsx'
import { Button as CossButton } from '@/components/ui/button'

/** rustX design fills (figma): primary is the dark capsule, not coss blue. */
const VARIANT_COLOR = {
  primary: 'border-0! [box-shadow:none]! bg-(--dsw-alias-button-primary-fill)! text-(--dsw-alias-label-primary-foreground)! hover:bg-(--dsw-alias-button-primary-hover)!',
  ghost: 'border-0! [box-shadow:none]! hover:bg-(--dsw-alias-interactive-bg-hover)! active:bg-(--dsw-alias-interactive-bg-active)!',
  outline: '[box-shadow:none]! bg-transparent! [border:0.5px_solid_var(--dsw-alias-border-l3)]! hover:bg-(--dsw-alias-interactive-bg-hover)!',
  toolbar: 'border-0! [box-shadow:none]! bg-(--dsw-alias-button-tool-bar-fill)! hover:bg-(--dsw-alias-button-tool-bar-hover)!',
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

/** Consumer sizing utilities win over the design geometry, restoring the CSS
 * Modules cascade where the consumer's rule always won: when the consumer
 * supplies a bare sizing token (`p-0`, `h-[28px]`, `rounded-full`), the
 * primitive drops that property from its own geometry instead of overriding
 * it. The important modifiers on the remaining geometry only ever compete
 * with coss's own base/size utilities, never with consumer intent.
 * Variant-prefixed tokens (`hover:`, `[&_svg]:`) don't count — they never
 * apply in the base state. */
const has = (className: string | undefined, pattern: RegExp): boolean =>
  className !== undefined && className.split(/\s+/).some(token => token !== '' && !token.includes(':') && pattern.test(token))
const geometryFor = (size: 'md' | 'sm', className: string | undefined): string => {
  const g = {
    md: { h: 'h-9!', rounded: 'rounded-[18px]!', px: 'px-[14px]!', text: 'text-[14px]!', leading: 'leading-[22px]!', gap: 'gap-[4px]!' },
    sm: { h: 'h-7!', rounded: 'rounded-[14px]!', px: 'px-[10px]!', text: 'text-[12px]!', leading: 'leading-[18px]!', gap: 'gap-[4px]!' },
  }[size]
  return [
    has(className, /^(?:h|size|min-h|max-h)-/) ? '' : g.h,
    has(className, /^rounded/) ? '' : g.rounded,
    has(className, /^(?:p|px|size)-/) ? '' : g.px,
    has(className, /^text-\[/) ? '' : g.text,
    has(className, /^leading-/) ? '' : g.leading,
    has(className, /^gap-/) ? '' : g.gap,
  ].filter(Boolean).join(' ')
}

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
      className={clsx(geometryFor(size, className), VARIANT_COLOR[variant], className)}
      {...rest}
    >
      {icon}
      {children}
    </CossButton>
  )
})

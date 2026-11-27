/* Copyright (c) 2026 DeepSeek. MIT. See PROVENANCE.md. */
// Input: coss ui text input atom. Composer textareas are NOT this atom — they
// live with the conversation package.

import { forwardRef, type InputHTMLAttributes, type ReactNode } from 'react'
import clsx from 'clsx'
import { Input as CossInput } from '@/components/ui/input'
import { InputGroup, InputGroupAddon, InputGroupInput } from '@/components/ui/input-group'

/**
 * Render a text input with an optional leading icon.
 * @param props.icon - optional leading icon node.
 * @returns the coss input, or an input group when an icon is present.
 */
/* rustX design geometry (pre-coss Input primitive): the wrapped field is h32
 * with a 6px icon gap, 16px icon box, 14/22 input text. coss owns the visual
 * implementation but its default geometry (18px addon icons, 34px leading,
 * negative svg margins) is foreign to the design language, so the design
 * tokens here are important-ified to win over the coss base. */
const GROUP_DESIGN = 'h-8 min-h-0! gap-[6px]! rounded-[8px]! px-[8px] shadow-none! before:hidden! has-focus-visible:ring-0! text-[14px]! sm:text-[14px]!'
const ADDON_DESIGN = 'order-first! w-4! h-4! p-0! gap-0! [color:var(--dsw-alias-label-tertiary)]! [&_svg]:size-4! [&_svg]:opacity-100! [&_svg]:mx-0!'
const INPUT_DESIGN = 'h-auto! py-px! px-0! text-[14px]! leading-[22px]! [color:var(--dsw-alias-label-primary)]! placeholder:[color:var(--dsw-alias-label-dimmed)] sm:h-auto! sm:leading-[22px]!'

export const Input = forwardRef<HTMLInputElement, {
  icon?: ReactNode
  className?: string
  unstyled?: boolean
} & InputHTMLAttributes<HTMLInputElement>>(function Input({ icon, className, unstyled, ...rest }, ref) {
  if (icon == null) {
    return <CossInput ref={ref} data-primitive-input="" className={clsx('h-8! text-[14px]! leading-[22px]! sm:h-8! sm:leading-[22px]!', className)} unstyled={unstyled} {...rest} />
  }
  return (
    <InputGroup className={clsx(GROUP_DESIGN, className)}>
      <InputGroupAddon className={ADDON_DESIGN}>{icon}</InputGroupAddon>
      <InputGroupInput ref={ref} data-primitive-input="" className={INPUT_DESIGN} {...rest} />
    </InputGroup>
  )
})

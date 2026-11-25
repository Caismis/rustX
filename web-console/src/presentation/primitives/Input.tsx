/* Copyright (c) 2026 DeepSeek. MIT. See PROVENANCE.md. */
// Input: coss ui text input atom. Composer textareas are NOT this atom — they
// live with the conversation package.

import { forwardRef, type InputHTMLAttributes, type ReactNode } from 'react'
import { Input as CossInput } from '@/components/ui/input'
import { InputGroup, InputGroupAddon, InputGroupInput } from '@/components/ui/input-group'

/**
 * Render a text input with an optional leading icon.
 * @param props.icon - optional leading icon node.
 * @returns the coss input, or an input group when an icon is present.
 */
export const Input = forwardRef<HTMLInputElement, {
  icon?: ReactNode
  className?: string
  unstyled?: boolean
} & InputHTMLAttributes<HTMLInputElement>>(function Input({ icon, className, unstyled, ...rest }, ref) {
  if (icon == null) {
    return <CossInput ref={ref} data-primitive-input="" className={className} unstyled={unstyled} {...rest} />
  }
  return (
    <InputGroup className={className}>
      <InputGroupAddon>{icon}</InputGroupAddon>
      <InputGroupInput ref={ref} data-primitive-input="" {...rest} />
    </InputGroup>
  )
})

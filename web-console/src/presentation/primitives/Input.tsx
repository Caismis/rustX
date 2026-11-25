/* Copyright (c) 2026 DeepSeek. MIT. See PROVENANCE.md. */
// Input: coss ui text input atom. Composer textareas are NOT this atom — they
// live with the conversation package.

import type { InputHTMLAttributes, ReactNode } from 'react'
import { Input as CossInput } from '@/components/ui/input'
import { InputGroup, InputGroupAddon, InputGroupInput } from '@/components/ui/input-group'

/**
 * Render a text input with an optional leading icon.
 * @param props.icon - optional leading icon node.
 * @returns the coss input, or an input group when an icon is present.
 */
export function Input({ icon, className, ...rest }: {
  icon?: ReactNode
  className?: string
} & InputHTMLAttributes<HTMLInputElement>) {
  if (icon == null) {
    return <CossInput data-primitive-input="" className={className} {...rest} />
  }
  return (
    <InputGroup className={className}>
      <InputGroupAddon>{icon}</InputGroupAddon>
      <InputGroupInput data-primitive-input="" {...rest} />
    </InputGroup>
  )
}

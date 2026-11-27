/* Copyright (c) 2026 DeepSeek. MIT. See PROVENANCE.md. */
// Switch: coss ui two-state toggle behind rustX's controlled-control contract.
// `label` is required and has no default, so a render site cannot ship the
// control without an accessible name.

import clsx from 'clsx'
import { Switch as CossSwitch } from '@/components/ui/switch'

/** rustX design geometry (figma): track 36×20 r10 padding 2, thumb 16px
 * translating 16px when on; track/thumb colors ride the brand tokens.
 * coss geometry is smaller and its checked translate is lost to a calc()
 * missing the spaces CSS requires around `-`, so the design lands here as
 * important overrides on the root plus descendant rules for the thumb. */
const DESIGN = clsx(
  'w-9! h-5! p-[2px]! rounded-[10px]! [corner-shape:round]',
  'data-checked:bg-(--dsw-alias-brand-primary)! data-unchecked:bg-(--dsw-alias-border-l3)!',
  '[&_[data-slot=switch-thumb]]:size-4! [&_[data-slot=switch-thumb]]:rounded-full! [&_[data-slot=switch-thumb]]:[corner-shape:round]! [&_[data-slot=switch-thumb]]:bg-(--dsw-alias-label-primary-foreground)! [&_[data-slot=switch-thumb]]:shadow-none!',
  '[&_[data-slot=switch-thumb][data-checked]]:translate-x-4!',
)

/**
 * Render a toggle switch.
 * @param props.checked - the current state; the control is fully controlled.
 * @param props.onChange - called with the state the click asks for.
 * @param props.label - localized accessible name, owned by the render site.
 * @param props.disabled - whether the control refuses input; owners also set it
 * while a write is in flight, not only when a deployment locks the toggle.
 * @param props.title - localized hover text, typically why the toggle is locked.
 * @param props.className - extra class for layout placement.
 * @returns the switch element.
 */
export function Switch({ checked, onChange, label, disabled = false, title, className }: {
  checked: boolean
  onChange: (next: boolean) => void
  label: string
  disabled?: boolean
  title?: string | undefined
  className?: string | undefined
}) {
  return (
    <CossSwitch
      checked={checked}
      onCheckedChange={next => onChange(next)}
      aria-label={label}
      title={title}
      disabled={disabled}
      className={clsx(DESIGN, className)}
    />
  )
}

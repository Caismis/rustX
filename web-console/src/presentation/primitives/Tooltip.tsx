/* Copyright (c) 2026 DeepSeek. MIT. See PROVENANCE.md. */
// Tooltip: coss ui/Base UI tooltip behind rustX's single-anchor contract.

import { useEffect, useRef, useState, type ReactElement } from 'react'
import { Tooltip as CossTooltip, TooltipPopup, TooltipTrigger } from '@/components/ui/tooltip'
import { ShortcutKeys } from './ShortcutKeys'

/** Bubble placement relative to the anchor. */
export type TooltipSide = 'right' | 'bottom' | 'top'

type TooltipLabel = string | (() => string)

/**
 * Attach a hover/focus tooltip to an anchor element.
 * @param props.label - bubble text, or a resolver evaluated for the visible bubble.
 * @param props.shortcutKeys - effective platform key labels rendered as keycaps beside the text.
 * @param props.side - placement relative to the anchor (default 'right').
 * @param props.delayMs - hover delay in milliseconds; keyboard focus remains immediate.
 * @param props.disabled - suppress the bubble while true.
 * @param props.maxWidth - bubble width cap in pixels.
 * @param props.children - a single anchor element.
 * @returns the anchor with a coss ui tooltip.
 */
export function Tooltip({ label, shortcutKeys, side = 'right', delayMs = 0, disabled = false, maxWidth, children }: {
  label: TooltipLabel
  shortcutKeys?: readonly string[] | undefined
  side?: TooltipSide
  delayMs?: number
  disabled?: boolean
  maxWidth?: number
  children: ReactElement
}) {
  const [open, setOpen] = useState(false)
  const resolvedLabel = open ? (typeof label === 'function' ? label() : label) : typeof label === 'string' ? label : undefined
  const openTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const cancelOpenTimer = () => {
    if (openTimer.current !== null) clearTimeout(openTimer.current)
    openTimer.current = null
  }
  useEffect(() => cancelOpenTimer, [])
  return (
    <CossTooltip disabled={disabled} open={open} onOpenChange={(next, details) => {
      if (next && delayMs > 0 && details.reason === 'trigger-hover' && details.event.type === 'mouseenter') {
        cancelOpenTimer()
        openTimer.current = setTimeout(() => {
          openTimer.current = null
          setOpen(true)
        }, delayMs)
        return
      }
      if (!next) cancelOpenTimer()
      setOpen(next)
    }}>
      <TooltipTrigger render={children} delay={0} closeOnClick />
      <TooltipPopup role="tooltip" side={side} sideOffset={side === 'right' ? 10 : 8} style={maxWidth === undefined ? undefined : { maxWidth }}>
        {resolvedLabel && <span>{resolvedLabel}</span>}
        {!!shortcutKeys?.length && <ShortcutKeys keys={shortcutKeys} variant="tooltip" />}
      </TooltipPopup>
    </CossTooltip>
  )
}

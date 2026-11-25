/* Copyright (c) 2026 DeepSeek. MIT. See PROVENANCE.md. */
// Tooltip: coss ui/Base UI tooltip behind rustX's single-anchor contract.

import { createContext, useContext, useEffect, useRef, useState, type ReactElement } from 'react'
import { Tooltip as CossTooltip, TooltipPopup, TooltipTrigger } from '@/components/ui/tooltip'
import { ShortcutKeys } from './ShortcutKeys'

/** Bubble placement relative to the anchor. */
export type TooltipSide = 'right' | 'bottom' | 'top'

type TooltipLabel = string | (() => string)

/** Suppression channel from a tooltip to the tooltips above it: a visible
 * descendant bubble withdraws the ancestor bubble for as long as it shows. */
const TooltipSuppression = createContext<((suppressed: boolean) => void) | null>(null)

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
  const [pressDismissed, setPressDismissed] = useState(false)
  const suppressAncestors = useContext(TooltipSuppression)
  const [suppressed, setSuppressed] = useState(false)
  useEffect(() => {
    suppressAncestors?.(open && !disabled)
    return () => suppressAncestors?.(false)
  }, [suppressAncestors, open, disabled])
  const resolvedLabel = open ? (typeof label === 'function' ? label() : label) : typeof label === 'string' ? label : undefined
  const openTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const cancelOpenTimer = () => {
    if (openTimer.current !== null) clearTimeout(openTimer.current)
    openTimer.current = null
  }
  useEffect(() => cancelOpenTimer, [])
  return (
    <TooltipSuppression.Provider value={setSuppressed}>
    <CossTooltip disabled={disabled} open={open} onOpenChange={(next, details) => {
      if (next && delayMs > 0 && details.reason === 'trigger-hover' && details.event.type === 'mouseenter') {
        cancelOpenTimer()
        openTimer.current = setTimeout(() => {
          openTimer.current = null
          setOpen(true)
        }, delayMs)
        return
      }
      if (!next) {
        cancelOpenTimer()
        setOpen(false)
        // Activating the anchor withdraws its bubble immediately: the action
        // often changes the label or surface, so the old text must not remain
        // exposed for the length of an exit animation.
        if (details.reason === 'trigger-press') setPressDismissed(true)
        return
      }
      setPressDismissed(false)
      setOpen(next)
    }}>
      <TooltipTrigger render={children} delay={0} closeOnClick />
      {!pressDismissed && !suppressed && <TooltipPopup role="tooltip" side={side} sideOffset={side === 'right' ? 10 : 8} style={maxWidth === undefined ? undefined : { maxWidth }}>
        {resolvedLabel && <span>{resolvedLabel}</span>}
        {!!shortcutKeys?.length && <ShortcutKeys keys={shortcutKeys} variant="tooltip" />}
      </TooltipPopup>}
    </CossTooltip>
    </TooltipSuppression.Provider>
  )
}

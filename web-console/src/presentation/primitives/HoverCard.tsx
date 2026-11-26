import { useTranslation } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. See PROVENANCE.md. */
import { createContext, useCallback, useContext, useEffect, useLayoutEffect, useRef, useState } from 'react'
import type { ReactNode, RefObject, SyntheticEvent } from 'react'
import { createPortal } from 'react-dom'
import { autoUpdate, flip, hide, offset, shift, size, useFloating } from '@floating-ui/react-dom'
import { writeClipboard } from './clipboard.ts'
import { usePointerGrace } from './pointer-grace.ts'
import css from './HoverCard.module.css'

const VIEWPORT_MARGIN = 8
const PREVIEW_INSET = 24
const PREVIEW_MAX_HEIGHT = 420

interface GroupState {
  isWarm: () => boolean
  enter: (overCard: boolean) => void
  leave: () => void
  containsCard: (target: EventTarget | null) => boolean
  shown: (owner: symbol, dismiss: () => void, element: HTMLElement) => void
  release: (owner: symbol) => void
}

const HoverCardGroupContext = createContext<GroupState | null>(null)

/**
 * Share first-display dwell across a list and its active portal, including the card's pointer grace.
 */
export function HoverCardGroup({ children, className, as: Container = 'div' }: {
  children: ReactNode
  className?: string | undefined
  as?: 'div' | 'span'
}) {
  const [group] = useState<GroupState>(() => {
    let pointer: 'outside' | 'list' | 'card' = 'outside'
    let warm = false
    let active: { owner: symbol; dismiss: () => void; element: HTMLElement } | null = null
    return {
      isWarm: () => warm,
      enter: (overCard) => {
        pointer = overCard ? 'card' : 'list'
        if (overCard) warm = true
      },
      leave: () => {
        pointer = 'outside'
        if (active === null) warm = false
      },
      containsCard: target => target instanceof Node && active !== null && active.element.contains(target),
      shown: (owner, dismiss, element) => {
        if (pointer === 'card') { pointer = 'outside'; warm = false }
        if (pointer === 'list') warm = true
        const previous = active
        active = { owner, dismiss, element }
        if (previous !== null && previous.owner !== owner) previous.dismiss()
      },
      release: (owner) => {
        if (active?.owner !== owner) return
        active = null
        if (pointer !== 'list') { pointer = 'outside'; warm = false }
      },
    }
  })
  return (
    <HoverCardGroupContext.Provider value={group}>
      <Container
        className={className}
        onPointerOverCapture={(event) => {
          const overCard = group.containsCard(event.target)
          if (event.currentTarget.contains(event.target as Node) || overCard) group.enter(overCard)
        }}
        onPointerOutCapture={(event) => {
          const fromGroup = event.currentTarget.contains(event.target as Node) || group.containsCard(event.target)
          const intoGroup = (event.relatedTarget instanceof Node && event.currentTarget.contains(event.relatedTarget))
            || group.containsCard(event.relatedTarget)
          if (fromGroup && !intoGroup) group.leave()
        }}
      >
        {children}
      </Container>
    </HoverCardGroupContext.Provider>
  )
}

/**
 * Render an anchor with a hover-triggered preview card.
 * @param props.anchor - the hover target (rendered in place inside a wrapper span).
 * @param props.content - card content; the pointer may rest on it, so it is
 * readable and selectable, but it carries no dismissal affordance of its own.
 * @param props.openDelayMs - hover dwell before the card shows (default 500).
 * @param props.disabled - suppress opening; turning true closes an open card.
 * @param props.grouped - share first-display dwell across the surrounding list and active portal.
 * @param props.previewAnchor - use a wide preview above or below the trigger, sized from this element.
 * @param props.copyText - optional primary value copied by activation and
 * included in the card's accessible name.
 * @param props.copyLabel - localized accessible activation-label prefix.
 * @param props.copiedLabel - localized visible success label.
 * @returns anchor wrapper with the conditional portaled card.
 */
export function HoverCard({
  anchor, content, openDelayMs = 500, disabled = false, grouped = false,
  copyText, copyLabel, copiedLabel, previewAnchor,
}: {
  anchor: ReactNode
  content: ReactNode
  openDelayMs?: number
  disabled?: boolean
  grouped?: boolean
  previewAnchor?: RefObject<HTMLElement | null>
} & ({ copyText?: string | undefined; copyLabel: string; copiedLabel: string } | {
  copyText?: undefined
  copyLabel?: string
  copiedLabel?: string
})) {
  const tx = useTranslation();
  const rootRef = useRef<HTMLSpanElement>(null)
  const cardRef = useRef<HTMLDivElement>(null)
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const copyTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const copyHeightRef = useRef<number | null>(null)
  const copyEpochRef = useRef(0)
  const copyingRef = useRef(false)
  const mountedRef = useRef(true)
  const [open, setOpen] = useState(false)
  const [copied, setCopied] = useState(false)
  const parentGroup = useContext(HoverCardGroupContext)
  const group = grouped ? parentGroup : null
  const [groupOwner] = useState(() => Symbol('hover-card'))

  const clearCopied = useCallback(() => {
    if (copyTimerRef.current !== null) {
      clearTimeout(copyTimerRef.current)
      copyTimerRef.current = null
    }
    copyHeightRef.current = null
    setCopied(false)
  }, [])

  const close = useCallback(() => {
    copyEpochRef.current += 1
    clearCopied()
    setOpen(false)
  }, [clearCopied])

  const { arm: armClose, cancel: cancelClose } = usePointerGrace(close)

  const clearTimer = useCallback(() => {
    if (timerRef.current !== null) {
      clearTimeout(timerRef.current)
      timerRef.current = null
    }
  }, [])

  const dismissImmediately = useCallback(() => {
    clearTimer()
    cancelClose()
    close()
  }, [clearTimer, cancelClose, close])

  // Owner disabling mid-hover (menu opened, drag started) closes immediately.
  useEffect(() => {
    if (!disabled) return
    clearTimer()
    cancelClose()
    close()
  }, [disabled, cancelClose, close])

  useEffect(() => {
    mountedRef.current = true
    return () => {
      mountedRef.current = false
      copyEpochRef.current += 1
      clearTimer()
      if (copyTimerRef.current !== null) {
        clearTimeout(copyTimerRef.current)
        copyTimerRef.current = null
      }
    }
  }, [])

  useEffect(() => {
    if (!open || !previewAnchor) return
    const dismiss = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return
      cancelClose()
      close()
    }
    window.addEventListener('keydown', dismiss)
    return () => window.removeEventListener('keydown', dismiss)
  }, [open, previewAnchor, cancelClose, close])

  const padding = () => {
    const frame = document.documentElement
    const clearance = Number.parseFloat(getComputedStyle(frame).getPropertyValue('--dsh-frame-top-clearance'))
    return {
      top: previewAnchor && !Number.isNaN(clearance)
        ? Math.max(VIEWPORT_MARGIN, (frame.hasAttribute('data-fullscreen') ? 0 : clearance) + 20)
        : VIEWPORT_MARGIN,
      right: VIEWPORT_MARGIN, bottom: VIEWPORT_MARGIN, left: VIEWPORT_MARGIN,
    }
  }
  const { refs, x, y, strategy, isPositioned, middlewareData } = useFloating({
    open, placement: previewAnchor ? 'top-start' : 'right-start', strategy: 'fixed',
    whileElementsMounted: (reference, floating, update) => {
      const follow = () => {
        if (rootRef.current?.closest('[inert], [hidden]')) { close(); return }
        void update()
      }
      const stop = autoUpdate(reference, floating, follow)
      // The file card determines preview width, even if its trigger has not resized.
      const observer = previewAnchor?.current && typeof ResizeObserver !== 'undefined' ? new ResizeObserver(follow) : undefined
      if (previewAnchor?.current) observer?.observe(previewAnchor.current)
      const visibility = new MutationObserver(follow)
      for (let element: HTMLElement | null = rootRef.current; element; element = element.parentElement) {
        visibility.observe(element, { attributes: true, attributeFilter: ['hidden', 'inert'] })
      }
      return () => { stop(); observer?.disconnect(); visibility.disconnect() }
    },
    middleware: [
      offset(VIEWPORT_MARGIN),
      previewAnchor && {
        name: 'previewAnchor',
        fn: ({ rects }) => {
          const bounds = previewAnchor.current?.getBoundingClientRect()
          if (!bounds) return {}
          const width = Math.max(0, Math.min(bounds.width - 2 * PREVIEW_INSET, window.innerWidth - 2 * VIEWPORT_MARGIN))
          const floating = cardRef.current
          if (floating && floating.style.width !== `${width}px`) {
            floating.style.width = `${width}px`
            return { reset: { rects: true } }
          }
          return { x: Math.max(VIEWPORT_MARGIN, Math.min(bounds.left + PREVIEW_INSET, window.innerWidth - rects.floating.width - VIEWPORT_MARGIN)) }
        },
      },
      flip(() => ({ padding: padding(), ...(previewAnchor ? { fallbackPlacements: ['bottom-start'] as const } : { fallbackAxisSideDirection: 'end' as const }) })),
      shift(() => ({ padding: padding(), crossAxis: !previewAnchor })),
      size(() => ({
        padding: padding(),
        apply: ({ availableWidth, availableHeight, elements }) => {
          elements.floating.style.maxWidth = `${Math.max(0, availableWidth)}px`
          elements.floating.style.maxHeight = `${Math.max(0, Math.min(previewAnchor ? PREVIEW_MAX_HEIGHT : Infinity, availableHeight))}px`
        },
      })),
      hide({ strategy: 'referenceHidden' }),
    ],
  })
  const setRoot = useCallback((node: HTMLSpanElement | null) => { rootRef.current = node; refs.setReference(node) }, [refs.setReference])
  const setCard = useCallback((node: HTMLDivElement | null) => { cardRef.current = node; refs.setFloating(node) }, [refs.setFloating])
  useEffect(() => {
    if (open && isPositioned && middlewareData.hide?.referenceHidden) close()
  }, [open, isPositioned, middlewareData.hide?.referenceHidden, close])

  const groupVisible = open && isPositioned
  useLayoutEffect(() => {
    if (!groupVisible || group === null || cardRef.current === null) return
    group.shown(groupOwner, dismissImmediately, cardRef.current)
    return () => { group.release(groupOwner) }
  }, [group, groupOwner, groupVisible, dismissImmediately])

  const copy = async (text: string): Promise<void> => {
    if (copied || copyingRef.current) return
    copyingRef.current = true
    const copyEpoch = copyEpochRef.current
    const accepted = await writeClipboard(text)
    copyingRef.current = false
    const card = cardRef.current
    if (!accepted || !mountedRef.current || copyEpoch !== copyEpochRef.current || card === null) return
    const height = card.offsetHeight
    copyHeightRef.current = height > 0 ? height : null
    setCopied(true)
    copyTimerRef.current = setTimeout(clearCopied, 1000)
  }

  const copyable = copyText !== undefined
  const dismissFromAnchor = (event: SyntheticEvent<HTMLSpanElement>) => {
    // Portal content can be scrolled or selected without activating the anchor.
    if (cardRef.current?.contains(event.target as Node)) return
    clearTimer()
    cancelClose()
    close()
  }
  const card = open && (
    <div
      ref={setCard}
      className={`${css.card}${previewAnchor ? ` ${css.preview}` : ''}${copyable ? ` ${css.copyable}` : ''}${copied ? ` ${css.feedback}` : ''}`}
      data-hover-card={previewAnchor ? 'preview' : 'default'}
      style={{ position: strategy, left: x ?? 0, top: y ?? 0, visibility: isPositioned ? undefined : 'hidden', minHeight: copied && copyHeightRef.current !== null ? copyHeightRef.current : undefined }}
      role={copyable ? 'button' : undefined}
      tabIndex={copyable ? 0 : undefined}
      aria-label={copyable ? tx('common:hover-card.value-value', { p0: copyLabel ?? '', p1: copyText }) : undefined}
      onClick={copyable
        ? (e) => {
          const selection = window.getSelection()
          if (selection !== null && !selection.isCollapsed) {
            for (let i = 0; i < selection.rangeCount; i += 1) {
              if (selection.getRangeAt(i).intersectsNode(e.currentTarget)) return
            }
          }
          void copy(copyText)
        }
        : undefined}
      onKeyDown={copyable
        ? (e) => {
          if (e.key !== 'Enter' && e.key !== ' ') return
          e.preventDefault()
          void copy(copyText)
        }
        : undefined}
    >
      {copied ? <span className={css.copied} aria-hidden="true">{copiedLabel}</span> : content}
    </div>
  )

  return (
    <span
      ref={setRoot}
      className={css.root}
      onPointerEnter={event => {
        if (disabled || previewAnchor && event.pointerType === 'touch') return
        if (event.pointerType === 'mouse' && typeof window.matchMedia === 'function' && !window.matchMedia('(any-hover: hover)').matches) return
        // Coming back inside during the grace (the gap, or the card itself)
        // keeps the current card rather than restarting the dwell.
        cancelClose()
        if (open) return
        clearTimer()
        if (group?.isWarm()) setOpen(true)
        else timerRef.current = setTimeout(() => { setOpen(true) }, openDelayMs)
      }}
      onPointerLeave={() => {
        clearTimer()
        // Leaving a closed card schedules a no-op close; only arm while
        // open, matching Menu's shape.
        if (open) armClose()
      }}
      onPointerDownCapture={dismissFromAnchor}
      onClickCapture={previewAnchor ? dismissFromAnchor : undefined}
    >
      {anchor}
      {open && copyable && <span className={css.status} role="status">{copied ? copiedLabel : ''}</span>}
      {card !== false && createPortal(card, document.body)}
    </span>
  )
}

import { useTranslation } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. See PROVENANCE.md. */
// HoverCard: coss ui/Base UI preview-card positioning and portal behavior
// behind rustX's grouped dwell, copy feedback and file-preview ownership rules.
import { createContext, useCallback, useContext, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import type { ReactNode, RefObject, SyntheticEvent } from 'react'
import clsx from 'clsx'
import { writeClipboard } from './clipboard.ts'
import { usePointerGrace } from './pointer-grace.ts'
import { PreviewCard, PreviewCardPopup } from '@/components/ui/preview-card'

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

/** Share first-display dwell across a list and its active portal, including the card's pointer grace. */
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
 * Render an anchor with a hover-triggered coss preview card.
 * @param props.previewAnchor - use a wide preview above or below the trigger,
 * horizontally inset from this element.
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
  const [previewWidth, setPreviewWidth] = useState<number>()
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
      if (copyTimerRef.current !== null) clearTimeout(copyTimerRef.current)
    }
  }, [clearTimer])

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

  const previewRect = useCallback(() => {
    const bounds = previewAnchor?.current?.getBoundingClientRect()
    if (!bounds) return new DOMRect()
    const width = Math.max(0, Math.min(bounds.width - 2 * PREVIEW_INSET, window.innerWidth - 2 * VIEWPORT_MARGIN))
    return new DOMRect(bounds.left + PREVIEW_INSET, bounds.top, width, bounds.height)
  }, [previewAnchor])
  const positioningAnchor = useMemo(() => previewAnchor
    ? { getBoundingClientRect: previewRect }
    : rootRef, [previewAnchor, previewRect])

  useEffect(() => {
    if (!open || !previewAnchor) return
    const measure = () => setPreviewWidth(previewRect().width)
    measure()
    const element = previewAnchor.current
    const observer = element && typeof ResizeObserver !== 'undefined' ? new ResizeObserver(measure) : undefined
    if (element) observer?.observe(element)
    window.addEventListener('resize', measure)
    return () => {
      observer?.disconnect()
      window.removeEventListener('resize', measure)
    }
  }, [open, previewAnchor, previewRect])

  // Base UI positions the card, but the product still owns retirement when
  // the row or file card that owns the anchor becomes hidden or clipped away.
  useEffect(() => {
    if (!open) return
    const target = previewAnchor?.current ?? rootRef.current?.querySelector<HTMLElement>('button, [role="button"]') ?? rootRef.current
    if (!target) return
    const hidden = () => target.closest('[hidden], [inert]') !== null
      || rootRef.current?.closest('[hidden], [inert]') !== null
      || (previewAnchor ? previewRect().width <= 0 : false)
    if (hidden()) {
      close()
      return
    }
    const observer = typeof IntersectionObserver === 'undefined' ? undefined : new IntersectionObserver(entries => {
      if (entries.some(entry => !entry.isIntersecting)) close()
    }, { threshold: 0 })
    observer?.observe(target)
    const check = () => { if (hidden()) close() }
    // Attribute hiding fires no scroll, resize or intersection event of its
    // own: watch the anchor chain the way the pre-coss floating-ui wiring did.
    const visibility = new MutationObserver(check)
    for (let element: HTMLElement | null = target; element; element = element.parentElement) {
      visibility.observe(element, { attributes: true, attributeFilter: ['hidden', 'inert'] })
    }
    window.addEventListener('scroll', check, true)
    window.addEventListener('resize', check)
    return () => {
      observer?.disconnect()
      visibility.disconnect()
      window.removeEventListener('scroll', check, true)
      window.removeEventListener('resize', check)
    }
  }, [open, previewAnchor, previewRect, close])

  useLayoutEffect(() => {
    if (!open || group === null || cardRef.current === null) return
    group.shown(groupOwner, dismissImmediately, cardRef.current)
    return () => { group.release(groupOwner) }
  }, [group, groupOwner, open, dismissImmediately])

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
    if (cardRef.current?.contains(event.target as Node)) return
    clearTimer()
    cancelClose()
    close()
  }

  return (
    <span
      ref={rootRef}
      className="relative block"
      onPointerEnter={event => {
        if (disabled || previewAnchor && event.pointerType === 'touch') return
        if (event.pointerType === 'mouse' && typeof window.matchMedia === 'function' && !window.matchMedia('(any-hover: hover)').matches) return
        cancelClose()
        if (open) return
        clearTimer()
        if (group?.isWarm()) setOpen(true)
        else timerRef.current = setTimeout(() => { setOpen(true) }, openDelayMs)
      }}
      onPointerLeave={() => {
        clearTimer()
        if (open) armClose()
      }}
      onPointerDownCapture={dismissFromAnchor}
      onClickCapture={previewAnchor ? dismissFromAnchor : undefined}
    >
      {anchor}
      <PreviewCard open={open} onOpenChange={next => { if (!next) close() }}>
        {open && <PreviewCardPopup
          ref={cardRef}
          anchor={positioningAnchor}
          align="start"
          side={previewAnchor ? 'top' : 'right'}
          sideOffset={VIEWPORT_MARGIN}
          collisionPadding={VIEWPORT_MARGIN}
          className={clsx(
            previewAnchor ? 'p-0 overflow-hidden' : 'w-[244px] px-4 py-3 overflow-auto',
            copyable && 'cursor-pointer focus-visible:outline-2 focus-visible:outline-ring focus-visible:outline-offset-2',
            copied && 'flex items-center justify-center',
          )}
          data-hover-card={previewAnchor ? 'preview' : 'default'}
          style={{
            width: previewAnchor ? previewWidth : undefined,
            // The positioner publishes the collision-aware space on its own side;
            // capping here is what lets flip/shift actually fit the card.
            maxHeight: previewAnchor ? `min(${PREVIEW_MAX_HEIGHT}px, var(--available-height, ${PREVIEW_MAX_HEIGHT}px))` : undefined,
            minHeight: copied && copyHeightRef.current !== null ? copyHeightRef.current : undefined,
          }}
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
          {copied ? <span className="text-sm leading-5 text-center" aria-hidden="true">{copiedLabel}</span> : content}
        </PreviewCardPopup>}
      </PreviewCard>
      {open && copyable && <span className="sr-only" role="status">{copied ? copiedLabel : ''}</span>}
    </span>
  )
}

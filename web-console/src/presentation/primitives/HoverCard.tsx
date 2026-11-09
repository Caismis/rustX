import { useTranslation } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. See PROVENANCE.md. */
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react'
import type { ReactNode, RefObject, SyntheticEvent } from 'react'
import { createPortal } from 'react-dom'
import { writeClipboard } from './clipboard.ts'
import { usePointerGrace } from './pointer-grace.ts'
import css from './HoverCard.module.css'

const VIEWPORT_MARGIN = 8
const PREVIEW_INSET = 24
const PREVIEW_MAX_HEIGHT = 420

interface CardPosition {
  left: number
  top: number
  width?: number
  maxHeight?: number
}

/**
 * Render an anchor with a hover-triggered preview card.
 * @param props.anchor - the hover target (rendered in place inside a wrapper span).
 * @param props.content - card content; the pointer may rest on it, so it is
 * readable and selectable, but it carries no dismissal affordance of its own.
 * @param props.openDelayMs - hover dwell before the card shows (default 500).
 * @param props.disabled - suppress opening; turning true closes an open card.
 * @param props.previewAnchor - use a wide preview above or below the trigger, sized from this element.
 * @param props.copyText - optional primary value copied by activation and
 * included in the card's accessible name.
 * @param props.copyLabel - localized accessible activation-label prefix.
 * @param props.copiedLabel - localized visible success label.
 * @returns anchor wrapper with the conditional portaled card.
 */
export function HoverCard({
  anchor, content, openDelayMs = 500, disabled = false,
  copyText, copyLabel, copiedLabel, previewAnchor,
}: {
  anchor: ReactNode
  content: ReactNode
  openDelayMs?: number
  disabled?: boolean
  copyText?: string | undefined
  copyLabel: string
  copiedLabel: string
  previewAnchor?: RefObject<HTMLElement | null>
}) {
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
  const [pos, setPos] = useState<CardPosition | null>(null)
  const previewMounted = previewAnchor !== undefined && pos !== null
  const [copied, setCopied] = useState(false)

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

  const clearTimer = () => {
    if (timerRef.current !== null) {
      clearTimeout(timerRef.current)
      timerRef.current = null
    }
  }

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

  // Fixed-position from the anchor rect before paint; track the anchor while
  // open (capture-phase scroll catches nested panes).
  useLayoutEffect(() => {
    if (!open) { setPos(null); return }
    const place = () => {
      const wrapper = rootRef.current
      /* v8 ignore next -- the ref is attached before the layout effect runs and the listeners die with it. */
      if (wrapper === null) return
      const r = wrapper.getBoundingClientRect()
      const h = cardRef.current?.offsetHeight ?? 0
      if (previewAnchor) {
        if (r.bottom <= 0 || r.top >= window.innerHeight || wrapper.closest('[inert], [hidden]')) {
          close()
          return
        }
        const bounds = previewAnchor.current?.getBoundingClientRect() ?? r
        const width = Math.max(0, Math.min(bounds.width - 2 * PREVIEW_INSET, window.innerWidth - 2 * VIEWPORT_MARGIN))
        const frame = document.documentElement
        const clearance = Number.parseFloat(getComputedStyle(frame).getPropertyValue('--dsh-frame-top-clearance'))
        const topMargin = Number.isNaN(clearance) ? VIEWPORT_MARGIN
          : Math.max(VIEWPORT_MARGIN, Math.min((frame.hasAttribute('data-fullscreen') ? 0 : clearance) + 20, r.bottom + VIEWPORT_MARGIN))
        const belowTop = Math.max(topMargin, r.bottom + VIEWPORT_MARGIN)
        const above = Math.max(0, r.top - VIEWPORT_MARGIN - topMargin)
        const below = Math.max(0, window.innerHeight - belowTop - VIEWPORT_MARGIN)
        const onTop = above >= Math.min(PREVIEW_MAX_HEIGHT, below)
        const maxHeight = Math.min(PREVIEW_MAX_HEIGHT, onTop ? above : below)
        setPos({
          left: Math.max(VIEWPORT_MARGIN, Math.min(bounds.left + PREVIEW_INSET, window.innerWidth - width - VIEWPORT_MARGIN)),
          top: onTop ? Math.max(topMargin, r.top - VIEWPORT_MARGIN - Math.min(h, maxHeight)) : belowTop,
          width,
          maxHeight,
        })
        return
      }
      const top = r.top + h > window.innerHeight - 8 ? window.innerHeight - h - 8 : r.top
      setPos({ left: r.right + 8, top })
    }
    place()
    const observer = previewAnchor && typeof ResizeObserver !== 'undefined' ? new ResizeObserver(place) : undefined
    for (const element of [rootRef.current, previewAnchor?.current, cardRef.current]) {
      if (element) observer?.observe(element)
    }
    const followScroll = (event: Event) => {
      if (previewAnchor && event.target instanceof Node && cardRef.current?.contains(event.target)) return
      place()
    }
    window.addEventListener('scroll', followScroll, true)
    window.addEventListener('resize', place)
    return () => {
      observer?.disconnect()
      window.removeEventListener('scroll', followScroll, true)
      window.removeEventListener('resize', place)
    }
  }, [open, previewAnchor, previewMounted, close])

  // The first placement ran before the card mounted (height read 0): once the
  // card's real height is measurable, correct the bottom-edge clamp. The
  // correction converges — a clamped top satisfies the guard, so it runs once.
  useLayoutEffect(() => {
    if (!open || pos === null || previewAnchor) return
    /* v8 ignore next -- the card is mounted whenever pos is set, so the ref is attached here. */
    const h = cardRef.current?.offsetHeight ?? 0
    if (pos.top + h > window.innerHeight - 8) {
      setPos({ left: pos.left, top: window.innerHeight - h - 8 })
    }
  }, [open, pos, previewAnchor])

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
  const card = open && pos !== null && (
    <div
      ref={cardRef}
      className={`${css.card}${previewAnchor ? ` ${css.preview}` : ''}${copyable ? ` ${css.copyable}` : ''}${copied ? ` ${css.feedback}` : ''}`}
      style={{ ...pos, minHeight: copied && copyHeightRef.current !== null ? copyHeightRef.current : undefined }}
      role={copyable ? 'button' : undefined}
      tabIndex={copyable ? 0 : undefined}
      aria-label={copyable ? tx('common:hover-card.value-value', { p0: copyLabel, p1: copyText }) : undefined}
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
      ref={rootRef}
      className={css.root}
      onPointerEnter={event => {
        if (disabled || previewAnchor && event.pointerType === 'touch') return
        // Coming back inside during the grace (the gap, or the card itself)
        // keeps the current card rather than restarting the dwell.
        cancelClose()
        if (open) return
        clearTimer()
        timerRef.current = setTimeout(() => { setOpen(true) }, openDelayMs)
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

/* Copyright (c) 2026 DeepSeek. MIT. See PROVENANCE.md. */
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { CSSProperties, ReactNode } from 'react'
import { createPortal } from 'react-dom'
import { autoUpdate, flip, offset, shift, size, useFloating, type Placement, type VirtualElement } from '@floating-ui/react-dom'
import clsx from 'clsx'
import { IconCheckOutline16 } from './icons/index.tsx'
import { usePointerGrace } from './pointer-grace.ts'
import css from './Menu.module.css'

/** Selectable row (optionally with a nested submenu). */
export interface MenuItem {
  id: string
  label: ReactNode
  disabled?: boolean
  /** Leading icon (figma .Menu_cell gap 8). */
  icon?: ReactNode
  /** Destructive row: error-colored text/icon and danger hover fill. */
  danger?: boolean
  /** Nested card opened to the right on hover/focus. */
  submenu?: readonly MenuItem[]
}

/** Hairline between item groups (not selectable). */
export interface MenuSeparator {
  type: 'separator'
  id: string
}

/** Non-interactive heading row above a group of items. */
export interface MenuLabel {
  type: 'label'
  id: string
  text: string
}

/** One primary-menu entry: a row, a separator, or a heading label. */
export type MenuEntry = MenuItem | MenuSeparator | MenuLabel

function isSeparator(entry: MenuEntry): entry is MenuSeparator {
  return 'type' in entry && entry.type === 'separator'
}

function isLabel(entry: MenuEntry): entry is MenuLabel {
  return 'type' in entry && entry.type === 'label'
}

/** Distance a portaled list keeps from the viewport edges. */
const VIEWPORT_MARGIN = 12

/** The rustX side/align vocabulary as one Floating UI placement. A side card
 * opens beside the anchor's top edge, as it always has. */
function placementOf(side: 'bottom' | 'top' | 'right', align: 'start' | 'end'): Placement {
  return side === 'right' ? 'right-start' : `${side}-${align}`
}

/**
 * Render an anchored dropdown menu. While the list is open its keys mirror the
 * composer's: Tab settles the focused row — from the trigger, Tab enters the
 * list instead — and Escape or Shift+Tab close it and return focus to the
 * anchor's first button, and selecting a row does the same — the rows unmount
 * with the list. Only a keyboard on the trigger or inside the list is
 * intercepted; Tab presses elsewhere on the page stay the browser's.
 * @param props.autoFocus - focus the first item on open; the arrow keys walk the list either way.
 * @param props.open - whether the list is showing (owner-controlled).
 * @param props.anchor - the trigger element (rendered in place).
 * @param props.items - selectable rows and optional separators.
 * @param props.selectedId - row shown as selected.
 * @param props.selectedIds - rows shown as selected when a menu contains independent option groups.
 * @param props.onSelect - row click callback (not called for disabled rows or submenu parents that only open children).
 * @param props.onClose - invoked on outside click, Escape, or a window blur
 * that moved focus into an iframe (the only signal a pointerdown inside a
 * cross-origin iframe leaves).
 * @param props.align - list alignment against the anchor (default 'start').
 * @param props.side - open below (`bottom`, default) or above (`top`) the anchor.
 * @param props.portal - render the list into document.body as a fixed,
 * Floating UI-placed top layer: it flips to the other side and shifts along
 * it to stay 12px inside the viewport, a scrollable list takes only the
 * height the viewport leaves, and it follows its anchor through scroll,
 * resize and layout changes while open. Use when an ancestor's overflow
 * clipping would crop the in-place list; default false keeps the pure-CSS
 * in-place behavior.
 * @param props.closeOnPointerLeave - close the list once the pointer has left
 * both trigger and list for the pointer grace (default false keeps it open
 * until outside click/Escape/selection). The grace makes the 4px trigger->list
 * gap and a brief overshoot survivable; coming back cancels the close.
 * @param props.dense - reduce vertical row spacing without changing the standard typography or card width.
 * @param props.compact - use reduced menu typography and spacing.
 * @param props.getAnchorRect - portal mode only: supply the anchor rect
 * directly (e.g. from a host-owned trigger button) instead of measuring the
 * Menu's own wrapper span. Required when the wrapper isn't itself laid out at
 * the trigger (render-prop anchors, effect-positioned proxies — measuring the
 * wrapper there races the host's layout effects). Called whenever Floating UI
 * repositions the list; return null to skip placement for that frame.
 * @param props.footer - rows pinned below the scrolling items area, separated
 * by a hairline; they stay visible while the items above scroll.
 * @param props.selection - how a selected row is marked: a trailing check
 * (`'check'`, default — figma .Menu_cell) or the hover fill held on the row
 * with no check (`'fill'`, for icon-labelled rows where a trailing glyph
 * crowds the cell).
 * @returns anchor wrapper with the conditional list.
 */
export function Menu({ open, anchor, items, selectedId, selectedIds, onSelect, onClose, align = 'start', side = 'bottom', portal = false, closeOnPointerLeave = false, dense = false, compact = false, autoFocus = false, selection = 'check', getAnchorRect, footer, className }: {
  open: boolean
  autoFocus?: boolean
  anchor: ReactNode
  items: readonly MenuEntry[]
  footer?: readonly MenuEntry[]
  selectedId?: string | undefined
  selectedIds?: readonly string[] | undefined
  onSelect: (id: string) => void
  onClose: () => void
  align?: 'start' | 'end'
  side?: 'bottom' | 'top' | 'right'
  portal?: boolean
  closeOnPointerLeave?: boolean
  dense?: boolean
  compact?: boolean
  selection?: 'check' | 'fill'
  getAnchorRect?: () => DOMRect | null
  className?: string | undefined
}) {
  const rootRef = useRef<HTMLSpanElement>(null)
  const listRef = useRef<HTMLDivElement>(null)
  /** Index the arrow walk last focused, the resume point when focus left the rows. */
  const walkIndex = useRef<number | null>(null)
  /**
   * The control that had the keyboard when this menu opened — its own trigger,
   * which an anchor that wraps several controls (a split button) would not be
   * able to name by position.
   */
  const triggerRef = useRef<HTMLElement | null>(null)

  /**
   * Hand the keyboard back to the trigger that opened the menu — or, when the
   * anchor never held it, to the anchor's first button. Focus left on a removed
   * row otherwise falls to the page body, where the next Tab restarts from the
   * top of the page.
   */
  const refocusAnchor = (): void => {
    const trigger = triggerRef.current
    if (trigger !== null && document.contains(trigger) && !(trigger as HTMLButtonElement).disabled) {
      trigger.focus()
      return
    }
    rootRef.current?.querySelector<HTMLButtonElement>('button:not(:disabled)')?.focus()
  }

  /**
   * Post-selection focus, for the paths where the rows unmount with the list.
   * A selection whose owner keeps the menu open is left alone, and so is an
   * owner that moved focus itself (a presented file card hands it to its
   * preview button): only a keyboard left on the closing list (or on the body
   * its removal produced) comes back to the trigger.
   */
  const refocusAfterSelection = (): void => {
    queueMicrotask(() => {
      if (openRef.current) return
      const active = document.activeElement
      if (active === null || active === document.body || listRef.current?.contains(active) === true) refocusAnchor()
    })
  }
  const openRef = useRef(open)
  openRef.current = open
  const [openSubmenuId, setOpenSubmenuId] = useState<string | null>(null)
  const { arm: armClose, cancel: cancelClose } = usePointerGrace(onClose)

  // The submenu card is absolutely positioned outside the list box; the
  // scroll clip would crop it, so only submenu-free menus get the height cap.
  const scrollable = !items.some(entry => !isSeparator(entry) && !isLabel(entry) && entry.submenu !== undefined && entry.submenu.length > 0)

  // Portal geometry is Floating UI's, and only Floating UI's: anchor
  // measurement, the side/align placement, flipping to the other side and
  // shifting along it to stay VIEWPORT_MARGIN inside the viewport, the height
  // the viewport leaves for a scrollable list, and repositioning whenever an
  // ancestor scrolls, the viewport resizes or either element's layout changes.
  // getAnchorRect trumps measuring the wrapper span: a child layout effect runs
  // before the parent's, so a wrapper the host positions in its own effect
  // measures stale — the host callback owns the truth, as a virtual reference.
  const [anchorElement, setAnchorElement] = useState<HTMLSpanElement | null>(null)
  const setRoot = useCallback((node: HTMLSpanElement | null) => {
    rootRef.current = node
    setAnchorElement(node)
  }, [])
  const anchorRect = useRef(getAnchorRect)
  anchorRect.current = getAnchorRect
  /** The last rect a host-owned anchor supplied. A host returning null skips
   * that frame: the list keeps its last placement, or stays hidden until the
   * host has ever supplied one. */
  const hostRect = useRef<DOMRect | null>(null)
  const hostAnchored = getAnchorRect !== undefined
  const reference = useMemo<HTMLSpanElement | VirtualElement | null>(() => {
    if (!hostAnchored || anchorElement === null) return anchorElement
    return {
      contextElement: anchorElement,
      getBoundingClientRect: () => {
        const rect = anchorRect.current?.() ?? null
        if (rect !== null) hostRect.current = rect
        return hostRect.current ?? new DOMRect()
      },
    }
  }, [hostAnchored, anchorElement])
  const { refs, floatingStyles, isPositioned } = useFloating({
    open: open && portal,
    placement: placementOf(side, align),
    strategy: 'fixed',
    transform: false,
    elements: { reference: portal ? reference : null },
    whileElementsMounted: autoUpdate,
    middleware: [
      offset(4),
      flip({ padding: VIEWPORT_MARGIN }),
      shift({ padding: VIEWPORT_MARGIN }),
      size({
        padding: VIEWPORT_MARGIN,
        apply({ availableWidth, availableHeight, elements }) {
          elements.floating.style.maxWidth = `${Math.max(0, availableWidth)}px`
          // A list with submenu rows is never clipped: the side card would be.
          elements.floating.style.maxHeight = scrollable ? `${Math.max(0, availableHeight)}px` : ''
        },
      }),
    ],
  })
  const setList = useCallback((node: HTMLDivElement | null) => {
    listRef.current = node
    refs.setFloating(node)
  }, [refs])
  // Floating UI places a portaled list in the commit that opens it: its first
  // computation resolves within the same task and is flushed synchronously,
  // before the browser paints. autoFocus still waits for that placement, so it
  // never lands on an unplaced row. A host-owned anchor that has never supplied
  // a rect has nothing to place against, so that list stays invisible.
  const placed = !portal || (isPositioned && (!hostAnchored || hostRect.current !== null))
  const portalStyle: CSSProperties = !hostAnchored || hostRect.current !== null ? floatingStyles : { ...floatingStyles, visibility: 'hidden' }

  // Opening remembers where the keyboard was, so closing can hand it back to
  // that control — an anchor wrapping several (a split button) cannot be asked
  // for it by position. Declared before the autoFocus effect so the capture
  // sees the trigger, not the row autoFocus is about to focus.
  useEffect(() => {
    if (!open) {
      triggerRef.current = null
      return
    }
    const active = document.activeElement
    triggerRef.current = active instanceof HTMLElement && rootRef.current?.contains(active) === true ? active : null
  }, [open])

  useEffect(() => {
    if (!open || !autoFocus || !placed) return
    const first = listRef.current?.querySelector<HTMLButtonElement>('button:not(:disabled)')
    walkIndex.current = first === undefined || first === null ? null : 0
    first?.focus()
  }, [open, autoFocus, placed])

  useEffect(() => {
    if (!open) {
      setOpenSubmenuId(null)
      walkIndex.current = null
      return
    }
    const onPointerDown = (e: PointerEvent) => {
      if (!(e.target instanceof Node)) return
      // The portaled list is outside the anchor subtree; check both.
      if (rootRef.current?.contains(e.target) === true) return
      if (listRef.current?.contains(e.target) === true) return
      onClose()
    }
    const onKeyDown = (e: KeyboardEvent) => {
      // Where the keyboard is, computed once: the menu owns it when it holds a
      // row or sits on its anchor region.
      const focused = document.activeElement
      const insideList = listRef.current?.contains(focused) === true
      const anchored = rootRef.current?.contains(focused) === true || insideList
      if (e.key === 'Escape') {
        // Escape belongs to this menu before its containing modal. The rustX
        // Modal reads the prevented default; a React Aria modal answers
        // Escape from React's own listener, which a portaled list's events
        // still reach through the React tree, so the event stops here, in
        // the document capture phase, before any of them sees it.
        e.preventDefault()
        e.stopPropagation()
        // Closing hands the keyboard back when the menu had it — and, as this
        // primitive always did for autoFocus menus, when it held the keyboard
        // and lost it again (a row that unmounted under it).
        onClose()
        if (anchored || autoFocus) refocusAnchor()
      }
      // Tab settles like Enter and Shift+Tab leaves like Escape, so a menu's
      // keys mean what they mean in the composer. Only a keyboard already on
      // the trigger or inside the list is intercepted: Tab elsewhere on the
      // page keeps the browser's traversal even while a menu is open.
      if (e.key === 'Tab') {
        const list = listRef.current
        if (list === null || !anchored) return
        if (e.shiftKey) {
          e.preventDefault()
          onClose()
          refocusAnchor()
          return
        }
        // Tab settles the row it is on; from anywhere else in the menu region
        // it enters the list. A focused control that is not a row (a retry
        // button inside an error strip) and a list with no enabled row keep the
        // browser's traversal instead of being swallowed.
        if (insideList) {
          if (focused instanceof Element && focused.getAttribute('role') === 'menuitem') {
            e.preventDefault()
            ;(focused as HTMLElement).click()
          }
          return
        }
        const row = list.querySelector<HTMLButtonElement>('button:not(:disabled)')
        if (row === null) return
        e.preventDefault()
        row.focus()
        walkIndex.current = 0
        return
      }
      // Arrows walk the list whether or not the menu focused its first item on
      // open, so `autoFocus` chooses only that entry behavior. A keyboard still
      // on the anchor enters at the end the step comes from — unless it already
      // walked, in which case the walk resumes where it left off. The walk resumes
      // from where it last put focus, not from `document.activeElement`: a row
      // that refused focus (a hidden portal frame, a detached node) would
      // otherwise re-enter at the near end on every press and the walk would
      // alternate between two rows.
      if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(e.key)) return
      const list = listRef.current
      if (list === null || !anchored) return
      const buttons = Array.from(list.querySelectorAll<HTMLButtonElement>('button:not(:disabled)'))
      if (buttons.length === 0) return
      const index = buttons.indexOf(focused as HTMLButtonElement)
      const from = index >= 0 ? index : walkIndex.current
      const next = e.key === 'Home' ? 0 : e.key === 'End' ? buttons.length - 1
        : from === null
          ? (e.key === 'ArrowDown' ? 0 : buttons.length - 1)
          : (from + (e.key === 'ArrowDown' ? 1 : -1) + buttons.length) % buttons.length
      e.preventDefault()
      walkIndex.current = next
      buttons[next]?.focus()
    }
    // A pointerdown inside a cross-origin iframe (a sandboxed HTML preview)
    // never reaches this document; the focus move it causes blurs the window
    // instead. Only that case closes: an app or tab switch leaves the
    // document's focus where it was, so activeElement is not an iframe.
    const onWindowBlur = () => {
      if (document.activeElement instanceof HTMLIFrameElement) onClose()
    }
    document.addEventListener('pointerdown', onPointerDown)
    document.addEventListener('keydown', onKeyDown, true)
    window.addEventListener('blur', onWindowBlur)
    return () => {
      document.removeEventListener('pointerdown', onPointerDown)
      document.removeEventListener('keydown', onKeyDown, true)
      window.removeEventListener('blur', onWindowBlur)
    }
  }, [open, onClose, autoFocus])

  // A close from selection/Escape/outside click outruns a pending grace close;
  // left armed it would shut a list reopened inside the grace window. Its own
  // effect, not the listener effect above: that one re-runs on every `onClose`
  // identity change and would cancel the grace mid-transit.
  useEffect(() => {
    if (!open) cancelClose()
  }, [open, cancelClose])

  const renderEntry = (entry: MenuEntry) => {
    if (isSeparator(entry)) {
      return <div key={entry.id} className={css.separator} role="separator" />
    }
    if (isLabel(entry)) {
      return <div key={entry.id} className={css.label} role="presentation">{entry.text}</div>
    }
    const hasSub = entry.submenu !== undefined && entry.submenu.length > 0
    const subOpen = hasSub && openSubmenuId === entry.id
    const selected = entry.id === selectedId || selectedIds?.includes(entry.id) === true
    return (
      <div
        key={entry.id}
        className={css.itemWrap}
        onMouseEnter={() => { setOpenSubmenuId(hasSub ? entry.id : null) }}
        onMouseLeave={() => { setOpenSubmenuId(null) }}
      >
        <button
          type="button"
          role="menuitem"
          className={clsx(css.item, selected && (selection === 'fill' ? css.selectedFill : css.selected), entry.danger === true && css.danger)}
          disabled={entry.disabled}
          // The selection marker is also announced: a row shown as selected is
          // the current one, never a visual-only state.
          aria-current={selected ? 'true' : undefined}
          aria-haspopup={hasSub ? 'menu' : undefined}
          aria-expanded={hasSub ? subOpen : undefined}
          onFocus={() => { setOpenSubmenuId(hasSub ? entry.id : null) }}
          onClick={() => {
            if (hasSub) {
              setOpenSubmenuId(entry.id)
              return
            }
            onSelect(entry.id)
            refocusAfterSelection()
          }}
        >
          {entry.icon !== undefined && <span className={css.itemIcon}>{entry.icon}</span>}
          <span className={css.itemLabel}>{entry.label}</span>
          {/* Selection marker is a trailing check (figma .Menu_cell) unless the fill mode carries it. */}
          {selected && selection === 'check' && <IconCheckOutline16 className={css.check} />}
        </button>
        {subOpen && entry.submenu !== undefined && (
          <div className={clsx(css.submenu, compact && css.compactList)} role="menu">
            {entry.submenu.map(sub => (
              <button
                key={sub.id}
                type="button"
                role="menuitem"
                className={css.item}
                disabled={sub.disabled}
                onClick={() => { onSelect(sub.id); refocusAfterSelection() }}
              >
                {sub.icon !== undefined && <span className={css.itemIcon}>{sub.icon}</span>}
                <span className={css.itemLabel}>{sub.label}</span>
              </button>
            ))}
          </div>
        )}
      </div>
    )
  }

  // The first painted frame of a portal list is already at its final position
  // (with getAnchorRect never returning a rect the list stays hidden). A
  // portaled list is a top
  // layer: a React Aria modal that contains its anchor keeps it visible to
  // assistive technology, lets focus enter it and does not treat a press
  // inside it as an interaction outside the modal.
  const list = open && (
    <div
      ref={setList}
      className={clsx(css.list, dense && css.denseList, compact && css.compactList, scrollable && css.scrollable, portal && css.portal, side === 'top' && !portal && css.sideTop, align === 'end' && !portal && css.alignEnd)}
      style={portal ? portalStyle : undefined}
      data-react-aria-top-layer={portal ? true : undefined}
      role="menu"
      // React portals bubble synthetic events through the REACT tree: without
      // this stop, an item click re-fires the anchor row's own onClick
      // (open/toggle) after onSelect.
      onClick={(e) => { e.stopPropagation() }}
    >
      <div className={css.viewport} role="presentation">
        {items.map(renderEntry)}
      </div>
      {footer !== undefined && footer.length > 0 && (
        <div className={css.footer} role="presentation">
          {footer.map(renderEntry)}
        </div>
      )}
    </div>
  )

  // Pointer-leave dismissal watches the WRAPPER, not the list: React's
  // enter/leave traversal runs over the React tree, so trigger and portaled
  // list are one region here. Aiming back at the trigger, or crossing the 4px
  // gap between them, therefore never counts as leaving.
  return (
    <span
      ref={setRoot}
      className={clsx(css.root, className)}
      onPointerEnter={closeOnPointerLeave ? cancelClose : undefined}
      onPointerLeave={closeOnPointerLeave ? () => { if (open) armClose() } : undefined}
    >
      {anchor}
      {portal ? (list !== false && createPortal(list, document.body)) : list}
    </span>
  )
}

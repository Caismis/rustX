/* Copyright (c) 2026 DeepSeek. MIT. See PROVENANCE.md. */
import { useCallback, useEffect, useId, useMemo, useRef, useState } from 'react'
import type { CSSProperties, ReactNode } from 'react'
import { createPortal } from 'react-dom'
import { autoUpdate, flip, hide, offset, shift, size, useFloating, type Placement, type VirtualElement } from '@floating-ui/react-dom'
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
  /** Nested card opened beside the row on hover/focus (right, or left when
   * the right does not fit); the keyboard enters it with ArrowRight, Enter,
   * Space or Tab on the row. */
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

/** Distance a floating menu surface keeps from the viewport edges. */
const VIEWPORT_MARGIN = 12

/** Row edge to submenu card: the parent card's 4px inset plus the 6px between
 * the two cards' outer edges (figma 419:16920). The card's pointer bridge
 * spans the same gap. */
const SUBMENU_GAP = 10

/** A submenu card ends 4px below its row — the parent card's inset — so a
 * submenu of the last row bottom-aligns with the parent card. */
const SUBMENU_DROP = 4

/**
 * Floating UI's measure of the room the viewport leaves a menu surface,
 * published as `--menu-available-width` / `--menu-available-height`.
 * Menu.module.css applies the design bounds within it, so the design decides
 * the card's size and the viewport only ever takes room away.
 */
const availableRoom = size({
  padding: VIEWPORT_MARGIN,
  apply({ availableWidth, availableHeight, elements }) {
    elements.floating.style.setProperty('--menu-available-width', `${Math.max(0, availableWidth)}px`)
    elements.floating.style.setProperty('--menu-available-height', `${Math.max(0, availableHeight)}px`)
  },
})

/**
 * Floating UI's measure of whether a surface's reference is still a visible
 * interaction anchor. `referenceHidden` is set once the reference is fully
 * clipped: scrolled out of its clipping context, or out of layout altogether —
 * a reference under a `display: none` ancestor, or a detached one, measures as
 * an empty rect that no clipping context contains. A surface whose reference
 * is hidden settles closed (see `Menu` and `Submenu`); it is never left
 * floating, or holding the keyboard, over a layout that no longer has its
 * anchor.
 */
const anchorLiveness = hide({ strategy: 'referenceHidden' })

/** Focus a target; whether it took the keyboard, which a disabled or unrendered one refuses. */
function takesFocus(target: HTMLElement | null | undefined, preventScroll = false): boolean {
  if (target === null || target === undefined || !target.isConnected) return false
  target.focus({ preventScroll })
  return document.activeElement === target
}

/**
 * Give the keyboard to the nearest rendered ancestor of `from` that takes
 * focus, such as a containing dialog or a focusable pane, without scrolling
 * anything to it; whether one took it.
 */
function focusContainingOwner(from: Element): boolean {
  for (let at = from.parentElement?.closest<HTMLElement>('[tabindex]'); at !== null && at !== undefined; at = at.parentElement?.closest<HTMLElement>('[tabindex]')) {
    if (takesFocus(at, true)) return true
  }
  return false
}

/** The rustX side/align vocabulary as one Floating UI placement. A side card
 * opens beside the anchor's top edge, as it always has. */
function placementOf(side: 'bottom' | 'top' | 'right', align: 'start' | 'end'): Placement {
  return side === 'right' ? 'right-start' : `${side}-${align}`
}

/**
 * Render an anchored dropdown menu. The list is rendered into document.body as
 * a fixed, Floating UI-placed top layer, so no ancestor's overflow clipping
 * crops it: it flips to the other side and shifts along it to stay 12px inside
 * the viewport, keeps its design size within the room the viewport leaves
 * (its rows scroll inside it), and follows its anchor through scroll, resize
 * and layout changes while open. A submenu is placed the same way against its
 * row. While the list is open its keys mirror the
 * composer's: Tab settles the focused row — from the trigger, Tab enters the
 * list instead — and Escape or Shift+Tab close it and return focus to the
 * anchor's first button, and selecting a row does the same — the rows unmount
 * with the list. Only a keyboard on the trigger or inside the list is
 * intercepted; Tab presses elsewhere on the page stay the browser's.
 *
 * A submenu is its own layer. Focusing or hovering its row shows it; settling
 * the row (Enter, Space, Tab) or ArrowRight moves the keyboard into it. The
 * arrows, Home and End walk only the layer that holds the keyboard, and
 * Escape or ArrowLeft inside a submenu closes just that layer and hands the
 * keyboard back to its row.
 *
 * A surface lives only while its reference is a visible interaction anchor.
 * When the anchor leaves rendered layout while the list is open — a container
 * query hides it, an ancestor collapses, it is scrolled out of its clipping
 * context — the menu asks its owner to close through `onClose`. That close is
 * not a dismissal: a keyboard the menu held settles on the anchor's nearest
 * containing focus owner (a dialog, a focusable pane) without scrolling,
 * never on the hidden trigger — focusing a trigger that was scrolled out of
 * view would scroll it back and undo the user's scroll — and never on a row
 * that is about to unmount. A submenu whose row stops being visible closes the
 * same way, leaving the parent list open and holding the keyboard itself
 * rather than the clipped row. The owner therefore only states whether the
 * menu is open; it never mirrors the layout rules that decide whether the
 * anchor is rendered.
 * @param props.autoFocus - focus the first item on open; the arrow keys walk the list either way.
 * @param props.open - whether the list is showing (owner-controlled).
 * @param props.anchor - the trigger element (rendered in place).
 * @param props.items - selectable rows and optional separators.
 * @param props.selectedId - row shown as selected.
 * @param props.selectedIds - rows shown as selected when a menu contains independent option groups.
 * @param props.onSelect - row click callback (not called for disabled rows or submenu parents that only open children).
 * @param props.onClose - invoked on outside click, Escape, a window blur
 * that moved focus into an iframe (the only signal a pointerdown inside a
 * cross-origin iframe leaves), or the anchor leaving rendered layout.
 * @param props.align - list alignment against the anchor (default 'start').
 * @param props.side - open below (`bottom`, default) or above (`top`) the anchor.
 * @param props.closeOnPointerLeave - close the list once the pointer has left
 * both trigger and list for the pointer grace (default false keeps it open
 * until outside click/Escape/selection). The grace makes the 4px trigger->list
 * gap and a brief overshoot survivable; coming back cancels the close.
 * @param props.dense - reduce vertical row spacing without changing the standard typography or card width.
 * @param props.compact - use reduced menu typography and spacing.
 * @param props.getAnchorRect - supply the anchor rect
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
export function Menu({ open, anchor, items, selectedId, selectedIds, onSelect, onClose, align = 'start', side = 'bottom', closeOnPointerLeave = false, dense = false, compact = false, autoFocus = false, selection = 'check', getAnchorRect, footer, className }: {
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
   * anchor never held it, to the anchor's first button. When neither can take
   * it (disabled, or out of layout with the anchor), it goes to the nearest
   * rendered ancestor that takes focus, such as a containing dialog. Focus
   * left on a removed row otherwise falls to the page body, where the next Tab
   * restarts from the top of the page and a containing dialog no longer hears
   * Escape.
   */
  const refocusAnchor = (): void => {
    const root = rootRef.current
    if (root === null) return
    if (takesFocus(triggerRef.current) || takesFocus(root.querySelector<HTMLButtonElement>('button:not(:disabled)'))) return
    focusContainingOwner(root)
  }

  /**
   * Settle a keyboard the menu held after its anchor stopped being a visible
   * interaction anchor. The anchor is no focus target then: one scrolled out
   * of its clipping context is still focusable, and focusing it would scroll
   * the container back to it, undoing the scroll that hid it. The keyboard
   * goes to the nearest containing owner instead, without scrolling; with no
   * such owner, a keyboard on the anchor stays where it is.
   */
  const settleAfterHiddenAnchor = (): void => {
    const root = rootRef.current
    if (root !== null) focusContainingOwner(root)
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
      if (active === null || active === document.body || inSurface(active)) refocusAnchor()
    })
  }
  const openRef = useRef(open)
  openRef.current = open
  const [openSubmenuId, setOpenSubmenuId] = useState<string | null>(null)
  /** The open submenu's card, while it is mounted. */
  const submenuRef = useRef<HTMLDivElement | null>(null)
  /** The submenu the keyboard is moving into, focused once it is placed. */
  const [enteringSubmenuId, setEnteringSubmenuId] = useState<string | null>(null)
  const entered = useCallback(() => { setEnteringSubmenuId(null) }, [])
  /** Close the open submenu on request (Escape, ArrowLeft): a keyboard inside
   * it goes back to its row, the visible anchor it was entered from. */
  const collapseSubmenu = useCallback((): void => {
    const inside = submenuRef.current?.contains(document.activeElement) === true
    const row = listRef.current?.querySelector<HTMLButtonElement>('[aria-expanded="true"]')
    setOpenSubmenuId(null)
    if (inside) row?.focus()
  }, [])
  /** Close the open submenu because its row was scrolled out of the list. A
   * keyboard inside the submenu, or on that row, settles on the still-open
   * list rather than on the clipped row: focusing the row would scroll the
   * list back to it, undoing the scroll that hid it. */
  const releaseHiddenSubmenu = useCallback((): void => {
    const active = document.activeElement
    const row = listRef.current?.querySelector<HTMLButtonElement>('[aria-expanded="true"]')
    if (submenuRef.current?.contains(active) === true || (row !== null && row === active)) takesFocus(listRef.current, true)
    setOpenSubmenuId(null)
  }, [])
  const submenuId = useId()
  const { arm: armClose, cancel: cancelClose } = usePointerGrace(onClose)

  /** Whether a node is inside one of this menu's surfaces: the list or its open submenu. */
  const inSurface = (node: Node | null): boolean => listRef.current?.contains(node) === true || submenuRef.current?.contains(node) === true

  // Menu geometry is Floating UI's, and only Floating UI's: anchor
  // measurement, the side/align placement, flipping to the other side and
  // shifting along it to stay VIEWPORT_MARGIN inside the viewport, the room
  // the viewport leaves the card, and repositioning whenever an ancestor
  // scrolls, the viewport resizes or either element's layout changes.
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
  const { refs, floatingStyles, isPositioned, middlewareData } = useFloating({
    open,
    placement: placementOf(side, align),
    strategy: 'fixed',
    transform: false,
    elements: { reference },
    whileElementsMounted: autoUpdate,
    middleware: [
      offset(4),
      flip({ padding: VIEWPORT_MARGIN }),
      shift({ padding: VIEWPORT_MARGIN }),
      availableRoom,
      anchorLiveness,
    ],
  })
  const setList = useCallback((node: HTMLDivElement | null) => {
    listRef.current = node
    refs.setFloating(node)
  }, [refs])
  // Floating UI places the list in the commit that opens it: its first
  // computation resolves within the same task and is flushed synchronously,
  // before the browser paints. autoFocus still waits for that placement, so it
  // never lands on an unplaced row. A host-owned anchor that has never supplied
  // a rect has nothing to place against, so that list stays invisible.
  const placed = isPositioned && (!hostAnchored || hostRect.current !== null)
  const listStyle: CSSProperties = !hostAnchored || hostRect.current !== null ? floatingStyles : { ...floatingStyles, visibility: 'hidden' }
  // Read only from a placement of this opening: an unplaced list's data is the
  // previous opening's, and a host anchor that has not supplied a rect yet
  // has not been measured at all.
  const anchorHidden = placed && middlewareData.hide?.referenceHidden === true

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

  // The anchor left rendered layout (or was scrolled out of its clipping
  // context) while the list is open: settle closed. Unlike every other close,
  // a keyboard the menu held does not go back to the anchor, which is hidden;
  // it settles on the anchor's containing owner.
  useEffect(() => {
    if (!open || !anchorHidden) return
    const active = document.activeElement
    const held = inSurface(active) || rootRef.current?.contains(active) === true
    onClose()
    if (held) settleAfterHiddenAnchor()
  }, [open, anchorHidden, onClose])

  useEffect(() => {
    if (!open) {
      setOpenSubmenuId(null)
      setEnteringSubmenuId(null)
      walkIndex.current = null
      return
    }
    const onPointerDown = (e: PointerEvent) => {
      if (!(e.target instanceof Node)) return
      // The portaled surfaces are outside the anchor subtree; check all.
      if (rootRef.current?.contains(e.target) === true) return
      if (inSurface(e.target)) return
      onClose()
    }
    const onKeyDown = (e: KeyboardEvent) => {
      // Where the keyboard is, computed once: the menu owns it when it holds a
      // row of either layer or sits on its anchor region.
      const focused = document.activeElement
      const submenu = submenuRef.current
      const inSubmenu = submenu !== null && submenu.contains(focused)
      const insideList = listRef.current?.contains(focused) === true || inSubmenu
      const anchored = rootRef.current?.contains(focused) === true || insideList
      if (e.key === 'Escape') {
        // Escape belongs to this menu before its containing modal. The rustX
        // Modal reads the prevented default; a React Aria modal answers
        // Escape from React's own listener, which a portaled list's events
        // still reach through the React tree, so the event stops here, in
        // the document capture phase, before any of them sees it.
        e.preventDefault()
        e.stopPropagation()
        // A submenu is the topmost layer: Escape inside it closes only it.
        if (inSubmenu) {
          collapseSubmenu()
          return
        }
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
        // Tab settles the row it is on (a submenu row settles by entering its
        // submenu); from anywhere else in the menu region it enters the list.
        // A focused control that is not a row (a retry button inside an error
        // strip) and a list with no enabled row keep the browser's traversal
        // instead of being swallowed; a keyboard settled on the list itself
        // enters its rows.
        if (insideList && focused !== list) {
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
      // alternate between two rows. A submenu is walked on its own, from
      // the row that holds the keyboard.
      if (e.key === 'ArrowRight') {
        // A submenu row enters its submenu, as settling it does.
        if (insideList && !inSubmenu && focused instanceof HTMLElement && focused.getAttribute('aria-haspopup') === 'menu') {
          e.preventDefault()
          focused.click()
        }
        return
      }
      if (e.key === 'ArrowLeft') {
        if (inSubmenu) {
          e.preventDefault()
          collapseSubmenu()
        }
        return
      }
      if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(e.key)) return
      const layer = inSubmenu ? submenu : listRef.current
      if (layer === null || !anchored) return
      const buttons = Array.from(layer.querySelectorAll<HTMLButtonElement>('button:not(:disabled)'))
      if (buttons.length === 0) return
      const index = buttons.indexOf(focused as HTMLButtonElement)
      const from = index >= 0 ? index : walkIndex.current
      const next = e.key === 'Home' ? 0 : e.key === 'End' ? buttons.length - 1
        : from === null
          ? (e.key === 'ArrowDown' ? 0 : buttons.length - 1)
          : (from + (e.key === 'ArrowDown' ? 1 : -1) + buttons.length) % buttons.length
      e.preventDefault()
      if (!inSubmenu) walkIndex.current = next
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
  }, [open, onClose, autoFocus, collapseSubmenu])

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
    const openSubmenu = subOpen ? entry.submenu : undefined
    const selected = entry.id === selectedId || selectedIds?.includes(entry.id) === true
    return (
      <ItemCell
        key={entry.id}
        onMouseEnter={() => { setOpenSubmenuId(hasSub ? entry.id : null) }}
        onMouseLeave={() => { setOpenSubmenuId(null) }}
        submenu={openSubmenu === undefined ? undefined : (row => (
          <Submenu
            id={submenuId}
            row={row}
            items={openSubmenu}
            dense={dense}
            compact={compact}
            enter={enteringSubmenuId === entry.id}
            onEntered={entered}
            onRowHidden={releaseHiddenSubmenu}
            cardRef={submenuRef}
            onSelect={(id) => { onSelect(id); refocusAfterSelection() }}
          />
        ))}
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
          aria-controls={subOpen ? submenuId : undefined}
          // Focus shows a row's submenu — except the keyboard coming back from
          // that submenu, which has just closed it.
          onFocus={(e) => { if (!(e.relatedTarget instanceof Node && submenuRef.current?.contains(e.relatedTarget) === true)) setOpenSubmenuId(hasSub ? entry.id : null) }}
          onClick={(e) => {
            if (hasSub) {
              setOpenSubmenuId(entry.id)
              // A keyboard activation (Enter, Space, Tab, ArrowRight: no
              // pointer detail) also moves the keyboard into the submenu.
              if (e.detail === 0) setEnteringSubmenuId(entry.id)
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
      </ItemCell>
    )
  }

  // The first painted frame of the list is already at its final position
  // (with getAnchorRect never returning a rect the list stays hidden). The
  // list is a top layer: a React Aria modal that contains its anchor keeps it
  // visible to assistive technology, lets focus enter it and does not treat a
  // press inside it as an interaction outside the modal.
  const list = open && (
    <div
      ref={setList}
      className={clsx(css.list, dense && css.denseList, compact && css.compactList)}
      style={listStyle}
      data-react-aria-top-layer
      role="menu"
      // The list itself holds a keyboard whose row was hidden under it (see
      // releaseHiddenSubmenu); it is never a Tab stop.
      tabIndex={-1}
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
      {list !== false && createPortal(list, document.body)}
    </span>
  )
}

/**
 * One item's cell: its row and, while open, the submenu anchored to it. The
 * cell holds its own element, so a submenu is only ever placed against the
 * row it belongs to.
 */
function ItemCell({ onMouseEnter, onMouseLeave, submenu, children }: {
  onMouseEnter: () => void
  onMouseLeave: () => void
  submenu: ((row: HTMLDivElement | null) => ReactNode) | undefined
  children: ReactNode
}) {
  const [row, setRow] = useState<HTMLDivElement | null>(null)
  return (
    <div ref={setRow} onMouseEnter={onMouseEnter} onMouseLeave={onMouseLeave}>
      {children}
      {submenu?.(row)}
    </div>
  )
}

/**
 * A submenu's card: a top layer of its own, Floating UI-placed against its
 * row exactly as the list is against its anchor. It opens beside the
 * row's end (bottom-aligned with the row's card inset), flips to the other
 * side when that one lacks the room, shifts along the row to stay inside the
 * viewport, keeps its design size within the room left (its rows scroll
 * inside it), and follows the row through scroll, resize and layout changes.
 * The card is a React child of the row, so the pointer moving from row to
 * card never leaves the row; its bridge spans the gap between them. Like the
 * list, the card lives only while its row is a visible anchor: a row scrolled
 * out of the list closes it.
 */
function Submenu({ id, row, items, dense, compact, enter, onEntered, onRowHidden, cardRef, onSelect }: {
  id: string
  row: HTMLDivElement | null
  items: readonly MenuItem[]
  /** The parent's row spacing and typography: the card is not its descendant. */
  dense: boolean
  compact: boolean
  /** Move the keyboard to the first enabled row once the card is placed. */
  enter: boolean
  onEntered: () => void
  /** The row stopped being a visible anchor: close this card. */
  onRowHidden: () => void
  cardRef: { current: HTMLDivElement | null }
  onSelect: (id: string) => void
}) {
  const { refs, floatingStyles, placement, isPositioned, elements, middlewareData } = useFloating({
    placement: 'right-end',
    strategy: 'fixed',
    transform: false,
    elements: { reference: row },
    whileElementsMounted: autoUpdate,
    middleware: [
      offset({ mainAxis: SUBMENU_GAP, crossAxis: SUBMENU_DROP }),
      // Only the side flips; the vertical fit is shift's and the size's.
      flip({ padding: VIEWPORT_MARGIN, crossAxis: false, flipAlignment: false }),
      shift({ padding: VIEWPORT_MARGIN }),
      availableRoom,
      anchorLiveness,
    ],
  })
  const rowHidden = isPositioned && middlewareData.hide?.referenceHidden === true
  useEffect(() => {
    if (rowHidden) onRowHidden()
  }, [rowHidden, onRowHidden])
  const setCard = useCallback((node: HTMLDivElement | null) => {
    cardRef.current = node
    refs.setFloating(node)
  }, [cardRef, refs])
  useEffect(() => {
    if (!enter || !isPositioned) return
    elements.floating?.querySelector<HTMLButtonElement>('button:not(:disabled)')?.focus()
    onEntered()
  }, [enter, isPositioned, elements.floating, onEntered])
  const style = { ...floatingStyles, '--menu-submenu-gap': `${SUBMENU_GAP}px` } as CSSProperties
  return createPortal(
    <div
      ref={setCard}
      id={id}
      className={clsx(css.submenu, dense && css.denseList, compact && css.compactList)}
      style={style}
      data-side={placement.split('-')[0]}
      data-react-aria-top-layer
      role="menu"
    >
      <div className={css.viewport} role="presentation">
        {items.map(sub => (
          <button
            key={sub.id}
            type="button"
            role="menuitem"
            className={css.item}
            disabled={sub.disabled}
            onClick={() => { onSelect(sub.id) }}
          >
            {sub.icon !== undefined && <span className={css.itemIcon}>{sub.icon}</span>}
            <span className={css.itemLabel}>{sub.label}</span>
          </button>
        ))}
      </div>
    </div>,
    document.body,
  )
}

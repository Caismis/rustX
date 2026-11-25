/* Copyright (c) 2026 DeepSeek. MIT. See PROVENANCE.md. */
// Menu: coss ui/Base UI menu behind rustX's declarative menu vocabulary.
// The adapter keeps host-owned open state, composite anchors and focus-owner
// settlement outside the external component API.

import { flushSync } from 'react-dom'
import clsx from 'clsx'
import { createContext, useContext, useEffect, useMemo, useRef, useState } from 'react'
import type { ComponentProps, KeyboardEvent as ReactKeyboardEvent, MouseEvent as ReactMouseEvent, PointerEvent as ReactPointerEvent, ReactNode, Ref, RefObject } from 'react'
import { useDialogPortal, useOverlayActive } from './DialogSurface'
import {
  Menu as CossMenu,
  MenuGroup,
  MenuGroupLabel,
  MenuItem as CossMenuItem,
  MenuPopup,
  MenuSeparator,
  MenuSub,
  MenuSubPopup,
  MenuSubTrigger,
} from '@/components/ui/menu'
import { IconCheckOutline16 } from './icons/index.tsx'
import css from './Menu.module.css'

/** Selectable row (optionally with a nested submenu). */
export interface MenuItem {
  id: string
  label: ReactNode
  disabled?: boolean
  /** Leading icon. */
  icon?: ReactNode
  /** Destructive row. */
  danger?: boolean
  /** Nested card opened beside the row on hover/focus. */
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

function takesFocus(element: HTMLElement | null | undefined, preventScroll = false): boolean {
  if (!element?.isConnected || element.matches(':disabled, [aria-disabled="true"]')) return false
  element.focus({ preventScroll })
  return document.activeElement === element
}


/** Enabled rows whose nearest menu surface is `surface`, in display order. */
function surfaceRows(surface: Element): HTMLElement[] {
  return [...surface.querySelectorAll<HTMLElement>('[role="menuitem"]')]
    .filter(row => row.closest('[data-slot="menu-popup"], [data-slot="menu-sub-content"]') === surface
      && !row.hasAttribute('data-disabled') && row.getAttribute('aria-disabled') !== 'true' && !(row as HTMLButtonElement).disabled)
}

/** Vertical arrows walk the rows of the surface holding the keyboard, one row
 * per keypress, wrapping at the ends. Base UI's own list navigation loses the
 * active row after pointer presses take rows over, so the adapter owns this
 * one contract directly. */
function navigateRows(event: ReactKeyboardEvent<HTMLElement>): void {
  if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return
  if (!(event.target instanceof HTMLElement)) return
  const surface = event.currentTarget
  const rows = surfaceRows(surface)
  if (rows.length === 0) return
  const current = event.target.closest('[role="menuitem"]')
  // Rows of a nested surface reach this capture through the React tree first:
  // their own layer's handler owns them.
  if (current !== null && rows.includes(current as HTMLElement) === false) return
  const index = current === null ? -1 : rows.indexOf(current as HTMLElement)
  const step = event.key === 'ArrowDown' ? 1 : -1
  const next = index === -1 ? rows[0] : rows[(index + step + rows.length) % rows.length]
  if (!next) return
  event.preventDefault()
  event.stopPropagation()
  next.focus()
}

function NativeMenuButton({ ref, ...props }: ComponentProps<'button'> & { ref?: Ref<HTMLButtonElement> }) {
  const innerRef = useRef<HTMLButtonElement | null>(null)
  useEffect(() => {
    const node = innerRef.current
    // Base UI exposes menu-item disablement as aria-disabled so it can keep
    // composite-menu focus semantics. rustX rows are native actions: mirror
    // that state onto the actual button as well so activation is impossible.
    if (node) node.disabled = props.disabled === true || node.getAttribute('aria-disabled') === 'true' || node.hasAttribute('data-disabled')
  })
  return <button
    {...props}
    ref={node => {
      innerRef.current = node
      if (node) node.disabled = props.disabled === true || node.getAttribute('aria-disabled') === 'true' || node.hasAttribute('data-disabled')
      if (typeof ref === 'function') ref(node)
      else if (ref !== null && ref !== undefined) ref.current = node
    }}
  />
}

interface SubmenuOwnership {
  owner: string | null
  claim: (id: string) => void
  release: (id: string) => void
  /** The row the pointer currently rests on; hover-opened submenus close when
   * it moves to a sibling row. */
  pointerRow: string | null
  enterRow: (id: string) => void
  /** True while a pointer press is in flight: the press focus must not open
   * submenus, because the trigger's own click toggle owns that open. */
  pressActive: RefObject<boolean>
}

const SubmenuOwnershipContext = createContext<SubmenuOwnership>({ owner: null, claim: () => {}, release: () => {}, pointerRow: null, enterRow: () => {}, pressActive: { current: false } })

interface HeldPress {
  id: string
  disabled: boolean
  element: HTMLElement
  invoke: () => void
}

interface PressControl {
  held: HeldPress | null
  begin: (item: MenuItem, event: ReactPointerEvent<HTMLElement>, invoke: () => void) => void
  click: (item: MenuItem, event: ReactMouseEvent<HTMLElement>) => void
  cancel: (id: string) => void
}

function itemControl(item: MenuItem, press: PressControl, select: (id: string) => void) {
  const held = press.held?.id === item.id ? press.held : null
  const disabled = held ? held.disabled : item.disabled === true
  return {
    disabled,
    nativeButton: true,
    render: <NativeMenuButton type="button" />,
    onPointerDown: (event: ReactPointerEvent<HTMLElement>) => press.begin(item, event, () => select(item.id)),
    onPointerCancel: () => press.cancel(item.id),
    onClick: (event: ReactMouseEvent<HTMLElement>) => press.click(item, event),
  }
}

function SubmenuRow({ entry, parentOpen, selected, select, selection, press }: {
  entry: MenuItem
  parentOpen: boolean
  selected: (id: string) => boolean
  select: (id: string) => void
  selection: 'check' | 'fill'
  press: PressControl
}) {
  const ownership = useContext(SubmenuOwnershipContext)
  const [submenuOpen, setSubmenuOpen] = useState(false)
  const [subPopupElement, setSubPopupElement] = useState<HTMLDivElement | null>(null)
  const triggerRef = useRef<HTMLDivElement | null>(null)
  const suppressFocusOpen = useRef(false)
  const keyboardOwned = ownership.owner === entry.id
  const hoverCloseTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  useEffect(() => () => {
    if (hoverCloseTimer.current !== null) clearTimeout(hoverCloseTimer.current)
  }, [])
  const cancelHoverClose = () => {
    if (hoverCloseTimer.current !== null) {
      clearTimeout(hoverCloseTimer.current)
      hoverCloseTimer.current = null
    }
  }
  const ownershipRef = useRef(ownership)
  ownershipRef.current = ownership
  const armHoverClose = () => {
    cancelHoverClose()
    hoverCloseTimer.current = setTimeout(() => {
      if (ownershipRef.current.owner !== entry.id) setSubmenuOpen(false)
    }, 100)
  }
  useEffect(() => {
    if (!parentOpen) {
      setSubmenuOpen(false)
      ownership.release(entry.id)
    }
  }, [parentOpen, ownership, entry.id])
  // A hover-opened submenu closes once the pointer moves onto a sibling row
  // of the parent list. Only pointer movement (not keyboard focus or the
  // open itself) counts, and keyboard-owned submenus ignore the pointer.
  const lastPointerRow = useRef(ownership.pointerRow)
  useEffect(() => {
    const previous = lastPointerRow.current
    lastPointerRow.current = ownership.pointerRow
    if (submenuOpen && !keyboardOwned && ownership.pointerRow !== previous && ownership.pointerRow !== entry.id) setSubmenuOpen(false)
  }, [submenuOpen, keyboardOwned, ownership.pointerRow, entry.id])
  useEffect(() => {
    if (!submenuOpen) return undefined
    const trigger = triggerRef.current
    const positioner = subPopupElement?.closest<HTMLElement>('[data-slot="menu-positioner"]')
    if (!positioner) return undefined
    let closing = false
    // Base UI's hide middleware reports every all-zero anchor rect as hidden,
    // so the signal is only truthful where layout measurement exists at all.
    const measurable = document.documentElement.getBoundingClientRect().width > 0
    const settle = () => {
      if (closing || !measurable || !positioner.hasAttribute('data-anchor-hidden')) return
      closing = true
      ownership.release(entry.id)
      // The keyboard lands on the parent list before the closing surface
      // unmounts, so it never falls through to the document.
      trigger?.closest<HTMLElement>('[data-slot="menu-popup"]')?.focus({ preventScroll: true })
      flushSync(() => setSubmenuOpen(false))
    }
    settle()
    const observer = new MutationObserver(settle)
    observer.observe(positioner, { attributes: true, attributeFilter: ['data-anchor-hidden'] })
    return () => observer.disconnect()
  }, [submenuOpen, ownership, entry.id, subPopupElement])

  const content = <>
    {entry.icon !== undefined && <span className={css.itemIcon}>{entry.icon}</span>}
    <span className={css.itemLabel}>{entry.label}</span>
    {selection === 'check' && selected(entry.id) && <IconCheckOutline16 className={css.check} />}
  </>
  return <MenuSub highlightItemOnHover={false} open={submenuOpen} onOpenChange={(next, details) => {
    if (next && ownership.owner !== null && ownership.owner !== entry.id) return
    if (!next && keyboardOwned && details.reason !== 'escape-key' && details.reason !== 'list-navigation' && details.reason !== 'item-press' && details.reason !== 'imperative-action') return
    setSubmenuOpen(next)
    if (!next) {
      if (details.reason === 'escape-key' || details.reason === 'list-navigation') suppressFocusOpen.current = true
      if (keyboardOwned && details.reason !== 'sibling-open') ownership.release(entry.id)
    }
  }}>
    <MenuSubTrigger
      ref={triggerRef}
      render={<NativeMenuButton type="button" />}
      className={clsx(css.item, entry.danger && css.danger, selected(entry.id) && (selection === 'fill' ? css.selectedFill : css.selected))}
      disabled={entry.disabled}
      delay={0}
      openOnHover={false}
      onPointerEnter={() => {
        ownership.enterRow(entry.id)
        if (entry.disabled || ownership.owner !== null) return
        cancelHoverClose()
        setSubmenuOpen(true)
      }}
      onPointerLeave={armHoverClose}
      data-variant={entry.danger ? 'destructive' : undefined}
      aria-current={selected(entry.id) ? 'true' : undefined}
      onFocus={() => {
        if (suppressFocusOpen.current) {
          suppressFocusOpen.current = false
          return
        }
        if (ownership.pressActive.current) return
        if (!entry.disabled) {
          // Focus reaches a row only through the keyboard or a pointer press;
          // the press path returns above, so an open driven from here is
          // keyboard-owned until a pointer press takes it over.
          ownership.claim(entry.id)
          setSubmenuOpen(true)
        }
      }}
      onBlur={(event) => {
        const next = event.relatedTarget
        const owned = event.currentTarget.getAttribute('aria-controls')
        const popup = owned === null ? null : document.getElementById(owned)
        if (next instanceof Node && popup?.contains(next) === true) return
        ownership.release(entry.id)
        setSubmenuOpen(false)
      }}
      onKeyDownCapture={(event) => {
        if (event.key === 'ArrowRight' || event.key === 'Enter' || event.key === ' ' || event.key === 'Tab') {
          if (entry.disabled) return
          event.preventDefault()
          event.stopPropagation()
          suppressFocusOpen.current = false
          cancelHoverClose()
          ownership.claim(entry.id)
          setSubmenuOpen(true)
          const trigger = event.currentTarget
          const focusFirst = () => {
            const owned = trigger.getAttribute('aria-controls')
            const popup = owned === null ? null : document.getElementById(owned)
            const first = [...(popup?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? [])]
              .find(row => !row.hasAttribute('data-disabled') && row.getAttribute('aria-disabled') !== 'true' && !(row as HTMLButtonElement).disabled)
            if (!first) return false
            first.focus()
            return true
          }
          // The portal mounts a frame after the open state lands; retry until
          // the first row can actually take the keyboard.
          let attempts = 0
          const enter = () => {
            attempts += 1
            if (!focusFirst() && attempts < 10) requestAnimationFrame(enter)
          }
          requestAnimationFrame(enter)
          return
        }
      }}
    >
      {content}
    </MenuSubTrigger>
    <MenuSubPopup ref={node => { node?.removeAttribute('tabindex'); setSubPopupElement(node) }} className={css.submenu} align="end" alignOffset={-4} sideOffset={10} collisionPadding={12} style={{ visibility: submenuOpen ? undefined : 'hidden' }} onPointerEnter={cancelHoverClose} onPointerLeave={armHoverClose} onKeyDownCapture={navigateRows}>
      {entry.submenu?.map(item => {
        const control = itemControl(item, press, select)
        return <CossMenuItem
          key={item.id}
          className={clsx(css.item, item.danger && css.danger, selected(item.id) && (selection === 'fill' ? css.selectedFill : css.selected))}
          variant={item.danger ? 'destructive' : 'default'}
          closeOnClick={false}
          aria-current={selected(item.id) ? 'true' : undefined}
          {...control}
        >
          {item.icon !== undefined && <span className={css.itemIcon}>{item.icon}</span>}
          <span className={css.itemLabel}>{item.label}</span>
          {selection === 'check' && selected(item.id) && <IconCheckOutline16 className={css.check} />}
        </CossMenuItem>
      })}
    </MenuSubPopup>
  </MenuSub>
}

function renderRows(entries: readonly MenuEntry[], selected: (id: string) => boolean, select: (id: string) => void, selection: 'check' | 'fill', press: PressControl, parentOpen: boolean, ownership: SubmenuOwnership) {
  const renderItem = (entry: MenuItem) => {
    const content = <>
      {entry.icon !== undefined && <span className={css.itemIcon}>{entry.icon}</span>}
      <span className={css.itemLabel}>{entry.label}</span>
      {selection === 'check' && selected(entry.id) && <IconCheckOutline16 className={css.check} />}
    </>
    if (entry.submenu?.length) {
      return <SubmenuRow key={entry.id} entry={entry} parentOpen={parentOpen} selected={selected} select={select} selection={selection} press={press} />
    }
    const control = itemControl(entry, press, select)
    return <CossMenuItem
      key={entry.id}
      className={clsx(css.item, entry.danger && css.danger, selected(entry.id) && (selection === 'fill' ? css.selectedFill : css.selected))}
      variant={entry.danger ? 'destructive' : 'default'}
      closeOnClick={false}
      data-selected={selection === 'fill' && selected(entry.id) || undefined}
      aria-current={selected(entry.id) ? 'true' : undefined}
      onPointerEnter={() => ownership.enterRow(entry.id)}
      {...control}
    >
      {content}
    </CossMenuItem>
  }
  const rows: ReactNode[] = []
  let group: { id: string; label: string; items: ReactNode[] } | null = null
  const flush = () => {
    if (!group) return
    rows.push(<MenuGroup key={group.id}>
      <MenuGroupLabel className={css.label}>{group.label}</MenuGroupLabel>
      {group.items}
    </MenuGroup>)
    group = null
  }
  for (const entry of entries) {
    if (isLabel(entry)) {
      flush()
      group = { id: entry.id, label: entry.text, items: [] }
    } else if (isSeparator(entry)) {
      flush()
      rows.push(<MenuSeparator key={entry.id} className={css.separator} />)
    } else {
      const item = renderItem(entry)
      if (group) group.items.push(item)
      else rows.push(item)
    }
  }
  flush()
  return rows
}

/**
 * Render a declarative rustX menu with coss ui interaction, positioning,
 * keyboard navigation, typeahead, submenu and portal behavior.
 *
 * The host still owns `open`; selecting a row reports through `onSelect` and
 * does not force close, preserving multi-select and asynchronous row owners.
 * @param props.getAnchorRect - host-owned trigger measurement when the wrapper
 * is not itself laid out at the trigger.
 * @param props.focusOwner - stable visible element that receives focus when the
 * anchor can no longer take it.
 * @returns anchor wrapper with the conditional coss menu.
 */
export function Menu({ open: requestedOpen, anchor, items, selectedId, selectedIds, onSelect, onClose, align = 'start', side = 'bottom', closeOnPointerLeave = false, dense = false, compact = false, autoFocus = false, selection = 'check', getAnchorRect, focusOwner, footer, className, listClassName }: {
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
  focusOwner?: RefObject<HTMLElement | null> | undefined
  className?: string | undefined
  /** DSH per-surface styling; does not alter shared placement or keyboard behavior. */
  listClassName?: string | undefined
}) {
  const open = useOverlayActive() && requestedOpen
  const portalContainer = useDialogPortal()
  const rootRef = useRef<HTMLSpanElement>(null)
  const popupRef = useRef<HTMLDivElement | null>(null)
  const [popupElement, setPopupElement] = useState<HTMLDivElement | null>(null)
  const triggerRef = useRef<HTMLElement | null>(null)
  const wasOpen = useRef(false)
  const closeTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const focusAfterClose = useRef(false)
  const selectionFocus = useRef<HTMLElement | null>(null)
  const anchorLost = useRef(false)
  const heldPressRef = useRef<HeldPress | null>(null)
  const [heldPress, setHeldPress] = useState<HeldPress | null>(null)
  const [submenuKeyboardOwner, setSubmenuKeyboardOwner] = useState<string | null>(null)
  const [pointerRow, setPointerRow] = useState<string | null>(null)
  const pressActive = useRef(false)
  useEffect(() => {
    const clear = () => { pressActive.current = false }
    document.addEventListener('pointerup', clear, true)
    document.addEventListener('pointercancel', clear, true)
    return () => {
      document.removeEventListener('pointerup', clear, true)
      document.removeEventListener('pointercancel', clear, true)
    }
  }, [])
  const submenuOwnership = useMemo<SubmenuOwnership>(() => ({
    owner: open ? submenuKeyboardOwner : null,
    claim: id => setSubmenuKeyboardOwner(id),
    release: id => setSubmenuKeyboardOwner(current => current === id ? null : current),
    pointerRow,
    enterRow: id => setPointerRow(id),
    pressActive,
  }), [open, submenuKeyboardOwner, pointerRow])

  const selected = (id: string) => selectedId === id || selectedIds?.includes(id) === true
  const positioningAnchor = useMemo(() => {
    if (!getAnchorRect) return rootRef
    return { getBoundingClientRect: () => getAnchorRect() ?? rootRef.current?.getBoundingClientRect() ?? new DOMRect() }
  }, [getAnchorRect])

  const focusable = (element: HTMLElement | null | undefined) => element?.isConnected === true && !element.matches(':disabled, [aria-disabled="true"]')
  const focusTarget = () => {
    const root = rootRef.current
    if (!root) return null
    const explicit = selectionFocus.current
    if (focusable(explicit)) {
      selectionFocus.current = null
      return explicit
    }
    const trigger = triggerRef.current
    if (trigger !== null && root.contains(trigger) && focusable(trigger)) return trigger
    const anchorButton = root.querySelector<HTMLButtonElement>('button[aria-haspopup="menu"]:not(:disabled)') ?? root.querySelector<HTMLButtonElement>('button:not(:disabled)')
    if (focusable(anchorButton)) return anchorButton
    if (focusable(focusOwner?.current)) return focusOwner?.current ?? null
    return null
  }
  const refocusAnchor = () => {
    const target = focusTarget()
    if (target && takesFocus(target, target === focusOwner?.current)) return
    const active = document.activeElement
    if (active instanceof HTMLElement && rootRef.current?.contains(active)) active.blur()
  }

  useEffect(() => {
    if (open && !wasOpen.current) {
      const active = document.activeElement
      triggerRef.current = active instanceof HTMLElement && rootRef.current?.contains(active) === true ? active : null
      selectionFocus.current = null
      if (autoFocus) queueMicrotask(() => {
        const rows = [...(popupRef.current?.querySelectorAll<HTMLElement>('[role^="menuitem"]') ?? [])]
          .filter(row => !row.hasAttribute('data-disabled') && row.getAttribute('aria-disabled') !== 'true' && !(row as HTMLButtonElement).disabled)
        ;(rows.find(row => row.getAttribute('aria-current') === 'true') ?? rows[0])?.focus()
      })
    }
    if (!open && wasOpen.current) {
      if (anchorLost.current) {
        anchorLost.current = false
        focusAfterClose.current = false
        selectionFocus.current = null
        queueMicrotask(() => {
          const owner = focusOwner?.current
          if (focusable(owner) && takesFocus(owner, true)) return
          const active = document.activeElement
          if (active instanceof HTMLElement && (rootRef.current?.contains(active) === true || active.closest('[data-slot="menu-popup"]') !== null)) active.blur()
        })
      } else if (focusAfterClose.current) {
        focusAfterClose.current = false
        queueMicrotask(() => {
          const active = document.activeElement
          const explicit = selectionFocus.current
          selectionFocus.current = null
          if (explicit?.isConnected && takesFocus(explicit)) return
          const inMenu = active instanceof HTMLElement && (rootRef.current?.contains(active) === true || active.closest('[data-slot="menu-popup"]') !== null)
          if (active === document.body || active === document.documentElement || active === triggerRef.current || inMenu) refocusAnchor()
        })
      } else {
        queueMicrotask(() => {
          const active = document.activeElement
          if (active === document.body || active === document.documentElement || active instanceof HTMLElement && rootRef.current?.contains(active)) refocusAnchor()
        })
      }
    }
    wasOpen.current = open
  }, [open, autoFocus])

  useEffect(() => {
    if (!open) return undefined
    const onKeyDown = (event: KeyboardEvent) => {
      if (!(event.target instanceof Element)) return
      const inSubmenu = event.target.closest('[data-slot="menu-sub-content"]') !== null
      const inMenu = event.target.closest('[data-slot="menu-popup"]') !== null
      const closeAll = event.key === 'Escape' && !inSubmenu && (inMenu || rootRef.current?.contains(event.target) === true)
        || event.key === 'Tab' && event.shiftKey && inSubmenu
      if (!closeAll) return
      event.preventDefault()
      event.stopPropagation()
      focusAfterClose.current = true
      onClose()
      refocusAnchor()
    }
    document.addEventListener('keydown', onKeyDown, true)
    return () => document.removeEventListener('keydown', onKeyDown, true)
  }, [open, onClose])

  useEffect(() => () => {
    if (closeTimer.current !== null) clearTimeout(closeTimer.current)
  }, [])

  // Floating UI's reference-hidden measurement is the menu's liveness signal:
  // once the trigger is wholly clipped or leaves layout, close before the next
  // paint and let the close path settle the keyboard on the host-named owner.
  useEffect(() => {
    if (!open) return undefined
    const positioner = popupElement?.closest<HTMLElement>('[data-slot="menu-positioner"]')
    if (!positioner) return undefined
    let closing = false
    // Base UI's hide middleware reports every all-zero anchor rect as hidden,
    // so the signal is only truthful where layout measurement exists at all.
    const measurable = document.documentElement.getBoundingClientRect().width > 0
    const settle = () => {
      if (closing || !measurable || !positioner.hasAttribute('data-anchor-hidden')) return
      closing = true
      anchorLost.current = true
      focusAfterClose.current = false
      // The keyboard leaves the closing surface for the host-named owner in
      // the same task: moving it before the unmount keeps it from ever
      // falling to the document.
      const owner = focusOwner?.current
      if (owner && focusable(owner)) owner.focus({ preventScroll: true })
      flushSync(() => onClose())
    }
    settle()
    const observer = new MutationObserver(settle)
    observer.observe(positioner, { attributes: true, attributeFilter: ['data-anchor-hidden'] })
    let frame = 0
    const poll = () => {
      settle()
      if (!closing) frame = requestAnimationFrame(poll)
    }
    frame = requestAnimationFrame(poll)
    return () => {
      observer.disconnect()
      cancelAnimationFrame(frame)
    }
  }, [open, onClose, popupElement])

  const setPress = (next: HeldPress | null) => {
    heldPressRef.current = next
    // Native pointer listeners outside React's event batching must settle the
    // frozen press state synchronously: row owners read the resulting disabled
    // surface before the browser dispatches the following click.
    flushSync(() => setHeldPress(next))
  }
  const select = (id: string) => {
    focusAfterClose.current = true
    onSelect(id)
    const active = document.activeElement
    selectionFocus.current = active instanceof HTMLElement && active.closest('[data-slot="menu-popup"]') === null ? active : null
  }
  const pressControl: PressControl = {
    held: heldPress,
    begin: (item, event, invoke) => {
      if (event.button !== 0 || event.isPrimary === false) return
      const element = event.currentTarget
      // The press owns the keyboard before any state flush: sibling submenus
      // unmounting in the same gesture must never release it to the document.
      element.focus({ preventScroll: true })
      setPress({ id: item.id, disabled: item.disabled === true, element, invoke })
    },
    click: (item, _event) => {
      const held = heldPressRef.current
      if (held?.id === item.id) {
        if (!held.disabled) held.invoke()
        setPress(null)
        return
      }
      if (!item.disabled) select(item.id)
    },
    cancel: (id) => {
      if (heldPressRef.current?.id === id) setPress(null)
    },
  }

  useEffect(() => {
    if (heldPress === null) return
    const release = (event: PointerEvent) => {
      const held = heldPressRef.current
      if (held === null) return
      if (!(event.target instanceof Node && held.element.contains(event.target))) setPress(null)
    }
    const cancel = () => setPress(null)
    document.addEventListener('pointerup', release, true)
    document.addEventListener('pointercancel', cancel, true)
    return () => {
      document.removeEventListener('pointerup', release, true)
      document.removeEventListener('pointercancel', cancel, true)
    }
  }, [heldPress])

  const cancelClose = () => {
    if (closeTimer.current !== null) {
      clearTimeout(closeTimer.current)
      closeTimer.current = null
    }
  }
  const armClose = () => {
    if (!closeOnPointerLeave) return
    cancelClose()
    closeTimer.current = setTimeout(onClose, 240)
  }

  return (
    <span
      ref={rootRef}
      className={className}
      data-menu-dense={dense || undefined}
      data-menu-compact={compact || undefined}
      onPointerEnter={closeOnPointerLeave ? cancelClose : undefined}
      onPointerLeave={closeOnPointerLeave ? armClose : undefined}
    >
      {anchor}
      <CossMenu highlightItemOnHover={false} open={open} modal={false} onOpenChange={(next, details) => {
        if (!next) {
          if (details.reason === 'sibling-open') return;
          if (details.reason === 'escape-key') focusAfterClose.current = true
          onClose()
        }
      }}>
        <SubmenuOwnershipContext.Provider value={submenuOwnership}>
        <MenuPopup
          ref={(node) => {
            popupRef.current = node
            setPopupElement(node)
          }}
          anchor={positioningAnchor}
          align={align}
          side={side}
          sideOffset={4}
          portalProps={{ container: portalContainer }}
          collisionPadding={12}
          className={clsx(css.list, dense && css.denseList, compact && css.compactList, listClassName)}
          data-autofocus={autoFocus || undefined}
          aria-hidden={!open}
          inert={!open}
          style={{ visibility: open ? undefined : 'hidden' }}
          finalFocus={() => false}
          onKeyDownCapture={navigateRows}
          onPointerDownCapture={(event) => {
            if (!(event.target instanceof Element) || event.target.closest('[data-slot="menu-popup"]') !== event.currentTarget) return
            setSubmenuKeyboardOwner(null)
            // A press owns the keyboard before any state flush in this same
            // gesture: surfaces unmounting in it must never find the keyboard
            // still inside them and release it to the document.
            if (event.button === 0) {
              const row = event.target.closest<HTMLElement>('[role="menuitem"]')
              if (row !== null && !row.matches(':disabled, [aria-disabled="true"]')) {
                pressActive.current = true
                row.focus({ preventScroll: true })
              }
            }
          }}
        >
          {renderRows(items, selected, select, selection, pressControl, open, submenuOwnership)}
          {!!footer?.length && <MenuSeparator className={css.separator} />}
          {!!footer?.length && renderRows(footer, selected, select, selection, pressControl, open, submenuOwnership)}
        </MenuPopup>
        </SubmenuOwnershipContext.Provider>
      </CossMenu>
    </span>
  )
}

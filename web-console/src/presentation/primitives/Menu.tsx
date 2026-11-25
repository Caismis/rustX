/* Copyright (c) 2026 DeepSeek. MIT. See PROVENANCE.md. */
// Menu: coss ui/Base UI menu behind rustX's declarative menu vocabulary.
// The adapter keeps host-owned open state, composite anchors and focus-owner
// settlement outside the external component API.

import { useEffect, useMemo, useRef, useState } from 'react'
import type { ComponentProps, MouseEvent as ReactMouseEvent, PointerEvent as ReactPointerEvent, ReactNode, Ref, RefObject } from 'react'
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


function NativeMenuButton({ ref, ...props }: ComponentProps<'button'> & { ref?: Ref<HTMLButtonElement> }) {
  return <button
    {...props}
    disabled={props.disabled || props['aria-disabled'] === true || props['aria-disabled'] === 'true'}
    ref={node => {
      if (typeof ref === 'function') ref(node)
      else if (ref !== null && ref !== undefined) ref.current = node
    }}
  />
}

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

function renderRows(entries: readonly MenuEntry[], selected: (id: string) => boolean, select: (id: string) => void, selection: 'check' | 'fill', press: PressControl) {
  const itemControl = (item: MenuItem) => {
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
  const renderItem = (entry: MenuItem) => {
    const content = <>
      {entry.icon !== undefined && <span className="inline-flex shrink-0">{entry.icon}</span>}
      <span className="min-w-0 flex-1 truncate">{entry.label}</span>
      {selection === 'check' && selected(entry.id) && <IconCheckOutline16 className="ms-auto" />}
    </>
    if (entry.submenu?.length) {
      return <MenuSub key={entry.id}>
        <MenuSubTrigger disabled={entry.disabled} delay={0} data-variant={entry.danger ? 'destructive' : undefined} aria-current={selected(entry.id) ? 'true' : undefined}>
          {content}
        </MenuSubTrigger>
        <MenuSubPopup>
          {entry.submenu.map(item => {
            const control = itemControl(item)
            return <CossMenuItem
              key={item.id}
              variant={item.danger ? 'destructive' : 'default'}
              closeOnClick={false}
              aria-current={selected(item.id) ? 'true' : undefined}
              {...control}
            >
              {item.icon !== undefined && <span className="inline-flex shrink-0">{item.icon}</span>}
              <span className="min-w-0 flex-1 truncate">{item.label}</span>
              {selection === 'check' && selected(item.id) && <IconCheckOutline16 className="ms-auto" />}
            </CossMenuItem>
          })}
        </MenuSubPopup>
      </MenuSub>
    }
    const control = itemControl(entry)
    return <CossMenuItem
      key={entry.id}
      variant={entry.danger ? 'destructive' : 'default'}
      closeOnClick={false}
      data-selected={selection === 'fill' && selected(entry.id) || undefined}
      aria-current={selected(entry.id) ? 'true' : undefined}
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
      <MenuGroupLabel>{group.label}</MenuGroupLabel>
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
      rows.push(<MenuSeparator key={entry.id} />)
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
  const triggerRef = useRef<HTMLElement | null>(null)
  const wasOpen = useRef(false)
  const closeTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const focusAfterClose = useRef(false)
  const selectionFocus = useRef<HTMLElement | null>(null)
  const heldPressRef = useRef<HeldPress | null>(null)
  const [heldPress, setHeldPress] = useState<HeldPress | null>(null)

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
    }
    if (!open && wasOpen.current) {
      if (focusAfterClose.current) {
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
  }, [open])

  useEffect(() => {
    if (!open) return undefined
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || !(event.target instanceof Element)) return
      const inSubmenu = event.target.closest('[data-slot="menu-sub-content"]') !== null
      const inMenu = event.target.closest('[data-slot="menu-popup"]') !== null
      if (inSubmenu || (!inMenu && rootRef.current?.contains(event.target) !== true)) return
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

  // A menu must not remain interactive after its trigger scrolls out of its
  // clipping owner or is otherwise hidden; Base UI only observes layout, not
  // ownership-specific visibility, so the adapter closes that edge itself.
  useEffect(() => {
    if (!open) return
    const target = rootRef.current?.querySelector<HTMLElement>('button, [role="button"]') ?? rootRef.current
    if (!target) return
    const hidden = () => {
      if (getAnchorRect) return getAnchorRect() === null
      return target.closest('[hidden], [inert]') !== null
    }
    if (hidden()) {
      onClose()
      return
    }
    const observer = typeof IntersectionObserver === 'undefined' || getAnchorRect ? undefined : new IntersectionObserver(entries => {
      if (entries.some(entry => !entry.isIntersecting)) onClose()
    }, { threshold: 0 })
    if (observer && target instanceof Element) observer.observe(target)
    const check = () => { if (hidden()) onClose() }
    window.addEventListener('scroll', check, true)
    window.addEventListener('resize', check)
    return () => {
      observer?.disconnect()
      window.removeEventListener('scroll', check, true)
      window.removeEventListener('resize', check)
    }
  }, [open, getAnchorRect, onClose])

  const setPress = (next: HeldPress | null) => {
    heldPressRef.current = next
    setHeldPress(next)
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
      <CossMenu open={open} modal={false} onOpenChange={(next, details) => {
        if (!next) {
          if (details.reason === 'sibling-open') return;
          if (details.reason === 'escape-key') focusAfterClose.current = true
          onClose()
        }
      }}>
        <MenuPopup
          anchor={positioningAnchor}
          align={align}
          side={side}
          sideOffset={4}
          portalProps={{ container: portalContainer }}
          className={listClassName}
          data-autofocus={autoFocus || undefined}
          finalFocus={() => focusTarget() ?? false}
        >
          {renderRows(items, selected, select, selection, pressControl)}
          {!!footer?.length && <MenuSeparator />}
          {!!footer?.length && renderRows(footer, selected, select, selection, pressControl)}
        </MenuPopup>
      </CossMenu>
    </span>
  )
}

/* Copyright (c) 2026 DeepSeek. MIT. See PROVENANCE.md. */
// Menu: coss ui/Base UI menu behind rustX's declarative menu vocabulary.
// The adapter keeps host-owned open state, composite anchors and focus-owner
// settlement outside the external component API.

import { useEffect, useMemo, useRef, useState } from 'react'
import type { ComponentProps, MouseEvent as ReactMouseEvent, PointerEvent as ReactPointerEvent, ReactNode, Ref, RefObject } from 'react'
import { useDialogPortal, useOverlayActive } from './DialogSurface'
import {
  Menu as CossMenu,
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
  const localRef = useRef<HTMLButtonElement | null>(null)
  useEffect(() => {
    if (localRef.current !== null) localRef.current.disabled = props['aria-disabled'] === true || props['aria-disabled'] === 'true'
  })
  return <button
    {...props}
    ref={node => {
      localRef.current = node
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
  return entries.map(entry => {
    if (isSeparator(entry)) return <MenuSeparator key={entry.id} />
    if (isLabel(entry)) return <MenuGroupLabel key={entry.id}>{entry.text}</MenuGroupLabel>
    const content = <>
      {entry.icon !== undefined && <span className="inline-flex shrink-0">{entry.icon}</span>}
      <span className="min-w-0 flex-1 truncate">{entry.label}</span>
      {selection === 'check' && selected(entry.id) && <IconCheckOutline16 className="ms-auto" />}
    </>
    if (entry.submenu?.length) {
      return <MenuSub key={entry.id}>
        <MenuSubTrigger disabled={entry.disabled} data-variant={entry.danger ? 'destructive' : undefined}>
          {content}
        </MenuSubTrigger>
        <MenuSubPopup>
          {entry.submenu.map(item => {
            const control = itemControl(item)
            return <CossMenuItem
              key={item.id}
              variant={item.danger ? 'destructive' : 'default'}
              closeOnClick={false}
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
      {...control}
    >
      {content}
    </CossMenuItem>
  })
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
  const heldPressRef = useRef<HeldPress | null>(null)
  const [heldPress, setHeldPress] = useState<HeldPress | null>(null)

  const selected = (id: string) => selectedId === id || selectedIds?.includes(id) === true
  const positioningAnchor = useMemo(() => {
    if (!getAnchorRect) return rootRef
    return { getBoundingClientRect: () => getAnchorRect() ?? rootRef.current?.getBoundingClientRect() ?? new DOMRect() }
  }, [getAnchorRect])

  const refocusAnchor = () => {
    const root = rootRef.current
    if (!root) return
    const trigger = triggerRef.current
    if (trigger !== null && root.contains(trigger) && takesFocus(trigger)) return
    if (takesFocus(root.querySelector<HTMLButtonElement>('button[aria-haspopup="menu"]:not(:disabled)') ?? root.querySelector<HTMLButtonElement>('button:not(:disabled)'))
      || takesFocus(triggerRef.current)) return
    if (!takesFocus(focusOwner?.current, true)) {
      const active = document.activeElement
      if (active instanceof HTMLElement && root.contains(active)) active.blur()
    }
  }

  useEffect(() => {
    if (open && !wasOpen.current) {
      triggerRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null
    }
    if (!open && wasOpen.current) {
      if (focusAfterClose.current) {
        focusAfterClose.current = false
        refocusAnchor()
      } else {
        queueMicrotask(() => {
          const active = document.activeElement
          if (active === document.body || active === document.documentElement || active instanceof HTMLElement && rootRef.current?.contains(active)) refocusAnchor()
        })
      }
    }
    wasOpen.current = open
  }, [open])

  useEffect(() => () => {
    if (closeTimer.current !== null) clearTimeout(closeTimer.current)
  }, [])

  const setPress = (next: HeldPress | null) => {
    heldPressRef.current = next
    setHeldPress(next)
  }
  const select = (id: string) => {
    focusAfterClose.current = true
    onSelect(id)
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
        >
          {renderRows(items, selected, select, selection, pressControl)}
          {!!footer?.length && <MenuSeparator />}
          {!!footer?.length && renderRows(footer, selected, select, selection, pressControl)}
        </MenuPopup>
      </CossMenu>
    </span>
  )
}

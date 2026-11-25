/* Copyright (c) 2026 DeepSeek. MIT. Source port; see PROVENANCE.md. */
/**
 * The per-tab context menu, opened by a secondary press on the chip. It carries
 * the close gesture and whatever the embedder appends; the copy and float
 * gestures have no menu item — copying is an embedder API, floating is a drag
 * released clear of the surface. A menu that would hold no item at all renders
 * no popup, so a secondary press on a chip with nothing to offer shows nothing.
 * Presentational — it renders what its props supply and dismisses itself on
 * outside presses or an unmodified Escape, returning menu focus to its tab.
 *
 * It renders in a portal, positioned against the control that opened it. The tab
 * strip clips its overflow on purpose (so it never becomes a scroll container
 * that claims a drag), and a menu drawn inside the strip would be clipped with
 * it; a portal puts it above every clipping ancestor. React still bubbles the
 * portal's synthetic events through the strip, which is why the press guards
 * below remain necessary.
 */
import { Children, useEffect, useRef } from 'react'
import type { ReactNode } from 'react'
import { focusWithoutRing, modalSelector, observeComposition } from '../primitives'
import type { DockLabels } from '../contract/adapter.ts'
import css from './dockkit.module.css'
import { Popover, PopoverPopup } from '@/components/ui/popover'

/** Gap between the opening control and the menu, and the viewport margin kept clear. */
const MENU_GAP = 4

/** What the menu offers, where it anchors, and how it closes. */
export interface TabMenuProps {
  readonly labels: DockLabels
  /** The control that opened the menu; the menu hangs below its left edge. */
  readonly anchor: HTMLElement
  /** Close the tab; `undefined` removes the kit's item, leaving the extras only. */
  readonly onClose: (() => void) | undefined
  /** Dismiss without acting. */
  readonly onDismiss: () => void
  /** Embedder items, rendered after the kit's own; absent means none. */
  readonly extras: ReactNode
}

/** The actions menu body, anchored to the control that opened it. */
export function TabMenu({ labels, anchor, onClose, onDismiss, extras }: TabMenuProps): ReactNode {
  const self = useRef<HTMLDivElement | null>(null)
  const anchorRef = useRef<HTMLElement>(anchor)
  anchorRef.current = anchor
  const hasItems = onClose !== undefined || Children.toArray(extras).some(item => item !== '')

  useEffect(() => {
    const menu = self.current
    /* v8 ignore next -- the ref is attached by effect time: the menu renders unconditionally. */
    if (menu === null) return undefined
    const document = menu.ownerDocument
    const composition = observeComposition(document)
    const onKeyDown = (event: KeyboardEvent): void => {
      if (event.key !== 'Escape') return
      if (composition.guards(event) || event.defaultPrevented || event.ctrlKey || event.altKey || event.metaKey || event.shiftKey) return
      const top = [...document.querySelectorAll(modalSelector)].at(-1)
      if (top !== menu) return
      event.preventDefault()
      event.stopPropagation()
      if (event.repeat) return
      const restoreFocus = menu.contains(document.activeElement)
      onDismiss()
      if (restoreFocus) focusWithoutRing(anchor)
    }
    document.addEventListener('keydown', onKeyDown, true)
    return () => {
      composition.dispose()
      document.removeEventListener('keydown', onKeyDown, true)
    }
  }, [anchor, onDismiss, hasItems])

  if (!hasItems) return null
  return (
    <Popover open onOpenChange={(next) => { if (!next) onDismiss() }}>
      <PopoverPopup
        className={css.menu}
        ref={self}
        role="menu"
        data-dockkit-tab-menu
        anchor={anchorRef}
        positionMethod="fixed"
        side="bottom"
        align="start"
        sideOffset={MENU_GAP}
        collisionPadding={MENU_GAP}
        initialFocus={false}
        // Base UI treats the returned element as a focus container rather than the
        // target itself, so restoration focuses the chip here and opts out.
        finalFocus={() => {
          const target = anchorRef.current
          if (target?.isConnected) focusWithoutRing(target)
          return false
        }}
        // React bubbles a portal's events to its tab chip, so both halves of a
        // press must stop here: the press would otherwise start a tab drag and
        // capture the pointer, which retargets the release and swallows this
        // item's click; the click would otherwise also focus a tab the item may
        // have removed.
        onPointerDown={(event) => { event.stopPropagation() }}
        onClick={(event) => { event.stopPropagation() }}
      >
        {onClose !== undefined && (
          <button type="button" role="menuitem" className={css.menuItem} data-dockkit-menu-close onClick={onClose}>
            {labels.closeTab}
          </button>
        )}
        {/* Embedder items last: the kit's own item is the same in every menu, so
            a reader looks for it in the same place every time. */}
        {extras}
      </PopoverPopup>
    </Popover>
  )
}

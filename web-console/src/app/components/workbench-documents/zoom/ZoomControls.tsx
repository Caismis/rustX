/* Copyright (c) 2026 DeepSeek. MIT. Source port; see PROVENANCE.md. */
/** Floating controls shared by every zoomable document renderer. */
import { forwardRef, useEffect, useImperativeHandle, useState, type ReactNode } from 'react'
import { Button, IconChevronDownOutlineRegular, IconPlusOutlineRegular, Menu, Tooltip,
  type MenuEntry } from '../primitives'
import { MAX_FIXED_ZOOM, MIN_FIXED_ZOOM, ZOOM_OPTIONS, ZOOM_STEP, type ZoomLabels } from './types.ts'

function steppedZoom(zoom: number, direction: -1 | 1): number {
  const step = direction === -1 ? Math.ceil(zoom / ZOOM_STEP) - 1 : Math.floor(zoom / ZOOM_STEP) + 1
  return Math.min(MAX_FIXED_ZOOM, Math.max(MIN_FIXED_ZOOM, step * ZOOM_STEP))
}

/** Live percentage display updated independently from the document tree. */
export interface ZoomControlsHandle {
  /** @param zoom - current gesture scale multiplier. */
  showZoom: (zoom: number) => void
}

/**
 * Present fit-width, fixed presets, and incremental controls.
 * @param props - current resolved zoom, preference mode, labels, and callbacks.
 * @returns a localized zoom toolbar.
 */
export const ZoomControls = forwardRef<ZoomControlsHandle, {
  readonly zoom: number
  readonly fitWidth: boolean
  readonly labels: ZoomLabels
  readonly visible: boolean
  readonly onZoom: (zoom: number) => void
  readonly onFitWidth: () => void
  readonly onActiveChange: (active: boolean) => void
}>(function ZoomControls({ zoom, fitWidth, labels, visible, onZoom, onFitWidth, onActiveChange }, ref): ReactNode {
  const [displayZoom, setDisplayZoom] = useState(zoom)
  const [open, setOpen] = useState(false)
  const [hovered, setHovered] = useState(false)
  const [focused, setFocused] = useState(false)
  useImperativeHandle(ref, () => ({ showZoom: setDisplayZoom }), [])
  useEffect(() => { onActiveChange(open || hovered || focused) }, [focused, hovered, onActiveChange, open])
  const items: readonly MenuEntry[] = [
    { id: 'fit-width', label: labels.fitWidth },
    { type: 'separator', id: 'fit-scale' },
    ...ZOOM_OPTIONS.map(value => ({ id: String(value), label: labels.value(value * 100) })),
  ]
  const selectedId = fitWidth ? 'fit-width'
    : ZOOM_OPTIONS.includes(displayZoom as typeof ZOOM_OPTIONS[number]) ? String(displayZoom) : undefined
  const panelBase = 'absolute z-[2] start-1/2 bottom-[14px] flex items-center gap-[2px] box-border px-[6px] py-[4px] border-0 rounded-[var(--dsw-radius-lg)] [color:var(--dsw-alias-label-primary)] bg-[var(--dsw-specific-menu)] shadow-[var(--dsw-elevation-prominent)] text-[12px] leading-[18px] [will-change:opacity,translate,scale] motion-reduce:[transition-duration:0.01ms]'
  const panelHidden = 'opacity-0 pointer-events-none translate-x-[-50%] translate-y-[16px] scale-[0.98] transition-[opacity,translate,scale] duration-[160ms] ease-in'
  const panelShown = 'opacity-100 pointer-events-auto translate-x-[-50%] translate-y-0 scale-100 transition-[opacity,translate,scale] duration-[180ms] ease-out'
  // Touch devices cannot hover the toolbar into view: it is always visible.
  const panelCoarse = '[@media(hover:none)]:opacity-100 [@media(hover:none)]:pointer-events-auto [@media(hover:none)]:translate-y-0 [@media(hover:none)]:scale-100 [@media(pointer:coarse)]:opacity-100 [@media(pointer:coarse)]:pointer-events-auto [@media(pointer:coarse)]:translate-y-0 [@media(pointer:coarse)]:scale-100'
  return <div className={`${panelBase} ${visible ? panelShown : panelHidden} ${panelCoarse}`} role="toolbar" aria-label={labels.controls}
    data-document-zoom-controls data-document-zoom-visible={visible || undefined}
    onPointerEnter={() => { setHovered(true) }} onPointerLeave={() => { setHovered(false) }}
    onFocusCapture={() => { setFocused(true) }} onBlurCapture={() => { setFocused(false) }}>
    <Tooltip label={labels.out} side="top">
      <span className="flex"><Button size="sm" className="box-border w-[28px] p-0 flex-none [&_svg]:size-[14px]" aria-label={labels.out}
        disabled={displayZoom <= MIN_FIXED_ZOOM} onMouseDown={(event) => { event.preventDefault() }}
        onClick={() => { onZoom(steppedZoom(displayZoom, -1)) }}>
        <svg width="14" height="14" viewBox="0 0 16 16" aria-hidden="true">
          <path d="M2 8h12" fill="none" stroke="currentColor" strokeWidth="1.5" />
        </svg>
      </Button></span>
    </Tooltip>
    <Menu open={open} align="start" dense listClassName="[--menu-min-width:112px] [box-shadow:var(--dsw-elevation-prominent)]"
      anchor={<Button size="sm" className="box-border inline-flex items-center justify-center gap-[2px] w-[58px] px-[6px] [font-variant-numeric:tabular-nums] [&_svg]:size-[12px] [&_svg]:flex-none" aria-label={labels.menu} aria-expanded={open}
        onMouseDown={(event) => { event.preventDefault() }} onClick={() => { setOpen(value => !value) }}>
        <span>{labels.value(Math.round(displayZoom * 100))}</span>
        <IconChevronDownOutlineRegular size={12} />
      </Button>}
      items={items} selectedId={selectedId} onClose={() => { setOpen(false) }} onSelect={(selected) => {
        setOpen(false)
        if (selected === 'fit-width') onFitWidth()
        else onZoom(Number(selected))
      }} />
    <Tooltip label={labels.into} side="top">
      <span className="flex"><Button size="sm" className="box-border w-[28px] p-0 flex-none [&_svg]:size-[14px]" aria-label={labels.into}
        disabled={displayZoom >= MAX_FIXED_ZOOM} onMouseDown={(event) => { event.preventDefault() }}
        onClick={() => { onZoom(steppedZoom(displayZoom, 1)) }}>
        <IconPlusOutlineRegular size={14} />
      </Button></span>
    </Tooltip>
  </div>
})

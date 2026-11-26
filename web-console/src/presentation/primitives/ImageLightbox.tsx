/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
import { useRef } from 'react'
import { DialogSurface } from './DialogSurface'
import { IconCloseOutline16 } from './icons/index.tsx'

/** Lightbox strings the owner resolves from its own locale namespace. */
export interface ImageLightboxLabels {
  /** Accessible name of the preview dialog. */
  dialog: string
  /** Accessible label of the close control. */
  close: string
}

/**
 * Document-level original-image preview opened by clicking a thumbnail.
 * coss ui/Base UI owns modality, Escape, focus containment and focus
 * restoration; rustX keeps the viewport geometry and image presentation.
 */
export function ImageLightbox({ src, alt, labels, onClose }: {
  src: string
  alt: string
  labels: ImageLightboxLabels
  onClose: () => void
}) {
  const closeRef = useRef<HTMLButtonElement | null>(null)
  return <DialogSurface open title={labels.dialog} onClose={onClose} overlayClassName="fixed inset-0 z-[1000]" className="absolute inset-x-0 bottom-0 top-[var(--dsh-frame-chrome-top,0px)] flex items-center justify-center p-[40px]"
    initialFocus={() => closeRef.current ?? false}>
    <div className="absolute inset-0 bg-[var(--dsw-alias-bg-mask-1)] [backdrop-filter:var(--dsw-mask-blur)]" aria-hidden="true" onMouseDown={onClose} />
    <img className="relative max-w-[min(100%,1600px)] max-h-full object-contain rounded-[var(--dsw-radius-lg)] bg-[var(--dsw-specific-input-major)] border-0 shadow-[var(--dsw-elevation-prominent)]" src={src} alt={alt} />
    <button ref={closeRef} type="button" data-lightbox-close="" className="absolute top-[20px] right-[20px] z-[1] grid place-items-center w-[36px] h-[36px] [border:0.5px_solid_var(--dsw-alias-border-l2-darkmode-thin)] rounded-full [corner-shape:round] bg-[var(--dsw-specific-input-major)] [color:var(--dsw-alias-label-primary)] cursor-pointer focus-visible:[outline:var(--dsw-focus-ring-width)_solid_var(--dsw-focus-ring-color,var(--dsw-alias-state-business-primary))] focus-visible:outline-offset-[3px]" aria-label={labels.close} onClick={onClose}>
      <IconCloseOutline16 size={16} />
    </button>
  </DialogSurface>
}

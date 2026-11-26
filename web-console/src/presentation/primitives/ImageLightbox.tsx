/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
import { useRef } from 'react'
import { DialogSurface } from './DialogSurface'
import { IconCloseOutline16 } from './icons/index.tsx'
import css from './ImageLightbox.module.css'

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
  return <DialogSurface open title={labels.dialog} onClose={onClose} overlayClassName={css.backdrop} className={css.stage}
    initialFocus={() => closeRef.current ?? false}>
    <div className={css.mask} aria-hidden="true" onMouseDown={onClose} />
    <img className={css.image} src={src} alt={alt} />
    <button ref={closeRef} type="button" data-lightbox-close="" className={css.close} aria-label={labels.close} onClick={onClose}>
      <IconCloseOutline16 size={16} />
    </button>
  </DialogSurface>
}
